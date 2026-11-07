//! 本体と補助プロセスの間の通信。Unixドメインソケット（ストリーム）で、長さ付きの
//! メッセージとファイル記述子（SCM_RIGHTS）を送る。要求と応答は1件ずつ交互に流れる。

use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    time::Instant,
};

/// 1件のメッセージの上限。ページ寸法の一覧も収まる大きさで、壊れた長さで大量に確保しない。
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// 1件に付けるファイル記述子の上限。
const MAX_FDS: usize = 2;

#[derive(Debug)]
pub enum RecvError {
    /// 期限までに届かなかった。
    Timeout,
    /// 相手が閉じた（補助プロセスの終了を含む）。
    Closed,
    Io(io::Error),
}
impl From<io::Error> for RecvError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 互いにつながったソケットの組。どちらもexec時に閉じる（子へ渡す側は呼び出し元が複製する）。
pub fn pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: fdsは2要素の配列で、成功時はカーネルが有効な記述子を書き込む。
    let result = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socketpairが返した記述子は、ここで初めて所有者を持つ。
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// cmsghdrの境界に揃えた制御データの領域。
#[repr(C, align(8))]
struct Control([u8; 64]);

/// payloadを送る。fdsは最初の断片に付ける（受信側は同じメッセージの一部として受け取る）。
pub fn send(socket: &OwnedFd, payload: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if payload.len() > MAX_MESSAGE || fds.len() > MAX_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message too large",
        ));
    }
    let mut data = Vec::with_capacity(4 + payload.len());
    data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data.extend_from_slice(payload);
    let mut sent = 0;
    while sent < data.len() {
        let mut iov = libc::iovec {
            iov_base: data[sent..].as_ptr() as *mut libc::c_void,
            iov_len: data.len() - sent,
        };
        let mut control = Control([0; 64]);
        // SAFETY: 全フィールドを0にしたmsghdrは有効な初期値。
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if sent == 0 && !fds.is_empty() {
            let length = std::mem::size_of_val(fds) as u32;
            // SAFETY: controlは64バイトで、2個までの記述子のCMSG_SPACEより大きい。
            unsafe {
                message.msg_control = control.0.as_mut_ptr().cast();
                message.msg_controllen = libc::CMSG_SPACE(length) as _;
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(length) as _;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr(),
                    libc::CMSG_DATA(header).cast::<RawFd>(),
                    fds.len(),
                );
            }
        }
        // SAFETY: messageはこの関数内のiovとcontrolを指し、呼び出しの間は有効。
        let written = unsafe { libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        sent += written as usize;
    }
    Ok(())
}

/// 1件を受け取る。deadlineがあれば、それまでに揃わなければTimeout。
pub fn recv(
    socket: &OwnedFd,
    deadline: Option<Instant>,
) -> Result<(Vec<u8>, Vec<OwnedFd>), RecvError> {
    let mut fds = Vec::new();
    let mut header = [0u8; 4];
    read_exact(socket, &mut header, deadline, &mut fds)?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_MESSAGE {
        return Err(RecvError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        )));
    }
    let mut payload = vec![0; length];
    read_exact(socket, &mut payload, deadline, &mut fds)?;
    Ok((payload, fds))
}

fn read_exact(
    socket: &OwnedFd,
    buffer: &mut [u8],
    deadline: Option<Instant>,
    fds: &mut Vec<OwnedFd>,
) -> Result<(), RecvError> {
    let mut filled = 0;
    while filled < buffer.len() {
        wait_readable(socket, deadline)?;
        let mut iov = libc::iovec {
            iov_base: buffer[filled..].as_mut_ptr().cast(),
            iov_len: buffer.len() - filled,
        };
        let mut control = Control([0; 64]);
        // SAFETY: 全フィールドを0にしたmsghdrは有効な初期値。
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast();
        message.msg_controllen = control.0.len() as _;
        // SAFETY: messageはこの関数内のiovとcontrolを指し、呼び出しの間は有効。
        let read =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        // SAFETY: recvmsgが書いた制御データを、カーネルが設定した長さの範囲で読む。
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let data = libc::CMSG_DATA(header).cast::<RawFd>();
                    let bytes = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    for index in 0..bytes / std::mem::size_of::<RawFd>() {
                        fds.push(OwnedFd::from_raw_fd(data.add(index).read_unaligned()));
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 || fds.len() > MAX_FDS {
            return Err(RecvError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many file descriptors",
            )));
        }
        if read == 0 {
            return Err(RecvError::Closed);
        }
        filled += read as usize;
    }
    Ok(())
}

/// timeoutまでに読めるデータか終了（POLLHUP）が届けばtrue。
pub fn readable(socket: &OwnedFd, timeout: std::time::Duration) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollは1要素のpollfdだけを読み書きする。
    let ready = unsafe {
        libc::poll(
            &mut poll,
            1,
            timeout.as_millis().min(i32::MAX as u128) as i32,
        )
    };
    if ready < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(ready > 0)
}

fn wait_readable(socket: &OwnedFd, deadline: Option<Instant>) -> Result<(), RecvError> {
    loop {
        let timeout = match deadline {
            None => -1,
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(RecvError::Timeout);
                }
                left.as_millis().clamp(1, i32::MAX as u128) as i32
            }
        };
        let mut poll = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollは1要素のpollfdだけを読み書きする。
        let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if ready > 0 {
            // POLLHUPでも未読のデータは読める。読めなければrecvmsgが0を返す。
            return Ok(());
        }
    }
}
