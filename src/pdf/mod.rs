//! PDFのページを描く補助プロセスと、本体側の仲介。
//! PDFiumは補助プロセス（`nagame --pdf-helper`）だけがロードする。本体はページ数・寸法と、
//! 描いたBGRAの画素だけを受け取る。設計と選定理由はdocs/pdf-support.mdを参照。

mod helper;
mod ipc;
#[cfg(any(test, feature = "ui-smoke"))]
pub mod synthetic;

use crate::i18n::tr;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{fs::FileExt, fs::MetadataExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// 文字・図形を含むページの基準解像度（DPI）。
pub const BASE_DPI: f32 = 150.;
/// 1辺の上限（GPUIのテクスチャと既存の画像の上限に揃える）。
pub const MAX_SIDE: u32 = 16384;
/// 補助プロセスの起動を示す引数。
const HELPER_ARG: &str = "--pdf-helper";
/// 補助プロセスへ渡すソケットの記述子番号と、メモリー上限（バイト）の環境変数。
const HELPER_FD: &str = "NAGAME_PDF_HELPER_FD";
const HELPER_LIMIT: &str = "NAGAME_PDF_HELPER_LIMIT";
/// PDFiumの共有ライブラリーの絶対パス。開発時は.cargo/config.tomlが設定する。
const LIBRARY_ENV: &str = "NAGAME_PDFIUM_LIBRARY";
/// 補助プロセスの起動と1件の処理の制限時間。超えたら停止させて起動し直す。
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// 同じページでこの回数だけ異常終了（時間切れ）したら、再試行しない。
const CRASH_LIMIT: u8 = 2;
/// 待つ人がいなくなった描画を、別の要求のために止めるまでの時間。
const ABANDON_AFTER: Duration = Duration::from_secs(1);
/// 応答を待つ間に、新しい要求と中止を確かめる間隔。
const POLL: Duration = Duration::from_millis(50);
/// 受け付けるページ数の上限。壊れた応答で大量に確保しない。
const MAX_PAGES: usize = 1_000_000;

/// 拡張子からPDFか判定する。
pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
}

/// 描く解像度の決め方。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    /// 基準解像度。limitは出力の上限（バイト）で、超える場合は縮める。
    Base { limit: u64 },
    /// 1ptあたりscale画素。見開きで2ページの倍率を揃えるときに使う。
    Scale { scale: f32, limit: u64 },
    /// 指定の画素寸法。表示解像度での描き直しに使う。
    Size { width: u32, height: u32 },
}
impl Mode {
    /// 本体が受け取る画素データの上限（バイト）。補助プロセスの応答をこれで検査する。
    fn limit(self) -> u64 {
        match self {
            Self::Base { limit } | Self::Scale { limit, .. } => limit,
            Self::Size { width, height } => u64::from(width) * u64::from(height) * 4,
        }
    }
}

/// 開けない・描けない理由。利用者へはmessageで示す。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// PDFiumが見つからない、または読み込めない。
    Library,
    /// 利用者パスワードで暗号化されている、または未対応の暗号化。
    Encrypted,
    /// PDFとして読めない。
    Corrupt,
    /// ページがない。
    Empty,
    /// ページを読めない、または描けない。
    Page,
    /// 補助プロセスが異常終了した。
    Crashed,
    /// 制限時間内に終わらなかった。
    Timeout,
    /// ファイルを読めない。
    Io,
    /// 呼び出し側が待つのをやめた、または新しい要求に置き換えられた。
    Cancelled,
}
impl Failure {
    pub fn message(self) -> String {
        match self {
            Self::Library => tr!("pdf-library-missing"),
            Self::Encrypted => tr!("pdf-encrypted"),
            Self::Corrupt => tr!("pdf-corrupt"),
            Self::Empty => tr!("pdf-empty"),
            Self::Page => tr!("pdf-page-error"),
            Self::Crashed => tr!("pdf-crashed"),
            Self::Timeout => tr!("pdf-timeout"),
            Self::Io => tr!("pdf-io"),
            Self::Cancelled => "cancelled".into(),
        }
    }
}

/// 描いたページ。pixelsの中身はBGRA（型名はRGBA）。
pub struct Rendered {
    pub pixels: image::RgbaImage,
    /// 1ptあたりの画素数。
    pub scale: f32,
    /// 文字・図形を含むページ（固有の画素寸法がない）。
    pub vector: bool,
    /// 上限（Mode::Base・Scaleのlimit）に合わせて縮めた。
    pub limited: bool,
}

/// 要求の種類。現在ページ、表示解像度、先読みの順に処理し、新しい要求は同じ種類の
/// 未着手の要求を置き換える。開く要求（一覧の解決）は順に処理し、最優先にする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Open,
    Current,
    Display,
    Prefetch,
}

/// 補助プロセスへの要求。ハンドルを含まない平文のデータで、PDFは記述子で渡す。
#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Open,
    Render {
        page: u32,
        mode: Mode,
    },
    /// 開いている文書を閉じる。
    Trim,
    #[cfg(test)]
    Hang,
    #[cfg(test)]
    Abort,
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    /// 起動してPDFiumを読み込めた。
    Ready,
    /// ページごとの論理寸法（pt。回転とCropBoxを反映）。
    Opened {
        pages: Vec<[f32; 2]>,
    },
    /// 画素は付けたmemfdにある（幅×4バイトの行が高さ分）。
    Rendered {
        width: u32,
        height: u32,
        scale: f32,
        vector: bool,
        limited: bool,
    },
    Trimmed,
    Failed(Failure),
}

/// PDFを開き、ページごとの論理寸法を返す。一覧の解決（バックグラウンド）から呼ぶ。
pub fn open(path: &Path) -> Result<Vec<[f32; 2]>, Failure> {
    match call(path, Request::Open, Class::Open, || false)? {
        Reply::Opened(pages) => Ok(pages),
        Reply::Rendered(_) => Err(Failure::Corrupt),
    }
}

/// pageを描く。cancelledがtrueになったら待つのをやめる（描画中の処理は最後まで続く）。
pub fn render(
    path: &Path,
    page: u32,
    mode: Mode,
    class: Class,
    cancelled: impl Fn() -> bool,
) -> Result<Rendered, Failure> {
    match call(path, Request::Render { page, mode }, class, cancelled)? {
        Reply::Rendered(rendered) => Ok(rendered),
        Reply::Opened(_) => Err(Failure::Corrupt),
    }
}

/// メモリー逼迫への対応。aggressiveでなければ開いている文書を閉じさせ、aggressiveなら
/// 補助プロセスを停止させる（次の要求で起動し直す）。
pub fn trim(aggressive: bool) {
    let _ = service().send(Message::Trim { aggressive });
}

/// pathの文書で記録した異常終了・時間切れを消し、もう一度試せるようにする（F5）。
pub fn forget(path: &Path) {
    if let Some(identity) = File::open(path).ok().as_ref().and_then(Identity::of) {
        let _ = service().send(Message::Forget(identity));
    }
}

/// 補助プロセスの常駐メモリー（バイト）。動いていなければ0。NAGAME_PROFILEのログ用。
pub fn helper_rss() -> u64 {
    HELPER_RSS.load(Ordering::Relaxed)
}

/// 補助プロセスとして起動されたら、その処理を行って終了コードを返す。mainの最初に呼ぶ。
pub fn helper_main() -> Option<i32> {
    (std::env::args_os().nth(1)? == HELPER_ARG).then(helper::run)
}

enum Reply {
    Opened(Vec<[f32; 2]>),
    Rendered(Rendered),
}

struct Job {
    class: Class,
    request: Request,
    file: File,
    reply: mpsc::Sender<Result<Reply, Failure>>,
    /// 呼び出し側が待つのをやめた。未着手なら処理しない。
    abandoned: Arc<AtomicBool>,
}

enum Message {
    Job(Job),
    Trim {
        aggressive: bool,
    },
    /// 文書の失敗の記録を消す（利用者が開き直したとき）。
    Forget(Identity),
}

static HELPER_RSS: AtomicU64 = AtomicU64::new(0);

fn service() -> &'static mpsc::Sender<Message> {
    static SENDER: OnceLock<mpsc::Sender<Message>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, input) = mpsc::channel();
        std::thread::Builder::new()
            .name("pdf".into())
            .spawn(move || Service::default().run(input))
            .expect("start the PDF thread");
        sender
    })
}

fn call(
    path: &Path,
    request: Request,
    class: Class,
    cancelled: impl Fn() -> bool,
) -> Result<Reply, Failure> {
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let file = File::open(path).map_err(|_| Failure::Io)?;
    let (reply, result) = mpsc::channel();
    let abandoned = Arc::new(AtomicBool::new(false));
    service()
        .send(Message::Job(Job {
            class,
            request,
            file,
            reply,
            abandoned: abandoned.clone(),
        }))
        .map_err(|_| Failure::Crashed)?;
    loop {
        match result.recv_timeout(Duration::from_millis(10)) {
            Ok(reply) => return reply,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancelled() {
                    abandoned.store(true, Ordering::Relaxed);
                    return Err(Failure::Cancelled);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Failure::Crashed),
        }
    }
}

/// ファイルの同一性（異常終了の記録と、補助プロセスの文書キャッシュのキー）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Identity {
    device: u64,
    inode: u64,
    size: u64,
    modified: i64,
    modified_nanos: i64,
}
impl Identity {
    fn of(file: &File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
        })
    }
}

/// 未着手の要求。開く要求は順に、他は種類ごとに最新の1件だけを持つ。
#[derive(Default)]
struct Queue {
    opens: VecDeque<Job>,
    current: Option<Job>,
    display: Option<Job>,
    prefetch: Option<Job>,
}
impl Queue {
    fn push(&mut self, job: Job) {
        let slot = match job.class {
            Class::Open => {
                self.opens.push_back(job);
                return;
            }
            Class::Current => &mut self.current,
            Class::Display => &mut self.display,
            Class::Prefetch => &mut self.prefetch,
        };
        if let Some(old) = slot.replace(job) {
            let _ = old.reply.send(Err(Failure::Cancelled));
        }
    }
    fn pop(&mut self) -> Option<Job> {
        self.opens
            .pop_front()
            .or_else(|| self.current.take())
            .or_else(|| self.display.take())
            .or_else(|| self.prefetch.take())
    }
    fn is_empty(&self) -> bool {
        self.opens.is_empty()
            && self.current.is_none()
            && self.display.is_none()
            && self.prefetch.is_none()
    }
}

/// ページ（開く要求は文書）ごとの失敗の記録。
#[derive(Clone, Copy, Default)]
struct Strikes {
    crashes: u8,
    timeouts: u8,
}

/// 本体側の仲介スレッドの状態。補助プロセスとの送受信はこのスレッドだけが行う。
#[derive(Default)]
struct Service {
    helper: Option<Helper>,
    queue: Queue,
    strikes: HashMap<(Identity, Option<u32>), Strikes>,
    /// 処理の合間に行う、メモリー逼迫への対応（trueなら補助プロセスを止める）。
    trim: Option<bool>,
}

/// 届いたメッセージを反映する。補助プロセスとの送受信は行わない（処理中にも呼ぶため）。
fn accept(
    queue: &mut Queue,
    trim: &mut Option<bool>,
    strikes: &mut HashMap<(Identity, Option<u32>), Strikes>,
    message: Message,
) {
    match message {
        Message::Job(job) => queue.push(job),
        Message::Trim { aggressive } => *trim = Some(aggressive || trim.unwrap_or(false)),
        Message::Forget(identity) => strikes.retain(|(id, _), _| *id != identity),
    }
}

impl Service {
    fn run(mut self, input: mpsc::Receiver<Message>) {
        loop {
            if self.queue.is_empty() && self.trim.is_none() {
                match input.recv() {
                    Ok(message) => {
                        accept(&mut self.queue, &mut self.trim, &mut self.strikes, message)
                    }
                    Err(_) => return,
                }
            }
            while let Ok(message) = input.try_recv() {
                accept(&mut self.queue, &mut self.trim, &mut self.strikes, message);
            }
            match self.trim.take() {
                Some(true) => self.helper = None,
                Some(false) => {
                    if let Some(helper) = &self.helper
                        && !matches!(
                            helper.call(&Request::Trim, None, &mut || false),
                            Ok((Response::Trimmed, _))
                        )
                    {
                        self.helper = None;
                    }
                }
                None => {}
            }
            if let Some(job) = self.queue.pop()
                && !job.abandoned.load(Ordering::Relaxed)
            {
                let result = self.execute(&job, &input);
                let _ = job.reply.send(result);
            }
            HELPER_RSS.store(
                self.helper.as_ref().map_or(0, Helper::rss),
                Ordering::Relaxed,
            );
        }
    }

    fn execute(&mut self, job: &Job, input: &mpsc::Receiver<Message>) -> Result<Reply, Failure> {
        let identity = Identity::of(&job.file).ok_or(Failure::Io)?;
        let page = match job.request {
            Request::Render { page, .. } => Some(page),
            _ => None,
        };
        let key = (identity, page);
        let strikes = self.strikes.get(&key).copied().unwrap_or_default();
        if strikes.crashes >= CRASH_LIMIT {
            return Err(Failure::Crashed);
        }
        // 時間内に描けなかったページは、先読みと表示解像度の描き直しでは試さない。
        // 利用者が開いたページだけを、上限まで試し直す。
        let tries = match job.class {
            Class::Prefetch | Class::Display => 1,
            Class::Open | Class::Current => CRASH_LIMIT,
        };
        if strikes.timeouts >= tries {
            return Err(Failure::Timeout);
        }
        for attempt in 0..2 {
            // 待機中に終わっていた補助プロセス（OOM killer等）は、このページの失敗に数えない。
            if self.helper.as_mut().is_some_and(Helper::exited) {
                self.helper = None;
            }
            if self.helper.is_none() {
                self.helper = Some(Helper::start()?);
            }
            let Self {
                helper,
                queue,
                trim,
                strikes,
            } = self;
            let started = Instant::now();
            // 待つ人がいなくなった描画が長引き、別の要求が待っているなら、止めて先へ進む。
            let mut interrupt = || {
                while let Ok(message) = input.try_recv() {
                    accept(queue, trim, strikes, message);
                }
                job.abandoned.load(Ordering::Relaxed)
                    && !queue.is_empty()
                    && started.elapsed() >= ABANDON_AFTER
            };
            let result = helper.as_ref().expect("started above").call(
                &job.request,
                Some(&job.file),
                &mut interrupt,
            );
            match result {
                Ok((response, fds)) => return decode(response, fds, &job.request),
                // 送れなかった（待機中に終わっていた）ら、起動し直して1回だけ送り直す。
                Err(CallError::Send) if attempt == 0 => self.helper = None,
                Err(CallError::Interrupted) => {
                    self.helper = None;
                    return Err(Failure::Cancelled);
                }
                Err(CallError::Timeout) => {
                    self.helper = None;
                    self.strikes.entry(key).or_default().timeouts += 1;
                    return Err(Failure::Timeout);
                }
                Err(error) => {
                    if std::env::var_os("NAGAME_PROFILE").is_some() {
                        let detail = match &error {
                            CallError::Broken(detail) => detail.as_str(),
                            _ => "send failed",
                        };
                        eprintln!("pdf helper stopped: {detail}");
                    }
                    self.helper = None;
                    self.strikes.entry(key).or_default().crashes += 1;
                    return Err(Failure::Crashed);
                }
            }
        }
        Err(Failure::Crashed)
    }
}

/// 補助プロセスの応答を検査して、本体の値にする。補助プロセスは信頼しない
/// （PDFiumの脆弱性で乗っ取られた場合も、本体に過大な確保をさせない）。
fn decode(response: Response, fds: Vec<OwnedFd>, request: &Request) -> Result<Reply, Failure> {
    match (response, request) {
        (Response::Opened { pages }, Request::Open) => {
            if pages.is_empty() {
                return Err(Failure::Empty);
            }
            if pages.len() > MAX_PAGES
                || pages
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite() || *value <= 0.)
            {
                return Err(Failure::Corrupt);
            }
            Ok(Reply::Opened(pages))
        }
        (
            Response::Rendered {
                width,
                height,
                scale,
                vector,
                limited,
            },
            Request::Render { mode, .. },
        ) => {
            let bytes = u64::from(width) * u64::from(height) * 4;
            let sized = match *mode {
                Mode::Size {
                    width: w,
                    height: h,
                } => w == width && h == height,
                _ => true,
            };
            if width == 0
                || height == 0
                || width > MAX_SIDE
                || height > MAX_SIDE
                || bytes > mode.limit().max(4)
                || !sized
                || !scale.is_finite()
                || scale <= 0.
            {
                return Err(Failure::Page);
            }
            let memory = File::from(fds.into_iter().next().ok_or(Failure::Page)?);
            if memory.metadata().map_err(|_| Failure::Page)?.len() < bytes {
                return Err(Failure::Page);
            }
            let mut pixels = vec![0; bytes as usize];
            memory
                .read_exact_at(&mut pixels, 0)
                .map_err(|_| Failure::Page)?;
            let pixels = image::RgbaImage::from_raw(width, height, pixels).ok_or(Failure::Page)?;
            Ok(Reply::Rendered(Rendered {
                pixels,
                scale,
                vector,
                limited,
            }))
        }
        (Response::Failed(failure), _) => Err(failure),
        _ => Err(Failure::Corrupt),
    }
}

/// PDFiumの共有ライブラリー。環境変数（絶対パスのみ）、なければ実行ファイルから見た
/// `../lib/libpdfium.so`（AppImageと通常のインストール配置）。作業ディレクトリは探さない。
fn library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(LIBRARY_ENV) {
        let path = PathBuf::from(path);
        return path.is_absolute().then_some(path);
    }
    let executable = std::env::current_exe().ok()?;
    let candidate = executable.parent()?.join("../lib/libpdfium.so");
    candidate.is_file().then_some(candidate)
}

/// 補助プロセスのメモリー上限（RLIMIT_DATA）。RAMの1/4を1〜4 GiBに収める。
fn helper_limit() -> u64 {
    const GIB: u64 = 1024 * 1024 * 1024;
    crate::memory::system().map_or(2 * GIB, |system| (system.total / 4).clamp(GIB, 4 * GIB))
}

#[cfg(not(test))]
fn helper_command() -> Command {
    let mut command = Command::new("/proc/self/exe");
    command.arg(HELPER_ARG);
    command
}

/// テストでは、テストバイナリーの `pdf::tests::helper_process` を補助プロセスとして動かす。
#[cfg(test)]
fn helper_command() -> Command {
    let mut command = Command::new("/proc/self/exe");
    command
        .args([
            "--exact",
            "pdf::tests::helper_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("NAGAME_TEST_PDF_HELPER", "1");
    command
}

/// 補助プロセスとの1件のやり取りの失敗。
#[derive(Debug)]
enum CallError {
    /// 送れなかった（補助プロセスが既に終わっていた）。
    Send,
    /// 待つのをやめた（interruptがtrueを返した）。
    Interrupted,
    Timeout,
    /// 応答の途中で閉じた、または読めない応答。
    Broken(String),
}

/// 動いている補助プロセス。破棄すると停止させ、別のスレッドで回収する。
struct Helper {
    child: Option<Child>,
    socket: OwnedFd,
}
impl Helper {
    fn start() -> Result<Self, Failure> {
        Self::start_with(&library().ok_or(Failure::Library)?)
    }

    /// libraryのPDFiumを読み込む補助プロセスを起動し、準備完了の応答を待つ。
    fn start_with(library: &Path) -> Result<Self, Failure> {
        let (socket, remote) = ipc::pair().map_err(|_| Failure::Library)?;
        let raw = remote.as_raw_fd();
        let mut command = helper_command();
        command
            .env(LIBRARY_ENV, library)
            .env(HELPER_FD, "3")
            .env(HELPER_LIMIT, helper_limit().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        // SAFETY: fork後・exec前に、非同期シグナル安全なdup2とfcntlだけを呼ぶ。
        unsafe {
            command.pre_exec(move || {
                // dup2は複製先のCLOEXECを外す。同じ番号ならdup2は何もしないので明示的に外す。
                let result = if raw == 3 {
                    libc::fcntl(3, libc::F_SETFD, 0)
                } else {
                    libc::dup2(raw, 3)
                };
                if result < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|_| Failure::Library)?;
        drop(remote);
        let helper = Self {
            child: Some(child),
            socket,
        };
        match ipc::recv(&helper.socket, Some(Instant::now() + START_TIMEOUT)) {
            Ok((payload, _)) => match serde_json::from_slice(&payload) {
                Ok(Response::Ready) => Ok(helper),
                _ => Err(Failure::Library),
            },
            Err(_) => Err(Failure::Library),
        }
    }

    /// 1件を送り、応答を待つ。待つ間はPOLLごとにinterruptを呼び、trueなら待つのをやめる。
    fn call(
        &self,
        request: &Request,
        file: Option<&File>,
        interrupt: &mut dyn FnMut() -> bool,
    ) -> Result<(Response, Vec<OwnedFd>), CallError> {
        let payload = serde_json::to_vec(request).expect("requests serialize");
        let fds: Vec<_> = file.iter().map(|file| file.as_raw_fd()).collect();
        ipc::send(&self.socket, &payload, &fds).map_err(|_| CallError::Send)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if ipc::readable(&self.socket, POLL)
                .map_err(|error| CallError::Broken(error.to_string()))?
            {
                break;
            }
            if interrupt() {
                return Err(CallError::Interrupted);
            }
            if Instant::now() >= deadline {
                return Err(CallError::Timeout);
            }
        }
        let (payload, fds) =
            ipc::recv(&self.socket, Some(deadline)).map_err(|error| match error {
                ipc::RecvError::Timeout => CallError::Timeout,
                ipc::RecvError::Closed => CallError::Broken("closed".into()),
                ipc::RecvError::Io(error) => CallError::Broken(error.to_string()),
            })?;
        let response = serde_json::from_slice(&payload)
            .map_err(|error| CallError::Broken(error.to_string()))?;
        Ok((response, fds))
    }

    /// 補助プロセスが既に終わっている。
    fn exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
    }

    /// /proc/PID/statmの常駐ページ数から求めた常駐メモリー。
    fn rss(&self) -> u64 {
        let Some(child) = &self.child else {
            return 0;
        };
        std::fs::read_to_string(format!("/proc/{}/statm", child.id()))
            .ok()
            .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
            // SAFETY: sysconfは値を返すだけ。
            .map_or(0, |pages| {
                pages * unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64
            })
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // 入出力で止まったプロセスはSIGKILLでもすぐには終わらない。仲介スレッドを
            // 止めないよう、回収は別のスレッドで待つ。
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// テストは仲介スレッドを共有する。先読みの種類は最新の1件に置き換わるため、先読みの種類を
/// 使うテストはこれで順番に動かす。
#[cfg(test)]
pub static PREFETCH_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests;
