//! The only module of this crate that uses `unsafe`: fd passing over the control link's fd
//! channel (`SCM_RIGHTS`, root/root.md §3.5.3), `FD_CLOEXEC`, `statvfs`, `clonefile(2)`,
//! `/dev/urandom`, the passwd home, the machine's memory size and `fchmodat` without following a link.
//!
//! Everything else in the crate is safe Rust over std.

use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// At most this many fds travel in one datagram (root/root.md §3.5.3).
pub const MAX_FDS: usize = 16;
/// A tag payload is a short ICJ object (`{"id":"k-42","n":3}`).
const MAX_TAG: usize = 512;

/// Set `FD_CLOEXEC` on an fd (the kernel does this for fds 3 and 4 at start, §3.13.1).
pub fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on an fd number; failure is reported through the return value.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    let r = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// True when `fd` is open.
pub fn fd_is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only queries.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

/// Take ownership of an inherited fd number (fds 3 and 4).
///
/// # Safety contract
/// The caller guarantees `fd` is open and owned by nobody else in this process.
pub fn own_inherited_fd(fd: RawFd) -> io::Result<OwnedFd> {
    if !fd_is_open(fd) {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("fd {fd} is not open")));
    }
    // SAFETY: checked open above; the caller hands over exclusive ownership.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Send one datagram with `payload` and `fds` as `SCM_RIGHTS` (used by test fakes of the Root).
pub fn send_fds(sock: BorrowedFd<'_>, payload: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many fds"));
    }
    let n = fds.len();
    let space = if n == 0 { 0 } else { (unsafe { libc::CMSG_SPACE((n * std::mem::size_of::<RawFd>()) as u32) }) as usize };
    let mut cbuf = vec![0u8; space.max(1)];
    let mut iov = libc::iovec { iov_base: payload.as_ptr() as *mut libc::c_void, iov_len: payload.len() };
    // SAFETY: zeroed msghdr is a valid initial value.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if n > 0 {
        msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = space as _;
        // SAFETY: the control buffer is CMSG_SPACE(n fds) bytes, so the first header fits.
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN((n * std::mem::size_of::<RawFd>()) as u32) as _;
            let data = libc::CMSG_DATA(c) as *mut RawFd;
            for (i, fd) in fds.iter().enumerate() {
                std::ptr::write_unaligned(data.add(i), fd.as_raw_fd());
            }
        }
    }
    // SAFETY: msg points at live buffers for the duration of the call.
    let r = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Receive one datagram: its payload and the fds it carried. Blocks up to `timeout`
/// (`None` = forever). Every received fd gets `FD_CLOEXEC` at once (macOS has no
/// `MSG_CMSG_CLOEXEC`). `Ok(None)` on timeout.
pub fn recv_fds(sock: BorrowedFd<'_>, timeout: Option<Duration>) -> io::Result<Option<(Vec<u8>, Vec<OwnedFd>)>> {
    if let Some(t) = timeout {
        let ms = t.as_millis().min(i32::MAX as u128) as libc::c_int;
        let mut p = libc::pollfd { fd: sock.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        loop {
            // SAFETY: one pollfd on the stack.
            let r = unsafe { libc::poll(&mut p, 1, ms) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if r == 0 {
                return Ok(None);
            }
            break;
        }
    }
    let mut buf = vec![0u8; MAX_TAG];
    let space = (unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) }) as usize;
    let mut cbuf = vec![0u8; space];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
    // SAFETY: zeroed msghdr is a valid initial value.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = space as _;
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let n = loop {
        // SAFETY: buffers live for the call.
        let r = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, flags) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        break r as usize;
    };
    if n == 0 && msg.msg_controllen == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "fd channel closed"));
    }
    buf.truncate(n);
    let mut fds = Vec::new();
    // SAFETY: walking the control messages the kernel filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c) as *const RawFd;
                let hdr = libc::CMSG_LEN(0) as usize;
                let count = ((*c).cmsg_len as usize).saturating_sub(hdr) / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    let fd = std::ptr::read_unaligned(data.add(i));
                    let _ = set_cloexec(fd);
                    fds.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    Ok(Some((buf, fds)))
}

/// Free bytes available to an unprivileged user on the filesystem holding `path`.
pub fn free_bytes(path: &Path) -> io::Result<u64> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: zeroed statvfs is a valid out-buffer.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and out-pointer.
    let r = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

/// `n` random bytes from `/dev/urandom`, as lowercase hex.
pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !ok {
        // Fallback: time and pid mixed through sha256 (never expected on macOS or Linux).
        use sha2::Digest;
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let h = sha2::Sha256::digest(format!("{t}:{}", std::process::id()).as_bytes());
        for (i, x) in b.iter_mut().enumerate() {
            *x = h[i % 32];
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A copy-on-write clone of a file or a directory tree. macOS: one `clonefile(2)` call
/// (`CLONE_NOFOLLOW`, so a symlink is cloned as a link); the destination must not exist.
/// Elsewhere: `cp -a --reflink=auto`.
pub fn clone_path(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let s = CString::new(src.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        let d = CString::new(dst.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        const CLONE_NOFOLLOW: u32 = 0x0001;
        // SAFETY: two valid C strings.
        let r = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // Not APFS (or a cross-device clone): fall back to a real copy.
        if e.raw_os_error() != Some(libc::ENOTSUP) && e.raw_os_error() != Some(libc::EXDEV) {
            return Err(e);
        }
    }
    let st = std::process::Command::new("/bin/cp")
        .args(if cfg!(target_os = "linux") { &["-a", "--reflink=auto"][..] } else { &["-pPR"][..] })
        .arg(src)
        .arg(dst)
        .env_clear()
        .status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other(format!("cp {} → {} failed: {st}", src.display(), dst.display()))) }
}

/// `fchmodat(path, mode, AT_SYMLINK_NOFOLLOW)`: a symlink at `path` is never followed (the probe
/// root's delete, X0d). Linux has no such mode for non-links (`EOPNOTSUPP`); a plain `chmod` is used
/// there, on a path the caller has just typed as a directory with `lstat`.
pub fn chmod_nofollow(path: &Path, mode: u32) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: a valid C string.
    let r = unsafe { libc::fchmodat(libc::AT_FDCWD, c.as_ptr(), mode as libc::mode_t, libc::AT_SYMLINK_NOFOLLOW) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if cfg!(target_os = "linux") && e.raw_os_error() == Some(libc::EOPNOTSUPP) {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Err(e)
}

/// The owner's home from the passwd entry (`HOME_REAL`, kernel/sandbox.md §3.3.2).
pub fn passwd_home() -> Option<PathBuf> {
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: zeroed passwd is a valid out-buffer.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buffers live for the call; getuid never fails.
    let r = unsafe { libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if r != 0 || out.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into buf, NUL-terminated.
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

/// Total physical memory in MiB (`boot.json.env.mem_total_mb`); 0 when unknown.
pub fn mem_total_mb() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        // SAFETY: valid name and out-buffer of the right size.
        let r = unsafe { libc::sysctlbyname(name.as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
        if r == 0 { v / (1024 * 1024) } else { 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }
}

/// `uname` release and machine (`boot.json.env.os`).
pub fn uname() -> (String, String) {
    // SAFETY: zeroed utsname is a valid out-buffer.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer.
    if unsafe { libc::uname(&mut u) } != 0 {
        return (String::new(), String::new());
    }
    let s = |a: &[libc::c_char]| {
        // SAFETY: utsname fields are NUL-terminated within their arrays.
        unsafe { std::ffi::CStr::from_ptr(a.as_ptr()) }.to_string_lossy().into_owned()
    };
    (s(&u.release), s(&u.machine))
}

/// A `socketpair(AF_UNIX, SOCK_DGRAM)` as two owned fds (std's `UnixDatagram::pair` is the
/// same; kept here so tests and the fake Root need no extra plumbing).
pub fn dgram_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let (a, b) = std::os::unix::net::UnixDatagram::pair()?;
    Ok((OwnedFd::from(a), OwnedFd::from(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsFd;

    #[test]
    fn fds_round_trip_over_a_datagram_pair() {
        let (a, b) = dgram_pair().unwrap();
        let (r, mut w) = std::io::pipe().unwrap();
        send_fds(a.as_fd(), br#"{"id":"k-1","n":1}"#, &[r.as_fd()]).unwrap();
        drop(r);
        let (payload, fds) = recv_fds(b.as_fd(), Some(Duration::from_secs(2))).unwrap().unwrap();
        assert_eq!(payload, br#"{"id":"k-1","n":1}"#);
        assert_eq!(fds.len(), 1);
        w.write_all(b"hi").unwrap();
        drop(w);
        let mut got = String::new();
        std::fs::File::from(fds.into_iter().next().unwrap()).read_to_string(&mut got).unwrap();
        assert_eq!(got, "hi");
        assert!(recv_fds(b.as_fd(), Some(Duration::from_millis(20))).unwrap().is_none());
    }

    #[test]
    fn small_helpers() {
        assert_eq!(random_hex(8).len(), 16);
        assert_ne!(random_hex(8), random_hex(8));
        assert!(free_bytes(Path::new("/")).unwrap() > 0);
        assert!(passwd_home().is_some());
        let (rel, mach) = uname();
        assert!(!rel.is_empty() && !mach.is_empty());
    }
}
