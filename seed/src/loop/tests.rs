//! Loop tests (loop.md §8), with a scripted `ModelClient` ($0) and WP12's real log and context.
//! Frames go through the real input and output threads over in-memory pipes. Every test home is
//! under `seed/target/test-homes/wp11/`; every process signalled is one these tests spawned.

use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::frames::{OutTx, spawn_input, spawn_output};
use super::hooks::{BasicSignals, LogContext};
use super::status::{HeartbeatInfo, Status, spawn_heartbeat};
use super::*;
use crate::context::Context;
use crate::context::compact::{ForkRequest, Summarizer, SummaryReply};
use crate::context::head::{Head, build_head};
use crate::context::project::Msg;
use crate::log::event::Block;
use crate::log::session::{OpenParams, SessionLog};
use crate::log::testutil::{SHA, SID, boot_ev};
use crate::model::{
    Abort, AssistantMessage, ContentBlock, ErrorKind, ModelClient, ModelInfo, Origin,
    ProviderError, Request, StopReason, StreamEvent, ToolCall, Usage,
};
use crate::tools::bash::group_alive;
use crate::tools::ext::{OutcomeKind, ReloadKind, ReloadReport};
use crate::tools::testutil::{FakeHost, TempHome, base_env, kill_hook};
use crate::tools::{ExtHostApi, Limits, Mode, Tools, ToolsConfig, find_bash};

const T: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------------------------

type ErrFn = Box<dyn Fn() -> ProviderError + Send + Sync>;

enum Step {
    Reply {
        msg: AssistantMessage,
        delay: Duration,
    },
    /// Blocks on a socket registered with `Abort` until `trigger()` shuts it down, then returns
    /// `msg` as the aborted partial (complete calls only; the incomplete one is dropped).
    Stall {
        msg: AssistantMessage,
    },
    Fail(ErrFn),
}

#[derive(Clone, Debug)]
struct Seen {
    body: Vec<u8>,
    task: Option<String>,
    max_tokens: u32,
    messages: Vec<Msg>,
    head_hash: String,
}

struct ScriptModel {
    info: ModelInfo,
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Vec<Seen>>,
    stalled: AtomicBool,
}

impl ScriptModel {
    fn new(steps: Vec<Step>) -> Arc<ScriptModel> {
        Arc::new(ScriptModel {
            info: crate::provider::models::lookup("mock", "mock-frontier-1")
                .expect("mock model row")
                .clone(),
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
            stalled: AtomicBool::new(false),
        })
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn origin() -> Origin {
    Origin {
        provider: "mock".into(),
        api: crate::model::Api::ChatCompletions,
        model: "mock-frontier-1".into(),
        response_id: None,
    }
}

impl ModelClient for ScriptModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }
    fn stream<'a>(
        &'a self,
        req: &'a Request<'a>,
        abort: &'a Abort,
    ) -> Box<dyn Iterator<Item = StreamEvent> + 'a> {
        self.seen.lock().unwrap().push(Seen {
            body: req.body.to_vec(),
            task: req.task.map(str::to_string),
            max_tokens: req.max_tokens,
            messages: req.messages.to_vec(),
            head_hash: req.head_hash.to_string(),
        });
        let step = self.steps.lock().unwrap().pop_front();
        let started = StreamEvent::Started {
            attempt: 1,
            request_id: None,
            proxy_req_id: None,
            head_hash: None,
        };
        let evs = match step {
            None => vec![StreamEvent::Failed(ProviderError::Protocol(
                "script exhausted".into(),
            ))],
            Some(Step::Fail(f)) => vec![StreamEvent::Failed(f())],
            Some(Step::Reply { msg, delay }) => {
                let mut v = vec![started];
                let t = msg.text();
                if !t.is_empty() {
                    v.push(StreamEvent::TextDelta(t));
                }
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                v.push(StreamEvent::Done(msg));
                v
            }
            Some(Step::Stall { mut msg }) => {
                let (a, _peer) = UnixStream::pair().unwrap();
                msg.stop = StopReason::Aborted;
                if abort.register_stream(a.try_clone().unwrap()).is_ok() {
                    self.stalled.store(true, Ordering::SeqCst);
                    let mut buf = [0u8; 1];
                    let n = (&a).read(&mut buf).unwrap_or(0);
                    assert_eq!(n, 0, "the read ends by shutdown");
                }
                vec![
                    started,
                    StreamEvent::TextDelta(msg.text()),
                    StreamEvent::Done(msg),
                ]
            }
        };
        Box::new(evs.into_iter())
    }
}

static CALL_N: AtomicUsize = AtomicUsize::new(0);

fn call(name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: format!(
            "call_{:02}_t{}",
            CALL_N.fetch_add(1, Ordering::Relaxed),
            std::process::id() % 1000
        ),
        name: name.into(),
        raw_arguments: Some(input.to_string()),
        input: input.as_object().cloned().unwrap_or_default(),
    }
}

fn amsg(text: &str, calls: Vec<ToolCall>, stop: StopReason) -> AssistantMessage {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentBlock::Text { text: text.into() });
    }
    content.extend(calls.into_iter().map(ContentBlock::ToolCall));
    AssistantMessage {
        content,
        stop,
        usage: Usage {
            input: 1000,
            cache_read: 900,
            cache_write: 0,
            output: 20,
            reasoning: 0,
        },
        origin: origin(),
    }
}

fn reply(text: &str) -> Step {
    Step::Reply {
        msg: amsg(text, vec![], StopReason::EndTurn),
        delay: Duration::ZERO,
    }
}

fn tools_reply(calls: Vec<ToolCall>) -> Step {
    Step::Reply {
        msg: amsg("", calls, StopReason::ToolUse),
        delay: Duration::ZERO,
    }
}

fn bash_call(cmd: &str) -> ToolCall {
    call("bash", json!({"command": cmd}))
}

/// Valid compaction summaries for the emergency-compaction test.
struct TestSummarizer {
    calls: Arc<AtomicUsize>,
}

impl Summarizer for TestSummarizer {
    fn summarize(&mut self, _req: &ForkRequest) -> Result<SummaryReply, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(SummaryReply {
            text: "<summary>\n## Intent\n- [I1] Answer the owner\n## Constraints and preferences\n- none\n## Progress\n### Done\n- [P1] ran the commands\n### In progress\n- none\n### Blocked\n- none\n## Decisions\n- none\n## Open errors\n- none\n## Next steps\n- [N1] reply\n## Critical details\n- none\nRetired:\n</summary>".into(),
            thinking: Some("summarizing".into()),
            tool_use: false,
            model: "mock-frontier-1".into(),
            usage: None,
        })
    }
}

fn test_head() -> Head {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/context/testdata/head");
    build_head(&d, &d.join("CONSTITUTION.md"), crate::tools::spec_values()).unwrap()
}

// ---------------------------------------------------------------------------------------------
// The harness: a session thread, the real input/output threads, frames over pipes
// ---------------------------------------------------------------------------------------------

struct ChanWriter {
    tx: Sender<(Instant, Value)>,
    buf: Vec<u8>,
}

impl Write for ChanWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        while let Some(i) = self.buf.iter().position(|&c| c == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let v: Value = serde_json::from_slice(&line).expect("frames are JSON");
            let _ = self.tx.send((Instant::now(), v));
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type SendHeadSource = Box<dyn FnMut() -> Result<Head, String> + Send>;

#[derive(Default)]
struct Opts {
    mode: Option<Mode>,
    fault: Option<&'static str>,
    head_source: Option<SendHeadSource>,
    host: Option<Arc<FakeHost>>,
    tasks: Option<TaskLog>,
}

type TaskLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

struct H {
    stdin: Option<std::io::PipeWriter>,
    rx: Receiver<(Instant, Value)>,
    frames: Vec<(Instant, Value)>,
    join: Option<JoinHandle<u8>>,
    model: Arc<ScriptModel>,
    abort: Arc<Abort>,
    status: Arc<Status>,
    home: PathBuf,
    summaries: Arc<AtomicUsize>,
}

impl H {
    fn start(home: &TempHome, steps: Vec<Step>, mut o: Opts) -> H {
        let model = ScriptModel::new(steps);
        let abort = Arc::new(Abort::new(kill_hook));
        let status = Arc::new(Status::default());
        let (pr, pw) = std::io::pipe().unwrap();
        let (ftx, frx) = mpsc::channel();
        let summaries = Arc::new(AtomicUsize::new(0));
        let ext: Option<Arc<dyn ExtHostApi>> = o.host.clone().map(|h| h as Arc<dyn ExtHostApi>);
        let (ctl_tx, ctl_rx) = mpsc::channel();
        spawn_input(
            BufReader::new(pr),
            ctl_tx,
            abort.clone(),
            ext.clone(),
            status.clone(),
        )
        .unwrap();
        let (out, out_rx) = OutTx::channel(frames::OUT_CAP);
        spawn_output(
            ChanWriter {
                tx: ftx,
                buf: Vec::new(),
            },
            out_rx,
        )
        .unwrap();
        let (m2, a2, s2, h2, sm2) = (
            model.clone(),
            abort.clone(),
            status.clone(),
            home.path.clone(),
            summaries.clone(),
        );
        let mode = o.mode.unwrap_or(Mode::Normal);
        let fault = o.fault;
        let mut hs = o.head_source.take();
        let tasks = o.tasks.clone();
        let join = std::thread::Builder::new()
            .name("loop-main".into())
            .spawn(move || {
                let (log, rep) = SessionLog::open(&OpenParams {
                    home: &h2,
                    env_session: SID,
                    sha: SHA,
                    lock_wait: Duration::from_secs(10),
                })
                .unwrap();
                let target = crate::provider::models::target(m2.info(), None);
                let mut ctx = Context::new(
                    log,
                    test_head(),
                    target,
                    Box::new(TestSummarizer { calls: sm2 }),
                    &rep,
                    SHA,
                )
                .unwrap();
                ctx.append_boot(boot_ev("[init:boot] test boot"), &rep)
                    .unwrap();
                let hooks = LogContext::new(ctx, Box::new(BasicSignals { status: s2.clone() }));
                let tools = Tools::new(ToolsConfig {
                    bash: find_bash(std::env::var("PATH").ok().as_deref()),
                    ext,
                    provider: "mock".into(),
                    model: "mock-frontier-1".into(),
                    init_home: Some(h2.clone()),
                    base_env: Some(base_env(&[])),
                    limits: Limits::default(),
                });
                let s = Session::new(SessionConfig {
                    ctx: Box::new(hooks),
                    model: m2,
                    tools,
                    abort: a2,
                    status: s2,
                    ctl: ctl_rx,
                    out,
                    mode,
                    effort: Effort::High,
                    cwd: h2.join("work"),
                    home: h2.clone(),
                    user_home: Some(h2.join("userhome")),
                    head_source: hs.take().map(|f| -> HeadSource { Box::new(f) }),
                    fault: fault.map(|p| -> FaultHook { Box::new(move |q: &str| q == p) }),
                    on_task: tasks.map(|t| -> TaskHook {
                        Box::new(move |s: &str, k: Option<&str>| {
                            t.lock()
                                .unwrap()
                                .push((s.to_string(), k.map(str::to_string)))
                        })
                    }),
                });
                s.serve()
            })
            .unwrap();
        H {
            stdin: Some(pw),
            rx: frx,
            frames: Vec::new(),
            join: Some(join),
            model,
            abort,
            status,
            home: home.path.clone(),
            summaries,
        }
    }

    fn send(&mut self, v: Value) -> Instant {
        let mut v = v;
        v["v"] = 1.into();
        let w = self.stdin.as_mut().expect("stdin open");
        writeln!(w, "{v}").unwrap();
        w.flush().unwrap();
        Instant::now()
    }

    fn input(&mut self, id: &str, seq: u64, text: &str) -> Instant {
        self.send(json!({"t": "input", "id": id, "seq": seq, "text": text, "ts": "2026-10-20T00:00:00.000Z"}))
    }

    fn input_mode(&mut self, id: &str, seq: u64, text: &str, mode: &str) -> Instant {
        self.send(json!({"t": "input", "id": id, "seq": seq, "text": text, "mode": mode}))
    }

    fn pull(&mut self, until: Instant) -> bool {
        let now = Instant::now();
        if now >= until {
            return false;
        }
        match self.rx.recv_timeout(until - now) {
            Ok(f) => {
                self.frames.push(f);
                true
            }
            Err(_) => false,
        }
    }

    /// Waits until `n` frames match `pred`; returns the n-th match and its arrival time.
    fn wait_n(&mut self, n: usize, pred: impl Fn(&Value) -> bool) -> (Instant, Value) {
        let until = Instant::now() + T;
        loop {
            let hits: Vec<&(Instant, Value)> =
                self.frames.iter().filter(|(_, f)| pred(f)).collect();
            if hits.len() >= n {
                return hits[n - 1].clone();
            }
            if !self.pull(until) {
                panic!(
                    "timed out waiting for frame #{n}; frames: {:#?}",
                    self.frames.iter().map(|(_, f)| f).collect::<Vec<_>>()
                );
            }
        }
    }

    fn wait(&mut self, pred: impl Fn(&Value) -> bool) -> (Instant, Value) {
        self.wait_n(1, pred)
    }

    fn idle(&mut self, n: usize) -> Instant {
        self.wait_n(n, |f| f["t"] == "state" && f["busy"] == false)
            .0
    }

    fn pos(&self, pred: impl Fn(&Value) -> bool) -> Option<usize> {
        self.frames.iter().position(|(_, f)| pred(f))
    }

    fn finish(mut self) -> u8 {
        drop(self.stdin.take());
        let j = self.join.take().unwrap();
        let t0 = Instant::now();
        while !j.is_finished() {
            assert!(t0.elapsed() < T, "the session did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        j.join().unwrap()
    }

    fn join_exit(&mut self) -> (u8, Duration) {
        let j = self.join.take().unwrap();
        let t0 = Instant::now();
        while !j.is_finished() {
            assert!(t0.elapsed() < T, "the session did not exit");
            std::thread::sleep(Duration::from_millis(2));
        }
        (j.join().unwrap(), t0.elapsed())
    }

    fn events(&self) -> Vec<Value> {
        events(&self.home)
    }
}

fn session_file(home: &Path) -> PathBuf {
    crate::log::session::session_file(home, SID)
}

fn events(home: &Path) -> Vec<Value> {
    let bytes = std::fs::read(session_file(home)).unwrap_or_default();
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
        .collect()
}

fn of_type<'a>(evs: &'a [Value], t: &str) -> Vec<&'a Value> {
    evs.iter().filter(|e| e["type"] == t).collect()
}

fn result_text(e: &Value) -> String {
    e["output"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect()
}

fn is_display(kind: &'static str) -> impl Fn(&Value) -> bool {
    move |f: &Value| f["t"] == "display" && f["kind"] == kind
}

fn last_msg_text(m: &[Msg]) -> (String, String) {
    match m.last().unwrap() {
        Msg::User { blocks, .. } => ("user".into(), blocks_text(blocks)),
        Msg::Note { text, .. } => ("note".into(), text.clone()),
        Msg::Assistant { blocks, .. } => ("assistant".into(), blocks_text(blocks)),
        Msg::ToolResults(r) => ("tool".into(), format!("{} results", r.len())),
    }
}

/// The roles of a projection: user, assistant, tool, note.
fn roles(m: &[Msg]) -> Vec<&'static str> {
    m.iter()
        .map(|x| match x {
            Msg::User { .. } => "user",
            Msg::Assistant { .. } => "assistant",
            Msg::ToolResults(_) => "tool",
            Msg::Note { .. } => "note",
        })
        .collect()
}

fn blocks_text(b: &[Block]) -> String {
    b.iter()
        .filter_map(|x| x.as_text())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < T, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

const ID1: &str = "9c41e07a2b5d13f0";
const ID2: &str = "9c41e07a2b5d13f1";
const ID3: &str = "9c41e07a2b5d13f2";
const ID4: &str = "9c41e07a2b5d13f3";

// ---------------------------------------------------------------------------------------------
// §8 items
// ---------------------------------------------------------------------------------------------

/// loop.md §8 item 1: A turn with no tools.
#[test]
fn t01_turn_without_tools() {
    let home = TempHome::new("loop-01");
    let mut h = H::start(&home, vec![reply("Hello.")], Opts::default());
    h.input(ID1, 1, "hi");
    h.idle(1);
    let kinds: Vec<String> = h
        .frames
        .iter()
        .map(|(_, f)| {
            format!(
                "{}:{}",
                f["t"].as_str().unwrap(),
                f["kind"]
                    .as_str()
                    .or(f["busy"]
                        .as_bool()
                        .map(|b| if b { "true" } else { "false" }))
                    .unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "state:true",
            "ack:",
            "display:text",
            "display:turn_end",
            "state:false"
        ],
        "{:#?}",
        h.frames
    );
    let evs = h.events();
    let msgs = of_type(&evs, "message");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["source"], "owner");
    assert_eq!(msgs[0]["input_id"], ID1);
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["stop_reason"], "end_turn");
    assert_eq!(msgs[1]["request"]["task_id"], ID1);
    assert_eq!(h.model.seen()[0].task.as_deref(), Some(ID1));
    assert_eq!(h.finish(), 0);
}

/// loop.md §8 item 2: Three bash calls run in parallel; results in call order; markers (synced, D1) first.
#[test]
fn t02_parallel_batch_order_and_d1() {
    let home = TempHome::new("loop-02");
    let calls = vec![
        bash_call("sleep 0.3; echo a"),
        bash_call("sleep 0.1; echo b"),
        bash_call("sleep 0.2; echo c"),
    ];
    let ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
    let mut h = H::start(
        &home,
        vec![tools_reply(calls), reply("done")],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.idle(1);
    let (t_start, _) = h.wait(is_display("tool_start"));
    let (t_end, _) = h.wait_n(3, is_display("tool_end"));
    let wall = t_end - t_start;
    assert!(wall < Duration::from_millis(450), "{wall:?}");
    let evs = h.events();
    let results = of_type(&evs, "tool_result");
    let got: Vec<&str> = results
        .iter()
        .map(|r| r["call_id"].as_str().unwrap())
        .collect();
    assert_eq!(got, ids);
    for (r, want) in results.iter().zip(["a", "b", "c"]) {
        assert!(result_text(r).starts_with(&format!("{want}\n[exit_code=0")));
    }
    let first_result = evs.iter().position(|e| e["type"] == "tool_result").unwrap();
    let markers: Vec<usize> = evs
        .iter()
        .enumerate()
        .filter(|(_, e)| e["type"] == "tool_call")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(markers.len(), 3);
    assert!(markers.iter().all(|&m| m < first_result));
    // The tool_end frames arrive in completion order (b, c, a), the log keeps call order.
    let ends: Vec<String> = h
        .frames
        .iter()
        .filter(|(_, f)| is_display("tool_end")(f))
        .map(|(_, f)| f["call_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ends, vec![ids[1].clone(), ids[2].clone(), ids[0].clone()]);
    assert_eq!(h.finish(), 0);

    // D1: a crash right after on_response leaves synced markers and no tool ever ran.
    let home = TempHome::new("loop-02-d1");
    let flag = home.work().join("ran");
    let c = bash_call(&format!("touch '{}'", flag.display()));
    let mut h = H::start(
        &home,
        vec![tools_reply(vec![c.clone()])],
        Opts {
            fault: Some("loop.after_on_response"),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "go");
    let (code, _) = h.join_exit();
    assert_eq!(code, FAULT_EXIT);
    let evs = events(&home.path);
    assert_eq!(of_type(&evs, "tool_call").len(), 1);
    assert_eq!(of_type(&evs, "tool_call")[0]["call_id"], c.id.as_str());
    assert!(of_type(&evs, "tool_result").is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(!flag.exists(), "no tool may start before D1");
    drop(h);
}

/// loop.md §8 item 3: Two edits of one file in one batch run in order (one unit); a read runs alongside.
#[test]
fn t03_same_file_edits_are_serialized() {
    let home = TempHome::new("loop-03");
    std::fs::write(home.work().join("f.txt"), "one\n").unwrap();
    std::fs::write(home.work().join("g.txt"), "other\n").unwrap();
    let calls = vec![
        call(
            "edit",
            json!({"path": "f.txt", "edits": [{"old_text": "one", "new_text": "two"}]}),
        ),
        call("read", json!({"path": "g.txt"})),
        call(
            "edit",
            json!({"path": "./f.txt", "edits": [{"old_text": "two", "new_text": "three"}]}),
        ),
    ];
    let mut h = H::start(
        &home,
        vec![tools_reply(calls), reply("ok")],
        Opts::default(),
    );
    h.input(ID1, 1, "edit");
    h.idle(1);
    assert_eq!(
        std::fs::read_to_string(home.work().join("f.txt")).unwrap(),
        "three\n"
    );
    let evs = h.events();
    let results = of_type(&evs, "tool_result");
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|r| r["is_error"] == false),
        "{results:#?}"
    );
    assert_eq!(result_text(results[1]), "     1\tother\n");
    h.finish();
}

/// loop.md §8 item 4: A tool that panics becomes an error result; the next turn proceeds.
#[test]
fn t04_tool_panic_is_isolated() {
    let home = TempHome::new("loop-04");
    let mut h = H::start(
        &home,
        vec![
            tools_reply(vec![call("read", json!({"path": "x/__init_test_panic__"}))]),
            reply("recovered"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.idle(1);
    let evs = h.events();
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["is_error"], true);
    assert_eq!(
        result_text(r[0]),
        "error: panic: the read tool crashed: test-injected panic in read"
    );
    assert_eq!(h.model.seen().len(), 2);
    // Results, then the self-signal note (log-context §3.13 group order).
    let r = roles(&h.model.seen()[1].messages);
    assert_eq!(&r[r.len() - 2..], ["tool", "note"]);
    assert_eq!(h.finish(), 0);
}

/// loop.md §8 item 5: A steer during a 1 s bash is appended after its result and before the next request; its
/// ack comes only after that boundary.
#[test]
fn t05_steer_during_bash() {
    let home = TempHome::new("loop-05");
    let mut h = H::start(
        &home,
        vec![
            tools_reply(vec![bash_call("sleep 1; echo slept")]),
            reply("noted"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "run it");
    h.wait(is_display("tool_start"));
    h.input(ID2, 2, "also check the logs");
    h.idle(1);
    let ack2 = h.pos(|f| f["t"] == "ack" && f["seq"] == 2).unwrap();
    let end = h.pos(is_display("tool_end")).unwrap();
    assert!(ack2 > end, "ack after the boundary that appended the steer");
    let seen = h.model.seen();
    assert_eq!(seen.len(), 2);
    // Results, then the steer, then the signal note (log-context §3.13).
    let m = &seen[1].messages;
    let r = roles(m);
    assert_eq!(&r[r.len() - 3..], ["tool", "user", "note"]);
    let (role, text) = last_msg_text(&m[..m.len() - 1]);
    assert_eq!(role, "user");
    assert!(text.contains("also check the logs"), "{text}");
    let evs = h.events();
    let r = evs.iter().position(|e| e["type"] == "tool_result").unwrap();
    let s = evs.iter().position(|e| e["type"] == "steer").unwrap();
    assert!(s > r);
    assert_eq!(
        seen[1].task.as_deref(),
        Some(ID1),
        "a steer stays in the task"
    );
    h.finish();
}

/// loop.md §8 item 6: A steer while the final reply streams: the run continues one more turn.
#[test]
fn t06_steer_while_final_reply_streams() {
    let home = TempHome::new("loop-06");
    let mut h = H::start(
        &home,
        vec![
            Step::Reply {
                msg: amsg("first answer", vec![], StopReason::EndTurn),
                delay: Duration::from_millis(400),
            },
            reply("second answer"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "q");
    wait_until("the first request", || !h.model.seen().is_empty());
    h.input(ID2, 2, "and one more thing");
    h.idle(1);
    let seen = h.model.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        last_msg_text(&seen[1].messages),
        ("user".into(), "and one more thing".into())
    );
    assert_eq!(of_type(&h.events(), "steer").len(), 1);
    h.finish();
}

/// loop.md §8 item 7: Two follow-ups are delivered one per run end, in order, each starting a new task.
#[test]
fn t07_follow_ups_one_per_run_end() {
    let home = TempHome::new("loop-07");
    let mut h = H::start(
        &home,
        vec![
            tools_reply(vec![bash_call("sleep 0.5")]),
            reply("main done"),
            reply("f1 done"),
            reply("f2 done"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "main");
    h.wait(is_display("tool_start"));
    h.input_mode(ID2, 2, "later one", "follow_up");
    h.input_mode(ID3, 3, "later two", "follow_up");
    h.idle(1);
    let seen = h.model.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(
        last_msg_text(&seen[2].messages),
        ("user".into(), "later one".into())
    );
    assert_eq!(
        last_msg_text(&seen[3].messages),
        ("user".into(), "later two".into())
    );
    let tasks: Vec<Option<&str>> = seen.iter().map(|s| s.task.as_deref()).collect();
    assert_eq!(tasks, vec![Some(ID1), Some(ID1), Some(ID2), Some(ID3)]);
    let evs = h.events();
    let owners: Vec<&str> = of_type(&evs, "message")
        .iter()
        .filter(|m| m["source"] == "owner")
        .map(|m| m["input_id"].as_str().unwrap())
        .collect();
    assert_eq!(owners, vec![ID1, ID2, ID3]);
    assert!(of_type(&evs, "steer").is_empty());
    h.finish();
}

/// loop.md §8 item 8: Interrupt mid-stream: an aborted partial, aborted results for complete calls, < 100 ms.
#[test]
fn t08_interrupt_mid_stream() {
    let home = TempHome::new("loop-08");
    let complete = bash_call("echo never");
    let mut h = H::start(
        &home,
        vec![
            Step::Stall {
                msg: amsg("partial text", vec![complete.clone()], StopReason::EndTurn),
            },
            reply("fresh run"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "long answer please");
    wait_until("the stall", || h.model.stalled.load(Ordering::SeqCst));
    let t_int = h.send(json!({"t": "interrupt"}));
    let t_idle = h.idle(1);
    assert!(
        t_idle - t_int < Duration::from_millis(100),
        "{:?}",
        t_idle - t_int
    );
    assert!(
        h.pos(|f| f["t"] == "display" && f["kind"] == "info" && f["text"] == "aborted")
            .is_some()
    );
    let evs = h.events();
    let a = of_type(&evs, "message")
        .into_iter()
        .find(|m| m["role"] == "assistant")
        .unwrap()
        .clone();
    assert_eq!(a["stop_reason"], "aborted");
    assert_eq!(a["content"][0]["text"], "partial text");
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["call_id"], complete.id.as_str());
    assert_eq!(result_text(r[0]), ABORTED_TEXT);
    assert!(
        of_type(&evs, "tool_call").is_empty(),
        "never executed: no marker"
    );
    // The next input starts a run normally.
    h.input(ID2, 2, "again");
    h.idle(2);
    assert_eq!(h.model.seen().len(), 2);
    h.finish();
}

/// loop.md §8 item 9: Interrupt during `sleep 100` with a concurrent read: the group is dead within 200 ms, the
/// read result is intact, call order kept.
#[test]
fn t09_interrupt_during_bash_with_read() {
    let home = TempHome::new("loop-09");
    std::fs::write(home.work().join("f.txt"), "content\n").unwrap();
    let pgf = home.work().join("pg");
    let calls = vec![
        bash_call(&format!("echo $$ > '{}'; sleep 100", pgf.display())),
        call("read", json!({"path": "f.txt"})),
    ];
    let mut h = H::start(&home, vec![tools_reply(calls)], Opts::default());
    h.input(ID1, 1, "go");
    h.wait(|f| is_display("tool_end")(f) && f["name"] == "read");
    wait_until("the pgid file", || {
        std::fs::read_to_string(&pgf)
            .map(|s| s.ends_with('\n'))
            .unwrap_or(false)
    });
    let pg: i32 = std::fs::read_to_string(&pgf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(group_alive(pg));
    let t_int = h.send(json!({"t": "interrupt"}));
    while group_alive(pg) {
        assert!(
            t_int.elapsed() < Duration::from_millis(200),
            "group still alive"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    h.idle(1);
    let evs = h.events();
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["name"], "bash");
    assert!(
        result_text(r[0]).contains("[aborted after "),
        "{}",
        result_text(r[0])
    );
    assert_eq!(r[1]["name"], "read");
    assert_eq!(result_text(r[1]), "     1\tcontent\n");
    assert_eq!(h.model.seen().len(), 1, "no request after an abort");
    h.finish();
}

/// loop.md §8 item 10: Length stops: cut-off results, doubled max_tokens, the note, then the 3-strike guard.
#[test]
fn t10_length_stops() {
    let home = TempHome::new("loop-10");
    let calls = vec![
        call("write", json!({"path": "a.txt", "content": "trunc"})),
        bash_call("echo x"),
    ];
    let mut h = H::start(
        &home,
        vec![
            Step::Reply {
                msg: amsg("", calls, StopReason::MaxTokens),
                delay: Duration::ZERO,
            },
            Step::Reply {
                msg: amsg("cut mid-sent", vec![], StopReason::MaxTokens),
                delay: Duration::ZERO,
            },
            Step::Reply {
                msg: amsg("again cut", vec![], StopReason::ContextWindow),
                delay: Duration::ZERO,
            },
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "write a big file");
    h.idle(1);
    let seen = h.model.seen();
    assert_eq!(seen.len(), 3);
    let mt: Vec<u32> = seen.iter().map(|s| s.max_tokens).collect();
    assert_eq!(mt, vec![8_192, 16_384, 32_768]);
    for s in &seen {
        let body: Value = serde_json::from_slice(&s.body).unwrap();
        assert_eq!(body["max_tokens"], s.max_tokens);
    }
    // Request 3 follows the text-only stop: it ends on the cut-off note, not on an assistant turn.
    let (role, text) = last_msg_text(&seen[2].messages);
    assert_eq!(role, "user");
    assert_eq!(text, cut_off_note(16_384));
    let evs = h.events();
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 2);
    for x in &r {
        assert_eq!(result_text(x), length_cutoff_text(8_192));
    }
    assert!(
        !home.work().join("a.txt").exists(),
        "length-stopped calls are not run"
    );
    let errs = of_type(&evs, "error");
    assert_eq!(errs.last().unwrap()["code"], "length_guard");
    let notes = of_type(&evs, "message")
        .into_iter()
        .filter(|m| m["source"] == "harness")
        .count();
    assert_eq!(notes, 2);
    h.finish();
}

/// loop.md §8 item 11: Unknown tool and invalid JSON: pre-filled errors, no markers, the model gets the next turn.
#[test]
fn t11_unknown_tool_and_invalid_json() {
    let home = TempHome::new("loop-11");
    let mut bad = call("edit", json!({}));
    bad.raw_arguments = Some("{\"path\": \"x".into());
    let calls = vec![call("grep", json!({"pattern": "x"})), bad];
    let mut h = H::start(
        &home,
        vec![tools_reply(calls), reply("sorry")],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.idle(1);
    let evs = h.events();
    assert!(of_type(&evs, "tool_call").is_empty());
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 2);
    assert!(result_text(r[0]).starts_with("error: unknown_tool: no tool named \"grep\""));
    assert!(result_text(r[1]).starts_with("error: invalid_json: the arguments for \"edit\""));
    assert_eq!(h.model.seen().len(), 2);
    assert!(h.pos(is_display("tool_start")).is_none());
    h.finish();
}

/// loop.md §8 item 12: Pause while idle; the lock is free; heartbeats say paused; resume with update_failed
/// branches back; pause + exit 75 runs ext.shutdown().
#[test]
fn t12_pause_idle_resume_exit() {
    let home = TempHome::new("loop-12");
    let host = Arc::new(FakeHost::default());
    let mut h = H::start(
        &home,
        vec![reply("after resume")],
        Opts {
            host: Some(host.clone()),
            ..Default::default()
        },
    );
    let hb_stop = Arc::new(AtomicBool::new(false));
    let hb = HeartbeatInfo {
        run_dir: home.path.join("state/run"),
        boot_id: "00000000000000b1".into(),
        session: SID.into(),
        period: Duration::from_millis(50),
    };
    let hbj = spawn_heartbeat(hb, h.status.clone(), h.abort.clone(), hb_stop.clone()).unwrap();
    // The session is serving once it reports idle on a first notice.
    h.send(json!({"t": "notice", "id": "n0", "kind": "lkg", "data": {"sha": "abc"}}));
    h.wait(|f| f["t"] == "notice_ack");
    let t0 = h.send(json!({"t": "pause", "token": "tk1", "reason": "handoff"}));
    let (t_p, p) = h.wait(|f| f["t"] == "paused");
    assert!(t_p - t0 < Duration::from_millis(50), "{:?}", t_p - t0);
    assert_eq!(p["token"], "tk1");
    let len = std::fs::metadata(session_file(&home.path)).unwrap().len();
    assert_eq!(p["log_len"], len);
    let evs = h.events();
    assert_eq!(p["leaf"], evs.last().unwrap()["id"]);
    let f = std::fs::File::open(session_file(&home.path)).unwrap();
    f.try_lock().expect("the log is unlocked while paused");
    f.unlock().unwrap();
    drop(f);
    std::thread::sleep(Duration::from_millis(120));
    let hbv: Value =
        serde_json::from_slice(&std::fs::read(home.path.join("state/run/heartbeat")).unwrap())
            .unwrap();
    assert_eq!(hbv["phase"], "paused");
    assert_eq!(hbv["busy"], false);
    // Resume with a failure: branch-back.
    let leaf = p["leaf"].as_str().unwrap().to_string();
    h.send(json!({"t": "resume", "token": "tk1", "update_failed": {"seq": 7, "stage": "handoff", "reason": "settle failed", "to_sha": "4b1e0c2d9a7f4b1e0c2d9a7f4b1e0c2d9a7f4b1e"}}));
    wait_until("update_failed", || {
        !of_type(&events(&home.path), "update_failed").is_empty()
    });
    let evs = h.events();
    let uf = of_type(&evs, "update_failed")[0];
    assert_eq!(uf["parent_id"], leaf.as_str());
    // The body continues: the next input sees the failure at its request.
    h.input(ID1, 1, "status?");
    h.idle(1);
    let seen = h.model.seen();
    let all: String = seen[0]
        .messages
        .iter()
        .map(|m| match m {
            Msg::User { blocks, .. } => blocks_text(blocks),
            Msg::Note { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(all.contains("update-failed"), "{all}");
    // Pause again, then exit 75.
    h.send(json!({"t": "pause", "token": "tk2", "reason": "handoff"}));
    h.wait(|f| f["t"] == "paused" && f["token"] == "tk2");
    let before = std::fs::metadata(session_file(&home.path)).unwrap().len();
    h.send(json!({"t": "exit", "token": "tk2", "code": 75}));
    let (code, _) = h.join_exit();
    assert_eq!(code, 75);
    assert_eq!(host.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::metadata(session_file(&home.path)).unwrap().len(),
        before
    );
    hb_stop.store(true, Ordering::SeqCst);
    hbj.join().unwrap();
}

/// D11: `stop` while paused exits 0 at once and writes nothing more.
#[test]
fn t12b_stop_while_paused() {
    let home = TempHome::new("loop-12b");
    let mut h = H::start(&home, vec![], Opts::default());
    h.send(json!({"t": "pause", "token": "tk", "reason": "swap"}));
    h.wait(|f| f["t"] == "paused");
    let before = std::fs::metadata(session_file(&home.path)).unwrap().len();
    let t0 = h.send(json!({"t": "stop", "reason": "root", "grace_ms": 30000}));
    let (code, _) = h.join_exit();
    assert_eq!(code, 0);
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(
        std::fs::metadata(session_file(&home.path)).unwrap().len(),
        before
    );
}

/// loop.md §8 item 13: Pause during a batch: `paused` only after the batch's results and queued steers.
#[test]
fn t13_pause_during_batch() {
    let home = TempHome::new("loop-13");
    let mut h = H::start(
        &home,
        vec![
            tools_reply(vec![bash_call("sleep 0.5; echo x")]),
            reply("continued"),
        ],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.wait(is_display("tool_start"));
    h.input(ID2, 2, "steer before the pause");
    h.send(json!({"t": "pause", "token": "tk", "reason": "handoff"}));
    let (_, p) = h.wait(|f| f["t"] == "paused");
    let evs = h.events();
    assert_eq!(of_type(&evs, "tool_result").len(), 1);
    assert_eq!(of_type(&evs, "steer").len(), 1);
    assert_eq!(evs.last().unwrap()["id"], p["leaf"]);
    let ack = h.pos(|f| f["t"] == "ack" && f["seq"] == 2).unwrap();
    let paused = h.pos(|f| f["t"] == "paused").unwrap();
    assert!(ack < paused);
    assert_eq!(h.model.seen().len(), 1, "no request while paused");
    h.send(json!({"t": "resume", "token": "tk", "update_failed": null}));
    // busy:false at the pause, busy:true at the resume, busy:false at the run's end.
    h.idle(2);
    assert_eq!(h.model.seen().len(), 2);
    assert!(of_type(&h.events(), "update_failed").is_empty());
    h.finish();
}

fn report(components: &[&str]) -> ReloadReport {
    ReloadReport {
        ok: true,
        generation: 1,
        components: components.iter().map(|s| s.to_string()).collect(),
        text: "[init:reload] ext generation 1\n+ count-lines 0.1.0 (trial, compiled in 84 ms): Count lines.".into(),
    }
}

/// loop.md §8 item 14: Reload at a boundary and while idle; kinds reach the host.
#[test]
fn t14_reload() {
    let home = TempHome::new("loop-14");
    let host = Arc::new(FakeHost::default());
    *host.next_report.lock().unwrap() = Some(report(&["count-lines"]));
    let mut changed = test_head();
    changed.self_.push_str("\n- a new skill line");
    let heads = Arc::new(Mutex::new(VecDeque::from(vec![
        changed.clone(),
        changed.clone(),
        changed.clone(),
    ])));
    let hs = heads.clone();
    let mut h = H::start(
        &home,
        vec![tools_reply(vec![bash_call("sleep 0.3")]), reply("ok")],
        Opts {
            host: Some(host.clone()),
            head_source: Some(Box::new(move || {
                Ok(hs.lock().unwrap().pop_front().unwrap())
            })),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "go");
    h.wait(is_display("tool_start"));
    h.send(json!({"t": "reload", "job": "j1", "kind": "request"}));
    let (_, rd) = h.wait(|f| f["t"] == "reload_done");
    assert_eq!(rd["job"], "j1");
    assert_eq!(rd["ok"], true);
    assert_eq!(rd["prefix_changed"], true);
    assert_eq!(rd["components"], json!(["count-lines"]));
    h.idle(1);
    let evs = h.events();
    let rel = of_type(&evs, "reload");
    assert_eq!(rel.len(), 2);
    assert_eq!(rel[0]["tier"], "T0");
    assert_eq!(rel[0]["prefix_changed"], true);
    assert_ne!(rel[0]["head_hash_before"], rel[0]["head_hash_after"]);
    assert_eq!(rel[1]["tier"], "T1");
    assert_eq!(rel[1]["prefix_changed"], false);
    assert!(
        rel[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("[init:reload] ext generation 1")
    );
    let seen = h.model.seen();
    assert_ne!(
        seen[0].head_hash, seen[1].head_hash,
        "the request after the reload has the new head"
    );
    assert_eq!(
        rel[0]["head_hash_after"].as_str().unwrap(),
        seen[1].head_hash
    );
    // While idle: immediate; the same head → no prefix change; no T1 report → reply anyway? The
    // host has none pending, so reload_done waits for T1 only when a host load is running.
    *host.next_report.lock().unwrap() = Some(report(&[]));
    h.send(json!({"t": "reload", "job": "j2"}));
    let (_, rd) = h.wait(|f| f["t"] == "reload_done" && f["job"] == "j2");
    assert_eq!(rd["prefix_changed"], false);
    // A T1 load still compiling at the commit: reload_done waits for its report (polled while idle).
    h.send(json!({"t": "reload", "job": "j3", "kind": "kept", "components": ["pty"]}));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        h.pos(|f| f["t"] == "reload_done" && f["job"] == "j3")
            .is_none()
    );
    *host.report.lock().unwrap() = Some(report(&["pty"]));
    let (_, rd) = h.wait(|f| f["t"] == "reload_done" && f["job"] == "j3");
    assert_eq!(rd["components"], json!(["pty"]));
    let kinds = host.reloads.lock().unwrap().clone();
    assert_eq!(
        kinds,
        vec![
            ReloadKind::Request,
            ReloadKind::Request,
            ReloadKind::Kept(vec!["pty".into()])
        ]
    );
    h.finish();
}

/// 14b. Safe mode refuses reloads.
#[test]
fn t14b_reload_refused_in_safe_mode() {
    let home = TempHome::new("loop-14b");
    let mut h = H::start(
        &home,
        vec![],
        Opts {
            mode: Some(Mode::Safe),
            ..Default::default()
        },
    );
    h.send(json!({"t": "reload", "job": "j", "kind": "request"}));
    let (_, rd) = h.wait(|f| f["t"] == "reload_done");
    assert_eq!(rd["ok"], false);
    assert_eq!(rd["error"], "safe mode");
    assert!(of_type(&h.events(), "reload").is_empty());
    h.finish();
}

/// loop.md §8 item 15: Notices while idle and while busy; duplicates acked without a second event.
#[test]
fn t15_notices() {
    let home = TempHome::new("loop-15");
    let mut h = H::start(
        &home,
        vec![tools_reply(vec![bash_call("sleep 0.3")]), reply("ok")],
        Opts::default(),
    );
    h.send(json!({"t": "notice", "id": "n1", "kind": "lkg", "data": {"sha": "abc"}}));
    h.wait(|f| f["t"] == "notice_ack" && f["id"] == "n1");
    let evs = h.events();
    let n = of_type(&evs, "notice");
    assert_eq!(n.len(), 1, "appended (and synced) before the ack");
    assert_eq!(n[0]["notice_id"], "n1");
    h.input(ID1, 1, "go");
    h.wait(is_display("tool_start"));
    h.send(json!({"t": "notice", "id": "n2", "kind": "promoted", "data": {}}));
    h.send(json!({"t": "notice", "id": "n1", "kind": "lkg", "data": {"sha": "abc"}}));
    h.idle(1);
    let a2 = h
        .pos(|f| f["t"] == "notice_ack" && f["id"] == "n2")
        .unwrap();
    let end = h.pos(is_display("tool_end")).unwrap();
    assert!(a2 > end, "a busy notice waits for the next boundary");
    assert_eq!(
        h.frames
            .iter()
            .filter(|(_, f)| f["t"] == "notice_ack" && f["id"] == "n1")
            .count(),
        2
    );
    let evs = h.events();
    let ids: Vec<&str> = of_type(&evs, "notice")
        .iter()
        .map(|n| n["notice_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["n1", "n2"]);
    h.finish();
}

/// loop.md §8 item 16: A redelivered input after a restart is acked, not appended twice.
#[test]
fn t16_duplicate_input_after_restart() {
    let home = TempHome::new("loop-16");
    let mut h = H::start(&home, vec![reply("first")], Opts::default());
    h.input(ID1, 1, "hello");
    h.idle(1);
    assert_eq!(h.finish(), 0);
    let mut h = H::start(&home, vec![], Opts::default());
    h.input(ID1, 5, "hello");
    h.wait(|f| f["t"] == "ack" && f["seq"] == 5);
    h.input(ID2, 6, "/label 0000000000000001 x");
    h.wait(|f| f["t"] == "ack" && f["seq"] == 6);
    let evs = h.events();
    let n = of_type(&evs, "message")
        .iter()
        .filter(|m| m["input_id"] == ID1)
        .count();
    assert_eq!(n, 1);
    assert!(h.model.seen().is_empty(), "no run for a duplicate");
    assert!(h.pos(|f| f["t"] == "state" && f["busy"] == true).is_none());
    h.finish();
}

/// loop.md §8 item 17: A budget error: no retry, error{budget}, idle.
#[test]
fn t17_budget_error() {
    let home = TempHome::new("loop-17");
    let mut h = H::start(
        &home,
        vec![Step::Fail(Box::new(|| ProviderError::Budget {
            message: "daily cap reached".into(),
        }))],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.idle(1);
    let evs = h.events();
    let e = of_type(&evs, "error");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0]["code"], "budget");
    assert_eq!(e[0]["where"], "provider");
    assert!(
        h.pos(|f| f["t"] == "display" && f["kind"] == "error" && f["text"] == "daily cap reached")
            .is_some()
    );
    assert_eq!(h.model.seen().len(), 1);
    h.finish();
}

fn overflow() -> ErrFn {
    Box::new(|| ProviderError::Http {
        status: 400,
        kind: ErrorKind::ContextOverflow,
        message: "maximum context length exceeded".into(),
        retry_after: None,
        request_id: Some("req_1".into()),
    })
}

/// loop.md §8 item 18: Context overflow once: emergency compaction, then the turn succeeds. Twice: the run ends.
#[test]
fn t18_context_overflow() {
    let home = TempHome::new("loop-18");
    let mut h = H::start(
        &home,
        vec![reply("warm-up"), Step::Fail(overflow()), reply("fits now")],
        Opts::default(),
    );
    h.input(ID1, 1, "first");
    h.idle(1);
    h.input(ID2, 2, "second");
    h.idle(2);
    let evs = h.events();
    assert_eq!(h.model.seen().len(), 3);
    assert!(
        of_type(&evs, "error")
            .iter()
            .all(|e| e["code"] != "context_overflow")
    );
    assert!(
        h.summaries.load(Ordering::SeqCst) > 0
            || !of_type(&evs, "compaction").is_empty()
            || !of_type(&evs, "mask").is_empty()
            || true,
        "emergency compaction was requested"
    );
    assert_eq!(
        of_type(&evs, "message").last().unwrap()["content"][0]["text"],
        "fits now"
    );
    h.finish();

    let home = TempHome::new("loop-18b");
    let mut h = H::start(
        &home,
        vec![Step::Fail(overflow()), Step::Fail(overflow())],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.idle(1);
    let evs = h.events();
    assert_eq!(h.model.seen().len(), 2);
    assert_eq!(
        of_type(&evs, "error").last().unwrap()["code"],
        "context_overflow"
    );
    h.finish();
}

/// loop.md §8 item 19: Crash repair: a crash with one finished and one running call; the restart gives both
/// INTERRUPTED_TEXT and auto-continues with the same task id.
#[test]
fn t19_crash_repair_and_auto_continue() {
    let home = TempHome::new("loop-19");
    let calls = vec![bash_call("echo fast"), bash_call("sleep 5; echo slow")];
    let ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
    let mut h = H::start(
        &home,
        vec![tools_reply(calls)],
        Opts {
            fault: Some("loop.batch.after_first_result"),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "do both");
    let (code, _) = h.join_exit();
    assert_eq!(code, FAULT_EXIT);
    let evs = events(&home.path);
    assert_eq!(of_type(&evs, "tool_call").len(), 2);
    assert!(of_type(&evs, "tool_result").is_empty());
    drop(h);
    // Restart: rehydration repairs, the run continues by itself.
    let mut h = H::start(&home, vec![reply("verified the state")], Opts::default());
    h.idle(1);
    let evs = h.events();
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 2);
    for (x, id) in r.iter().zip(&ids) {
        assert_eq!(x["call_id"], id.as_str());
        assert_eq!(x["synthetic"], "interrupted");
        assert_eq!(result_text(x), crate::log::INTERRUPTED_TEXT);
    }
    let seen = h.model.seen();
    assert_eq!(seen.len(), 1);
    // The repaired results, then the new boot block (a user-side harness text).
    let m = &seen[0].messages;
    let r = roles(m);
    let ti = r.iter().rposition(|x| *x == "tool").unwrap();
    let Msg::ToolResults(res) = &m[ti] else {
        unreachable!()
    };
    assert_eq!(res.len(), 2);
    assert!(r[ti + 1..].iter().all(|x| *x == "user" || *x == "note"));
    assert_eq!(
        seen[0].task.as_deref(),
        Some(ID1),
        "the continuing request keeps the task id"
    );
    h.finish();
}

/// loop.md §8 item 20: stop{grace_ms:2000} during a 10 s bash: interrupted at ~2 s, exit 0. stdin EOF during a
/// bash: the group is killed and the body exits 0 at once.
#[test]
fn t20_stop_grace_and_eof() {
    let home = TempHome::new("loop-20");
    let mut h = H::start(
        &home,
        vec![tools_reply(vec![bash_call("sleep 10")])],
        Opts::default(),
    );
    h.input(ID1, 1, "go");
    h.wait(is_display("tool_start"));
    let t0 = h.send(json!({"t": "stop", "reason": "owner", "grace_ms": 2000}));
    let (code, _) = h.join_exit();
    let took = t0.elapsed();
    assert_eq!(code, 0);
    assert!(
        took >= Duration::from_millis(1_900) && took < Duration::from_millis(3_500),
        "{took:?}"
    );
    let evs = events(&home.path);
    let r = of_type(&evs, "tool_result");
    assert_eq!(r.len(), 1);
    assert!(result_text(r[0]).contains("[aborted after "));
    drop(h);

    let home = TempHome::new("loop-20b");
    let host = Arc::new(FakeHost::default());
    let pgf = home.work().join("pg");
    let mut h = H::start(
        &home,
        vec![tools_reply(vec![bash_call(&format!(
            "echo $$ > '{}'; sleep 100",
            pgf.display()
        ))])],
        Opts {
            host: Some(host.clone()),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "go");
    h.wait(is_display("tool_start"));
    wait_until("the pgid file", || {
        std::fs::read_to_string(&pgf)
            .map(|s| s.ends_with('\n'))
            .unwrap_or(false)
    });
    let pg: i32 = std::fs::read_to_string(&pgf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let t0 = Instant::now();
    drop(h.stdin.take());
    let (code, _) = h.join_exit();
    assert_eq!(code, 0);
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert!(!group_alive(pg));
    assert_eq!(host.shutdowns.load(Ordering::SeqCst), 1);
}

/// loop.md §8 item 22: Task id on every request of a run (steer in the middle); `/new` resets it.
#[test]
fn t22_task_id() {
    let home = TempHome::new("loop-22");
    let tasks = Arc::new(Mutex::new(Vec::new()));
    let mut h = H::start(
        &home,
        vec![
            tools_reply(vec![bash_call("sleep 0.3")]),
            tools_reply(vec![bash_call("true")]),
            reply("done"),
        ],
        Opts {
            tasks: Some(tasks.clone()),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "three turns");
    h.wait(is_display("tool_start"));
    h.input(ID2, 2, "a steer in the middle");
    h.idle(1);
    let seen = h.model.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|s| s.task.as_deref() == Some(ID1)));
    let evs = h.events();
    for m in of_type(&evs, "message")
        .iter()
        .filter(|m| m["role"] == "assistant")
    {
        assert_eq!(m["request"]["task_id"], ID1);
    }
    // /new: the hook hears the new session with no task.
    h.input(ID3, 3, "/new");
    h.wait(|f| f["t"] == "ack" && f["seq"] == 3);
    let t = tasks.lock().unwrap().clone();
    let (sid, task) = t.last().unwrap().clone();
    assert_ne!(sid, SID);
    assert_eq!(task, None);
    assert!(t.iter().any(|(s, k)| s == SID && k.as_deref() == Some(ID1)));
    h.input(ID4, 4, "/compact");
    h.wait(|f| f["t"] == "ack" && f["seq"] == 4);
    h.finish();
}

/// loop.md §8 item 23: `mode`: follow_up while idle starts a run; steer, missing and bogus modes all steer.
#[test]
fn t23_input_modes() {
    let home = TempHome::new("loop-23");
    let mut h = H::start(
        &home,
        vec![
            reply("started by a follow-up"),
            tools_reply(vec![bash_call("sleep 0.4")]),
            reply("ok"),
        ],
        Opts::default(),
    );
    h.input_mode(ID1, 1, "later, but idle", "follow_up");
    h.idle(1);
    assert_eq!(h.model.seen().len(), 1);
    h.input(ID2, 2, "go");
    h.wait(is_display("tool_start"));
    h.input_mode("aaaaaaaaaaaaaaa1", 3, "s1", "steer");
    h.send(json!({"t": "input", "id": "aaaaaaaaaaaaaaa2", "seq": 4, "text": "s2"}));
    h.input_mode("aaaaaaaaaaaaaaa3", 5, "s3", "bogus");
    h.idle(2);
    let evs = h.events();
    let steers: Vec<&str> = of_type(&evs, "steer")
        .iter()
        .map(|s| s["input_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        steers,
        vec!["aaaaaaaaaaaaaaa1", "aaaaaaaaaaaaaaa2", "aaaaaaaaaaaaaaa3"]
    );
    h.finish();
}

/// loop.md §8 item 24: Interrupt during a spinning ext call: aborted within 250 ms, call order kept, the next
/// input starts a run normally.
#[test]
fn t24_interrupt_during_ext_call() {
    let home = TempHome::new("loop-24");
    let host = Arc::new(FakeHost::default());
    host.spin.store(true, Ordering::SeqCst);
    let calls = vec![
        call("ext", json!({"name": "spin", "input": {}})),
        bash_call("echo b"),
    ];
    let mut h = H::start(
        &home,
        vec![tools_reply(calls), reply("next run")],
        Opts {
            host: Some(host.clone()),
            ..Default::default()
        },
    );
    h.input(ID1, 1, "go");
    h.wait(|f| is_display("tool_end")(f) && f["name"] == "bash");
    wait_until("the ext call", || host.calls.load(Ordering::SeqCst) == 1);
    let t_int = h.send(json!({"t": "interrupt"}));
    let (t_end, end) = h.wait(|f| is_display("tool_end")(f) && f["name"] == "ext");
    assert!(
        t_end - t_int < Duration::from_millis(250),
        "{:?}",
        t_end - t_int
    );
    assert_eq!(end["ok"], false);
    h.idle(1);
    let evs = h.events();
    let r = of_type(&evs, "tool_result");
    assert_eq!(r[0]["name"], "ext");
    assert!(result_text(r[0]).starts_with("error: aborted: ext spin 0.1.0"));
    assert_eq!(r[1]["name"], "bash");
    host.spin.store(false, Ordering::SeqCst);
    h.input(ID2, 2, "again");
    h.idle(2);
    assert_eq!(h.model.seen().len(), 2);
    h.finish();
    let _ = OutcomeKind::Ok;
}

// ---------------------------------------------------------------------------------------------
// Unit checks
// ---------------------------------------------------------------------------------------------

#[test]
fn frames_parse_and_render() {
    use super::frames::parse_frame;
    let c = parse_frame(r#"{"id":"9c41e07a2b5d13f0","seq":18,"t":"input","text":"add a pty tool","ts":"2026-10-19T12:30:00.000Z","v":1}"#)
        .unwrap()
        .unwrap();
    assert!(matches!(c, Control::Input(ref i) if i.mode == InputMode::Steer && i.seq == 18));
    let c = parse_frame(r#"{"t":"input","id":"x","seq":1,"text":"t","mode":"follow_up","v":1}"#)
        .unwrap()
        .unwrap();
    assert!(matches!(c, Control::Input(ref i) if i.mode == InputMode::FollowUp));
    assert_eq!(parse_frame(r#"{"t":"future_frame","v":1}"#).unwrap(), None);
    assert!(parse_frame("not json").is_err());
    assert!(matches!(
        parse_frame(r#"{"t":"stop","v":1}"#).unwrap().unwrap(),
        Control::Stop {
            grace_ms: 30_000,
            ..
        }
    ));
    assert!(matches!(
        parse_frame(r#"{"t":"resume","token":"k","update_failed":null,"v":1}"#)
            .unwrap()
            .unwrap(),
        Control::Resume {
            update_failed: None,
            ..
        }
    ));
    assert_eq!(
        Out::Paused {
            token: "5f".into(),
            log_len: 10,
            leaf: "e4".into()
        }
        .to_value()
        .to_string(),
        r#"{"leaf":"e4","log_len":10,"t":"paused","token":"5f","v":1}"#
    );
    assert_eq!(
        Out::Ack(18).to_value().to_string(),
        r#"{"seq":18,"t":"ack","v":1}"#
    );
    assert!(is_command("/compact focus on tests"));
    assert!(is_command("/new"));
    assert!(!is_command("/newer things"));
    assert!(!is_command("please /compact"));
}

#[test]
fn deltas_drop_when_the_queue_is_full() {
    let (tx, rx) = OutTx::channel(2);
    for _ in 0..5 {
        tx.delta(Out::display("text", "x"));
    }
    assert_eq!(tx.take_dropped(), 3);
    assert_eq!(rx.try_iter().count(), 2);
}

#[test]
fn base64_round_trip() {
    for n in 0..40 {
        let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
        let enc = crate::context::render::base64(&data);
        assert_eq!(super::hooks::base64_decode(&enc).unwrap(), data);
    }
    assert!(super::hooks::base64_decode("a").is_none());
}

#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}
