//! The in-sandbox probe (`sandbox-selftest-inner`, kernel/sandbox.md §8): performs exactly one
//! operation and prints one JSON line, so the suite needs no python or bash inside the sandbox.
//!
//! Output: `{"op":…,"ok":bool,"errno":n|null,"msg":…,"data":…}` on stdout; exit 0 whenever the
//! operation was attempted (allowed or refused), 2 on a usage error. Three ops (`wait-stdin`,
//! `serve-once`, `serve-lane`) first print a `{"ready":true,"pid":n}` line.
//!
//! Safety: the probe signals only `kill -0` targets it is handed by its runner (which spawned
//! them) and its own child (`spawn-term-child`). It never scans processes.

use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::sys;

fn io_result<T>(op: &str, r: io::Result<T>) -> Value {
    match r {
        Ok(_) => json!({"op": op, "ok": true, "errno": null, "msg": "", "data": null}),
        Err(e) => {
            json!({"op": op, "ok": false, "errno": e.raw_os_error(), "msg": e.to_string(), "data": null})
        }
    }
}

fn with_data(mut v: Value, data: Value) -> Value {
    v["data"] = data;
    v
}

fn setup_error(op: &str, stage: &str, e: io::Error) -> Value {
    // errno -1: never mistaken for a sandbox refusal.
    json!({"op": op, "ok": false, "errno": -1, "msg": format!("setup ({stage}): {e}"), "data": null})
}

fn tail(b: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(b);
    let start = s.len().saturating_sub(n);
    let start = (start..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
    s[start..].to_string()
}

/// The kernel's lane-socket sequence (kernel-serve `add_lane` after FIX1, kernel-proxy
/// `bind_socket`): create the directory; chmod it 0700 only when its mode differs (tail Z4 refuses
/// any chmod of `run/` itself, which is what broke `run/kernel.sock` in Slice-1); unlink a stale
/// socket by its exact path; bind; chmod the socket 0600. A failure names its stage.
fn lane_bind(path: &Path) -> Result<UnixListener, (&'static str, io::Error)> {
    let dir = path.parent().ok_or(("parent", io::Error::other("no parent directory")))?;
    std::fs::create_dir_all(dir).map_err(|e| ("mkdir", e))?;
    let mode = std::fs::metadata(dir).map_err(|e| ("stat-dir", e))?.permissions().mode() & 0o777;
    if mode != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| ("chmod-dir", e))?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => std::fs::remove_file(path).map_err(|e| ("unlink-stale", e))?,
        Ok(_) => return Err(("exists", io::Error::from(io::ErrorKind::AlreadyExists))),
        Err(_) => {}
    }
    let l = UnixListener::bind(path).map_err(|e| ("bind", e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| ("chmod-sock", e))?;
    Ok(l)
}

fn stage_error(op: &str, stage: &str, e: io::Error) -> Value {
    json!({"op": op, "ok": false, "errno": e.raw_os_error(), "msg": format!("{stage}: {e}"), "data": {"stage": stage}})
}

/// Accept exactly one connection on `l` and read one line from it.
fn accept_line(op: &str, l: &UnixListener) -> Value {
    let r = l.accept().and_then(|(s, _)| {
        let mut line = String::new();
        io::BufReader::new(s).read_line(&mut line).map(|_| line)
    });
    match r {
        Ok(line) => with_data(io_result(op, Ok(())), json!({"line": line.trim_end()})),
        Err(e) => io_result(op, Err::<(), _>(e)),
    }
}

fn print_line(v: &Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// Entry point: `args` = `[op, operands…]`. Returns the process exit code.
pub fn inner_main(args: &[String]) -> i32 {
    let Some((op, a)) = args.split_first() else {
        eprintln!("usage: <probe> <op> [operands…]");
        return 2;
    };
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    match run(op, &a) {
        Some(v) => {
            print_line(&v);
            0
        }
        None => {
            eprintln!("probe: unknown op or wrong operands: {op} {a:?}");
            2
        }
    }
}

fn run(op: &str, a: &[&str]) -> Option<Value> {
    let p = |i: usize| Path::new(a[i]);
    let v = match (op, a.len()) {
        ("noop", 0) => io_result(op, Ok(())),
        ("write", 1) => io_result(op, std::fs::write(p(0), b"wp6a\n")),
        ("read", 1) => io_result(op, std::fs::read(p(0))),
        ("list", 1) => match std::fs::read_dir(p(0)) {
            Ok(rd) => with_data(io_result(op, Ok(())), json!({"entries": rd.count()})),
            Err(e) => io_result(op, Err::<(), _>(e)),
        },
        ("stat", 1) => io_result(op, std::fs::metadata(p(0))),
        ("mkdir", 1) => io_result(op, std::fs::create_dir(p(0))),
        ("rename", 2) => io_result(op, std::fs::rename(p(0), p(1))),
        ("unlink", 1) => io_result(op, std::fs::remove_file(p(0))),
        ("rmdir", 1) => io_result(op, std::fs::remove_dir(p(0))),
        ("hardlink", 2) => io_result(op, std::fs::hard_link(p(0), p(1))),
        // symlink-write LINK TARGET FILE: create LINK -> TARGET, then write FILE (through it).
        ("symlink-write", 3) => match std::os::unix::fs::symlink(p(1), p(0)) {
            Ok(()) => io_result(op, std::fs::write(p(2), b"wp6a\n")),
            Err(e) => setup_error(op, "symlink", e),
        },
        // symlink-connect LINK TARGET: create LINK -> TARGET, then connect to LINK.
        ("symlink-connect", 2) => match std::os::unix::fs::symlink(p(1), p(0)) {
            Ok(()) => io_result(op, UnixStream::connect(p(0))),
            Err(e) => setup_error(op, "symlink", e),
        },
        ("connect", 1) => io_result(op, UnixStream::connect(p(0))),
        ("bind", 1) => io_result(op, UnixListener::bind(p(0))),
        // socket DOMAIN TYPE: create and close a socket; no address, no traffic.
        ("socket", 2) => {
            let domain = match a[0] {
                "unix" => libc::AF_UNIX,
                "inet" => libc::AF_INET,
                "inet6" => libc::AF_INET6,
                _ => return None,
            };
            let ty = match a[1] {
                "stream" => libc::SOCK_STREAM,
                "dgram" => libc::SOCK_DGRAM,
                _ => return None,
            };
            io_result(op, sys::socket_probe(domain, ty))
        }
        // tcp-connect PORT / udp-send PORT: to 127.0.0.1:PORT, a socket the runner itself holds.
        // Nothing leaves the host and no foreign service is touched.
        ("tcp-connect", 1) => {
            let port: u16 = a[0].parse().ok()?;
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            io_result(op, std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2)))
        }
        // tcp6-connect PORT: to [::1]:PORT, the runner's own IPv6 listener.
        ("tcp6-connect", 1) => {
            let port: u16 = a[0].parse().ok()?;
            let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port));
            io_result(op, std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2)))
        }
        ("udp-send", 1) => io_result(op, sys::udp_send_loopback(a[0].parse().ok()?)),
        // Loopback binds on an ephemeral port: local only, nothing is sent.
        ("tcp-bind-loopback", 0) => io_result(op, std::net::TcpListener::bind("127.0.0.1:0")),
        ("udp-bind-loopback", 0) => io_result(op, std::net::UdpSocket::bind("127.0.0.1:0")),
        // kill0 PID: signal 0 (no signal is delivered), to a PID the runner spawned.
        ("kill0", 1) => io_result(op, sys::kill(a[0].parse().ok()?, 0)),
        ("exec", n) if n >= 1 => match Command::new(a[0]).args(&a[1..]).stdin(Stdio::null()).output() {
            Ok(out) => {
                let data = json!({"code": out.status.code(), "signal": out.status.signal()});
                let msg = if out.status.success() { String::new() } else { tail(&out.stderr, 600) };
                json!({"op": op, "ok": out.status.success(), "errno": null, "msg": msg, "data": data})
            }
            Err(e) => io_result(op, Err::<(), _>(e)),
        },
        ("devfd", 0) => with_data(io_result(op, Ok(())), json!({"fds": sys::open_fds(1024)})),
        // spawn-term-child: start /bin/sleep, SIGTERM it by the PID just returned, reap it.
        ("spawn-term-child", 0) => match Command::new("/bin/sleep").arg("5").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(mut child) => {
                let pid = child.id() as i32;
                let k = sys::kill(pid, libc::SIGTERM);
                if k.is_err() {
                    let _ = child.kill(); // the same PID; then reap
                }
                let st = child.wait().ok();
                with_data(io_result(op, k), json!({"signal": st.and_then(|s| s.signal())}))
            }
            Err(e) => setup_error(op, "spawn", e),
        },
        // wait-stdin: announce our pid, then block until stdin reaches EOF.
        ("wait-stdin", 0) => {
            print_line(&json!({"ready": true, "pid": std::process::id()}));
            let mut sink = Vec::new();
            io_result(op, io::stdin().lock().read_to_end(&mut sink))
        }
        // serve-once PATH: bind, announce, accept exactly one connection, read one line.
        ("serve-once", 1) => match UnixListener::bind(p(0)) {
            Ok(l) => {
                print_line(&json!({"ready": true, "pid": std::process::id()}));
                accept_line(op, &l)
            }
            Err(e) => io_result(op, Err::<(), _>(e)),
        },
        // chmod MODE PATH (octal MODE): the S1 gap-3 operation is `chmod 700 run/`.
        ("chmod", 2) => {
            let mode = u32::from_str_radix(a[0], 8).ok()?;
            io_result(op, std::fs::set_permissions(p(1), std::fs::Permissions::from_mode(mode)))
        }
        // lane-bind PATH: the kernel's lane-socket sequence, then close the listener.
        ("lane-bind", 1) => match lane_bind(p(0)) {
            Ok(_) => io_result(op, Ok(())),
            Err((stage, e)) => stage_error(op, stage, e),
        },
        // serve-lane PATH: the lane-socket sequence, announce, accept one connection, read one line.
        ("serve-lane", 1) => match lane_bind(p(0)) {
            Ok(l) => {
                print_line(&json!({"ready": true, "pid": std::process::id()}));
                accept_line(op, &l)
            }
            Err((stage, e)) => stage_error(op, stage, e),
        },
        // connect-send PATH: connect and send one line (the peer of serve-once).
        ("connect-send", 1) => io_result(op, UnixStream::connect(p(0)).and_then(|mut s| s.write_all(b"hello from body\n"))),
        #[cfg(target_os = "macos")]
        ("sbcheck", n) if n >= 1 => {
            // sbcheck PID [PATH…]: sandbox_check(PID, NULL, 0) and file-read-data per PATH.
            let pid: i32 = a[0].parse().ok()?;
            let sandboxed = sys::sbcheck::sandboxed(pid);
            let paths: Vec<Option<i32>> = a[1..].iter().map(|x| sys::sbcheck::path(pid, "file-read-data", Path::new(x))).collect();
            with_data(io_result(op, Ok(())), json!({"sandboxed": sandboxed, "paths": paths}))
        }
        #[cfg(target_os = "macos")]
        ("policy-write", 1) => {
            // A pure policy query: may this process write PATH? Nothing is written. PATH must exist:
            // for a missing path sandbox_check reports "denied" even unsandboxed (sandbox.md §3.3.4).
            let rc = sys::sbcheck::path(std::process::id() as i32, "file-write-create", p(0));
            with_data(io_result(op, Ok(())), json!({"rc": rc}))
        }
        _ => return None,
    };
    Some(v)
}
