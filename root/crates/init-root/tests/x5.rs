//! X5 integration test (ADR-011 §5.2): the attribution tap on the live egress path, end to end. A test-mode Root
//! forwards the recorded X5 fixtures (tests/fixtures/x5/, all three families) from a loopback upstream; the bytes
//! the kernel receives must be the recorded bytes, unchanged, and `x-egress-caps` must never reach the upstream.
//! Keys are fake (`test-…`); no real API is called ($0). The home is `root/target/test-homes/x5/<name>`, with
//! `HOME` pointed there; the daemon is killed by its own handle.
#![cfg(feature = "test-fixtures")]

use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");
const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/x5");

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait(secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "condition not met within {secs} s");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The request heads an upstream saw.
type Heads = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// A loopback upstream answering each request with the next queued reply, recording the request heads.
fn upstream(replies: Vec<Vec<u8>>) -> (u16, Heads) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (seen, q) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(VecDeque::from(replies))));
    let s = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            let (s, q) = (s.clone(), q.clone());
            std::thread::spawn(move || serve(c, &s, &q));
        }
    });
    (port, seen)
}

fn serve(c: TcpStream, seen: &Mutex<Vec<Vec<(String, String)>>>, q: &Mutex<VecDeque<Vec<u8>>>) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    let mut first = String::new();
    if r.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" || l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').unwrap();
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let n: usize = headers.iter().find(|(k, _)| k == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; n];
    r.read_exact(&mut body).unwrap();
    seen.lock().unwrap().push(headers);
    let reply = q.lock().unwrap().pop_front().unwrap();
    let _ = w.write_all(&reply);
}

/// One request on the egress socket, as the kernel proxy sends it; returns (status, head, de-chunked body).
fn request(sock: &Path, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut s = UnixStream::connect(sock).unwrap();
    let mut h = format!("POST {path} HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    s.write_all(format!("{h}\r\n").as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..at]).to_string();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let (rest, mut body, mut i) = (&raw[at + 4..], Vec::new(), 0);
    while let Some(e) = rest[i..].windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(std::str::from_utf8(&rest[i..i + e]).unwrap(), 16).unwrap();
        if n == 0 {
            break;
        }
        body.extend_from_slice(&rest[i + e + 2..i + e + 2 + n]);
        i += e + 2 + n + 2;
    }
    (status, head, body)
}

/// RF-19 on the wire: each family's recorded stream reaches the kernel byte for byte through the Root (the tap
/// reads copies after the relay and never alters a frame), the zone's `x-egress-caps` and `x-init-*` headers never
/// reach the upstream, and each `end` record keeps its usage and finish.
#[test]
fn x5_live_relay_is_unchanged_and_caps_never_leave() {
    let dir = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap().join("test-homes/x5/wire");
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, made by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    let (rh, ih): (PathBuf, PathBuf) = (dir.join("rh"), dir.join("ih"));
    let cases = [("chat.sse", "/deepseek/chat/completions"), ("messages.sse", "/deepseek/anthropic/v1/messages"), ("responses.sse", "/deepseek/responses")];
    let fixtures: Vec<Vec<u8>> = cases.iter().map(|(f, _)| fs::read(Path::new(FIX).join(f)).unwrap()).collect();
    let mut replies: Vec<Vec<u8>> = fixtures.iter().map(|b| {
        let mut r = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", b.len()).into_bytes();
        r.extend_from_slice(b);
        r
    }).collect();
    replies.push(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_vec());
    let (port, seen) = upstream(replies);
    let charter = format!("schema_version = 1\n[purpose]\ntext = \"X5 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\ndisk_min_free_gib = 0\n\
        [sealed]\nreserve_usd = \"0.00\"\naudit_usd = \"0.00\"\n[root]\ntest_mode = true\n[money]\nday_usd = \"5.00\"\ntotal_usd = \"10.00\"\n\
        [actions.providers]\ndestinations = [\"127.0.0.1:{port}\"]\npaths = [\"chat/completions\", \"anthropic/v1/messages\", \"responses\"]\n\
        [actions.providers.models.\"deepseek-flash\"]\nmin_level = 0\ninput_musd_per_mtok = 300000\noutput_musd_per_mtok = 1200000\n\
        [actions.providers.providers.deepseek]\nhost = \"127.0.0.1:{port}\"\n\
        [actions.post-api]\nkind = \"http\"\nmin_level = 0\ndestinations = [\"127.0.0.1:{port}\"]\nmethods = [\"POST\"]\npaths = [\"*\"]\nrequests_per_day = 100\nbytes_per_day = 1000000\nrate_per_min = 60\n");
    let cmd = || {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", &dir).env("INIT_ROOT_HOME", &rh).env("INIT_HOME", &ih).env("PATH", "/usr/bin:/bin");
        c
    };
    fs::write(dir.join("charter.toml"), charter).unwrap();
    let o = cmd().args(["install", "--charter", dir.join("charter.toml").to_str().unwrap()]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
    let key = rh.join("keys/providers/deepseek");
    fs::write(&key, "test-key-x5-0000").unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let _d = Daemon(cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let sock = rh.join("run/egress.sock");
    wait(10, || UnixStream::connect(&sock).is_ok());
    // the agent's view of its caps is published at start, before any turn (ICJ, 0644)
    let caps: Value = serde_json::from_str(&fs::read_to_string(ih.join("root/caps.json")).unwrap()).unwrap();
    assert_eq!((caps["caps"].as_array().map(Vec::len), caps["v"].as_u64(), caps["day"].as_str().map(str::len)), (Some(0), Some(1), Some(10)), "{caps}");
    assert_eq!(fs::metadata(ih.join("root/caps.json")).unwrap().permissions().mode() & 0o777, 0o644);
    let bodies = [r#"{"max_tokens":256,"messages":[],"model":"deepseek-flash","stream":true}"#,
        r#"{"max_tokens":256,"messages":[],"model":"deepseek-flash","stream":true}"#,
        r#"{"input":[],"max_output_tokens":256,"model":"deepseek-flash","stream":true}"#];
    for (i, ((_, path), want)) in cases.iter().zip(&fixtures).enumerate() {
        let hs = [("content-type", "application/json"), ("x-egress-caps", "mem/vec_index,loop/plan2"), ("x-egress-scope", "live"), ("x-init-caps", "mem/x")];
        let (status, head, body) = request(&sock, path, &hs, bodies[i].as_bytes());
        assert_eq!(status, 200, "{path}: {head}");
        assert!(body == *want, "{path}: the relayed bytes differ from the recording ({} vs {} bytes)", body.len(), want.len());
    }
    // an effect (a non-GET http class) in the same turn: its `egress_action` names the turn's caps so far (the
    // model calls above, root evidence, and this request's own zone header) and the open groups (ADR-011 §5.3)
    let (status, head, _) = request(&sock, &format!("/x/post-api/127.0.0.1:{port}/hook"), &[("x-egress-caps", "mem/vec_index")], b"{}");
    assert_eq!(status, 200, "{head}");
    let lineage = fs::read_to_string(rh.join("lineage/lineage.jsonl")).unwrap();
    let act: Value = lineage.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|r| r["kind"] == "egress_action").unwrap();
    let caps: Vec<&str> = act["data"]["caps"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(caps.contains(&"ext/semver-sort") && caps.contains(&"mem/vec_index") && caps.len() <= 16, "{act}");
    assert_eq!(act["data"]["open_groups"], serde_json::json!([]), "{act}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 4);
    for h in &seen {
        assert!(!h.iter().any(|(k, _)| k.starts_with("x-egress") || k.starts_with("x-init")), "a zone header reached the upstream: {h:?}");
    }
    let mut ends = Vec::new();
    for e in fs::read_dir(rh.join("egress/usage")).unwrap().flatten() {
        ends.extend(fs::read_to_string(e.path()).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|v| v["t"] == "end"));
    }
    assert_eq!(ends.len(), 4);
    assert!(ends.iter().filter(|e| e["finish"] == "tool_use").all(|e| e["usage"]["output"] == 120 && e["status"] == 200), "{ends:?}");
    assert_eq!(ends.iter().filter(|e| e["finish"] == "tool_use").count(), 3, "{ends:?}");
}
