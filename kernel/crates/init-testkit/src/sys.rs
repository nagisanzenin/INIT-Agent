//! The only module of the test kit's fakes that uses `unsafe`: thin wrappers over libc 0.2.191 for
//! what std does not offer on stable (root/root.md §3.5.3, kernel/supervise.md §3.13.1):
//! `SCM_RIGHTS` fd passing over a `SOCK_DGRAM` socketpair, `FD_CLOEXEC`, placing inherited fds at
//! 3 and 4 in a child, process-group signals, `waitid(WNOWAIT)` + `wait4` (exit status with
//! `rusage`), `getuid`, and signal dispositions.
//!
//! Live-system safety (CONVENTIONS §1, testing.md §3.1a): every signal sent through this module
//! targets a pid or process group the caller spawned and still holds. Nothing here scans the
//! process table.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// At most this many fds travel in one datagram (root/root.md §3.5.3).
pub const MAX_FDS: usize = 16;
/// Largest tag payload accepted on the fd channel.
pub const MAX_TAG: usize = 512;

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Set or clear `FD_CLOEXEC`.
pub fn set_cloexec(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: fcntl on an fd number; an invalid fd yields EBADF, never UB.
    unsafe {
        let flags = cvt(libc::fcntl(fd, libc::F_GETFD))?;
        let new = if on {
            flags | libc::FD_CLOEXEC
        } else {
            flags & !libc::FD_CLOEXEC
        };
        if new != flags {
            cvt(libc::fcntl(fd, libc::F_SETFD, new))?;
        }
    }
    Ok(())
}

/// `dup` an fd to a number ≥ `min`, with `FD_CLOEXEC` set.
pub fn dup_cloexec_above(fd: RawFd, min: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC returns a fresh fd that we own exclusively.
    unsafe {
        let n = cvt(libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, min))?;
        Ok(OwnedFd::from_raw_fd(n))
    }
}

/// Send `payload` plus `fds` as one datagram with an `SCM_RIGHTS` control message.
pub fn send_fds(sock: &UnixDatagram, payload: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many fds for one datagram",
        ));
    }
    let fd_bytes = std::mem::size_of_val(fds) as u32;
    // SAFETY: CMSG_SPACE/CMSG_LEN are pure arithmetic; the buffers outlive the sendmsg call and
    // the control buffer is sized with CMSG_SPACE and aligned to u64 (≥ cmsghdr alignment).
    unsafe {
        let space = libc::CMSG_SPACE(fd_bytes) as usize;
        let mut cbuf = vec![0u64; space.div_ceil(8).max(1)];
        let mut iov = libc::iovec {
            iov_base: payload.as_ptr() as *mut libc::c_void,
            iov_len: payload.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !fds.is_empty() {
            msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = space as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(fd_bytes) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr() as *const u8,
                libc::CMSG_DATA(c),
                fd_bytes as usize,
            );
        }
        loop {
            let r = libc::sendmsg(sock.as_raw_fd(), &msg, 0);
            if r >= 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// Receive one datagram with its fds (each set `FD_CLOEXEC`). `timeout` uses `SO_RCVTIMEO`;
/// an expired timeout returns `WouldBlock`/`TimedOut`.
pub fn recv_fds(
    sock: &UnixDatagram,
    timeout: Option<Duration>,
) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    sock.set_read_timeout(timeout.map(|t| t.max(Duration::from_millis(1))))?;
    let mut buf = vec![0u8; MAX_TAG];
    // SAFETY: recvmsg writes into buffers we own and size; the kernel reports the control length,
    // and we only read complete `int` fds from SCM_RIGHTS messages inside it.
    unsafe {
        let fd_space = (MAX_FDS * std::mem::size_of::<RawFd>()) as u32;
        let space = libc::CMSG_SPACE(fd_space) as usize;
        let mut cbuf = vec![0u64; space.div_ceil(8)];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        let n = loop {
            let r = libc::recvmsg(sock.as_raw_fd(), &mut msg, flags);
            if r >= 0 {
                break r as usize;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        };
        let mut fds = Vec::new();
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c);
                let header = data as usize - c as usize;
                let len = ((*c).cmsg_len as usize).saturating_sub(header);
                let count = len / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let mut fd: RawFd = -1;
                    std::ptr::copy_nonoverlapping(
                        data.add(i * std::mem::size_of::<RawFd>()),
                        &mut fd as *mut RawFd as *mut u8,
                        std::mem::size_of::<RawFd>(),
                    );
                    if fd >= 0 {
                        fds.push(OwnedFd::from_raw_fd(fd));
                    }
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        for f in &fds {
            set_cloexec(f.as_raw_fd(), true)?;
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fd datagram: control data truncated",
            ));
        }
        buf.truncate(n);
        Ok((buf, fds))
    }
}

/// Make `fd3` and `fd4` the child's fds 3 and 4 (without `FD_CLOEXEC`), as root/root.md §3.5.1
/// does for the kernel. The sources are first duplicated above 10 so the `dup2`s cannot clobber
/// each other; the caller keeps ownership of the returned duplicates until after `spawn`.
pub fn inherit_3_4(cmd: &mut Command, fd3: RawFd, fd4: RawFd) -> io::Result<(OwnedFd, OwnedFd)> {
    let a = dup_cloexec_above(fd3, 10)?;
    let b = dup_cloexec_above(fd4, 10)?;
    let (ra, rb) = (a.as_raw_fd(), b.as_raw_fd());
    // SAFETY: the closure runs in the forked child before exec and calls only dup2, which is
    // async-signal-safe. dup2 clears FD_CLOEXEC on the target.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(ra, 3) == -1 || libc::dup2(rb, 4) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((a, b))
}

/// Start the child in a new session (`setsid`): the `escape_setsid` fake modes, disposable only.
pub fn new_session(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// `kill(pid, sig)` on a pid the caller spawned and holds.
pub fn kill(pid: i32, sig: i32) -> io::Result<()> {
    if pid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal pid <= 1",
        ));
    }
    // SAFETY: plain syscall.
    cvt(unsafe { libc::kill(pid, sig) }).map(|_| ())
}

/// `killpg(pgid, sig)` on a process group the caller created (its leader's pid) and holds.
pub fn killpg(pgid: i32, sig: i32) -> io::Result<()> {
    if pgid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to signal process group <= 1",
        ));
    }
    // SAFETY: plain syscall.
    cvt(unsafe { libc::killpg(pgid, sig) }).map(|_| ())
}

/// Block until child `pid` has exited, **without reaping it** (`waitid(WEXITED | WNOWAIT)`): the
/// zombie keeps the pid and the process-group id reserved, so the caller can still signal the
/// instance's group safely before reaping.
pub fn wait_exit_nowait(pid: i32) -> io::Result<()> {
    loop {
        // SAFETY: siginfo_t is plain data, zero-initialised and written by the kernel.
        let r = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// How a reaped child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// `ru_utime + ru_stime`, milliseconds.
    pub rusage_ms: u64,
}

/// Reap child `pid` with `wait4`, returning its status and CPU time.
pub fn reap(pid: i32) -> io::Result<ExitInfo> {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: rusage is plain data written by the kernel.
        let (r, ru) = unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            let r = libc::wait4(pid, &mut status, 0, &mut ru);
            (r, ru)
        };
        if r == pid {
            let ms = |t: libc::timeval| (t.tv_sec as u64) * 1000 + (t.tv_usec as u64) / 1000;
            let (code, signal) = if libc::WIFEXITED(status) {
                (Some(libc::WEXITSTATUS(status)), None)
            } else if libc::WIFSIGNALED(status) {
                (None, Some(libc::WTERMSIG(status)))
            } else {
                (None, None)
            };
            return Ok(ExitInfo {
                code,
                signal,
                rusage_ms: ms(ru.ru_utime) + ms(ru.ru_stime),
            });
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// The real uid of this process.
pub fn getuid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

/// Ignore `SIGTERM` in this process (the hostile `ignore_term` kernel mode).
pub fn ignore_sigterm() {
    // SAFETY: setting a disposition to SIG_IGN has no handler code to be unsafe.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

static SIG_USR1: AtomicBool = AtomicBool::new(false);
static SIG_TERM: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(sig: libc::c_int) {
    // Async-signal-safe: one atomic store.
    if sig == libc::SIGUSR1 {
        SIG_USR1.store(true, Ordering::SeqCst);
    } else {
        SIG_TERM.store(true, Ordering::SeqCst);
    }
}

/// Install flag-setting handlers for `SIGUSR1` (panic channel) and `SIGTERM`/`SIGINT` (orderly
/// shutdown). Poll with [`take_usr1`] and [`term_requested`].
pub fn install_flag_handlers() {
    // SAFETY: the handler only stores to atomics, which is async-signal-safe.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        for s in [libc::SIGUSR1, libc::SIGTERM, libc::SIGINT] {
            libc::sigaction(s, &sa, std::ptr::null_mut());
        }
    }
}

/// True once after each `SIGUSR1`.
pub fn take_usr1() -> bool {
    SIG_USR1.swap(false, Ordering::SeqCst)
}

/// True after `SIGTERM` or `SIGINT`.
pub fn term_requested() -> bool {
    SIG_TERM.load(Ordering::SeqCst)
}

/// Signal numbers by name, for scenario steps.
pub fn signal_by_name(name: &str) -> Option<i32> {
    Some(
        match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
            "TERM" => libc::SIGTERM,
            "KILL" => libc::SIGKILL,
            "INT" => libc::SIGINT,
            "USR1" => libc::SIGUSR1,
            "HUP" => libc::SIGHUP,
            "STOP" => libc::SIGSTOP,
            "CONT" => libc::SIGCONT,
            _ => return None,
        },
    )
}

pub use libc::{SIGKILL, SIGTERM, SIGUSR1};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn fd_passing_round_trip() {
        let (a, b) = UnixDatagram::pair().unwrap();
        let (r, mut w) = std::io::pipe().unwrap();
        send_fds(&a, br#"{"id":"k-1","n":1}"#, &[r.as_raw_fd()]).unwrap();
        drop(r);
        let (tag, fds) = recv_fds(&b, Some(Duration::from_secs(2))).unwrap();
        assert_eq!(tag, br#"{"id":"k-1","n":1}"#);
        assert_eq!(fds.len(), 1);
        w.write_all(b"hello").unwrap();
        drop(w);
        let mut f = std::fs::File::from(fds.into_iter().next().unwrap());
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello");
        // A datagram without fds still carries its tag.
        send_fds(&a, b"plain", &[]).unwrap();
        let (tag, fds) = recv_fds(&b, Some(Duration::from_secs(2))).unwrap();
        assert_eq!((tag.as_slice(), fds.len()), (&b"plain"[..], 0));
        // Timeout.
        let e = recv_fds(&b, Some(Duration::from_millis(20))).unwrap_err();
        assert!(
            matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ),
            "{e:?}"
        );
    }

    // The children are reaped by pid with `reap` (wait4), which clippy cannot see.
    #[allow(clippy::zombie_processes)]
    #[test]
    fn reap_reports_code_and_signal() {
        let c = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let pid = c.id() as i32;
        wait_exit_nowait(pid).unwrap();
        let e = reap(pid).unwrap();
        assert_eq!((e.code, e.signal), (Some(7), None));
        let c = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = c.id() as i32;
        kill(pid, SIGKILL).unwrap();
        let e = reap(pid).unwrap();
        assert_eq!((e.code, e.signal), (None, Some(SIGKILL)));
        assert!(kill(1, 0).is_err() && killpg(0, 0).is_err());
    }
}
