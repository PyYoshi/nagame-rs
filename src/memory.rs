//! 画素データのメモリー予算、保持量の台帳、メモリー逼迫の検知、glibcの返却設定。
//! 予算は設定（自動／上限指定）を基本とし、OSの空き容量と逼迫通知では下げる方向にだけ使う。

use gpui::ImageId;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub const MIB: u64 = 1024 * 1024;
/// 予算の下限。設定や逼迫で予算を下げても、1枚の画像と全フレームを保持するGIFは
/// この大きさまで扱う。以前の固定上限と同じ値。
pub const FLOOR: u64 = 128 * MIB;
/// 1辺16,384ピクセルのRGBA。単体画像はこれを超えない。
const STILL_MAX: u64 = 16384 * 16384 * 4;
/// 全フレームを保持するアニメーションは、同じ量をGPUアトラスにも載せる。
/// GPUIからVRAMの残量を取得できないため、空きメモリーが多くてもこの量で止める。
const RESIDENT_MAX: u64 = 1024 * MIB;
/// 自動予算（RAMの1/8）の範囲。
const AUTO_MIN: u64 = 256 * MIB;
const AUTO_MAX: u64 = 2048 * MIB;
/// メモリー情報を取得できない環境の自動予算。
const AUTO_UNKNOWN: u64 = 1024 * MIB;
/// 逼迫通知の後、この時間は予算を下げたままにする。
pub const PRESSURE_HOLD: Duration = Duration::from_secs(30);
/// 設定画面で選べる上限（MiB）。0は自動。
pub const LIMIT_CHOICES: [u32; 7] = [0, 256, 512, 1024, 2048, 4096, 8192];

/// 用途ごとの上限。totalから導く。ahead（32 MiB以上）を除き、FLOOR未満にはしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// キャッシュ・表示中の画像・アニメーション・補間結果を合わせた保持量の目標。
    pub total: u64,
    /// 1枚の画像（GIFの1フレーム、見開き合成を含む）の展開上限。
    pub still: u64,
    /// 全フレームを保持して再生するアニメーションの上限。超えると逐次デコードへ切り替える。
    pub resident: u64,
    /// 逐次デコードで再生位置より先に保持する量の上限。
    pub ahead: u64,
    /// CPU補間の出力（全フレーム合計）の上限。超えるとGPU表示へ戻る。
    pub resample: u64,
}
impl Budget {
    pub fn from_total(total: u64) -> Self {
        let total = total.max(FLOOR);
        Self {
            total,
            still: total.clamp(FLOOR, STILL_MAX),
            resident: (total / 2).clamp(FLOOR, RESIDENT_MAX),
            ahead: (total / 8).clamp(32 * MIB, 256 * MIB),
            resample: (total / 4).clamp(FLOOR, 512 * MIB),
        }
    }
}

/// OSが報告するメモリー量。cgroupは祖先を含めて最も厳しい制限を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct System {
    /// MemTotal。
    pub total: u64,
    /// MemAvailable。
    pub available: u64,
    /// cgroupの残り。memory.maxから、回収できるページキャッシュ（inactive_file）を除いた
    /// memory.currentを引いた量で、祖先を含めて最も少ない値。制限がなければNone。
    pub cgroup_headroom: Option<u64>,
    /// 祖先を含めて最も小さいmemory.max。制限がなければNone。
    pub cgroup_limit: Option<u64>,
}
impl System {
    fn ram(&self) -> u64 {
        self.cgroup_limit
            .map_or(self.total, |limit| limit.min(self.total))
    }
    fn headroom(&self) -> u64 {
        self.cgroup_headroom
            .map_or(self.available, |headroom| headroom.min(self.available))
    }
}

/// `/proc/meminfo` からMemTotalとMemAvailableを読む。
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kib| kib.parse::<u64>().ok())
            .map(|kib| kib * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// `/proc/self/cgroup` のcgroup v2の行から、cgroupfs上のディレクトリを得る。
fn parse_cgroup(text: &str, root: &Path) -> Option<PathBuf> {
    let relative = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(root.join(relative.trim().trim_start_matches('/')))
}

/// cgroupの階層を上へたどり、(最小の制限, 最小の残り)を返す。
fn cgroup_limits(directory: &Path, root: &Path) -> (Option<u64>, Option<u64>) {
    let mut limit: Option<u64> = None;
    let mut headroom: Option<u64> = None;
    let mut current = Some(directory);
    while let Some(dir) = current {
        if !dir.starts_with(root) {
            break;
        }
        let read = |name| std::fs::read_to_string(dir.join(name)).ok();
        if let Some(max) = read("memory.max").and_then(|v| v.trim().parse::<u64>().ok()) {
            limit = Some(limit.map_or(max, |l| l.min(max)));
            if let Some(used) = read("memory.current").and_then(|v| v.trim().parse::<u64>().ok()) {
                // memory.currentには、制限に近づくとカーネルが回収するページキャッシュも含まれる。
                // 除かないと、多くの画像ファイルを見ただけでcgroupが満杯に見える。
                let reclaimable = read("memory.stat")
                    .and_then(|stat| {
                        stat.lines()
                            .find_map(|line| line.strip_prefix("inactive_file "))
                            .and_then(|v| v.trim().parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                let rest = max.saturating_sub(used.saturating_sub(reclaimable));
                headroom = Some(headroom.map_or(rest, |h| h.min(rest)));
            }
        }
        if dir == root {
            break;
        }
        current = dir.parent();
    }
    (limit, headroom)
}

/// 現在のメモリー量。`/proc/meminfo` を読めなければNone。
pub fn system() -> Option<System> {
    let (total, available) = parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)?;
    let root = Path::new("/sys/fs/cgroup");
    let (cgroup_limit, cgroup_headroom) = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|text| parse_cgroup(&text, root))
        .map(|directory| cgroup_limits(&directory, root))
        .unwrap_or((None, None));
    Some(System {
        total,
        available,
        cgroup_headroom,
        cgroup_limit,
    })
}

/// 設定の上限（MiB、0は自動）から基本の予算を決める。自動はRAM（cgroupの制限を含む）の
/// 1/8を256 MiB〜2 GiBに収めた値。
pub fn base_total(limit_mib: u32, system: Option<&System>) -> u64 {
    if limit_mib > 0 {
        return u64::from(limit_mib) * MIB;
    }
    system.map_or(AUTO_UNKNOWN, |s| (s.ram() / 8).clamp(AUTO_MIN, AUTO_MAX))
}

/// 基本の予算を、OSの空き容量と逼迫の状態で下げる。上げる方向には使わない。
/// heldはこのアプリが既に保持している量。空き容量には含まれないため足し戻す。
pub fn effective_total(base: u64, system: Option<&System>, held: u64, pressure: bool) -> u64 {
    let mut total = base;
    if let Some(system) = system {
        total = total.min(held.saturating_add(system.headroom() / 2));
    }
    if pressure {
        total = total.min(base / 4);
    }
    total.max(FLOOR)
}

static LIMIT_MIB: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "ui-smoke")]
static OVERRIDE: Mutex<Option<Budget>> = Mutex::new(None);

/// 設定画面の上限を反映する。0は自動。
pub fn set_limit_mib(limit: u32) {
    LIMIT_MIB.store(limit, Ordering::Relaxed);
}

/// 実ウィンドウテストで逐次デコードなどを小さなGIFで再現するための差し替え。
#[cfg(feature = "ui-smoke")]
pub fn override_budget(budget: Option<Budget>) {
    *OVERRIDE.lock().unwrap() = budget;
}

/// OSのメモリー情報。逐次デコード中はチャンクが変わるたびに予算を求め直すため、
/// 0.5秒は同じ値を使う。
fn recent_system() -> Option<System> {
    static RECENT: Mutex<Option<(Instant, Option<System>)>> = Mutex::new(None);
    let mut recent = RECENT.lock().unwrap();
    match *recent {
        Some((at, system)) if at.elapsed() < Duration::from_millis(500) => system,
        _ => {
            let system = system();
            *recent = Some((Instant::now(), system));
            system
        }
    }
}

/// 現在の予算。heldは台帳の保持量。
pub fn current(held: u64) -> Budget {
    #[cfg(feature = "ui-smoke")]
    if let Some(budget) = *OVERRIDE.lock().unwrap() {
        return budget;
    }
    let system = recent_system();
    let base = base_total(LIMIT_MIB.load(Ordering::Relaxed), system.as_ref());
    Budget::from_total(effective_total(
        base,
        system.as_ref(),
        held,
        under_pressure(),
    ))
}

/// 台帳上の画像の持ち主。ビットで表し、同じ画像を両方が持てる。
#[derive(Clone, Copy)]
pub enum Holder {
    Cache = 1,
    Viewer = 2,
}

/// 保持中の画素データを `ImageId` 単位で数える。キャッシュとViewerが同じ画像を
/// 共有していても一度だけ数える。
#[derive(Default)]
pub struct Ledger {
    entries: Mutex<HashMap<ImageId, (u64, u8)>>,
}
impl Ledger {
    /// holderが保持する画像の集合を置き換える。
    pub fn replace(&self, holder: Holder, images: impl IntoIterator<Item = (ImageId, u64)>) {
        let bit = holder as u8;
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, (_, holders)| {
            *holders &= !bit;
            *holders != 0
        });
        for (id, bytes) in images {
            entries.entry(id).or_insert((bytes, 0)).1 |= bit;
        }
    }
    /// holderがidの画像を保持しているか。
    pub fn holds(&self, holder: Holder, id: ImageId) -> bool {
        self.entries
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|(_, holders)| holders & holder as u8 != 0)
    }
    /// 保持中の画像の合計。
    pub fn held(&self) -> u64 {
        self.entries.lock().unwrap().values().map(|(b, _)| b).sum()
    }
    /// holderが保持していない画像の合計。
    #[cfg(test)]
    pub fn outside(&self, holder: Holder) -> u64 {
        self.outside_except(holder, &std::collections::HashSet::new())
    }
    /// holderが保持していない画像のうち、exceptを除いた合計。キャッシュの容量計算に使う。
    pub fn outside_except(
        &self,
        holder: Holder,
        except: &std::collections::HashSet<ImageId>,
    ) -> u64 {
        let bit = holder as u8;
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, (_, holders))| holders & bit == 0 && !except.contains(id))
            .map(|(_, (b, _))| b)
            .sum()
    }
}

/// 画像の全フレームの画素データの容量。
pub fn image_bytes(image: &gpui::RenderImage) -> u64 {
    (0..image.frame_count())
        .filter_map(|i| image.as_bytes(i))
        .map(|bytes| bytes.len() as u64)
        .sum()
}

static PRESSURE_EVENTS: AtomicU64 = AtomicU64::new(0);
static PRESSURE_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// メモリー逼迫を記録する。監視スレッドと実ウィンドウテストから呼ぶ。
pub fn signal_pressure() {
    *PRESSURE_AT.lock().unwrap() = Some(Instant::now());
    PRESSURE_EVENTS.fetch_add(1, Ordering::Relaxed);
}
/// これまでの逼迫の回数。Viewerは増えたら対応する。
pub fn pressure_events() -> u64 {
    PRESSURE_EVENTS.load(Ordering::Relaxed)
}
/// 最後の逼迫から `PRESSURE_HOLD` が経っていない。
pub fn under_pressure() -> bool {
    PRESSURE_AT
        .lock()
        .unwrap()
        .is_some_and(|at| at.elapsed() < PRESSURE_HOLD)
}

/// 逼迫とみなすか。PSIの停止通知（psi）は他のプロセスのスワップ読み戻しなどでも届くため、
/// 空き容量がRAMの10%と512 MiBの大きい方を下回る場合だけ採用する。通知がなくても、
/// RAMの5%と256 MiBの大きい方を下回れば逼迫とする。
fn pressing(system: &System, psi: bool) -> bool {
    let (share, floor) = if psi {
        (10, 512 * MIB)
    } else {
        (20, 256 * MIB)
    };
    system.headroom() < (system.ram() / share).max(floor)
}

/// 空き容量を確かめ、逼迫なら記録する。
fn check(psi: bool) {
    if system().is_some_and(|s| pressing(&s, psi)) {
        signal_pressure();
    }
}

/// メモリー逼迫の監視を一度だけ開始する。PSIのトリガー（2秒間に150msの停止）を待ちながら、
/// 2秒ごとに空き容量も確認する。PSIを使えない環境では空き容量の確認だけを行う。
pub fn start_monitor() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("nagame-memory".into())
            .spawn(|| {
                #[cfg(target_os = "linux")]
                psi::watch(check);
                loop {
                    check(false);
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
    });
}

#[cfg(target_os = "linux")]
mod psi {
    use std::{io::Write, os::fd::AsRawFd, path::PathBuf};

    /// 非特権ユーザーのPSIトリガーは監視間隔を2秒の倍数にする必要がある。
    /// カーネルは書き込みの最後の1バイトを終端に置き換えるため、NULまで書く。
    const TRIGGER: &[u8] = b"some 150000 2000000\0";

    /// cgroup（コンテナの制限を含む）を優先し、なければシステム全体。
    fn files() -> Vec<PathBuf> {
        let root = std::path::Path::new("/sys/fs/cgroup");
        let mut files = Vec::new();
        if let Some(directory) = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|text| super::parse_cgroup(&text, root))
        {
            files.push(directory.join("memory.pressure"));
        }
        files.push("/proc/pressure/memory".into());
        files
    }

    /// トリガーを登録できたファイルで、通知（true）と2秒ごとの確認（false）のたびにcheckを呼ぶ。
    /// 監視できる間は戻らない。pollが失敗したら次のファイルを試し、どれも使えなければ戻る
    /// （呼び出し側は空き容量の確認だけに切り替える）。
    pub fn watch(check: fn(bool)) {
        for path in files() {
            let Ok(mut file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            else {
                continue;
            };
            if file.write_all(TRIGGER).is_err() {
                continue;
            }
            let mut poll = libc::pollfd {
                fd: file.as_raw_fd(),
                events: libc::POLLPRI,
                revents: 0,
            };
            loop {
                poll.revents = 0;
                // SAFETY: pollは有効なfdを1つだけ持つ配列を読む。fileはループ中有効。
                let result = unsafe { libc::poll(&mut poll, 1, 2000) };
                if result < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if poll.revents & libc::POLLERR != 0 {
                    break;
                }
                check(poll.revents & libc::POLLPRI != 0);
            }
        }
    }
}

/// glibcは大きなブロックの解放後にmmapの閾値を最大32 MiBまで引き上げるため、
/// 以後のフレームバッファがヒープから確保され、解放してもRSSが下がらない。
/// 閾値を固定して、1 MiB以上の画素バッファを解放時にOSへ返す。
pub fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: 起動直後、他のスレッドが確保を始める前に一度だけ呼ぶ。
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
    }
}

/// 1 MiB未満の画素バッファが残したヒープの空きをOSへ返す。
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trimに前提条件はない。
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_derives_floors_and_caps_from_total() {
        let small = Budget::from_total(0);
        assert_eq!(small.total, FLOOR);
        assert_eq!(small.still, FLOOR);
        assert_eq!(small.resident, FLOOR);
        assert_eq!(small.resample, FLOOR);
        assert_eq!(small.ahead, 32 * MIB);
        let large = Budget::from_total(16 * 1024 * MIB);
        assert_eq!(large.still, STILL_MAX);
        assert_eq!(large.resident, RESIDENT_MAX);
        assert_eq!(large.ahead, 256 * MIB);
        assert_eq!(large.resample, 512 * MIB);
        let auto = Budget::from_total(2048 * MIB);
        assert_eq!(auto.resident, 1024 * MIB);
        assert_eq!(auto.resample, 512 * MIB);
    }

    #[test]
    fn effective_total_only_lowers_the_configured_budget() {
        let roomy = System {
            total: 64 * 1024 * MIB,
            available: 40 * 1024 * MIB,
            cgroup_headroom: None,
            cgroup_limit: None,
        };
        assert_eq!(base_total(0, Some(&roomy)), AUTO_MAX);
        assert_eq!(base_total(0, None), AUTO_UNKNOWN);
        assert_eq!(base_total(512, Some(&roomy)), 512 * MIB);
        assert_eq!(effective_total(AUTO_MAX, Some(&roomy), 0, false), AUTO_MAX);
        assert_eq!(
            effective_total(AUTO_MAX, Some(&roomy), 0, true),
            AUTO_MAX / 4
        );
        let tight = System {
            available: 600 * MIB,
            ..roomy
        };
        // 自分が持つ500 MiBはMemAvailableに含まれないため、足し戻す。
        assert_eq!(
            effective_total(AUTO_MAX, Some(&tight), 500 * MIB, false),
            800 * MIB
        );
        let container = System {
            cgroup_limit: Some(2048 * MIB),
            cgroup_headroom: Some(100 * MIB),
            ..roomy
        };
        assert_eq!(base_total(0, Some(&container)), 256 * MIB);
        assert_eq!(effective_total(AUTO_MAX, Some(&container), 0, false), FLOOR);
        assert!(pressing(&container, false));
        assert!(!pressing(&roomy, true));
        // 空き容量が十分なら、PSIの停止通知だけでは逼迫としない（他のプロセスのスワップ
        // 読み戻しなど）。空き容量が少なければ逼迫とする。
        let low = System {
            available: 4 * 1024 * MIB,
            ..roomy
        };
        assert!(pressing(&low, true));
        assert!(!pressing(&low, false));
    }

    #[test]
    fn meminfo_and_cgroup_paths_are_parsed() {
        let text = "MemTotal:       57395748 kB\nMemFree: 1 kB\nMemAvailable:   43302160 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some((57395748 * 1024, 43302160 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
        let root = Path::new("/sys/fs/cgroup");
        assert_eq!(
            parse_cgroup("0::/user.slice/app.scope\n", root),
            Some(root.join("user.slice/app.scope"))
        );
        assert_eq!(parse_cgroup("1:memory:/legacy\n", root), None);
    }

    #[test]
    fn cgroup_limits_use_the_tightest_ancestor() {
        let root = std::env::temp_dir().join(format!("nagame-cgroup-{}", std::process::id()));
        let child = root.join("parent/child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("parent/memory.max"), "1000\n").unwrap();
        std::fs::write(root.join("parent/memory.current"), "900\n").unwrap();
        std::fs::write(child.join("memory.max"), "max\n").unwrap();
        std::fs::write(child.join("memory.current"), "10\n").unwrap();
        assert_eq!(cgroup_limits(&child, &root), (Some(1000), Some(100)));
        // 回収できるページキャッシュは使用量に数えない。
        std::fs::write(
            root.join("parent/memory.stat"),
            "anon 500\ninactive_file 300\nactive_file 100\n",
        )
        .unwrap();
        assert_eq!(cgroup_limits(&child, &root), (Some(1000), Some(400)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ledger_counts_shared_images_once() {
        let ledger = Ledger::default();
        ledger.replace(Holder::Cache, [(ImageId(1), 10), (ImageId(2), 20)]);
        ledger.replace(Holder::Viewer, [(ImageId(2), 20), (ImageId(3), 5)]);
        assert_eq!(ledger.held(), 35);
        assert_eq!(ledger.outside(Holder::Cache), 5);
        assert_eq!(ledger.outside(Holder::Viewer), 10);
        ledger.replace(Holder::Cache, []);
        assert_eq!(ledger.held(), 25);
        assert_eq!(ledger.outside(Holder::Cache), 25);
    }
}
