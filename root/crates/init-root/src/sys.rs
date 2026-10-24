//! FFI, the only module allowed `unsafe` (root/root.md §4.8). Each block carries a SAFETY note.
//! WP20: euid, TTY echo, peer credentials, `sandbox_check`, `statvfs`, zeroing.
//! WP21: `sendmsg`/`recvmsg` with `SCM_RIGHTS`, `sigaction` and the self-pipe, `waitid`/`wait4`,
//! `kill`/`killpg` (held pids only), `exec-clean`, `dup2` for fds 3/4, the live-sweep process table.
//! R-SAFE (ADR-008): `signal` checks `held` (an unreaped child) itself; the live table is read-only and
//! built in every build for `doctor --sweep-dry-run`; only `signal_member` (live-sweep) reaches non-children.
#![allow(unsafe_code)]

use std::ffi::{CString, c_int};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

/// A platform without a confinement backend must never execute the requested payload.
#[cfg(not(target_os = "macos"))]
pub fn refuse_unsupported_spawn(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the post-fork closure performs no allocation, I/O or locking; it only
    // returns a constant OS error so std aborts exec and reports the failure.
    unsafe { cmd.pre_exec(|| Err(io::Error::from_raw_os_error(libc::ENOTSUP))); }
}

#[cfg(all(test, not(target_os = "macos")))]
#[test]
fn unsupported_confinement_refuses_even_a_successful_program() {
    let mut cmd = std::process::Command::new("/bin/true");
    refuse_unsupported_spawn(&mut cmd);
    assert_eq!(cmd.status().unwrap_err().raw_os_error(), Some(libc::ENOTSUP));
}

pub fn euid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

pub fn isatty(fd: RawFd) -> bool {
    // SAFETY: isatty only inspects the descriptor number.
    unsafe { libc::isatty(fd) == 1 }
}

/// Terminal echo off on `fd` until the guard drops (`keys set`, R-F4: secrets only from the TTY).
pub struct EchoOff(RawFd, libc::termios);

pub fn echo_off(fd: RawFd) -> io::Result<EchoOff> {
    let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr fully initialises `t` when it returns 0, which is checked before assume_init.
    let old = unsafe {
        if libc::tcgetattr(fd, t.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        t.assume_init()
    };
    let mut new = old;
    new.c_lflag &= !(libc::ECHO as libc::tcflag_t);
    // SAFETY: `new` is a valid termios copied from the one the kernel returned.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &new) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(EchoOff(fd, old))
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restores the termios saved from the same descriptor.
        unsafe { libc::tcsetattr(self.0, libc::TCSANOW, &self.1) };
    }
}

/// `(uid, pid)` of the peer of a connected Unix socket.
#[cfg(target_os = "macos")]
pub fn peer(fd: RawFd) -> io::Result<(u32, i32)> {
    let (mut uid, mut gid, mut pid) = (0, 0, 0 as libc::pid_t);
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: out-pointers reference live locals of the right sizes; `len` holds pid_t's size.
    let ok = unsafe {
        libc::getpeereid(fd, &mut uid, &mut gid) == 0
            && libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) == 0
    };
    if ok { Ok((uid, pid)) } else { Err(io::Error::last_os_error()) }
}

/// UNVERIFIED (no Linux host): `SO_PEERCRED`.
#[cfg(target_os = "linux")]
pub fn peer(fd: RawFd) -> io::Result<(u32, i32)> {
    let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `c` is a live ucred and `len` is its size.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut c as *mut libc::ucred).cast(), &mut len) };
    if r == 0 { Ok((c.uid, c.pid)) } else { Err(io::Error::last_os_error()) }
}

/// Is `pid` inside any sandbox? macOS: `sandbox_check(pid, NULL, 0)` looked up with `dlsym`
/// (None if the symbol is gone, RF13). Linux: the pid namespace differs from ours.
#[cfg(target_os = "macos")]
pub fn sandboxed(pid: i32) -> Option<bool> {
    type Check = unsafe extern "C" fn(libc::pid_t, *const libc::c_char, libc::c_int, ...) -> libc::c_int;
    // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name; a null result is handled.
    let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()) };
    if sym.is_null() {
        return None;
    }
    // SAFETY: libsystem_sandbox exports `int sandbox_check(pid_t, const char *, int, ...)`; a null
    // operation with type 0 asks only whether the process is sandboxed (tested, root.md §3.3.5).
    Some(unsafe { std::mem::transmute::<*mut libc::c_void, Check>(sym)(pid, std::ptr::null(), 0) } != 0)
}

#[cfg(target_os = "linux")]
pub fn sandboxed(pid: i32) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let ino = |p: String| std::fs::metadata(p).map(|m| m.ino()).ok();
    Some(ino(format!("/proc/{pid}/ns/pid"))? != ino("/proc/self/ns/pid".into())?)
}

/// Bytes available to us on the filesystem holding `path`.
pub fn free_bytes(path: &std::path::Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c` is NUL-terminated; statvfs initialises `s` when it returns 0 (checked).
    let s = unsafe {
        if libc::statvfs(c.as_ptr(), s.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        s.assume_init()
    };
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

/// One fd-channel datagram (§3.5.3): `payload` plus one `SCM_RIGHTS` message with `fds` (≤ 16).
pub fn send_fds(sock: RawFd, payload: &[u8], fds: &[RawFd]) -> io::Result<()> {
    let n = std::mem::size_of_val(fds) as u32;
    // SAFETY: CMSG_SPACE is pure arithmetic.
    let space = unsafe { libc::CMSG_SPACE(n) } as usize;
    let mut cbuf = vec![0u64; space / 8 + 1]; // u64: cmsghdr alignment
    let mut iov = libc::iovec { iov_base: payload.as_ptr() as *mut libc::c_void, iov_len: payload.len() };
    // SAFETY: msghdr is plain data; every pointer set below references a live local that outlives sendmsg;
    // CMSG_FIRSTHDR/CMSG_DATA point inside `cbuf`, which holds CMSG_SPACE(n) bytes.
    let r = unsafe {
        let mut m: libc::msghdr = std::mem::zeroed();
        m.msg_iov = &mut iov;
        m.msg_iovlen = 1;
        m.msg_control = cbuf.as_mut_ptr().cast();
        m.msg_controllen = space as _;
        let c = libc::CMSG_FIRSTHDR(&m);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(n) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(c).cast::<c_int>(), fds.len());
        libc::sendmsg(sock, &m, 0)
    };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Receives one datagram and its fds (marked close-on-exec at once; exec-clean makes the window harmless).
pub fn recv_fds(sock: RawFd, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
    // SAFETY: as in send_fds; only control messages the kernel wrote (cmsg_len bounds the copy) are read.
    unsafe {
        let space = libc::CMSG_SPACE(16 * 4) as usize;
        let mut cbuf = vec![0u64; space / 8 + 1];
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        let mut m: libc::msghdr = std::mem::zeroed();
        m.msg_iov = &mut iov;
        m.msg_iovlen = 1;
        m.msg_control = cbuf.as_mut_ptr().cast();
        m.msg_controllen = space as _;
        let r = libc::recvmsg(sock, &mut m, 0);
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        let mut c = libc::CMSG_FIRSTHDR(&m);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let k = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / 4;
                for i in 0..k {
                    let fd = *libc::CMSG_DATA(c).cast::<c_int>().add(i);
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    out.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&m, c);
        }
        Ok((r as usize, out))
    }
}

/// A close-on-exec copy of `fd` numbered ≥ 10, so `map_fds` can never clobber its own sources.
pub fn dup_high(fd: &impl AsRawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC returns a new descriptor we own, or -1.
    let r = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(r) }) }
}

/// The kernel spawn (§3.5.1): `src[i]` becomes the child's fd `3 + i`, inherited (dup2 clears CLOEXEC).
pub fn map_fds(cmd: &mut std::process::Command, src: [RawFd; 2]) {
    use std::os::unix::process::CommandExt;
    // SAFETY: runs between fork and exec; dup2 is async-signal-safe and the closure allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            for (i, s) in src.into_iter().enumerate() {
                if libc::dup2(s, 3 + i as c_int) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        })
    };
}

/// `exec-clean` (§3.5.2): empty signal mask, SIGPIPE back to default, every fd ≥ 3 not in `keep`
/// closed, then `execv(argv[0])`. Returns only on failure.
pub fn exec_clean(keep: &[RawFd], argv: &[String]) -> io::Error {
    let Ok(c) = argv.iter().map(|a| CString::new(a.as_str())).collect::<Result<Vec<_>, _>>() else { return util_bad("NUL in argv") };
    if c.is_empty() {
        return util_bad("empty argv");
    }
    let mut p: Vec<*const libc::c_char> = c.iter().map(|a| a.as_ptr()).collect();
    p.push(std::ptr::null());
    // SAFETY: `set` is initialised by sigemptyset; close on unknown descriptors fails harmlessly;
    // `p` is a NULL-terminated array of pointers into `c`, alive until execv.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        for fd in 3..libc::getdtablesize().min(1 << 16) {
            if !keep.contains(&fd) {
                libc::close(fd);
            }
        }
        libc::execv(p[0], p.as_ptr());
    }
    io::Error::last_os_error()
}

fn util_bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, m)
}

static SIG_W: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: c_int) {
    let b = sig as u8;
    // SAFETY: write(2) is async-signal-safe; the descriptor is the self-pipe's write end, never closed.
    unsafe { libc::write(SIG_W.load(Ordering::Relaxed), (&b as *const u8).cast(), 1) };
}

/// §3.5.2: handlers for `sigs` that write one byte (the signal number) to a self-pipe; the caller
/// reads the returned end. Nothing is ever blocked, and caught signals reset to default on exec.
pub fn signal_pipe(sigs: &[c_int]) -> io::Result<std::io::PipeReader> {
    let (r, w) = std::io::pipe()?;
    SIG_W.store(w.into_raw_fd(), Ordering::Relaxed);
    for &s in sigs {
        // SAFETY: `sa` is zeroed then filled; the handler is an extern "C" fn of the right shape.
        let ok = unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(s, &sa, std::ptr::null_mut()) == 0
        };
        if !ok {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(r)
}

/// Blocks until `pid` (our child) has exited, without reaping it: its pid and process group stay ours.
pub fn wait_exit(pid: i32) -> io::Result<()> {
    loop {
        // SAFETY: `info` is a zeroed out-parameter; P_PID with our own child's pid.
        let r = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT)
        };
        let e = io::Error::last_os_error();
        if r == 0 || e.kind() != io::ErrorKind::Interrupted {
            return if r == 0 { Ok(()) } else { Err(e) };
        }
    }
}

/// Reaps our child `pid` (`wait4`): exit code, terminating signal, and user + system CPU in ms.
pub fn reap(pid: i32) -> io::Result<(Option<i32>, Option<i32>, u64)> {
    let (mut st, mut ru) = (0, std::mem::MaybeUninit::<libc::rusage>::zeroed());
    // SAFETY: out-pointers reference live locals; wait4 fills `ru` when it returns `pid` (checked).
    let ru = unsafe {
        while libc::wait4(pid, &mut st, 0, ru.as_mut_ptr()) != pid {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        ru.assume_init()
    };
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    let (code, sig) = (libc::WIFEXITED(st).then(|| libc::WEXITSTATUS(st)), libc::WIFSIGNALED(st).then(|| libc::WTERMSIG(st)));
    Ok((code, sig, ms(ru.ru_utime) + ms(ru.ru_stime)))
}

/// Is `pid` a child of this process that is not reaped yet (running, stopped or a zombie)? `waitid` with
/// `WNOHANG | WNOWAIT` only looks: it reaps nothing. ECHILD means "not ours": never our child, or already
/// reaped (its pid, and so its process group id, may since belong to anyone). ADR-008.
pub fn held(pid: i32) -> bool {
    // SAFETY: `info` is a zeroed out-parameter; P_PID names one pid; WNOWAIT leaves its state untouched.
    pid > 1 && unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT)
    } == 0
}

/// Has our child `pid` exited (a zombie not reaped yet)? Looks only (`WNOHANG | WNOWAIT`); false for a running child
/// and for anything that is not our unreaped child.
pub fn has_exited(pid: i32) -> bool {
    // SAFETY: `info` is a zeroed out-parameter; P_PID names one pid; WNOWAIT leaves its state untouched.
    pid > 1 && unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0 && {
            #[cfg(target_os = "linux")]
            let who = info.si_pid();
            #[cfg(not(target_os = "linux"))]
            let who = info.si_pid;
            who != 0
        }
    }
}

/// Signals a pid (or with `group`, the process group) that the Root holds: a child not yet reaped. Every
/// spawn is its own process group (`process_group(0)`), and an unreaped leader keeps both ids ours. Any
/// other pid is refused here, at the only `kill`/`killpg` of a default build (ADR-008, tests/rsafe.rs).
pub fn signal(pid: i32, sig: c_int, group: bool) -> bool {
    // SAFETY: plain syscalls on a pid `held` just confirmed is our unreaped child.
    held(pid) && unsafe { if group { libc::killpg(pid, sig) } else { libc::kill(pid, sig) } } == 0
}

/// SIGKILL to a D1 member that is not our child (an escapee, root/root.md §3.5.4). Only the D1 sweep
/// calls it, only in `live-sweep` builds, which run only in a disposable environment (`sweep::disposable`).
#[cfg(all(feature = "live-sweep", target_os = "macos"))]
pub fn signal_member(pid: i32) -> bool {
    // SAFETY: a plain syscall; the caller checked D1 for this pid in the same pass.
    pid > 1 && pid != std::process::id() as i32 && unsafe { libc::kill(pid, libc::SIGKILL) } == 0
}

/// The live process table (root/root.md §3.5.4): our uid's pids and `sandbox_check` queries. Read-only:
/// it asks the kernel about processes and never signals one. The live D1 sweep uses it only with the
/// `live-sweep` feature; `init-root doctor --sweep-dry-run` uses it in every build (ADR-008).
#[cfg(target_os = "macos")]
pub struct LiveTable;

#[cfg(target_os = "macos")]
impl LiveTable {
    /// `sandbox_check` with the result kept three-valued: 0 allowed, > 0 denied, < 0 an error (`None`;
    /// an error never counts as "denied", so it can never make a process a D1 member).
    fn check(pid: i32, path: Option<&std::path::Path>) -> Option<c_int> {
        use std::os::unix::ffi::OsStrExt;
        type Check = unsafe extern "C" fn(libc::pid_t, *const libc::c_char, c_int, ...) -> c_int;
        let p = path.map(|p| CString::new(p.as_os_str().as_bytes())).transpose().ok()?;
        // SAFETY: as in `sandboxed`; `SANDBOX_CHECK_NO_REPORT` is an exported `const int`; the
        // SANDBOX_FILTER_PATH (1) form reads exactly one `const char *` argument.
        let r = unsafe {
            let (sym, nr) = (libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()), libc::dlsym(libc::RTLD_DEFAULT, c"SANDBOX_CHECK_NO_REPORT".as_ptr()));
            if sym.is_null() {
                return None;
            }
            let f = std::mem::transmute::<*mut libc::c_void, Check>(sym);
            match &p {
                None => f(pid, std::ptr::null(), 0),
                Some(p) => f(pid, c"file-read-data".as_ptr(), 1 | if nr.is_null() { 0 } else { *(nr as *const c_int) }, p.as_ptr()),
            }
        };
        (r >= 0).then_some(r)
    }

    /// The executable path of `pid` (`proc_pidpath`), for the dry-run report.
    pub fn path(pid: i32) -> Option<String> {
        let mut b = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer holds `b.len()` bytes; proc_pidpath returns the length it wrote, or ≤ 0.
        let n = unsafe { libc::proc_pidpath(pid, b.as_mut_ptr().cast(), b.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&b[..n as usize]).into_owned())
    }
}

#[cfg(target_os = "macos")]
impl crate::sweep::ProcTable for LiveTable {
    fn pids(&self) -> Vec<i32> {
        let mut v = vec![0i32; 8192];
        // SAFETY: the buffer holds v.len() pids; proc_listpids returns the bytes it wrote.
        const PROC_UID_ONLY: u32 = 4; // <sys/proc_info.h>; not exported by libc 0.2.191
        let n = unsafe { libc::proc_listpids(PROC_UID_ONLY, euid(), v.as_mut_ptr().cast(), (v.len() * 4) as c_int) };
        v.truncate(n.max(0) as usize / 4);
        v.retain(|&p| p > 1);
        v
    }
    fn sandboxed(&self, pid: i32) -> Option<bool> {
        Self::check(pid, None).map(|r| r == 1)
    }
    fn may_read(&self, pid: i32, path: &std::path::Path) -> Option<bool> {
        Self::check(pid, Some(path)).map(|r| r == 0)
    }
}

/// The Root's own descriptor budget (R-SEC): egress up to 96 connections, the sealed lane 32, the fd-channel
/// stash, three pipes per zone instance. launchd starts agents at a soft `RLIMIT_NOFILE` of 256. Raises the
/// soft limit to min(hard, `want`), never lowers it, and returns (soft before, soft after, hard).
#[allow(clippy::unnecessary_cast)] // rlim_t is u64 on macOS and Linux today; the casts keep it portable
/// SBX-009 (ADR-009 snapshots): `clonefile(src, dst, CLONE_NOFOLLOW | CLONE_NOOWNERCOPY)`. A directory is
/// cloned as a whole hierarchy; links inside it are cloned as links, never followed. `dst` must not exist.
#[cfg(target_os = "macos")]
pub fn clonefile(src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    const CLONE_NOFOLLOW: u32 = 0x0001; // <sys/clonefile.h>
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let (s, d) = (CString::new(src.as_os_str().as_bytes())?, CString::new(dst.as_os_str().as_bytes())?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    match unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW | CLONE_NOOWNERCOPY) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(target_os = "linux")]
mod clone_tree;
#[cfg(target_os = "linux")]
pub fn clonefile(src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    clone_tree::clonefile(src, dst)
}
/// Root system snapshots bound logical bytes while copying, before consuming disk.
#[cfg(target_os = "linux")]
pub fn clonefile_bounded(src: &std::path::Path, dst: &std::path::Path, max_bytes: u64, skip: &[&str]) -> io::Result<u64> {
    clone_tree::clonefile_bounded(src, dst, max_bytes, skip)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn clonefile(_: &std::path::Path, _: &std::path::Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "snapshot cloning is not implemented on this OS"))
}

/// The file system type of the volume holding `path` (`statfs` `f_fstypename`, e.g. `apfs`).
#[cfg(target_os = "macos")]
pub fn fs_type(path: &std::path::Path) -> io::Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let p = CString::new(path.as_os_str().as_bytes())?;
    let mut st = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `p` is NUL-terminated; statfs fully initialises `st` when it returns 0, checked before use.
    let st = unsafe {
        if libc::statfs(p.as_ptr(), st.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        st.assume_init()
    };
    let name: Vec<u8> = st.f_fstypename.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    Ok(String::from_utf8_lossy(&name).into_owned())
}

/// Linux exposes a 32-bit filesystem magic rather than a filesystem name.
/// ext2/3/4 share one identity; recognizing it does not establish CoW support.
#[cfg(target_os = "linux")]
fn linux_filesystem_name(magic: u32) -> Option<&'static str> {
    match magic {
        0x0000_ef53 => Some("ext2/3/4"),
        0x9123_683e => Some("btrfs"),
        0x5846_5342 => Some("xfs"),
        0x794c_7630 => Some("overlayfs"),
        0x0102_1994 => Some("tmpfs"),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
pub fn fs_type(path: &std::path::Path) -> io::Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable statfs storage.
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful statfs above fully initialized this output.
    // f_type is signed on some targets; its low 32 bits carry Linux's identity.
    let magic = unsafe { stat.assume_init() }.f_type as u32;
    Ok(linux_filesystem_name(magic).map(str::to_string).unwrap_or_else(|| format!("linux-statfs-0x{magic:x}")))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn fs_type(_: &std::path::Path) -> io::Result<String> {
    Ok("unsupported".into())
}

#[cfg(all(test, target_os = "linux"))]
mod linux_filesystem_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn known_linux_filesystem_magic_is_normalized_without_claiming_cow() {
        for (magic, name) in [(0xef53, "ext2/3/4"), (0x9123_683e, "btrfs"),
            (0x5846_5342, "xfs"), (0x794c_7630, "overlayfs"), (0x0102_1994, "tmpfs")] {
            assert_eq!(linux_filesystem_name(magic), Some(name));
        }
        // Network, pseudo and unknown filesystem identities stay unqualified.
        for magic in [0, 0x6969, 0x9fa0, 0xdead_beef] {
            assert_eq!(linux_filesystem_name(magic), None);
        }
        let signed_32bit_magic = 0x9123_683e_u32 as i32;
        assert_eq!(linux_filesystem_name(signed_32bit_magic as u32), Some("btrfs"));
    }

    #[test]
    fn linux_statfs_queries_real_path_and_preserves_syscall_errors() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let identity = fs_type(root).expect("actual source filesystem statfs");
        assert!(!identity.is_empty());
        assert!(matches!(identity.as_str(), "ext2/3/4" | "btrfs" | "xfs" | "overlayfs" | "tmpfs")
            || identity.strip_prefix("linux-statfs-0x").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit())));
        let missing = root.join(format!(".init-statfs-absent-{}", std::process::id())).join("child");
        assert_eq!(fs_type(&missing).unwrap_err().raw_os_error(), Some(libc::ENOENT));
        let nul = Path::new("invalid\0path");
        assert_eq!(fs_type(nul).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}

pub fn raise_nofile(want: u64) -> io::Result<(u64, u64, u64)> {
    let get = || {
        let mut r = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: `r` is a live out-parameter of the right type.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } == 0 { Ok(r) } else { Err(io::Error::last_os_error()) }
    };
    let r = get()?;
    let (before, hard) = (r.rlim_cur as u64, r.rlim_max as u64);
    let target = want.min(hard);
    if target > before {
        let n = libc::rlimit { rlim_cur: target as libc::rlim_t, rlim_max: r.rlim_max };
        // SAFETY: `n` is a valid rlimit whose soft value does not exceed its hard value.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &n) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((before, get()?.rlim_cur as u64, hard))
}

/// Overwrites a buffer that held a secret, in a way the optimiser keeps.
pub fn zero(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

/// MEM-RUNAWAY (memwatch.rs): one live process as the memory watchdog sees it. `start_us` is its start time in
/// microseconds since the epoch; a pid is reused only after its process is gone, the pair (pid, start) never is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub start_us: u64,
    pub uid: u32,
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)` of one pid: None when it is gone, a zombie, or unreadable. Read-only.
#[cfg(target_os = "macos")]
pub fn proc_info(pid: i32) -> Option<Proc> {
    const SZOMB: u32 = 5; // <sys/proc.h>
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
    let mut b = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: the buffer holds `size` bytes; proc_pidinfo returns the bytes it wrote, and the struct is read
    // only when it wrote all of them (a zeroed proc_bsdinfo is a valid value of this plain C struct anyway).
    let b = unsafe {
        if pid <= 0 || libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, b.as_mut_ptr().cast(), size) != size {
            return None;
        }
        b.assume_init()
    };
    (b.pbi_status != SZOMB).then_some(Proc { pid: b.pbi_pid as i32, ppid: b.pbi_ppid as i32, pgid: b.pbi_pgid as i32,
        start_us: b.pbi_start_tvsec.saturating_mul(1_000_000).saturating_add(b.pbi_start_tvusec), uid: b.pbi_uid })
}

/// Every live process of our uid (`proc_listpids(PROC_UID_ONLY)`), each read once. Read-only: it only asks.
#[cfg(target_os = "macos")]
pub fn proc_table() -> Vec<Proc> {
    const PROC_UID_ONLY: u32 = 4; // <sys/proc_info.h>
    let mut v = vec![0i32; 16_384];
    // SAFETY: the buffer holds v.len() pids; proc_listpids returns the bytes it wrote.
    let n = unsafe { libc::proc_listpids(PROC_UID_ONLY, euid(), v.as_mut_ptr().cast(), (v.len() * 4) as c_int) };
    v.truncate(n.max(0) as usize / 4);
    v.into_iter().filter(|&p| p > 1).filter_map(proc_info).collect()
}

/// The physical footprint of `pid` in bytes (resident plus compressed memory: what jetsam measures), from
/// `proc_pid_rusage(RUSAGE_INFO_V4)`; None for a gone or exited process. Read-only.
#[cfg(target_os = "macos")]
pub fn footprint(pid: i32) -> Option<u64> {
    let mut ri = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: `ri` is a rusage_info_v4, the struct the V4 flavor writes in full when the call returns 0 (checked).
    let ri = unsafe {
        if pid <= 0 || libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, ri.as_mut_ptr().cast::<libc::rusage_info_t>()) != 0 {
            return None;
        }
        ri.assume_init()
    };
    (ri.ri_proc_exit_abstime == 0).then_some(ri.ri_phys_footprint)
}

/// MEM-RUNAWAY, the ADR-008 amendment: SIGSTOP or SIGKILL to one workload process the memory watchdog observed
/// below an instance the Root holds (memwatch.rs, its only caller, by pid). Refused unless, right before the
/// syscall, the pid still has the start time and uid recorded then (so a pid that exited and was reused never
/// matches), belongs to our uid, and is sandboxed (`sandbox_check`). pid ≤ 1 and our own pid are never signalled.
#[cfg(target_os = "macos")]
pub fn signal_tracked(p: &Proc, sig: c_int) -> bool {
    if p.pid <= 1 || p.pid == std::process::id() as i32 || !(sig == libc::SIGKILL || sig == libc::SIGSTOP) {
        return false;
    }
    let same = proc_info(p.pid).is_some_and(|q| q.start_us == p.start_us && q.uid == p.uid && q.uid == euid());
    // SAFETY: a plain syscall on a pid whose identity (pid, start time, uid) was checked just above.
    same && sandboxed(p.pid) == Some(true) && unsafe { libc::kill(p.pid, sig) } == 0
}

// Unsupported non-macOS/non-Linux targets remain inert; Linux polling and pidfds are implemented below.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn proc_info(_: i32) -> Option<Proc> {
    None
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn proc_table() -> Vec<Proc> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn footprint(_: i32) -> Option<u64> {
    None
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn signal_tracked(_: &Proc, _: c_int) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn own_process_is_not_sandboxed_and_peer_is_us() {
        use std::os::fd::AsRawFd;
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let (uid, pid) = super::peer(a.as_raw_fd()).unwrap();
        assert_eq!(uid, super::euid());
        assert_eq!(pid as u32, std::process::id());
        assert_ne!(super::sandboxed(pid), Some(true));
        assert!(super::free_bytes(std::path::Path::new("/")).unwrap() > 0);
    }

    /// ADR-008: `signal` reaches only an unreaped child of this process. Everything here is this test's
    /// own child (spawned below, in its own process group); pids 1, our own and our parent's are only
    /// passed to `held`/`signal`, which refuse them before any syscall that could deliver a signal.
    #[test]
    fn signal_reaches_only_unreaped_children() {
        use std::os::unix::process::CommandExt;
        let me = std::process::id() as i32;
        let parent = std::os::unix::process::parent_id() as i32;
        for pid in [-1, 0, 1, me, parent] {
            assert!(!super::held(pid), "{pid} is not our child");
            assert!(!super::signal(pid, 0, false) && !super::signal(pid, 0, true), "{pid} is refused");
        }
        let c = std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let pid = c.id() as i32;
        assert!(super::held(pid), "a running child is held");
        assert!(super::signal(pid, 0, false) && super::signal(pid, 0, true), "signal 0 to the child and its group");
        assert!(super::signal(pid, libc::SIGKILL, true), "SIGKILL to its own group");
        super::wait_exit(pid).unwrap();
        assert!(super::held(pid), "a zombie is still held: its pid and group id stay ours");
        let (code, sig, _) = super::reap(pid).unwrap();
        assert_eq!((code, sig), (None, Some(libc::SIGKILL)));
        assert!(!super::held(pid), "a reaped pid is not held");
        assert!(!super::signal(pid, 0, false) && !super::signal(pid, 0, true), "a reaped pid is refused");
        std::mem::forget(c); // reaped above by wait4; `Child` must not wait on it again
    }

    /// R-SEC: the soft NOFILE limit is raised to min(hard, 4096) and read back; asking again changes nothing.
    #[test]
    #[allow(clippy::unnecessary_cast)]
    fn nofile_soft_limit_is_raised_and_read_back() {
        let (before, after, hard) = super::raise_nofile(4096).unwrap();
        assert_eq!(after, before.max(4096.min(hard)), "soft {before} -> {after} (hard {hard})");
        let mut r = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) }, 0);
        assert_eq!(r.rlim_cur as u64, after, "getrlimit reads back the raised soft limit");
        assert_eq!(super::raise_nofile(64).unwrap().1, after, "never lowered");
    }

    /// The live table only reads; this test asks it about this test process alone.
    #[cfg(target_os = "macos")]
    #[test]
    fn live_table_reads_our_own_entry() {
        use crate::sweep::ProcTable;
        let (t, me) = (super::LiveTable, std::process::id() as i32);
        assert!(t.pids().contains(&me));
        assert!(t.sandboxed(me).is_some(), "sandbox_check answers for a live pid");
        assert!(super::LiveTable::path(me).is_some_and(|p| p.starts_with('/')));
        // a pid that is gone: sandbox_check(NULL) answers 1 ("sandboxed") for it on macOS 14, but the path
        // form never answers "allowed", so D1's condition (b) fails and it can never be a member
        let mut c = std::process::Command::new("/usr/bin/true").spawn().unwrap(); // our own child
        let dead = c.id() as i32;
        c.wait().unwrap();
        for pid in [dead, i32::MAX] {
            assert_ne!(t.may_read(pid, std::path::Path::new("/")), Some(true), "a gone pid never may read");
            let m = crate::sweep::Marks { mark: std::path::Path::new("/"), decoy: std::path::Path::new("/nonexistent-a"), canary: std::path::Path::new("/nonexistent-b") };
            assert!(!crate::sweep::member(&t, pid, &m), "a gone pid is never a D1 member");
        }
    }
}

/// A sealed anonymous cBPF file. No path in the agent's mount namespace can change it.
#[cfg(target_os = "linux")]
pub fn linux_seccomp() -> io::Result<OwnedFd> {
    use seccompiler::{BpfProgram, SeccompAction as A, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as C, SeccompFilter, SeccompRule as R};
    use std::io::{Seek, Write};
    let bad = |e| io::Error::other(format!("Linux seccomp: {e}"));
    let mut rules = std::collections::BTreeMap::new();
    for n in [libc::SYS_io_uring_setup, libc::SYS_io_uring_enter, libc::SYS_io_uring_register, libc::SYS_ptrace,
        libc::SYS_process_vm_readv, libc::SYS_process_vm_writev, libc::SYS_bpf, libc::SYS_perf_event_open,
        libc::SYS_userfaultfd, libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key, libc::SYS_kexec_load,
        libc::SYS_open_by_handle_at, libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_setns,
        libc::SYS_unshare, libc::SYS_reboot] { rules.insert(n, vec![]); }
    #[cfg(target_arch = "x86_64")]
    rules.insert(libc::SYS_kexec_file_load, vec![]);
    for n in [libc::SYS_socket, libc::SYS_socketpair] {
        let c = C::new(0, Len::Dword, Op::Ne, libc::AF_UNIX as u64).map_err(|e| bad(e.to_string()))?;
        rules.insert(n, vec![R::new(vec![c]).map_err(|e| bad(e.to_string()))?]);
    }
    let ioctls = [libc::TIOCSTI as u64, 0x541c /* TIOCLINUX */].into_iter().map(|request|
        C::new(1, Len::Dword, Op::Eq, request).and_then(|c| R::new(vec![c]))).collect::<Result<Vec<_>, _>>().map_err(|e| bad(e.to_string()))?;
    rules.insert(libc::SYS_ioctl, ioctls);
    let arch = std::env::consts::ARCH.try_into().map_err(|e: seccompiler::BackendError| bad(e.to_string()))?;
    let filter = SeccompFilter::new(rules, A::Allow, A::Errno(libc::EPERM as u32), arch).map_err(|e| bad(e.to_string()))?;
    let program: BpfProgram = filter.try_into().map_err(|e: seccompiler::BackendError| bad(e.to_string()))?;
    #[cfg(target_arch = "x86_64")]
    let program = {
        use seccompiler::sock_filter;
        // x32 shares AUDIT_ARCH_X86_64 but sets bit 30 in the syscall number.
        // Native-number rules would otherwise miss every x32 variant. Preserve
        // the compiler's fail-closed architecture check before rejecting x32.
        let architecture = [
            sock_filter { code: 0x20, jt: 0, jf: 0, k: 4 }, // LD W ABS arch
            sock_filter { code: 0x15, jt: 1, jf: 0, k: 0xc000003e }, // JEQ AUDIT_ARCH_X86_64
            sock_filter { code: 0x06, jt: 0, jf: 0, k: libc::SECCOMP_RET_KILL_PROCESS },
        ];
        if !program.starts_with(&architecture) {
            return Err(bad("unexpected seccompiler architecture prelude".to_string()));
        }
        let mut program = program;
        program.splice(architecture.len()..architecture.len(), [
            sock_filter { code: 0x20, jt: 0, jf: 0, k: 0 }, // LD W ABS nr
            sock_filter { code: 0x45, jt: 0, jf: 1, k: 0x40000000 }, // JSET __X32_SYSCALL_BIT
            sock_filter { code: 0x06, jt: 0, jf: 0, k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32 },
        ]);
        program
    };
    // SAFETY: static NUL-terminated name, known flags; successful descriptor is owned here.
    let fd = unsafe { libc::memfd_create(c"init-zone-seccomp".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: memfd_create returned a new owned descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    for i in program {
        file.write_all(&i.code.to_ne_bytes())?;
        file.write_all(&[i.jt, i.jf])?;
        file.write_all(&i.k.to_ne_bytes())?;
    }
    file.rewind()?;
    // SAFETY: our live descriptor, supported fcntl operation and integer flags.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL) } < 0 {
        return Err(io::Error::last_os_error());
    }
    dup_high(&file)
}

/// fd 5 belongs only to bwrap's seccomp loader. exec-clean keeps it; bwrap consumes/closes it.
#[cfg(target_os = "linux")]
pub fn map_seccomp(cmd: &mut std::process::Command, fd: RawFd) {
    use std::os::unix::process::CommandExt;
    // SAFETY: dup2 only in the post-fork closure; source descriptor is held through spawn.
    unsafe { cmd.pre_exec(move || if libc::dup2(fd, 5) < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }); }
}

/// Pin every bind source before spawning bwrap. Component-wise O_PATH|O_NOFOLLOW prevents
/// an agent swapping a validated ancestor for a symlink; bwrap's fd binds verify inode/dev.
/// Older bwrap without fd-bind options refuses the invocation before executing the payload.
#[cfg(target_os = "linux")]
pub fn linux_bind_fds(argv: &mut [std::ffi::OsString]) -> io::Result<Vec<OwnedFd>> {
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path};
    let mut held = Vec::new();
    let mut at = 0;
    while at < argv.len() {
        if argv[at] != "--bind" && argv[at] != "--ro-bind" { at += 1; continue; }
        if at + 2 >= argv.len() { return Err(util_bad("incomplete Linux bind")); }
        let original = Path::new(&argv[at + 1]);
        // Only these Root-fixed system paths may resolve distribution symlinks (/bin -> /usr/bin).
        let source = if ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/passwd", "/etc/group"].iter().any(|s| original == Path::new(s)) {
            std::fs::canonicalize(original)?
        } else { original.to_path_buf() };
        if !source.is_absolute() { return Err(util_bad("non-absolute Linux mount")); }
        // SAFETY: static valid path, flags; successful fd is newly owned.
        let first = unsafe { libc::open(c"/".as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if first < 0 { return Err(io::Error::last_os_error()); }
        let mut current = unsafe { OwnedFd::from_raw_fd(first) };
        for part in source.components() {
            let Component::Normal(part) = part else {
                if part == Component::RootDir { continue; }
                return Err(util_bad("non-normal Linux mount component"));
            };
            let name = CString::new(part.as_bytes()).map_err(|_| util_bad("NUL mount component"))?;
            // SAFETY: live dirfd and valid C string; no symlink is followed at any component.
            let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if fd < 0 { return Err(io::Error::last_os_error()); }
            let next = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: initialized output and held descriptor.
            if unsafe { libc::fstat(next.as_raw_fd(), &mut st) } != 0 { return Err(io::Error::last_os_error()); }
            if st.st_mode & libc::S_IFMT == libc::S_IFLNK { return Err(util_bad("symlink in Linux mount source")); }
            current = next;
        }
        let fd = dup_high(&current)?;
        argv[at] = if argv[at] == "--bind" { "--bind-fd" } else { "--ro-bind-fd" }.into();
        argv[at + 1] = fd.as_raw_fd().to_string().into();
        held.push(fd);
        at += 3;
    }
    Ok(held)
}

#[cfg(target_os = "linux")]
pub fn inherit_fds(cmd: &mut std::process::Command, fds: Vec<RawFd>) {
    use std::os::unix::process::CommandExt;
    // SAFETY: fcntl only after fork; vector allocated before fork; owner holds each fd through spawn.
    unsafe { cmd.pre_exec(move || {
        for fd in &fds {
            if libc::fcntl(*fd, libc::F_SETFD, 0) < 0 { return Err(io::Error::last_os_error()); }
        }
        Ok(())
    }); }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_zone_tests {
    use super::*;
    use std::{collections::BTreeMap, ffi::OsString, fs, path::PathBuf};
    use std::io::{Read, Write};
    use std::os::unix::fs::symlink;
    use init_sandbox_profiles::{ProfileId, access::Grant, linux::{Layout, compose}};

    fn fixture(tag: &str) -> PathBuf {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/linux-{tag}-{}", std::process::id()));
        fs::create_dir_all(&p).unwrap(); fs::canonicalize(p).unwrap()
    }
    #[test]
    fn linux_proc_parser_preserves_identity_and_refuses_self_signals() {
        let mut fields = vec!["0"; 22];
        fields[0] = "S"; fields[1] = "12"; fields[2] = "13"; fields[19] = "150";
        let stat = format!("42 (name with ) spaces) {}", fields.join(" "));
        assert_eq!(linux_proc_stat(42, &stat, 1000, 100, 10), Some(Proc { pid: 42, ppid: 12, pgid: 13, uid: 1000, start_us: 11_500_000 }));
        assert!(linux_proc_stat(42, "bad", 0, 100, 10).is_none());
        let me = proc_info(std::process::id() as i32).unwrap();
        assert_eq!(me.uid, euid());
        assert!(footprint(me.pid).is_some());
        assert!(!signal_tracked(&me, libc::SIGKILL));
    }

    /// Native watchdog qualification: real namespace membership, nonzero RSS and
    /// pidfd signaling. Unsupported pidfds/user namespaces fail rather than skip.
    #[test]
    #[ignore = "requires native Linux user/pid namespaces and pidfd signaling"]
    fn linux_pidfd_tracked_stop_kill_isolates_owned_target() {
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        // These are the only processes this test may clean up. unshare's
        // --kill-child pins a parent-death kill to its own namespace child, so
        // an assertion failure cannot leave a stopped descendant running.
        struct Owned(Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        fn until(mut ready: impl FnMut() -> bool, why: &str) {
            let end = Instant::now() + Duration::from_secs(10);
            while Instant::now() < end {
                if ready() { return; }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("{why}");
        }

        let mut sibling = Owned(Command::new("/bin/sleep").arg("60")
            .env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("spawn independent owned sibling"));
        let sibling_info = proc_info(sibling.0.id() as i32).expect("sibling process identity");
        assert_eq!(sandboxed(sibling_info.pid), Some(false));
        assert!(!signal_tracked(&sibling_info, libc::SIGKILL), "unsandboxed process must be refused");

        let mut leader = Owned(Command::new("/usr/bin/unshare")
            .args(["--user", "--map-root-user", "--pid", "--fork", "--mount-proc", "--kill-child=KILL", "--",
                "/usr/bin/python3", "-c",
                "import time; allocation=bytearray(2*1024*1024); time.sleep(60)"])
            .env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::null())
            .stdout(Stdio::null()).stderr(Stdio::inherit())
            .spawn().expect("spawn owned namespace launcher"));
        let mut target = None;
        until(|| {
            assert!(leader.0.try_wait().unwrap().is_none(), "namespace launcher exited before qualification");
            target = proc_table().into_iter().find(|p| p.ppid == leader.0.id() as i32
                && sandboxed(p.pid) == Some(true) && footprint(p.pid).is_some_and(|n| n >= 2 * 1024 * 1024));
            target.is_some()
        }, "no owned sandboxed workload with nonzero memory metrics");
        let target = target.unwrap();
        assert!(target.start_us > 0);
        assert_eq!(target.uid, euid());
        let before = footprint(target.pid).expect("target memory metrics");
        assert!(before >= 2 * 1024 * 1024, "allocated workload RSS must be measured");
        let stale_start = Proc { start_us: target.start_us.saturating_add(1), ..target };
        let stale_uid = Proc { uid: target.uid.wrapping_add(1), ..target };
        assert!(!signal_tracked(&stale_start, libc::SIGKILL), "stale process identity must be refused");
        assert!(!signal_tracked(&stale_uid, libc::SIGKILL), "wrong owner identity must be refused");
        assert!(signal_tracked(&target, libc::SIGSTOP), "actual pidfd SIGSTOP failed; this host is unqualified");
        until(|| fs::read_to_string(format!("/proc/{}/stat", target.pid)).ok()
            .and_then(|s| s.rsplit_once(") ").map(|(_, fields)| fields.starts_with("T "))).unwrap_or(false),
            "target never reached stopped state");
        assert!(sibling.0.try_wait().unwrap().is_none(), "independent sibling was affected by stop");
        assert_eq!(proc_info(sibling_info.pid).unwrap().start_us, sibling_info.start_us);
        assert!(signal_tracked(&target, libc::SIGKILL), "actual pidfd SIGKILL failed; this host is unqualified");
        until(|| leader.0.try_wait().unwrap().is_some(), "namespace launcher did not reap its killed workload");
        assert!(proc_info(target.pid).is_none(), "killed workload must be gone or reaped");
        assert!(sibling.0.try_wait().unwrap().is_none(), "independent sibling was affected by kill");
        assert_eq!(proc_info(sibling_info.pid).unwrap().start_us, sibling_info.start_us);
        assert!(!signal_tracked(&target, libc::SIGKILL), "exited identity must not receive another signal");
        eprintln!("linux pidfd qualification passed: measured {before} bytes; stopped and killed only the owned sandboxed workload; sibling retained");
    }

    #[test]
    fn linux_seccomp_memfd_is_sealed_and_mount_sources_are_pinned() {
        let fd = linux_seccomp().unwrap();
        let mut f = fs::File::from(fd);
        let mut bytes = Vec::new(); f.read_to_end(&mut bytes).unwrap();
        assert!(!bytes.is_empty() && bytes.len() % 8 == 0);
        assert!(f.write_all(b"bad").is_err());
        let d = fixture("fds");
        let source = d.join("source"); fs::create_dir_all(&source).unwrap();
        let mut args = vec![OsString::from("--bind"), source.clone().into_os_string(), OsString::from("/target")];
        let held = linux_bind_fds(&mut args).unwrap();
        assert_eq!(args[0], "--bind-fd");
        let original = fs::metadata(&source).unwrap();
        fs::rename(&source, d.join("moved")).unwrap();
        symlink("/", &source).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(format!("/proc/self/fd/{}", held[0].as_raw_fd())).unwrap().ino(), original.ino());
        let mut race = vec![OsString::from("--bind"), source.into_os_string(), OsString::from("/target")];
        assert!(linux_bind_fds(&mut race).is_err(), "symlink sources must not be followed");
        drop(held); fs::remove_dir_all(d).unwrap();
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn linux_seccomp_x32_guard_keeps_architecture_validation_first() {
        let mut file = fs::File::from(linux_seccomp().unwrap());
        let mut bytes = [0u8; 6 * 8];
        file.read_exact(&mut bytes).unwrap();
        // Evaluate the actual serialized BPF prelude. Native calls continue into
        // the existing rules; both ordinary and high-bit foreign-ABI calls die.
        let evaluate = |arch: u32, syscall: u32| -> Option<u32> {
            let (mut pc, mut accumulator) = (0usize, 0u32);
            while pc < 6 {
                let b = &bytes[pc * 8..(pc + 1) * 8];
                let code = u16::from_ne_bytes([b[0], b[1]]);
                let immediate = u32::from_ne_bytes(b[4..8].try_into().unwrap());
                match code {
                    0x20 => accumulator = match immediate { 0 => syscall, 4 => arch, _ => panic!("unexpected BPF load") },
                    0x15 => pc += usize::from(if accumulator == immediate { b[2] } else { b[3] }),
                    0x45 => pc += usize::from(if accumulator & immediate != 0 { b[2] } else { b[3] }),
                    0x06 => return Some(immediate),
                    _ => panic!("unexpected BPF prelude instruction"),
                }
                pc += 1;
            }
            None
        };
        assert_eq!(evaluate(0xc000003e, libc::SYS_socket as u32), None);
        for syscall in [libc::SYS_socket as u32, libc::SYS_ptrace as u32, 0x7fff] {
            assert_eq!(evaluate(0xc000003e, syscall | 0x40000000), Some(libc::SECCOMP_RET_ERRNO | libc::EPERM as u32));
        }
        for syscall in [libc::SYS_socket as u32, libc::SYS_socket as u32 | 0x40000000] {
            assert_eq!(evaluate(0x40000003, syscall), Some(libc::SECCOMP_RET_KILL_PROCESS));
        }
    }

    /// Qualification test, never silently skipped on an incapable runner. Parent runs --ignored
    /// explicitly with a freshly-built Root binary; missing namespaces/options/seccomp fails it.
    #[test]
    #[ignore = "requires native Linux namespace support and bubblewrap >=0.13.0"]
    fn linux_sealed_real_isolation() {
        let binary = std::env::var_os("INIT_TEST_ROOT_BIN").expect("set INIT_TEST_ROOT_BIN");
        let dir = fixture("sealed-isolation");
        let rh = dir.join("rh"); let run = rh.join("sealed/run/probe"); let tool = dir.join("tool");
        fs::create_dir_all(run.join("bin")).unwrap(); fs::create_dir_all(&tool).unwrap();
        fs::write(rh.join("secret"), "hidden").unwrap();
        let mut args = init_sandbox_profiles::linux::sealed(&run, &rh, &tool, None).unwrap();
        let mut mounts = linux_bind_fds(&mut args).unwrap();
        let filter = linux_seccomp().unwrap();
        let mut cmd = std::process::Command::new(binary);
        cmd.args(["exec-clean", "--keep", "5"]);
        for fd in &mounts { cmd.arg("--keep").arg(fd.as_raw_fd().to_string()); }
        cmd.arg("--").args(args).args(["--", "/usr/bin/python3", "-c", r#"
import os,socket,sys,errno,ctypes,platform
assert not os.path.exists(sys.argv[1]+'/secret')
try: socket.socket(socket.AF_INET)
except OSError as e: assert e.errno==errno.EPERM
else: raise AssertionError('sealed network allowed')
if platform.machine()=='x86_64':
 libc=ctypes.CDLL(None,use_errno=True); libc.syscall.restype=ctypes.c_long
 ctypes.set_errno(0)
 result=libc.syscall(ctypes.c_long(0x40000000|41),socket.AF_INET,socket.SOCK_STREAM,0)
 assert result==-1 and ctypes.get_errno()==errno.EPERM, ('x32 socket was not filtered',result,ctypes.get_errno())
try: open(sys.argv[2]+'/bin/overwrite','w')
except OSError: pass
else: raise AssertionError('sealed binary writable')
open(sys.argv[2]+'/result','w').write('confined')
for fd in os.listdir('/proc/self/fd'):
 try: target=os.readlink('/proc/self/fd/'+fd)
 except OSError: continue
 assert int(fd)<=2, (fd,target)
"#]).arg(&rh).arg(&run).current_dir(&run).env_clear();
        map_seccomp(&mut cmd, filter.as_raw_fd());
        inherit_fds(&mut cmd, mounts.iter().map(AsRawFd::as_raw_fd).collect());
        mounts.push(filter); hold_spawn_fds(&mut cmd, mounts);
        assert!(cmd.status().unwrap().success(), "sealed confinement must actually execute");
        assert_eq!(fs::read_to_string(run.join("result")).unwrap(), "confined");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires qualified Linux user namespaces, fd-bind bubblewrap and INIT_TEST_ROOT_BIN"]
    fn linux_zone_real_isolation() {
        let binary = std::env::var_os("INIT_TEST_ROOT_BIN").expect("set INIT_TEST_ROOT_BIN to the freshly-built init-root");
        let dir = fixture("isolation");
        let l = Layout { root_home: dir.join("rh"), init_home: dir.join("ih"), toolchain: dir.join("tool"), extra_read: vec![], read_deny: vec![], frozen: false, benchmark: false };
        for d in [&l.root_home, &l.init_home, &l.toolchain] { fs::create_dir_all(d).unwrap(); }
        fs::write(l.root_home.join("secret"), "must never be visible").unwrap();
        let mark = l.init_home.join("mark"); fs::write(&mark, "").unwrap();
        let mut p = BTreeMap::new();
        for k in ["SRC", "OUT", "KERNEL"] { let d = l.init_home.join(k.to_lowercase()); fs::create_dir_all(&d).unwrap(); p.insert(k.into(), d.display().to_string()); }
        fs::write(PathBuf::from(&p["SRC"]).join("readonly"), "source").unwrap();
        let mut args = compose(ProfileId::Build, &p, &l, &mark, &Grant::default()).unwrap();
        let mounts = linux_bind_fds(&mut args).unwrap();
        let seccomp = linux_seccomp().unwrap();
        let (mut left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut left4, right4) = std::os::unix::net::UnixStream::pair().unwrap();
        let (a, b) = (dup_high(&right).unwrap(), dup_high(&right4).unwrap());
        left.write_all(b"control3").unwrap(); left4.write_all(b"control4").unwrap();
        let leaked = dup_high(&fs::File::open(l.root_home.join("secret")).unwrap()).unwrap();
        let mut cmd = std::process::Command::new(binary);
        cmd.args(["exec-clean", "--keep", "3", "--keep", "4", "--keep", "5"]);
        for fd in &mounts { cmd.arg("--keep").arg(fd.as_raw_fd().to_string()); }
        cmd.arg("--").args(args).args(["--", "/usr/bin/python3", "-c", r#"
import os,socket,sys,errno,ctypes,platform
assert os.read(3,8)==b'control3'
assert os.read(4,8)==b'control4'
for family in [socket.AF_INET,socket.AF_INET6,getattr(socket,'AF_VSOCK',40)]:
 try: socket.socket(family)
 except OSError as e: assert e.errno==errno.EPERM
 else: raise AssertionError('network family allowed')
if platform.machine()=='x86_64':
 libc=ctypes.CDLL(None,use_errno=True); libc.syscall.restype=ctypes.c_long
 ctypes.set_errno(0)
 result=libc.syscall(ctypes.c_long(0x40000000|41),socket.AF_INET,socket.SOCK_STREAM,0)
 assert result==-1 and ctypes.get_errno()==errno.EPERM, ('x32 socket was not filtered',result,ctypes.get_errno())
a,b=socket.socketpair(); a.send(b'x'); assert b.recv(1)==b'x'; a.close(); b.close()
assert not os.path.exists(sys.argv[1]+'/secret')
try: open(sys.argv[2]+'/readonly','w')
except OSError: pass
else: raise AssertionError('read-only source writable')
open(sys.argv[3]+'/output','w').write('ok')
# Exercise the actual distribution cc selector, GCC helper tools, system headers,
# linker and resulting ELF inside the same Build mount/seccomp confinement.
import subprocess
probe=sys.argv[3]+'/link-probe'
built=subprocess.run(['cc','-x','c','-o',probe,'-'],input=b'#include <stdio.h>\nint main(void) { puts("init-link-probe"); return 0; }\n',stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=30)
assert built.returncode==0, built.stdout.decode(errors='replace')
linked=subprocess.run([probe],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=10)
assert linked.returncode==0 and linked.stdout==b'init-link-probe\n', (linked.returncode,linked.stdout)
for fd in os.listdir('/proc/self/fd'):
 try: target=os.readlink('/proc/self/fd/'+fd)
 except OSError: continue
 assert 'secret' not in target and 'seccomp' not in target, target
 assert int(fd)<=4, (fd,target)
assert os.getuid()==int(sys.argv[4]) and os.getgid()==int(sys.argv[5]), ('the zone must keep the owner ids',os.getuid(),os.getgid())
print('linux isolation passed')
"#]).arg(&l.root_home).arg(&p["SRC"]).arg(&p["OUT"]).arg(euid().to_string()).arg(unsafe { libc::getegid() }.to_string()).current_dir("/").env_clear().env("PATH", "/usr/bin:/bin");
        map_fds(&mut cmd, [a.as_raw_fd(), b.as_raw_fd()]); map_seccomp(&mut cmd, seccomp.as_raw_fd());
        let mut inherited: Vec<_> = mounts.iter().map(AsRawFd::as_raw_fd).collect(); inherited.push(leaked.as_raw_fd());
        inherit_fds(&mut cmd, inherited);
        assert!(cmd.status().unwrap().success(), "confinement must actually run; unsupported host is not a pass");
        assert_eq!(fs::read_to_string(PathBuf::from(&p["OUT"]).join("output")).unwrap(), "ok");
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(target_os = "linux")]
fn linux_proc_stat(pid: i32, stat: &str, uid: u32, hz: u64, boot: u64) -> Option<Proc> {
    // comm may contain spaces and parentheses; fields after its final ')' start at state (#3).
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    if pid <= 0 || fields.first() == Some(&"Z") || hz == 0 { return None; }
    Some(Proc { pid, ppid: fields.get(1)?.parse().ok()?, pgid: fields.get(2)?.parse().ok()?, uid,
        start_us: boot.saturating_mul(1_000_000).saturating_add(fields.get(19)?.parse::<u64>().ok()?.saturating_mul(1_000_000) / hz) })
}

#[cfg(target_os = "linux")]
pub fn proc_info(pid: i32) -> Option<Proc> {
    static CLOCK: std::sync::OnceLock<Option<(u64, u64)>> = std::sync::OnceLock::new();
    let (hz, boot) = (*CLOCK.get_or_init(|| {
        // SAFETY: sysconf has no pointers or side effects.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let boot = stat.lines().find_map(|s| s.strip_prefix("btime "))?.parse().ok()?;
        (hz > 0).then_some((hz as u64, boot))
    }))?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status.lines().find_map(|s| s.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()?;
    linux_proc_stat(pid, &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?, uid, hz, boot)
}

#[cfg(target_os = "linux")]
pub fn proc_table() -> Vec<Proc> {
    std::fs::read_dir("/proc").into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok()).filter_map(proc_info).filter(|p| p.uid == euid()).collect()
}

/// RSS plus swap is a conservative Linux polling estimate; this is not a cgroup hard ceiling.
#[cfg(target_os = "linux")]
pub fn footprint(pid: i32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb = |key: &str| status.lines().find_map(|s| s.strip_prefix(key))?.split_whitespace().next()?.parse::<u64>().ok();
    Some(kb("VmRSS:")?.saturating_add(kb("VmSwap:").unwrap_or(0)).saturating_mul(1024))
}

#[cfg(target_os = "linux")]
pub fn signal_tracked(p: &Proc, sig: c_int) -> bool {
    if p.pid <= 1 || p.pid == std::process::id() as i32 || ![libc::SIGKILL, libc::SIGSTOP].contains(&sig) { return false; }
    // SAFETY: pidfd_open pins the process identity before checking /proc; no pointers.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, p.pid, 0) };
    if fd < 0 { return false; } // no racy kill(pid) fallback on older kernels
    // SAFETY: newly owned descriptor from successful pidfd_open.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as c_int) };
    let same = proc_info(p.pid).is_some_and(|q| q.start_us == p.start_us && q.uid == p.uid && q.uid == euid());
    // SAFETY: held pidfd, permitted signal, null siginfo uses ordinary signal semantics.
    same && sandboxed(p.pid) == Some(true)
        && unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd.as_raw_fd(), sig, std::ptr::null::<libc::siginfo_t>(), 0) } == 0
}

/// Retain pinned resources in Command until spawn completes. The closure only keeps ownership.
#[cfg(target_os = "linux")]
pub fn hold_spawn_fds(cmd: &mut std::process::Command, owned: Vec<OwnedFd>) {
    use std::os::unix::process::CommandExt;
    // SAFETY: no operations after fork; descriptors are dropped with Command in the parent.
    unsafe { cmd.pre_exec(move || { let _keep = &owned; Ok(()) }); }
}

/// LINUX (memwatch `launcher_view`): whether `pid` is a bubblewrap process (`/proc/<pid>/comm`). Read-only.
#[cfg(target_os = "linux")]
pub fn is_bwrap(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim_end() == "bwrap")
}

/// LINUX: `pid` as the innermost pid namespace sees it (the last `NSpid:` field), which is the pid a confined body
/// knows its tool call by. None when it is gone, or off Linux. Read-only.
#[cfg(target_os = "linux")]
pub fn ns_pid(pid: i32) -> Option<i32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|s| s.strip_prefix("NSpid:"))?.split_whitespace().last()?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
pub fn ns_pid(_: i32) -> Option<i32> {
    None
}

/// LINUX (cgroup.rs): the child moves itself into the workload cgroup whose `cgroup.procs` the Root opened (`fd`),
/// after fork and before exec, so every process of the instance starts inside its `memory.max`. A failed write fails
/// the spawn: a workload that was promised a hard limit never runs without it.
#[cfg(target_os = "linux")]
pub fn join_cgroup_on_exec(cmd: &mut std::process::Command, fd: RawFd) {
    use std::os::unix::process::CommandExt;
    // SAFETY: only write(2) after fork (async-signal-safe); the descriptor is held by the parent through spawn.
    unsafe { cmd.pre_exec(move || if libc::write(fd, b"0".as_ptr().cast(), 1) == 1 { Ok(()) } else { Err(io::Error::last_os_error()) }); }
}
