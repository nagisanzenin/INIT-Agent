//! The mock as a server: TCP (upstream mode) and Unix socket (socket mode), keep-alive, chunked
//! request bodies, cut and stall faults over a real connection, and the `init-mockllm` binary.

use init_testkit::mockllm::assemble;
use init_testkit::mockllm::client::{self, exchange};
use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::sse::parse_all;
use init_testkit::mockllm::{Mock, MockConfig, MockRequest, Mode, Script, read_log, summarize};
use serde_json::{Value, json};
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn test_home(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp2");
    std::fs::create_dir_all(&base).unwrap();
    let d = base.canonicalize().unwrap().join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap(); // exact path, owned by this test
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ds_body(messages: Value) -> String {
    json!({"messages": messages, "model": "deepseek-flash", "stream": true, "thinking": {"type": "enabled"},
           "tools": [{"function": {"name": "bash", "parameters": {"type": "object"}}, "type": "function"}]})
    .to_string()
}

fn hi() -> Value {
    json!([{"content": "SYSTEM", "role": "system"}, {"content": "hi", "role": "user"}])
}

const SCRIPT: &str = r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"server","chunk":4}
{"v":1,"kind":"turn","reply":{"think":"Greet.","text":"Hello there!","stop":"end_turn"}}
{"v":1,"kind":"turn","reply":{"calls":[{"name":"bash","input":{"command":"ls -la"}}]}}
{"v":1,"kind":"turn","fault":{"kind":"cut","after_events":3}}
{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":2,"ms":150},"reply":{"text":"late"}}
"#;

fn server(mode: Mode, listen: Listen) -> MockServer {
    let script = Script::parse(SCRIPT, Path::new(".")).unwrap();
    MockServer::start(Mock::new(MockConfig::new(mode, script)).unwrap(), listen).unwrap()
}

#[test]
fn upstream_tcp_keep_alive_cut_and_stall() {
    let _wd = init_testkit::watchdog::arm("upstream_tcp_keep_alive_cut_and_stall", WD);
    let s = server(Mode::Upstream, Listen::Tcp("127.0.0.1:0".into()));
    let addr = s.addr().unwrap().to_string();
    let req = MockRequest::post(
        "/chat/completions",
        &[("content-type", "application/json")],
        ds_body(hi()),
    );

    // Two requests on one keep-alive connection.
    let mut conn = BufReader::new(TcpStream::connect(&addr).unwrap());
    let r1 = exchange(&mut conn, &req, false).unwrap();
    assert_eq!((r1.status, r1.complete), (200, true));
    assert_eq!(r1.header("transfer-encoding"), Some("chunked"));
    assert!(
        r1.header("content-type")
            .unwrap()
            .starts_with("text/event-stream")
    );
    let a1 = assemble::deepseek(&parse_all(&r1.body).unwrap()).unwrap();
    assert_eq!(
        (a1.reasoning.as_str(), a1.text.as_str()),
        ("Greet.", "Hello there!")
    );
    let r2 = exchange(&mut conn, &req, false).unwrap();
    let a2 = assemble::deepseek(&parse_all(&r2.body).unwrap()).unwrap();
    assert_eq!(a2.calls[0].args, r#"{"command":"ls -la"}"#);

    // A cut closes the connection before the terminating chunk.
    let r3 = exchange(&mut conn, &req, false).unwrap();
    assert_eq!(r3.status, 200);
    assert!(!r3.complete, "cut stream must be incomplete");
    assert_eq!(parse_all(&r3.body).unwrap().len(), 3);
    let mut rest = Vec::new();
    assert_eq!(
        conn.read_to_end(&mut rest).unwrap(),
        0,
        "connection closed after a cut"
    );

    // A stall is real silence on the wire.
    let t = Instant::now();
    let r4 = client::request_tcp(&addr, &req).unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(150),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(
        assemble::deepseek(&parse_all(&r4.body).unwrap())
            .unwrap()
            .text,
        "late"
    );

    let log = s.mock().log();
    assert_eq!(log.len(), 4);
    assert!(log[3].t_done_ms >= log[3].t_recv_ms + 150);
    assert_eq!(log[2].fault.as_deref(), Some("cut"));
    s.stop();
}

#[test]
fn chunked_request_body_and_expect_continue() {
    let _wd = init_testkit::watchdog::arm("chunked_request_body_and_expect_continue", WD);
    let s = server(Mode::Upstream, Listen::Tcp("127.0.0.1:0".into()));
    let body = ds_body(hi());
    let mut c = TcpStream::connect(s.addr().unwrap()).unwrap();
    let (a, b) = body.as_bytes().split_at(body.len() / 2);
    write!(
        c,
        "POST /chat/completions HTTP/1.1\r\nhost: x\r\nexpect: 100-continue\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    for part in [a, b] {
        write!(c, "{:x}\r\n", part.len()).unwrap();
        c.write_all(part).unwrap();
        c.write_all(b"\r\n").unwrap();
    }
    c.write_all(b"0\r\n\r\n").unwrap();
    let mut r = BufReader::new(c);
    // read_response skips the interim 100 Continue.
    let resp = client::read_response(&mut r).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(
        assemble::deepseek(&parse_all(&resp.body).unwrap())
            .unwrap()
            .text,
        "Hello there!"
    );
    // Malformed request lines get a 400 and a closed connection.
    let mut bad = TcpStream::connect(s.addr().unwrap()).unwrap();
    bad.write_all(b"NONSENSE\r\n\r\n").unwrap();
    let resp = client::read_response(&mut BufReader::new(bad)).unwrap();
    assert_eq!(resp.status, 400);
}

#[test]
fn socket_mode_over_unix_socket() {
    let _wd = init_testkit::watchdog::arm("socket_mode_over_unix_socket", WD);
    let home = test_home("unix-socket");
    let sock = home.join("proxy.sock");
    assert!(sock.as_os_str().len() <= 103);
    let s = server(Mode::Socket, Listen::Unix(sock.clone()));
    let req = MockRequest::post(
        "/deepseek/chat/completions",
        &[
            ("x-init-session", "20261020-010203-abcdef"),
            ("x-init-purpose", "turn"),
            ("x-init-attempt", "1"),
        ],
        ds_body(hi()),
    );
    let r = client::request_unix(&sock, &req).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.header("connection"), Some("close"));
    assert_eq!(
        assemble::deepseek(&parse_all(&r.body).unwrap())
            .unwrap()
            .text,
        "Hello there!"
    );
    let st = client::request_unix(&sock, &MockRequest::get("/_init/stats")).unwrap();
    let st: Value = serde_json::from_slice(&st.body).unwrap();
    assert_eq!(st["requests_today"], 1);
    let log = s.mock().log();
    assert_eq!(log[0].session, "20261020-010203-abcdef");
    s.stop();
    assert!(!sock.exists(), "stop removes the socket it created");
}

// ------------------------------------------------------------------ the binary

/// Kills the child (by its own PID, via `Child::kill`) when dropped.
struct Spawned(Child);
impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &Path) {
    let t = Instant::now();
    while !path.exists() {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn binary_upstream_mode_with_port_file_and_strict_checks() {
    let _wd = init_testkit::watchdog::arm("binary_upstream_mode_with_port_file_and_strict_checks", WD);
    let home = test_home("bin-upstream");
    let script = home.join("s.mock.jsonl");
    std::fs::write(&script, SCRIPT).unwrap();
    let port_file = home.join("mock.port");
    let rec = home.join("rec");
    let child = init_testkit::watchdog::spawn(Command::new(env!("CARGO_BIN_EXE_init-mockllm"))
        .args([
            "--mode",
            "upstream",
            "--listen",
            "127.0.0.1:0",
            "--strict",
            "deepseek",
            "--simulate-cache",
        ])
        .arg("--port-file")
        .arg(&port_file)
        .arg("--script")
        .arg(&script)
        .arg("--record-dir")
        .arg(&rec)
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap();
    let guard = Spawned(child);
    wait_for(&port_file);
    let port: u16 = std::fs::read_to_string(&port_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let addr = format!("127.0.0.1:{port}");
    let ok = client::request_tcp(
        &addr,
        &MockRequest::post("/chat/completions", &[], ds_body(hi())),
    )
    .unwrap();
    assert_eq!(ok.status, 200);
    // History without reasoning_content while tools are present: strict 400.
    let bad = json!([{"content": "S", "role": "system"}, {"content": "a", "role": "user"},
                     {"content": "x", "role": "assistant"}, {"content": "b", "role": "user"}]);
    let r = client::request_tcp(
        &addr,
        &MockRequest::post("/chat/completions", &[], ds_body(bad)),
    )
    .unwrap();
    assert_eq!(r.status, 400);
    assert_eq!(
        String::from_utf8(r.body).unwrap(),
        r#"{"error":{"message":"reasoning_content missing in assistant message 2","type":"invalid_request_error"}}"#
    );
    // The proxy must strip x-init-*: a leaked header is a protocol error in upstream mode.
    let leaked = MockRequest::post(
        "/chat/completions",
        &[("x-init-session", "s")],
        ds_body(hi()),
    );
    assert_eq!(client::request_tcp(&addr, &leaked).unwrap().status, 400);
    drop(guard);
    let log = read_log(&rec.join("mock.log.jsonl")).unwrap();
    let s = summarize(&log);
    assert_eq!((s.requests, s.protocol_errors), (3, 2));
    assert_eq!(log[0].cache_read, Some(0));
    assert!(rec.join("000001-turn.json").exists());
}

#[test]
fn binary_socket_mode_and_exit_codes() {
    let _wd = init_testkit::watchdog::arm("binary_socket_mode_and_exit_codes", WD);
    let home = test_home("bin-socket");
    let sock = home.join("m.sock");
    let child = init_testkit::watchdog::spawn(Command::new(env!("CARGO_BIN_EXE_init-mockllm"))
        .args(["--mode", "socket", "--socket"])
        .arg(&sock)
        .stdout(Stdio::null())
        .stderr(Stdio::null()))
        .unwrap();
    let guard = Spawned(child);
    wait_for(&sock);
    // Probe requests are served by the built-in responder even with no script.
    let probe = json!([{"content": "S", "role": "system"}, {"content": "[init:probe] Gate probe", "role": "user"}]);
    let req = MockRequest::post(
        "/deepseek/chat/completions",
        &[("x-init-session", "p"), ("x-init-purpose", "probe")],
        ds_body(probe),
    );
    let mut r = None;
    for _ in 0..50 {
        // The socket file appears at bind; accept may need a moment.
        if let Ok(x) = client::request_unix(&sock, &req) {
            r = Some(x);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let r = r.expect("mock answered");
    let a = assemble::deepseek(&parse_all(&r.body).unwrap()).unwrap();
    assert_eq!(a.calls[0].args, r#"{"command":"true"}"#);
    drop(guard);

    let code = |args: &[&str]| {
        init_testkit::watchdog::status(Command::new(env!("CARGO_BIN_EXE_init-mockllm"))
            .args(args)
            .stderr(Stdio::null()))
            .unwrap()
            .code()
    };
    assert_eq!(code(&["--bogus"]), Some(64));
    assert_eq!(code(&["--mode", "socket"]), Some(64));
    let bad_script = home.join("bad.mock.jsonl");
    std::fs::write(&bad_script, "{\"v\":1,\"kind\":\"turn\"}\n").unwrap();
    assert_eq!(code(&["--script", bad_script.to_str().unwrap()]), Some(66));
    assert_eq!(code(&["--help"]), Some(0));
}
