//! Peer authentication (kernel/initctl.md §3.3).
//!
//! On every `accept()`, before any request is read:
//! 1. read the peer's credentials (`getpeereid` + `LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux);
//! 2. require `uid == geteuid()`, else close silently (done by the server);
//! 3. attribute the peer to the lane's instance ([`PeerPolicy`]);
//! 4. on failure send `E_PEER` with a null id, close, and log `{pid, lane, reason}`.

use crate::backend::LaneCtx;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// A connected peer's credentials as of `connect()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
}

/// The peer of a connected Unix stream (initctl.md §3.3, verbatim).
#[cfg(target_vendor = "apple")]
pub fn peer_of(s: &UnixStream) -> io::Result<Peer> {
    let fd = s.as_raw_fd();
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: `fd` is a live socket owned by `s`; both out-pointers are valid for writes.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one `pid_t` into the buffer of `len` bytes.
    let rc = unsafe {
        libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len)
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer { uid, gid, pid })
}

/// The peer of a connected Unix stream (initctl.md §3.3, verbatim).
#[cfg(target_os = "linux")]
pub fn peer_of(s: &UnixStream) -> io::Result<Peer> {
    // SAFETY: an all-zero `ucred` is a valid value.
    let mut c: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one `ucred` into the buffer of `len` bytes.
    let rc = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut c as *mut libc::ucred).cast(), &mut len)
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer { uid: c.uid, gid: c.gid, pid: c.pid })
}

/// The kernel's effective uid.
pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Step 3 of §3.3: attribute an accepted, same-uid peer to the lane's instance.
pub trait PeerPolicy: Send + Sync + 'static {
    /// `marks` are the instance marks of the lane's current instances (the backend's
    /// [`crate::Backend::marks`]; two during an overlapped handoff). `Err` carries the reason
    /// that is logged; the client sees only `E_PEER`.
    fn attribute(&self, lane: &LaneCtx, marks: &[PathBuf], peer: &Peer) -> Result<(), String>;
}

/// The production policy.
///
/// - **macOS:** the three-condition rule (D1; supervise.md §3.8.2): the peer is sandboxed, may
///   read one of the lane's instance marks, and is denied the Root's canary.
/// - **Linux:** attribution by socket path (each instance's mount namespace holds only its own
///   lane sockets, sandbox.md §3.6.1); the `SO_PEERCRED` pid is only logged.
#[derive(Clone, Debug)]
pub struct SandboxPolicy {
    /// `ROOT_HOME/run/canary`.
    pub canary: PathBuf,
}

impl PeerPolicy for SandboxPolicy {
    #[cfg(target_os = "macos")]
    fn attribute(&self, _lane: &LaneCtx, marks: &[PathBuf], peer: &Peer) -> Result<(), String> {
        attribute_macos(peer.pid, marks, &self.canary)
    }

    #[cfg(not(target_os = "macos"))]
    fn attribute(&self, _lane: &LaneCtx, _marks: &[PathBuf], _peer: &Peer) -> Result<(), String> {
        Ok(())
    }
}

/// The three conditions for `pid` (fail closed when `sandbox_check` is unavailable).
#[cfg(target_os = "macos")]
pub fn attribute_macos(pid: i32, marks: &[PathBuf], canary: &std::path::Path) -> Result<(), String> {
    match sbcheck::sandboxed(pid) {
        None => return Err("sandbox_check is unavailable".into()),
        Some(1) => {}
        Some(_) => return Err("peer is not sandboxed".into()),
    }
    if marks.is_empty() {
        return Err("the lane has no live instance mark".into());
    }
    let reads_mark = marks.iter().any(|m| sbcheck::path(pid, "file-read-data", m) == Some(0));
    if !reads_mark {
        return Err("peer cannot read the lane's instance mark".into());
    }
    match sbcheck::path(pid, "file-read-data", canary) {
        Some(0) => Err("peer can read the Root canary".into()),
        Some(_) => Ok(()),
        None => Err("sandbox_check is unavailable".into()),
    }
}

/// Same-uid only, no instance attribution. **Only** for tests and disposable environments
/// where no sandbox runs (the uid check of step 2 still applies).
#[derive(Clone, Copy, Debug, Default)]
pub struct SameUidPolicy;

impl PeerPolicy for SameUidPolicy {
    fn attribute(&self, _lane: &LaneCtx, _marks: &[PathBuf], _peer: &Peer) -> Result<(), String> {
        Ok(())
    }
}

/// `sandbox_check(3)`: private libsystem_sandbox API, resolved with `dlsym` (kernel/sandbox.md §3.8;
/// the same binding as root/sandbox-profiles).
#[cfg(target_os = "macos")]
pub mod sbcheck {
    use std::ffi::{CString, c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// `int sandbox_check(pid_t, const char *operation, enum sandbox_filter_type, ...)`.
    /// Variadic: on arm64 Apple the extra argument goes on the stack, so the type must say so.
    type SandboxCheck = unsafe extern "C" fn(libc::pid_t, *const c_char, c_int, ...) -> c_int;

    /// `SANDBOX_FILTER_PATH` in `enum sandbox_filter_type`.
    const SANDBOX_FILTER_PATH: c_int = 1;

    fn resolve() -> Option<(SandboxCheck, c_int)> {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name only looks a symbol up.
        let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()) };
        if f.is_null() {
            return None;
        }
        // SAFETY: the symbol `sandbox_check` has exactly the variadic C signature above.
        let f: SandboxCheck = unsafe { std::mem::transmute::<*mut libc::c_void, SandboxCheck>(f) };
        // SAFETY: as above; `SANDBOX_CHECK_NO_REPORT` is an exported `const` enum variable.
        let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"SANDBOX_CHECK_NO_REPORT".as_ptr()) };
        // SAFETY: when present the symbol is a readable, aligned `const int`.
        let no_report = if p.is_null() { 0 } else { unsafe { *(p as *const c_int) } };
        Some((f, no_report))
    }

    /// `sandbox_check(pid, NULL, 0)`: 1 when `pid` is sandboxed.
    pub fn sandboxed(pid: i32) -> Option<c_int> {
        let (f, _) = resolve()?;
        // SAFETY: NULL operation with filter type 0 reads no variadic argument.
        Some(unsafe { f(pid, std::ptr::null(), 0) })
    }

    /// `sandbox_check(pid, op, SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, path)`: 0 when
    /// `pid`'s policy allows `op` on `path`. A pure query. A missing path reads as "denied".
    pub fn path(pid: i32, op: &str, path: &Path) -> Option<c_int> {
        let (f, no_report) = resolve()?;
        let op = CString::new(op).ok()?;
        let p = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: both strings are NUL-terminated and outlive the call; SANDBOX_FILTER_PATH
        // reads exactly one `const char *` variadic argument.
        Some(unsafe { f(pid, op.as_ptr(), SANDBOX_FILTER_PATH | no_report, p.as_ptr()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use init_proto::initctl::Lane;
    use std::os::unix::net::UnixStream;

    #[test]
    fn socketpair_peer_is_us() {
        let (a, _b) = UnixStream::pair().unwrap();
        let p = peer_of(&a).unwrap();
        assert_eq!(p.uid, euid());
        assert_eq!(p.pid, std::process::id() as i32);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unsandboxed_peer_is_refused() {
        // This test process is not sandboxed: condition (a) fails, fail closed.
        let lane = LaneCtx::live();
        let r = SandboxPolicy { canary: "/nonexistent/canary".into() }.attribute(&lane, &[PathBuf::from("/tmp")], &Peer {
            uid: euid(),
            gid: 0,
            pid: std::process::id() as i32,
        });
        assert_eq!(r, Err("peer is not sandboxed".to_string()));
        assert_eq!(lane.lane, Lane::Live);
    }
}
