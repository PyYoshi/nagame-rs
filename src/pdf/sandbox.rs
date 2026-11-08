//! 補助プロセスの権限を絞る。PDFiumとフォントの準備（ライブラリーと設定の読み込み）の後に呼ぶ。
//!
//! - Landlock: ファイルの読み取りをフォントのディレクトリーだけにし、書き込み・実行・作成と
//!   TCPの接続・待ち受け、外部へのシグナルを禁止する。PDFは本体が開いた記述子で受け取るため、
//!   パスで開く必要がない。
//! - seccomp: プロセスの生成・プログラムの実行・新しいソケット・カーネル管理系の呼び出しを、
//!   エラー（EPERM）で拒否する。スレッドの作成は許す。
//!
//! どちらもカーネルの対応に依存する。使えない環境では制限なしで続ける。

use std::{os::fd::AsRawFd, path::Path};

/// 掛けた制限。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// 使えたLandlockのABIの版。
    pub landlock: Option<u32>,
    pub seccomp: bool,
}

/// readableのディレクトリー（の中）だけを読み取り可能にして、制限を掛ける。
pub fn restrict(readable: &[&Path]) -> Report {
    // SAFETY: 自分のプロセスの属性を変えるだけ。LandlockとseccompはこれをNo New Privsを前提にする。
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
    }
    Report {
        landlock: landlock(readable),
        seccomp: seccomp(),
    }
}

// linux/landlock.hの定義。
const CREATE_RULESET_VERSION: u32 = 1;
const RULE_PATH_BENEATH: u32 = 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;
/// ABIの版ごとに扱えるファイルの権限（1: 実行〜シンボリックリンク作成、2: 移動、3: 切り詰め、
/// 5: デバイスのioctl）。
const FS_BY_ABI: [(u32, u64); 4] = [(1, (1 << 13) - 1), (2, 1 << 13), (3, 1 << 14), (5, 1 << 15)];
/// ABI 4: TCPの待ち受けと接続。
const NET_TCP: u64 = 0b11;
/// ABI 6: 抽象Unixソケットへの接続とシグナル送信を、同じ制限の中に限る。
const SCOPE_ALL: u64 = 0b11;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

fn landlock(readable: &[&Path]) -> Option<u32> {
    // SAFETY: 版の問い合わせは属性を渡さず、版の番号を返す。
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    let abi = u32::try_from(abi).ok().filter(|abi| *abi >= 1)?;
    let handled = FS_BY_ABI
        .iter()
        .filter(|(since, _)| abi >= *since)
        .fold(0, |access, (_, bits)| access | bits);
    let attr = RulesetAttr {
        handled_access_fs: handled,
        handled_access_net: if abi >= 4 { NET_TCP } else { 0 },
        scoped: if abi >= 6 { SCOPE_ALL } else { 0 },
    };
    // 古いカーネルは知らない末尾のフィールドが0なら、大きい構造体も受け付ける。
    // SAFETY: attrはこの呼び出しの間有効。
    let ruleset = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr,
            std::mem::size_of::<RulesetAttr>(),
            0u32,
        )
    };
    let ruleset = i32::try_from(ruleset).ok().filter(|fd| *fd >= 0)?;
    // SAFETY: 作ったルールセットの記述子を所有する。
    let ruleset = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(ruleset) };
    let mut allowed = 0;
    for directory in readable {
        let Ok(directory) = std::fs::File::open(directory) else {
            continue;
        };
        let rule = PathBeneathAttr {
            allowed_access: (FS_READ_FILE | FS_READ_DIR) & handled,
            parent_fd: directory.as_raw_fd(),
        };
        // SAFETY: ruleはこの呼び出しの間有効。
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                RULE_PATH_BENEATH,
                &rule,
                0u32,
            )
        };
        allowed += usize::from(result == 0);
    }
    // フォントを1つも読めなくなる制限は掛けない（埋め込みのない文字が描けなくなる）。
    if allowed == 0 && !readable.is_empty() {
        return None;
    }
    // SAFETY: このスレッド（補助プロセスの唯一のスレッド）に制限を掛ける。
    let result =
        unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0u32) };
    (result == 0).then_some(abi)
}

/// BPFの命令（linux/filter.hのsock_filter）。
#[repr(C)]
#[derive(Clone, Copy)]
struct Filter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct Program {
    len: u16,
    filter: *const Filter,
}

const LOAD: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
const JGE: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
const JSET: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
const RET: u16 = 0x06; // BPF_RET | BPF_K
/// struct seccomp_dataの位置。
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const OFFSET_ARG0: u32 = 16;
#[cfg(target_arch = "x86_64")]
const ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const ARCH: u32 = 0xC000_00B7;
const ALLOW: u32 = 0x7fff_0000;
const ERRNO: u32 = 0x0005_0000;

/// 拒否するシステムコール。プロセスの生成と実行、デバッグと他のプロセスの操作（pidfdで
/// 本体の記述子を取り出すことを含む）、新しいソケット、名前空間・マウント・モジュール・
/// カーネル機能の操作、io_uring。
fn denied() -> Vec<libc::c_long> {
    let mut calls = vec![
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_send_signal,
        libc::SYS_process_madvise,
        libc::SYS_kcmp,
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_personality,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
    ];
    #[cfg(target_arch = "x86_64")]
    calls.extend([libc::SYS_fork, libc::SYS_vfork, libc::SYS_kexec_file_load]);
    calls
}

/// seccompのフィルター。対応しないアーキテクチャーではNone。
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn program() -> Vec<Filter> {
    let op = |code, k, jt, jf| Filter { code, jt, jf, k };
    let deny = |errno: i32| op(RET, ERRNO | errno as u32, 0, 0);
    // 想定外のアーキテクチャー（x86_64上の32ビット呼び出し等）は番号が違うため、すべて拒否する。
    let mut program = vec![
        op(LOAD, OFFSET_ARCH, 0, 0),
        op(JEQ, ARCH, 1, 0),
        deny(libc::EPERM),
        op(LOAD, OFFSET_NR, 0, 0),
    ];
    #[cfg(target_arch = "x86_64")]
    program.extend([op(JGE, 0x4000_0000, 0, 1), deny(libc::EPERM)]);
    for call in denied() {
        program.extend([op(JEQ, call as u32, 0, 1), deny(libc::EPERM)]);
    }
    // clone3は引数を調べられないので、glibcがcloneへ切り替えるようENOSYSを返す。
    program.extend([
        op(JEQ, libc::SYS_clone3 as u32, 0, 1),
        deny(libc::ENOSYS),
        // cloneはスレッドの作成（CLONE_THREAD）だけを許す。
        op(JEQ, libc::SYS_clone as u32, 0, 3),
        op(LOAD, OFFSET_ARG0, 0, 0),
        op(JSET, libc::CLONE_THREAD as u32, 1, 0),
        deny(libc::EPERM),
        op(RET, ALLOW, 0, 0),
    ]);
    program
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn seccomp() -> bool {
    let program = program();
    let fprog = Program {
        len: program.len() as u16,
        filter: program.as_ptr(),
    };
    // SAFETY: fprogはこの呼び出しの間有効。No New Privsは設定済み。
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0u32,
            &fprog,
        )
    };
    result == 0
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn seccomp() -> bool {
    false
}

/// テストで、制限が効いているかを補助プロセスの中で確かめる。
#[cfg(test)]
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Probe {
    pub report: Report,
    /// フォントのディレクトリー外のファイルを読めない。
    pub read_denied: bool,
    pub fork_denied: bool,
    pub socket_denied: bool,
    /// 本体のpidfdを開けない（本体の記述子を取り出せない）。
    pub pidfd_denied: bool,
}

#[cfg(test)]
pub fn probe(report: Report) -> Probe {
    let read_denied = std::fs::read("/proc/self/status").is_err();
    // SAFETY: 拒否されなければ子は直ちに終了する。
    let fork_denied = unsafe {
        let pid = libc::fork();
        if pid == 0 {
            libc::_exit(0);
        }
        if pid > 0 {
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        pid < 0
    };
    // SAFETY: 作れたら閉じる。
    let socket_denied = unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd >= 0 {
            libc::close(fd);
        }
        fd < 0
    };
    // SAFETY: 開けたら閉じる。
    let pidfd_denied = unsafe {
        let fd = libc::syscall(libc::SYS_pidfd_open, libc::getppid(), 0u32);
        if fd >= 0 {
            libc::close(fd as i32);
        }
        fd < 0
    };
    Probe {
        report,
        read_denied,
        fork_denied,
        socket_denied,
        pidfd_denied,
    }
}
