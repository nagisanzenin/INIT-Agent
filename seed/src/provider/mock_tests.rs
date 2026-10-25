//! Integration tests over a real Unix socket (provider.md §8 items 12–14, plus the transport rules
//! of §3.6/§3.9): against `init-mockllm` (WP2) when `INIT_MOCKLLM_BIN` is set (or the kernel lane's
//! debug build exists), and against an in-test stub server for byte-exact edge cases. No real API.
//!
//! Every test home lives under `<target dir>/test-homes/wp10/` (`$CARGO_TARGET_DIR` when set, as
//! in the gate's G0; else `seed/target`) and is removed by its exact path.
//! The only processes signalled are the mock children these tests spawn (by their `Child` handle).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{Client, cx};
use super::deepseek::tests::{ds_assistant, result, strip_reasoning, tools, user};
use super::http::Cx;
use super::summarizer::{ProxySummarizer, SummaryCx};
use super::transport::Transport;
use super::{Prompt, PromptHead, models};
use crate::context::compact::{ForkRequest, Summarizer};
use crate::model::{
    Abort, Api, AssistantMessage, ContentBlock, Effort, ErrorKind, Message, ModelClient, Origin,
    ProviderError, Purpose, Request, StopReason, StreamEvent, TimeoutPhase, ToolCall, Usage,
};

static N: AtomicU64 = AtomicU64::new(0);
const SID: &str = "20261020-031500-a1b2c3";

struct Home {
    path: PathBuf,
    /// The same directory relative to the test's cwd, for socket paths: an absolute path under a
    /// long checkout (or a gate's build slot) can exceed `sun_path` (103 bytes on macOS).
    rel: PathBuf,
}

/// `to` relative to `from` (both absolute): `..` for every component of `from` past the common
/// prefix, then the rest of `to`.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let (f, t): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut r = PathBuf::new();
    for _ in common..f.len() {
        r.push("..");
    }
    for c in &t[common..] {
        r.push(c);
    }
    r
}

impl Home {
    /// Under the build's target dir: `$CARGO_TARGET_DIR` when the environment sets it (the gate's G0
    /// does, and its sandbox allows Unix sockets only under the build output, never in the source
    /// export), else `<package>/target`.
    fn new(name: &str) -> Home {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
        let path = target
            .join("test-homes/wp10")
            .join(format!("{name}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let rel = relative(&cwd, &path);
        assert_eq!(
            cwd.join(&rel).canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
        Home { path, rel }
    }

    fn sock(&self) -> PathBuf {
        let abs = self.path.join("p.sock");
        if abs.as_os_str().len() <= super::transport::MAX_SOCK_PATH {
            abs
        } else {
            self.rel.join("p.sock")
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if std::env::var("INIT_WP10_KEEP").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn abort() -> Abort {
    Abort::new(|_| {})
}

fn fast(sock: &Path) -> Transport {
    let mut t = Transport::new(sock);
    t.backoff_base = Duration::from_millis(5);
    t.timeouts.idle = Duration::from_secs(5);
    t
}

fn turn_cx() -> Cx {
    cx(SID, Purpose::Turn, Some("in-0001"), None)
}

fn prompt<'a>(msgs: &'a [Message], ts: &'a [crate::model::ToolSpec]) -> Prompt<'a> {
    Prompt {
        head: PromptHead {
            core: "You are INIT (test).",
            self_: "# SELF.md\n\ntest",
        },
        tools: ts,
        messages: msgs,
        b3: None,
        max_tokens: 8_192,
        effort: Effort::High,
        session: SID,
    }
}

fn done(evs: &[StreamEvent]) -> &AssistantMessage {
    match evs.last() {
        Some(StreamEvent::Done(m)) => m,
        other => panic!("expected Done, got {other:?}\nall: {evs:#?}"),
    }
}

fn retries(evs: &[StreamEvent]) -> Vec<&ProviderError> {
    evs.iter()
        .filter_map(|e| match e {
            StreamEvent::Retry { error, .. } => Some(error),
            _ => None,
        })
        .collect()
}

fn terminal_count(evs: &[StreamEvent]) -> usize {
    evs.iter()
        .filter(|e| matches!(e, StreamEvent::Done(_) | StreamEvent::Failed(_)))
        .count()
}

// ---------------------------------------------------------------------------------------------
// init-mockllm
// ---------------------------------------------------------------------------------------------

fn mock_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("INIT_MOCKLLM_BIN").map(PathBuf::from) {
        // The gate sets INIT_MOCKLLM_BIN=$INIT_HOME/kernel/bin/init-mockllm; an install without the
        // mock binary skips these tests (with a note) instead of failing every promote at G0.
        if !p.is_file() {
            eprintln!("INIT_MOCKLLM_BIN={} does not exist", p.display());
            return None;
        }
        return Some(p);
    }
    // Any lane's debug build of the kernel workspace (kernel/target/<lane>/debug/init-mockllm).
    let lanes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/target");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&lanes)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path().join("debug/init-mockllm")))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

struct MockLlm {
    child: Child,
    sock: PathBuf,
    record: PathBuf,
    _home: Home,
}

impl MockLlm {
    /// None (test skipped, with a note) when no mock binary is available.
    fn start(name: &str, script: &str) -> Option<MockLlm> {
        let Some(bin) = mock_bin() else {
            eprintln!(
                "SKIP {name}: set INIT_MOCKLLM_BIN to an init-mockllm binary to run this test"
            );
            return None;
        };
        let home = Home::new(name);
        let script_path = home.path.join("s.mock.jsonl");
        std::fs::write(&script_path, script).unwrap();
        let sock = home.sock();
        let record = home.path.join("rec");
        let child = Command::new(bin)
            .args([
                "--mode",
                "socket",
                "--strict",
                "deepseek",
                "--no-probe-responder",
                "--no-g3-responder",
            ])
            .arg("--socket")
            .arg(&sock)
            .arg("--script")
            .arg(&script_path)
            .arg("--record-dir")
            .arg(&record)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn init-mockllm");
        let mut m = MockLlm {
            child,
            sock,
            record,
            _home: home,
        };
        let t0 = Instant::now();
        while UnixStream::connect(&m.sock).is_err() {
            if let Ok(Some(st)) = m.child.try_wait() {
                panic!("init-mockllm exited early: {st}");
            }
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "init-mockllm did not listen"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        Some(m)
    }

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.record.join("mock.log.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok()) // a line still being appended is not there yet
            .collect()
    }

    /// The mock's log once it has at least `n` lines. The mock appends a request's line after it has
    /// sent the response, so a client that has just seen `Done` can read the file before the line
    /// lands (G0-SEED: seen once under G0's load). Waits at most 5 s; the caller's assertions report
    /// a shortfall.
    fn log_n(&self, n: usize) -> Vec<Value> {
        let t0 = Instant::now();
        loop {
            let log = self.log();
            if log.len() >= n || t0.elapsed() > Duration::from_secs(5) {
                return log;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn bodies(&self) -> Vec<Value> {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&self.record)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        names.sort();
        names
            .iter()
            .map(|p| serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap())
            .collect()
    }
}

impl Drop for MockLlm {
    fn drop(&mut self) {
        // Our own child, by handle (never by pattern).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const META: &str = r#"{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"wp10","chunk":7}"#;

/// §8 item 12 + the strict-mode `reasoning_content` round trip: a full tool turn, the results, a
/// follow-up, across 3 requests with `prefix:"same"`; `--strict deepseek` rejects any assistant
/// message without `reasoning_content`, so acceptance proves the pass-back.
#[test]
fn mock_full_turn_strict_reasoning_round_trip() {
    let script = [
        META,
        r#"{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Fix","tools":["bash","delegate","edit","ext","read","write"]},"reply":{"think":"Run the tests first, then read the file.","text":"","calls":[{"name":"bash","input":{"command":"cargo test"}},{"name":"read","input":{"path":"src/lib.rs"}}],"stop":"tool_use","usage":{"input":12034,"cache_read":11776,"output":31}}}"#,
        r#"{"v":1,"kind":"turn","expect":{"last_role":"tool","prefix":"same"},"reply":{"think":"The sign is wrong.","text":"Fixed the sign in add().","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Commit","prefix":"same"},"reply":{"think":"Commit.","text":"Committed.","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("full", &script) else {
        return;
    };
    let info = models::lookup("deepseek", "deepseek-flash").unwrap();
    let client = Client::new(info, fast(&mock.sock));
    let ts = tools();
    let a = abort();
    let mut msgs = vec![user("Fix the failing test in work/demo.")];

    // Turn 1: reasoning + two tool calls.
    let evs: Vec<StreamEvent> = client
        .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
        .unwrap()
        .collect();
    assert!(
        matches!(evs[0], StreamEvent::Started { attempt: 1, .. }),
        "{:?}",
        evs[0]
    );
    assert_eq!(terminal_count(&evs), 1);
    let think: String = evs
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ThinkingDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(think, "Run the tests first, then read the file.");
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallReady(_)))
            .count(),
        2
    );
    let m1 = done(&evs).clone();
    assert_eq!(m1.stop, StopReason::ToolUse);
    let calls: Vec<&ToolCall> = m1.tool_calls().collect();
    assert_eq!(
        (calls[0].name.as_str(), calls[1].name.as_str()),
        ("bash", "read")
    );
    assert_eq!(calls[0].input["command"], "cargo test");
    assert_eq!(calls[1].input["path"], "src/lib.rs");
    assert_eq!(
        (m1.usage.input, m1.usage.cache_read, m1.usage.output),
        (12034, 11776, 31)
    );
    assert_eq!(m1.origin.provider, "deepseek");
    let ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
    msgs.push(Message::Assistant(m1));
    msgs.push(Message::ToolResults(vec![
        result(
            &ids[0],
            "test add ... FAILED\n[exit_code=101 wall_time=3.21s]",
            true,
        ),
        result(
            &ids[1],
            "1\tpub fn add(a: i32, b: i32) -> i32 { a - b }",
            false,
        ),
    ]));

    // Turn 2: text reply.
    let evs: Vec<StreamEvent> = client
        .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
        .unwrap()
        .collect();
    let m2 = done(&evs).clone();
    assert_eq!(m2.text(), "Fixed the sign in add().");
    assert_eq!(m2.stop, StopReason::EndTurn);
    msgs.push(Message::Assistant(m2));
    msgs.push(user("Thanks. Commit it."));

    // Turn 3.
    let evs: Vec<StreamEvent> = client
        .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
        .unwrap()
        .collect();
    assert_eq!(done(&evs).text(), "Committed.");

    let log = mock.log_n(3);
    assert_eq!(log.len(), 3, "{log:#?}");
    for l in &log {
        assert_eq!(l["expect_ok"], true, "{l}");
        assert!(l.get("protocol_error").is_none_or(Value::is_null), "{l}");
        assert_eq!(l["session"], SID);
        assert_eq!(l["purpose"], "turn");
    }
    // The replayed reasoning is in the recorded bodies, verbatim, with the raw arguments.
    let bodies = mock.bodies();
    let b3 = &bodies[2]["messages"];
    assert_eq!(
        b3[2]["reasoning_content"],
        "Run the tests first, then read the file."
    );
    assert_eq!(
        b3[2]["tool_calls"][0]["function"]["arguments"],
        "{\"command\":\"cargo test\"}"
    );
    assert_eq!(b3[5]["reasoning_content"], "The sign is wrong.");
}

/// Strict mode pins the pass-back rule. A foreign tool turn goes out with `"reasoning_content": ""`
/// and the tool calls kept (live V-DS-3: DeepSeek accepts that), so its first request is accepted.
/// The safety net: a body without the field is rejected (400), and the first §3.10 fallback retries
/// once with `"reasoning_content": ""` and succeeds; no flatten is needed.
#[test]
fn mock_strict_foreign_turn_and_missing_reasoning_fallback() {
    let script = [
        META,
        r#"{"v":1,"kind":"turn","reply":{"think":"ok","text":"Accepted.","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"ok","text":"Continuing.","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("flat", &script) else {
        return;
    };
    let info = models::lookup("deepseek", "deepseek-flash").unwrap();
    let client = Client::new(info, fast(&mock.sock));
    let ts = tools();
    let foreign = Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "Listing.".into(),
            },
            ContentBlock::ToolCall(ToolCall {
                id: "toolu_01A".into(),
                name: "bash".into(),
                input: ToolCall::parse_input("{\"command\":\"ls\"}"),
                raw_arguments: None,
            }),
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
        origin: Origin {
            provider: "anthropic".into(),
            api: Api::Messages,
            model: "claude-opus-5-5".into(),
            response_id: None,
        },
    });
    let msgs = vec![
        user("list"),
        foreign,
        Message::ToolResults(vec![result("toolu_01A", "a\nb", false)]),
        user("go on"),
    ];
    let a = abort();
    // As built: one request, accepted, no fallback.
    let req = client.provider().build(&prompt(&msgs, &ts)).unwrap();
    let mut es = client.send_body(req.body.clone(), turn_cx(), &a);
    let evs: Vec<StreamEvent> = es.by_ref().collect();
    assert!(es.fallbacks().is_empty(), "{:?}", es.fallbacks());
    assert!(retries(&evs).is_empty());
    assert_eq!(done(&evs).text(), "Accepted.");
    let log = mock.log_n(1);
    assert_eq!(log.len(), 1);
    assert!(log[0].get("protocol_error").is_none_or(Value::is_null), "{}", log[0]);
    // The safety net: the same body without the field.
    let mut es = client.send_body(strip_reasoning(&req.body), turn_cx(), &a);
    let evs: Vec<StreamEvent> = es.by_ref().collect();
    assert_eq!(es.fallbacks(), [super::deepseek::EMPTY_REASONING_CAUSE]);
    let warn = evs.iter().find_map(|e| match e {
        StreamEvent::Warning { code, detail } => Some((*code, detail.clone())),
        _ => None,
    });
    let (code, detail) = warn.expect("fallback cause emitted as a Warning");
    assert_eq!(code, super::deepseek::EMPTY_REASONING_CAUSE);
    assert!(detail.contains("400"), "{detail}");
    let r = retries(&evs);
    assert_eq!(r.len(), 1);
    assert!(
        matches!(
            r[0],
            ProviderError::Http {
                status: 400,
                kind: ErrorKind::InvalidRequest,
                ..
            }
        ),
        "{:?}",
        r[0]
    );
    assert_eq!(done(&evs).text(), "Continuing.");
    let log = mock.log_n(3);
    assert_eq!(log.len(), 3);
    assert!(
        log[1]["protocol_error"]
            .as_str()
            .unwrap()
            .contains("reasoning_content missing"),
        "{}",
        log[1]
    );
    assert!(log[2].get("protocol_error").is_none_or(Value::is_null));
    // The retried request kept the foreign turn's structure: the call, its id and its result.
    let b = mock.bodies();
    let m = &b.last().expect("a recorded body")["messages"];
    let asst = m
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["role"] == "assistant")
        .unwrap();
    assert_eq!(asst["reasoning_content"], "");
    assert_eq!(asst["tool_calls"][0]["id"], "toolu_01A");
    assert!(
        m.as_array()
            .unwrap()
            .iter()
            .any(|x| x["role"] == "tool" && x["tool_call_id"] == "toolu_01A")
    );
}

/// §8 item 13: `http 429` + `retry-after` → one Retry then success; `cut` → Truncated then retry;
/// `stall` with a shrunk idle timeout → Timeout(Idle) then retry; `budget` → Failed(Budget), no retry.
#[test]
fn mock_faults() {
    let script = [
        META,
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"headers":{"retry-after":"1"},"body":{"error":{"message":"rate limited","type":"rate_limit_error"}}}}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"a","text":"after 429","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","fault":{"kind":"cut","after_events":2},"reply":{"think":"abcdefghijklmnop","text":"partial text here","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"a","text":"after cut","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":1,"ms":1500},"reply":{"think":"a","text":"stalled","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"a","text":"after stall","stop":"end_turn"}}"#,
        r#"{"v":1,"kind":"turn","fault":{"kind":"budget"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("faults", &script) else {
        return;
    };
    let info = models::lookup("deepseek", "deepseek-flash").unwrap();
    let mut t = fast(&mock.sock);
    t.timeouts.idle = Duration::from_millis(300);
    let client = Client::new(info, t);
    let ts = tools();
    let msgs = vec![user("hi")];
    let a = abort();
    let run = || -> Vec<StreamEvent> {
        client
            .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
            .unwrap()
            .collect()
    };

    let t0 = Instant::now();
    let evs = run();
    let r = retries(&evs);
    assert_eq!(r.len(), 1);
    assert!(
        matches!(r[0], ProviderError::Http { status: 429, kind: ErrorKind::RateLimit, retry_after: Some(d), .. } if *d == Duration::from_secs(1))
    );
    match evs.iter().find(|e| matches!(e, StreamEvent::Retry { .. })) {
        Some(StreamEvent::Retry {
            attempt: 1, delay, ..
        }) => assert!(*delay >= Duration::from_secs(1)),
        other => panic!("{other:?}"),
    }
    assert!(t0.elapsed() >= Duration::from_secs(1));
    assert_eq!(done(&evs).text(), "after 429");
    assert!(
        evs.iter()
            .any(|e| matches!(e, StreamEvent::Started { attempt: 2, .. }))
    );

    let evs = run();
    let r = retries(&evs);
    assert!(matches!(r[..], [ProviderError::Truncated]), "{r:?}");
    assert_eq!(done(&evs).text(), "after cut");

    let evs = run();
    let r = retries(&evs);
    assert!(
        matches!(r[..], [ProviderError::Timeout(TimeoutPhase::Idle)]),
        "{r:?}"
    );
    assert_eq!(done(&evs).text(), "after stall");

    let evs = run();
    assert!(retries(&evs).is_empty());
    assert_eq!(terminal_count(&evs), 1);
    assert!(
        matches!(
            evs.last(),
            Some(StreamEvent::Failed(ProviderError::Budget { .. }))
        ),
        "{evs:?}"
    );
}

/// §8 item 14: abort mid-stream during a `stall`: `Done(partial, Aborted)` within 50 ms.
#[test]
fn mock_abort_mid_stream() {
    let script = [
        META,
        r#"{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":3,"ms":5000},"reply":{"think":"thinking slowly about the problem","text":"never","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("abort", &script) else {
        return;
    };
    let info = models::lookup("deepseek", "deepseek-flash").unwrap();
    let client = Client::new(info, fast(&mock.sock));
    let ts = tools();
    let msgs = vec![user("hi")];
    let a = abort();
    let (tx, rx) = mpsc::channel::<(Instant, StreamEvent)>();
    std::thread::scope(|s| {
        let client = &client;
        let a = &a;
        let p = prompt(&msgs, &ts);
        s.spawn(move || {
            for ev in client.send_prompt(&p, turn_cx(), a).unwrap() {
                tx.send((Instant::now(), ev)).unwrap();
            }
        });
        // Wait for the first thinking delta, then abort.
        loop {
            let (_, ev) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            if matches!(ev, StreamEvent::ThinkingDelta(_)) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50)); // the stream is now silent (stalled)
        let t0 = Instant::now();
        a.trigger();
        let mut last = None;
        while let Ok((t, ev)) = rx.recv_timeout(Duration::from_secs(10)) {
            last = Some((t, ev));
        }
        let (t, ev) = last.unwrap();
        let took = t.duration_since(t0);
        assert!(took < Duration::from_millis(50), "abort took {took:?}");
        match ev {
            StreamEvent::Done(m) => {
                assert_eq!(m.stop, StopReason::Aborted);
                assert!(
                    m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Thinking(t) if !t.text.is_empty())),
                    "{m:?}"
                );
                assert_eq!(m.tool_calls().count(), 0);
            }
            other => panic!("{other:?}"),
        }
    });
}

/// The summarizer sends the fork body with purpose `compaction` and maps the reply (WP12 seam).
#[test]
fn mock_summarizer() {
    let script = [
        META,
        r#"{"v":1,"kind":"turn","purpose":"compaction","expect":{"last_text_contains":"[init:compact]"},"reply":{"think":"Summarize.","text":"<summary>\n## Intent\n- I1 test\n</summary>","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("summ", &script) else {
        return;
    };
    let info = models::lookup("deepseek", "deepseek-flash").unwrap();
    let client = Arc::new(Client::new(info, fast(&mock.sock)));
    let ts = tools();
    let msgs = vec![
        user("work"),
        ds_assistant("r", "did work", &[]),
        user("[init:compact] Write the summary."),
    ];
    let body = client.provider().build(&prompt(&msgs, &ts)).unwrap().body;
    let scx = Arc::new(Mutex::new(SummaryCx {
        session: SID.into(),
        task: None,
    }));
    let mut s = ProxySummarizer::new(client.clone(), Arc::new(abort()), scx);
    let r = s
        .summarize(&ForkRequest {
            messages: &[],
            body: &body,
            max_tokens: 16_000,
            attempt: 0,
        })
        .unwrap();
    assert!(r.text.starts_with("<summary>"));
    assert_eq!(r.thinking.as_deref(), Some("Summarize."));
    assert!(!r.tool_use);
    assert_eq!(r.model, "deepseek-flash");
    assert!(r.usage.is_some());
    let log = mock.log_n(1);
    assert_eq!(log[0]["purpose"], "compaction");
    assert_eq!(log[0]["expect_ok"], true);
}

/// The `mock-frontier-1` model routes to `/mock/chat/completions` and decodes as DeepSeek.
#[test]
fn mock_frontier_model_route() {
    let script = [
        r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"wp10m","chunk":16}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"m","text":"pong","stop":"end_turn"}}"#,
    ]
    .join("\n");
    let Some(mock) = MockLlm::start("route", &script) else {
        return;
    };
    let info = models::lookup("mock", "mock-frontier-1").unwrap();
    let client = Client::new(info, fast(&mock.sock));
    let ts = tools();
    let msgs = vec![user("ping")];
    let a = abort();
    let evs: Vec<StreamEvent> = client
        .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
        .unwrap()
        .collect();
    let m = done(&evs);
    assert_eq!(m.text(), "pong");
    assert_eq!(m.origin.provider, "mock");
    assert_eq!(mock.log_n(1)[0]["path"], "/mock/chat/completions");
}

// ---------------------------------------------------------------------------------------------
// In-test stub server (byte-exact responses)
// ---------------------------------------------------------------------------------------------

enum Resp {
    /// Write these bytes and close.
    Bytes(Vec<u8>),
    /// Write the prefix, then stay silent for the duration, then close.
    Stall(Vec<u8>, Duration),
}

/// (request head, body) per accepted connection.
type Recorded = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct Stub {
    sock: PathBuf,
    reqs: Recorded,
    handle: Option<JoinHandle<()>>,
    _home: Home,
}

impl Stub {
    fn start(name: &str, resps: Vec<Resp>) -> Stub {
        let home = Home::new(name);
        let sock = home.sock();
        let l = UnixListener::bind(&sock).unwrap();
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let r2 = reqs.clone();
        let handle = std::thread::spawn(move || {
            for resp in resps {
                let Ok((s, _)) = l.accept() else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                let _ = r.read_exact(&mut body);
                r2.lock().unwrap().push((head, body));
                let mut s = s;
                match resp {
                    Resp::Bytes(b) => {
                        let _ = s.write_all(&b);
                    }
                    Resp::Stall(b, d) => {
                        let _ = s.write_all(&b);
                        let _ = s.flush();
                        std::thread::sleep(d);
                    }
                }
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        });
        Stub {
            sock,
            reqs,
            handle: Some(handle),
            _home: home,
        }
    }

    fn heads(&self) -> Vec<String> {
        self.reqs
            .lock()
            .unwrap()
            .iter()
            .map(|(h, _)| h.clone())
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        // Unblock a pending accept so the thread ends.
        let _ = UnixStream::connect(&self.sock);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A 200 SSE response, chunked: one HTTP chunk per `split` bytes of the stream.
fn sse_200(sse: &[u8], extra: &str, split: usize) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n{extra}\r\n").into_bytes();
    for c in sse.chunks(split.max(1)) {
        out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
        out.extend_from_slice(c);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

fn error_resp(status: u16, extra: &str, body: &Value) -> Vec<u8> {
    let b = body.to_string();
    format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}\r\n{b}", b.len()).into_bytes()
}

fn fixture(name: &str) -> Vec<u8> {
    super::testutil::fixture(name)
}

fn ds_client(sock: &Path) -> Client {
    Client::new(
        models::lookup("deepseek", "deepseek-flash").unwrap(),
        fast(sock),
    )
}

/// The loop's `ModelClient` seam: the context-rendered body goes out unchanged with the right
/// headers (no credentials); `Started` carries the proxy's ids and head hash; 1-byte HTTP chunks.
#[test]
fn stub_model_client_seam_and_headers() {
    let stub = Stub::start(
        "seam",
        vec![Resp::Bytes(sse_200(
            &fixture("deepseek/split-args.sse"),
            "x-init-req-id: r-77\r\nrequest-id: up-9\r\nx-init-head-hash: sha256:abcd\r\n",
            1,
        ))],
    );
    let client = ds_client(&stub.sock);
    assert_eq!(ModelClient::info(&client).id, "deepseek-flash");
    let body = br#"{"messages":[],"model":"deepseek-flash","stream":true}"#.to_vec();
    let req = Request {
        body: &body,
        head_hash: "sha256:abcd",
        messages: &[],
        max_tokens: 65_536,
        effort: Effort::High,
        purpose: Purpose::Probe,
        session: SID,
        task: Some("in-42"),
        subtask: None,
    };
    let a = abort();
    let evs: Vec<StreamEvent> = ModelClient::stream(&client, &req, &a).collect();
    match &evs[0] {
        StreamEvent::Started {
            attempt,
            request_id,
            proxy_req_id,
            head_hash,
        } => {
            assert_eq!(*attempt, 1);
            assert_eq!(request_id.as_deref(), Some("up-9"));
            assert_eq!(proxy_req_id.as_deref(), Some("r-77"));
            assert_eq!(head_hash.as_deref(), Some("sha256:abcd"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(done(&evs).tool_calls().count(), 2);
    let reqs = stub.reqs.lock().unwrap();
    let (head, sent) = &reqs[0];
    assert_eq!(sent, &body);
    assert!(
        head.starts_with("POST /deepseek/chat/completions HTTP/1.1\r\n"),
        "{head}"
    );
    for h in [
        "x-init-session: 20261020-031500-a1b2c3\r\n",
        "x-init-purpose: probe\r\n",
        "x-init-attempt: 1\r\n",
        "x-init-task: in-42\r\n",
        "connection: close\r\n",
    ] {
        assert!(head.contains(h), "{h} missing in {head}");
    }
    let lower = head.to_ascii_lowercase();
    assert!(
        !lower.contains("authorization")
            && !lower.contains("x-api-key")
            && !lower.contains("cookie")
    );
}

/// Proxy-generated errors (`x-init-error`) are not retried; caps are `Budget` by header or by
/// body type; `x-init-retries` makes the body's backoff jump; `retry-after-ms` is honored.
#[test]
fn stub_proxy_errors_and_backoff_jump() {
    let stub = Stub::start(
        "proxyerr",
        vec![
            Resp::Bytes(error_resp(
                502,
                "x-init-error: upstream.connect\r\n",
                &json!({"error": {"message": "upstream down", "type": "init_proxy_error"}}),
            )),
            Resp::Bytes(error_resp(
                429,
                "",
                &json!({"error": {"code": "budget.day", "type": "init_cap_exceeded", "message": "day cap"}}),
            )),
            Resp::Bytes(error_resp(
                503,
                "x-init-retries: 3\r\nretry-after-ms: 20\r\n",
                &json!({"error": {"message": "busy"}}),
            )),
            Resp::Bytes(sse_200(&fixture("deepseek/text.sse"), "", 64)),
        ],
    );
    let client = ds_client(&stub.sock);
    let a = abort();
    let run = || -> Vec<StreamEvent> { client.send_body(b"{}".to_vec(), turn_cx(), &a).collect() };

    let evs = run();
    assert!(retries(&evs).is_empty());
    assert!(
        matches!(
            evs.last(),
            Some(StreamEvent::Failed(ProviderError::Http { status: 502, .. }))
        ),
        "{evs:?}"
    );

    let evs = run();
    assert!(retries(&evs).is_empty());
    assert!(
        matches!(evs.last(), Some(StreamEvent::Failed(ProviderError::Budget { message })) if message == "day cap"),
        "{evs:?}"
    );

    let evs = run();
    match evs.iter().find(|e| matches!(e, StreamEvent::Retry { .. })) {
        // delay_step = 1 + 3 (x-init-retries) → base 5 ms · 2^3 = 40 ms, jitter [20, 40]; hint 20 ms.
        Some(StreamEvent::Retry {
            delay,
            error:
                ProviderError::Http {
                    status: 503,
                    kind: ErrorKind::Overloaded,
                    ..
                },
            ..
        }) => {
            assert!(
                *delay >= Duration::from_millis(20) && *delay <= Duration::from_millis(40),
                "{delay:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!done(&evs).text().is_empty());
    let heads = stub.heads();
    assert!(heads[3].contains("x-init-attempt: 2\r\n"));
}

/// A missing proxy socket is `Connect`, retried until `max_attempts`, then `Failed(Connect)`.
#[test]
fn stub_connect_errors_retry_then_fail() {
    let home = Home::new("noconn");
    let mut t = fast(&home.path.join("absent.sock"));
    t.max_attempts = 3;
    t.backoff_base = Duration::from_millis(1);
    let client = Client::new(models::lookup("deepseek", "deepseek-flash").unwrap(), t);
    let a = abort();
    let evs: Vec<StreamEvent> = client.send_body(b"{}".to_vec(), turn_cx(), &a).collect();
    assert_eq!(retries(&evs).len(), 2);
    assert!(
        retries(&evs)
            .iter()
            .all(|e| matches!(e, ProviderError::Connect(_)))
    );
    assert!(matches!(
        evs.last(),
        Some(StreamEvent::Failed(ProviderError::Connect(_)))
    ));
    assert_eq!(terminal_count(&evs), 1);
}

/// An abort during a backoff wakes at once and ends with an empty `Done(Aborted)`.
#[test]
fn stub_abort_during_backoff() {
    let stub = Stub::start(
        "backoff",
        vec![Resp::Bytes(error_resp(
            429,
            "retry-after: 30\r\n",
            &json!({"error": {"message": "slow down"}}),
        ))],
    );
    let client = ds_client(&stub.sock);
    let a = abort();
    std::thread::scope(|s| {
        let a2 = &a;
        let h = s.spawn(|| {
            client
                .send_body(b"{}".to_vec(), turn_cx(), a2)
                .collect::<Vec<StreamEvent>>()
        });
        std::thread::sleep(Duration::from_millis(200));
        let t0 = Instant::now();
        a.trigger();
        let evs = h.join().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(retries(&evs).len(), 1);
        let m = done(&evs);
        assert_eq!(m.stop, StopReason::Aborted);
        assert!(m.content.is_empty());
    });
    // A connection after the flag is set is refused at registration.
    let evs: Vec<StreamEvent> = client.send_body(b"{}".to_vec(), turn_cx(), &a).collect();
    assert!(matches!(evs.last(), Some(StreamEvent::Done(m)) if m.stop == StopReason::Aborted));
}

/// Idle timeout on a silent stream (stub stall), then a clean retry; content-length framing too.
#[test]
fn stub_idle_timeout_then_content_length_body() {
    let sse = fixture("deepseek/usage-explicit.sse");
    let mut plain = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
        sse.len()
    )
    .into_bytes();
    plain.extend_from_slice(&sse);
    let first = sse_200(&sse[..200], "", 50);
    let first = first[..first.len() - 5].to_vec(); // no terminating chunk: the stream just stops
    let stub = Stub::start(
        "idle",
        vec![
            Resp::Stall(first, Duration::from_millis(1500)),
            Resp::Bytes(plain),
        ],
    );
    let mut t = fast(&stub.sock);
    t.timeouts.idle = Duration::from_millis(200);
    let client = Client::new(models::lookup("deepseek", "deepseek-flash").unwrap(), t);
    let a = abort();
    let evs: Vec<StreamEvent> = client.send_body(b"{}".to_vec(), turn_cx(), &a).collect();
    assert!(
        matches!(
            retries(&evs)[..],
            [ProviderError::Timeout(TimeoutPhase::Idle)]
        ),
        "{evs:?}"
    );
    let m = done(&evs);
    assert_eq!(m.text(), "cached");
    assert_eq!(m.usage.cache_read, 11776);
}

/// §8 item 12 for the other two adapters: a tool turn and a follow-up through the mock socket
/// (Anthropic Messages and OpenAI Responses wire formats), with `prefix:"same"` on the second.
#[test]
fn mock_full_turn_anthropic_and_openai() {
    for (provider, model) in [("anthropic", "claude-opus-5-5"), ("openai", "gpt-6-astra")] {
        let script = [
            format!(r#"{{"v":1,"kind":"meta","provider":"{provider}","model":"{model}","name":"wp10-{provider}","chunk":5}}"#),
            r#"{"v":1,"kind":"turn","expect":{"last_role":"user","tools":["bash","delegate","edit","ext","read","write"]},"reply":{"think":"plan","text":"Listing.","calls":[{"name":"bash","input":{"command":"ls -la"}}],"stop":"tool_use","usage":{"input":900,"cache_read":800,"cache_write":50,"output":12}}}"#.to_string(),
            r#"{"v":1,"kind":"turn","expect":{"last_role":"tool","prefix":"same"},"reply":{"think":"done","text":"Two files.","stop":"end_turn"}}"#.to_string(),
        ]
        .join("\n");
        let Some(mock) = MockLlm::start(&format!("{provider}-turn"), &script) else {
            return;
        };
        let info = models::lookup(provider, model).unwrap();
        let client = Client::new(info, fast(&mock.sock));
        let ts = tools();
        let a = abort();
        let mut msgs = vec![user("List the files.")];
        let evs: Vec<StreamEvent> = client
            .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
            .unwrap()
            .collect();
        let m1 = done(&evs).clone();
        assert_eq!(m1.stop, StopReason::ToolUse, "{provider}");
        assert_eq!(m1.text(), "Listing.", "{provider}");
        let c = m1.tool_calls().next().unwrap().clone();
        assert_eq!(c.input["command"], "ls -la", "{provider}");
        assert!(
            m1.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking(t) if t.native.is_some())),
            "{provider}: {m1:?}"
        );
        assert_eq!(m1.origin.provider, provider);
        msgs.push(Message::Assistant(m1));
        msgs.push(Message::ToolResults(vec![result(
            &c.id,
            "a.txt\nb.txt",
            false,
        )]));
        let evs: Vec<StreamEvent> = client
            .send_prompt(&prompt(&msgs, &ts), turn_cx(), &a)
            .unwrap()
            .collect();
        assert_eq!(done(&evs).text(), "Two files.", "{provider}");
        let log = mock.log_n(2);
        assert_eq!(log.len(), 2, "{provider}: {log:#?}");
        assert!(
            log.iter().all(|l| l["expect_ok"] == true),
            "{provider}: {log:#?}"
        );
        let bodies = mock.bodies();
        let replay = bodies[1].to_string();
        // The signed thinking block / encrypted reasoning item is replayed verbatim.
        assert!(
            replay.contains("mock-sig-") || replay.contains("mock-enc-"),
            "{provider}: {replay}"
        );
    }
}
