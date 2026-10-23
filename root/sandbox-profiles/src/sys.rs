//! FFI used by the probe and the self-test runner. The only module allowed to use `unsafe`.
//!
//! Signals: [`kill`] is called only with a PID the caller spawned itself or was handed by its own
//! spawner (CONVENTIONS §1). Nothing here scans or matches processes.
#![allow(unsafe_code)]

use std::ffi::c_int;
use std::io;

/// `kill(pid, sig)`. `sig = 0` sends nothing and only checks permission.
pub fn kill(pid: i32, sig: c_int) -> io::Result<()> {
    if pid <= 0 {
        // Refuse process-group and broadcast forms outright.
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: plain syscall wrapper; pid > 0 so it addresses exactly one process.
    let rc = unsafe { libc::kill(pid, sig) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Create a socket of the given domain and type and close it again.
pub fn socket_probe(domain: c_int, ty: c_int) -> io::Result<()> {
    // SAFETY: socket(2) takes no pointers; the returned fd is closed below.
    let fd = unsafe { libc::socket(domain, ty, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a descriptor this function just opened and owns.
    unsafe { libc::close(fd) };
    Ok(())
}

/// One UDP datagram to 127.0.0.1:`port` from an unbound socket (the bind is implicit in
/// `sendto`, so only `network-outbound` is exercised).
pub fn udp_send_loopback(port: u16) -> io::Result<()> {
    // SAFETY: socket(2) takes no pointers.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: an all-zero sockaddr_in is valid; the fields are set below.
    let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    #[cfg(target_os = "macos")]
    {
        sa.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
    }
    sa.sin_family = libc::AF_INET as libc::sa_family_t;
    sa.sin_port = port.to_be();
    sa.sin_addr = libc::in_addr { s_addr: u32::from_be_bytes([127, 0, 0, 1]).to_be() };
    let msg = b"wp6a";
    // SAFETY: `msg` and `sa` are live locals; the length arguments match their sizes.
    let rc = unsafe {
        libc::sendto(
            fd,
            msg.as_ptr().cast(),
            msg.len(),
            0,
            (&sa as *const libc::sockaddr_in).cast(),
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    let r = if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) };
    // SAFETY: fd was opened above and is owned here.
    unsafe { libc::close(fd) };
    r
}

/// The open file descriptors below `limit` (F_GETFD succeeds).
pub fn open_fds(limit: c_int) -> Vec<c_int> {
    (0..limit)
        // SAFETY: F_GETFD only queries the descriptor table; invalid fds return -1 (EBADF).
        .filter(|&fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } != -1)
        .collect()
}

/// The size of the descriptor table.
pub fn dtablesize() -> c_int {
    // SAFETY: no arguments, no side effects.
    unsafe { libc::getdtablesize() }
}

/// Runs in a forked child before `exec` (via `CommandExt::pre_exec`): mark every fd ≥ 3 below
/// `limit` close-on-exec and clear the signal mask. This is what the Root's `exec-clean` does
/// (root/root.md §3.5.2), so an fd the test process inherited (a cargo jobserver pipe, a socket
/// opened by another thread) can never reach a sandboxed probe.
///
/// Only async-signal-safe calls (`fcntl`, `sigprocmask`); no allocation.
pub fn exec_clean_in_child(limit: c_int) -> io::Result<()> {
    for fd in 3..limit {
        // SAFETY: fcntl on an arbitrary integer is safe; EBADF is ignored.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags != -1 && flags & libc::FD_CLOEXEC == 0 {
            // SAFETY: as above; sets only the close-on-exec flag of an open descriptor.
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        }
    }
    // SAFETY: `set` is a local, fully initialised by sigemptyset before use.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        if libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// `sandbox_check(3)` (private libsandbox API, resolved with `dlsym` as kernel/sandbox.md §3.8 says).
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
        let name = c"sandbox_check";
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name only looks a symbol up.
        let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
        if f.is_null() {
            return None;
        }
        // SAFETY: the symbol `sandbox_check` has exactly the variadic C signature above
        // (libsandbox; also used by Chromium and WebKit).
        let f: SandboxCheck = unsafe { std::mem::transmute::<*mut libc::c_void, SandboxCheck>(f) };
        // `SANDBOX_CHECK_NO_REPORT` is an exported `const enum sandbox_filter_type` variable.
        let nr = c"SANDBOX_CHECK_NO_REPORT";
        // SAFETY: as above.
        let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, nr.as_ptr()) };
        // SAFETY: when present the symbol is a readable, aligned `const int`.
        let no_report = if p.is_null() { 0 } else { unsafe { *(p as *const c_int) } };
        Some((f, no_report))
    }

    pub fn available() -> bool {
        resolve().is_some()
    }

    /// `sandbox_check(pid, NULL, 0)`: 1 when `pid` is sandboxed.
    pub fn sandboxed(pid: i32) -> Option<c_int> {
        let (f, _) = resolve()?;
        // SAFETY: NULL operation with filter type 0 is the documented "is it sandboxed" form;
        // no variadic arguments are read for SANDBOX_FILTER_NONE.
        Some(unsafe { f(pid, std::ptr::null(), 0) })
    }

    /// `sandbox_check(pid, op, SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, path)`:
    /// 0 when `pid`'s policy allows `op` on `path`, non-zero when it denies it. A pure query.
    pub fn path(pid: i32, op: &str, path: &Path) -> Option<c_int> {
        let (f, no_report) = resolve()?;
        let op = CString::new(op).ok()?;
        let p = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: both strings are NUL-terminated and outlive the call; SANDBOX_FILTER_PATH reads
        // exactly one `const char *` variadic argument.
        Some(unsafe { f(pid, op.as_ptr(), SANDBOX_FILTER_PATH | no_report, p.as_ptr()) })
    }
}

/// Register [`exec_clean_in_child`] on `cmd`. `pre_exec` makes std fork and exec instead of
/// `posix_spawn`; the closure only calls async-signal-safe functions.
pub fn set_exec_clean(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let limit = dtablesize();
    // SAFETY: the closure runs between fork and exec and performs only fcntl and sigprocmask,
    // both async-signal-safe; it captures a plain integer and allocates nothing.
    unsafe {
        cmd.pre_exec(move || exec_clean_in_child(limit));
    }
}
