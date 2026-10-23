//! The thin client: connect, send one request line, relay progress, print the final answer and
//! map it to an exit code (kernel/initctl.md §3.1, §4.5). One connection per invocation.

use crate::cli::{CliError, Opts};
use init_proto::envelope::MAX_RESPONSE_LINE;
use init_proto::errors::{ErrorCode, exit};
use init_proto::initctl::{Command, CommandName, WAIT_DEFAULT_S};
use init_proto::{Frame, Response};
use init_proto::serde_json::{self, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Connect timeout (§4.5).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Extra time the client gives the server beyond `wait`'s own timeout to answer `timed_out`.
pub const WAIT_GRACE: Duration = Duration::from_secs(30);

/// `--sock`, else `$INIT_KERNEL_SOCK`, else `$INIT_HOME/run/kernel.sock`.
pub fn socket_path(opts: &Opts, env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    if let Some(s) = &opts.sock {
        return Ok(s.clone());
    }
    if let Some(s) = env("INIT_KERNEL_SOCK").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(s));
    }
    if let Some(h) = env("INIT_HOME").filter(|s| !s.is_empty()) {
        return Ok(Path::new(&h).join("run/kernel.sock"));
    }
    Err(CliError::usage("no kernel socket: pass --sock PATH, or set INIT_KERNEL_SOCK or INIT_HOME"))
}

fn connect(path: &Path) -> io::Result<UnixStream> {
    let (tx, rx) = mpsc::channel();
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(&p));
    });
    match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(r) => r,
        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out after 2 s")),
    }
}

enum ReadErr {
    Timeout,
    TooLong,
    Io(io::Error),
}

/// Read one `\n`-terminated line of at most `max` bytes. `Ok(None)` is EOF (also EOF inside a
/// partial line: the connection dropped before the frame ended).
fn read_line(r: &mut BufReader<UnixStream>, max: usize, deadline: Option<Instant>) -> Result<Option<Vec<u8>>, ReadErr> {
    let mut buf = Vec::new();
    loop {
        if let Some(d) = deadline {
            let left = d.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ReadErr::Timeout);
            }
            // macOS fails setsockopt with EINVAL once the peer has closed; buffered data and EOF
            // are still readable without blocking then, so the error is ignored.
            let _ = r.get_ref().set_read_timeout(Some(left));
        }
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Err(ReadErr::Timeout),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadErr::Io(e)),
        };
        if chunk.is_empty() {
            return Ok(None);
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                buf.extend_from_slice(&chunk[..i]);
                r.consume(i + 1);
                if buf.len() > max {
                    return Err(ReadErr::TooLong);
                }
                return Ok(Some(buf));
            }
            None => {
                let n = chunk.len();
                buf.extend_from_slice(chunk);
                r.consume(n);
                if buf.len() > max {
                    return Err(ReadErr::TooLong);
                }
            }
        }
    }
}

/// `[j-000047 G2 55% eta 40s] probe: waiting for ready`
pub fn render_progress(p: &init_proto::initctl::Progress) -> String {
    let stage = p.stage.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| p.state.as_str().to_string());
    let eta = p.eta_s.map(|e| format!(" eta {e}s")).unwrap_or_default();
    format!("[{} {} {}%{}] {}", p.job, stage, p.pct, eta, p.msg)
}

/// The exit code of a final response for `cmd` (§4.5).
pub fn exit_code(cmd: &Command, r: &Response) -> u8 {
    let v = match &r.result {
        Err(e) => return e.code.exit_code(),
        Ok(v) => v,
    };
    if v.get("timed_out") == Some(&Value::Bool(true)) {
        return exit::TIMED_OUT;
    }
    match cmd.name() {
        CommandName::Gate if v.get("pass") == Some(&Value::Bool(false)) => exit::FAILED,
        CommandName::TaskSubmit if v.get("admitted") == Some(&Value::Bool(false)) => exit::FAILED,
        // A wait that returns a failed or cancelled record exits with that job's error code.
        CommandName::JobWait => match v.get("state").and_then(Value::as_str) {
            Some("failed") | Some("cancelled") => v
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .map(|c| ErrorCode::parse(c).exit_code())
                .filter(|&c| c != exit::OK)
                .unwrap_or(exit::FAILED),
            _ => exit::OK,
        },
        _ => exit::OK,
    }
}

/// The client-side deadline for the final response.
fn deadline(opts: &Opts, cmd: &Command) -> Option<Instant> {
    let d = match cmd {
        Command::JobWait(a) => Some(Duration::from_secs(a.timeout_s.unwrap_or(WAIT_DEFAULT_S)) + WAIT_GRACE),
        _ => opts.timeout.map(Duration::from_secs),
    };
    d.map(|d| Instant::now() + d)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Send `cmd` and relay the answer. Returns the exit code.
pub fn run(opts: &Opts, cmd: &Command, env: &dyn Fn(&str) -> Option<String>, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let path = match socket_path(opts, env) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(err, "initctl: {}", e.message);
            return e.code;
        }
    };
    let deadline = deadline(opts, cmd);
    let stream = match connect(&path) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "initctl: cannot connect to {}: {e}", path.display());
            return exit::UNAVAILABLE;
        }
    };
    let id = format!("{}-1", std::process::id());
    let req = cmd.to_request(id.clone());
    if let Err(e) = (&stream).write_all(&req.to_line()).and_then(|_| (&stream).flush()) {
        let _ = writeln!(err, "initctl: connection to {} dropped while sending: {e}", path.display());
        return exit::UNAVAILABLE;
    }
    let mut reader = BufReader::new(stream);
    let mut last_job: Option<String> = None;
    loop {
        let line = match read_line(&mut reader, MAX_RESPONSE_LINE, deadline) {
            Ok(Some(l)) => l,
            Ok(None) => {
                let _ = writeln!(err, "initctl: the kernel closed the connection before the final response");
                return exit::UNAVAILABLE;
            }
            Err(ReadErr::Timeout) => {
                let job = last_job.as_deref().map(|j| format!("; the job continues: initctl wait {j}")).unwrap_or_default();
                let _ = writeln!(err, "initctl: timed out waiting for the kernel{job}");
                return exit::TIMED_OUT;
            }
            Err(ReadErr::TooLong) => {
                let _ = writeln!(err, "initctl: response line longer than 1 MiB");
                return exit::PROTOCOL;
            }
            Err(ReadErr::Io(e)) => {
                let _ = writeln!(err, "initctl: connection error: {e}");
                return exit::UNAVAILABLE;
            }
        };
        if opts.json {
            let _ = out.write_all(&line);
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
        match Frame::parse_line(&line) {
            Err(e) => {
                let _ = writeln!(err, "initctl: unparsable response: {e}");
                return exit::PROTOCOL;
            }
            Ok(Frame::Progress(p)) => {
                if p.id != id {
                    let _ = writeln!(err, "initctl: progress frame for another request ({})", p.id);
                    return exit::PROTOCOL;
                }
                last_job = Some(p.progress.job.clone());
                if !opts.json && !opts.quiet {
                    let _ = writeln!(err, "{}", render_progress(&p.progress));
                }
            }
            Ok(Frame::Final(r)) => {
                if r.id.as_deref().is_some_and(|rid| rid != id) {
                    let _ = writeln!(err, "initctl: response for another request ({})", r.id.unwrap_or_default());
                    return exit::PROTOCOL;
                }
                if !opts.json {
                    let doc = match &r.result {
                        Ok(v) => pretty(v),
                        Err(e) => pretty(&serde_json::json!({ "error": e })),
                    };
                    let _ = writeln!(out, "{doc}");
                    let _ = out.flush();
                }
                return exit_code(cmd, &r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use init_proto::ErrorBody;
    use init_proto::initctl::{JobState, Progress, Stage};
    use init_proto::serde_json::json;

    #[test]
    fn progress_line_format() {
        let p = Progress::new("j-000047", JobState::Gating, Some(Stage::G2), 55, Some(40), "probe: waiting for ready", "t");
        assert_eq!(render_progress(&p), "[j-000047 G2 55% eta 40s] probe: waiting for ready");
        let p = Progress::new("j-000046", JobState::Running, None, 2, None, "fetching", "t");
        assert_eq!(render_progress(&p), "[j-000046 running 2%] fetching");
    }

    #[test]
    fn exit_codes_of_results() {
        let gate = Command::parse("gate", &json!({"quick": true, "repo": "work/wt", "rev": "HEAD"})).unwrap();
        assert_eq!(exit_code(&gate, &Response::ok_value("1", json!({"pass": false}))), 1);
        assert_eq!(exit_code(&gate, &Response::ok_value("1", json!({"pass": true}))), 0);
        let sub = Command::parse("task.submit", &json!({"dir": "/h/work/t"})).unwrap();
        assert_eq!(exit_code(&sub, &Response::ok_value("1", json!({"admitted": false}))), 1);
        let wait = Command::parse("wait", &json!({"job": "j-000001"})).unwrap();
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "awaiting_boundary"}))), 0);
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "awaiting_swap"}))), 0);
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "running", "timed_out": true}))), 2);
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "failed", "error": {"code": "E_PAUSE_TIMEOUT"}}))), 1);
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "failed", "error": {"code": "E_ROOT"}}))), 75);
        assert_eq!(exit_code(&wait, &Response::ok_value("1", json!({"state": "cancelled", "error": null}))), 1);
        let ping = Command::Ping;
        assert_eq!(exit_code(&ping, &Response::err(Some("1".into()), ErrorBody::new(ErrorCode::Internal, "bug"))), 70);
        assert_eq!(exit_code(&ping, &Response::err(None, ErrorBody::new(ErrorCode::Peer, "no"))), 77);
    }

    #[test]
    fn socket_selection() {
        let none = |_: &str| None;
        let o = Opts::default();
        assert_eq!(socket_path(&o, &none).unwrap_err().code, 64);
        let home = |k: &str| (k == "INIT_HOME").then(|| "/h".to_string());
        assert_eq!(socket_path(&o, &home).unwrap(), PathBuf::from("/h/run/kernel.sock"));
        let both = |k: &str| match k {
            "INIT_HOME" => Some("/h".to_string()),
            "INIT_KERNEL_SOCK" => Some("/p/probe/1/run/kernel.sock".to_string()),
            _ => None,
        };
        assert_eq!(socket_path(&o, &both).unwrap(), PathBuf::from("/p/probe/1/run/kernel.sock"));
        let o = Opts { sock: Some("/x.sock".into()), ..Opts::default() };
        assert_eq!(socket_path(&o, &both).unwrap(), PathBuf::from("/x.sock"));
    }
}
