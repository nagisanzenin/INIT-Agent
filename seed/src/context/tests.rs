//! Context tests (log-context.md §8: T-CTX-1…7, T-CMP-1…5, T-TOK-1, and the context-level T-LOG-5,
//! T-LOG-6, T-LOG-10).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Value, json};

use super::compact::{self, Summarizer, SummaryReply};
use super::engine::*;
use super::hash;
use super::head::{Head, build_head};
use super::model::*;
use super::project::{self, Msg};
use super::render;
use crate::log::clock::{id_hex, parse_id};
use crate::log::event::*;
use crate::log::schema_check::assert_file_valid;
use crate::log::session::OpenReport;
use crate::log::testutil::*;
use crate::log::writer::read_all;
use crate::log::{INTERRUPTED_TEXT, icj};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/context/testdata")
}

fn tools() -> Vec<Value> {
    serde_json::from_str(include_str!("testdata/tools.json")).unwrap()
}

fn test_head() -> Head {
    let d = testdata().join("head");
    build_head(&d, &d.join("CONSTITUTION.md"), tools()).unwrap()
}

struct Sig;
impl SignalSource for Sig {
    fn signal(&mut self, t: u64, p: u64, _th: &Thresholds) -> SignalEv {
        SignalEv {
            ctx_tokens: t,
            ctx_permille: p,
            text: format!(
                "[init:signals] L0 · context {}% ({}k of 200k) · up 1m",
                p / 10,
                t / 1000
            ),
            extra: obj(json!({"uptime_s": 60})),
        }
    }
}

type Seen = Rc<RefCell<Vec<(u32, Vec<Msg>, Vec<u8>)>>>;

/// Scripted summarizer: pops replies in order and records each fork request.
struct Script {
    replies: VecDeque<Result<SummaryReply, String>>,
    seen: Seen,
    /// When the queue is empty, answer with this text (if any).
    fallback: Option<String>,
}

impl Summarizer for Script {
    fn summarize(&mut self, req: &compact::ForkRequest) -> Result<SummaryReply, String> {
        self.seen
            .borrow_mut()
            .push((req.attempt, req.messages.to_vec(), req.body.to_vec()));
        match self.replies.pop_front() {
            Some(r) => r,
            None => match &self.fallback {
                Some(t) => Ok(reply(t)),
                None => Err("script exhausted".into()),
            },
        }
    }
}

fn reply(text: &str) -> SummaryReply {
    SummaryReply {
        text: text.into(),
        thinking: Some("summarizing".into()),
        tool_use: false,
        model: "mock-frontier-1".into(),
        usage: None,
    }
}

fn script(
    replies: Vec<Result<SummaryReply, String>>,
    fallback: Option<&str>,
) -> (Box<Script>, Seen) {
    let seen: Seen = Rc::new(RefCell::new(Vec::new()));
    (
        Box::new(Script {
            replies: replies.into(),
            seen: seen.clone(),
            fallback: fallback.map(str::to_string),
        }),
        seen,
    )
}

fn no_summarizer() -> Box<Script> {
    script(vec![], None).0
}

fn small_target() -> Target {
    Target {
        provider: "mock".into(),
        api: Api::ChatCompletions,
        model: "mock-frontier-1".into(),
        effort: "high".into(),
        context_window: 64_000,
        max_tokens: 4_096,
        images: false,
        profile: CacheProfile::deepseek("mock", 64_000, 4_096),
    }
}

fn new_ctx(home: &TempHome, target: Target, summ: Box<dyn Summarizer>) -> (Context, OpenReport) {
    let (log, rep) = open(&home.path, SID);
    let mut ctx = Context::new(log, test_head(), target, summ, &rep, SHA).unwrap();
    ctx.append_boot(
        boot_ev("[init:boot] Cold start: no running predecessor handed over to you. New session."),
        &rep,
    )
    .unwrap();
    (ctx, rep)
}

fn reopen_ctx(home: &TempHome, target: Target, summ: Box<dyn Summarizer>) -> (Context, OpenReport) {
    let (log, rep) = open(&home.path, SID);
    let ctx = Context::new(log, test_head(), target, summ, &rep, SHA).unwrap();
    (ctx, rep)
}

fn owner_in(rng: &mut Rng, text: &str) -> BoundaryIn {
    BoundaryIn {
        owner: vec![OwnerIn {
            input_id: rng.hex16(),
            text: text.into(),
            channel: None,
        }],
        ..Default::default()
    }
}

fn res(call_id: &str, name: &str, text: &str) -> ResultIn {
    ResultIn {
        call_id: call_id.into(),
        name: name.into(),
        output: vec![Block::text(text)],
        is_error: false,
        spill_path: None,
        exit_code: Some(0),
        signal: None,
        wall_ms: Some(5),
    }
}

fn err_res(call_id: &str, text: &str) -> ResultIn {
    ResultIn {
        is_error: true,
        exit_code: Some(1),
        ..res(call_id, "bash", text)
    }
}

fn head_bytes(h: &Head) -> u64 {
    h.core.len() as u64
        + h.self_.len() as u64
        + h.tools
            .iter()
            .map(|t| icj::to_vec(t).unwrap().len() as u64)
            .sum::<u64>()
}

/// What the mock provider "sees": all content bytes of a request.
fn req_bytes(ctx: &Context, req: &RequestOut) -> u64 {
    project::content_bytes(&req.messages, &ctx.target.profile).0 + head_bytes(&ctx.head)
}

/// The mock model: tool calls with raw (pretty) argument strings, usage from a known k.
fn respond(
    ctx: &mut Context,
    req: &RequestOut,
    calls: &[(&str, &str, Value)],
    text: &str,
    k_true: u64,
) -> u64 {
    let thinking = format!("reasoning about step {}", ctx.log.index.len());
    let mut content = vec![Block::Thinking {
        provider: ctx.target.provider.clone(),
        model: ctx.target.model.clone(),
        text: thinking.clone(),
        native: None,
    }];
    if !text.is_empty() {
        content.push(Block::text(text));
    }
    let mut started = Vec::new();
    let mut out_bytes = (thinking.len() + text.len()) as u64;
    for (cid, name, input) in calls {
        let raw = serde_json::to_string_pretty(input).unwrap();
        out_bytes += raw.len() as u64 + name.len() as u64;
        content.push(Block::ToolUse {
            call_id: (*cid).into(),
            name: (*name).into(),
            input: input.as_object().unwrap().clone(),
            raw_arguments: Some(raw),
        });
        started.push(CallStart {
            call_id: (*cid).into(),
            name: (*name).into(),
            idempotency_key: None,
        });
    }
    // floor(k·bytes) + image tokens: what a provider with exactly k tokens per byte would report.
    let images = project::content_bytes(&req.messages, &ctx.target.profile).1;
    let prompt = (k_true as u128 * req_bytes(ctx, req) as u128 / 1_000_000) as u64 + images;
    let read = prompt * 9 / 10;
    let usage = UsageEv {
        input_tokens: prompt - read,
        cache_read_tokens: read,
        cache_write_tokens: 0,
        output_tokens: crate::log::tokens_for_bytes(k_true, out_bytes),
        reasoning_tokens: None,
        native: None,
    };
    ctx.on_response(
        AssistantIn {
            content,
            provider: ctx.target.provider.clone(),
            api: "chat_completions".into(),
            model: ctx.target.model.clone(),
            response_id: Some(format!("resp-{}", ctx.log.index.len())),
            stop_reason: if calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            }
            .into(),
            stop_details: None,
            usage,
            wall_ms: Some(900),
            ttft_ms: Some(200),
        },
        &started,
    )
    .unwrap()
}

fn messages(body: &[u8]) -> Vec<Value> {
    let v: Value = serde_json::from_slice(body).unwrap();
    v["messages"].as_array().unwrap().clone()
}

fn bash(cmd: &str) -> Value {
    json!({"command": cmd})
}

/// The estimate-only projection (M5 lane) equals the full one wherever the estimate reads it:
/// every message's tokens, and the whole tail after the last assistant message.
fn check_light(ctx: &Context) {
    let full = ctx.project();
    let light = project::project_for_estimate(&ctx.view, &ctx.target.profile, &ctx.calib);
    assert_eq!(light.len(), full.len());
    for (a, b) in light.iter().zip(&full) {
        assert_eq!(a.tokens(), b.tokens(), "{a:?} vs {b:?}");
    }
    let tail = full
        .iter()
        .rposition(Msg::is_assistant)
        .map(|i| i + 1)
        .unwrap_or(0);
    assert_eq!(&light[tail..], &full[tail..]);
    assert_eq!(ctx.estimate(&light), ctx.estimate(&full));
}

/// One full step: request, model response with calls, and the results boundary.
fn step(
    ctx: &mut Context,
    calls: &[(&str, &str, Value)],
    results: Vec<ResultIn>,
    k: u64,
) -> RequestOut {
    check_light(ctx);
    let req = ctx.next_request(&mut Sig, ctx.target.max_tokens).unwrap();
    check_light(ctx);
    respond(ctx, &req, calls, "", k);
    ctx.boundary_append(BoundaryIn {
        results,
        ..Default::default()
    })
    .unwrap();
    req
}

// ---------------------------------------------------------------------------------------------

/// The DeepSeek request layout (§3.10.1; provider.md §3.8.1) and signal placement (§3.13).
#[test]
fn deepseek_layout_and_signal_placement() {
    let home = TempHome::new("layout");
    let mut rng = Rng(1);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "Fix the failing test in work/demo."))
        .unwrap();
    assert!(ctx.has_pending_work());
    let r1 = ctx.next_request(&mut Sig, 8192).unwrap();
    assert!(icj::is_canonical(&r1.body));
    let v: Value = serde_json::from_slice(&r1.body).unwrap();
    assert_eq!(v["model"], "mock-frontier-1");
    assert_eq!(v["reasoning_effort"], "high");
    assert_eq!(v["thinking"], json!({"type": "enabled"}));
    assert_eq!(v["stream"], true);
    assert_eq!(v["max_tokens"], 8192);
    let names: Vec<&str> = v["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ask_owner", "bash", "delegate", "edit", "ext", "read", "write"]);
    assert_eq!(v["tools"][0]["type"], "function");
    let m = messages(&r1.body);
    assert_eq!(m.len(), 3);
    assert_eq!(m[0]["role"], "system");
    assert_eq!(
        m[0]["content"],
        format!("{}\n{}", ctx.head.core, ctx.head.self_)
    );
    assert!(m[1]["content"].as_str().unwrap().starts_with("[init:boot]"));
    assert_eq!(
        m[2],
        json!({"content": "Fix the failing test in work/demo.", "role": "user"})
    );
    assert_eq!(r1.task_id.as_deref().map(str::len), Some(16));
    // A tool round.
    respond(
        &mut ctx,
        &r1,
        &[
            ("call_00_a", "bash", bash("cargo test")),
            ("call_01_b", "read", json!({"path": "src/lib.rs"})),
        ],
        "",
        250_000,
    );
    ctx.boundary_append(BoundaryIn {
        results: vec![
            res("call_00_a", "bash", "FAILED\n[exit_code=101]"),
            res("call_01_b", "read", "     1\tfn x() {}"),
        ],
        ..Default::default()
    })
    .unwrap();
    assert!(ctx.has_pending_work());
    let r2 = ctx.next_request(&mut Sig, 8192).unwrap();
    let m = messages(&r2.body);
    assert_eq!(m.len(), 7);
    let a = &m[3];
    assert_eq!(a["role"], "assistant");
    assert_eq!(a["content"], "");
    assert!(
        a["reasoning_content"]
            .as_str()
            .unwrap()
            .starts_with("reasoning about")
    );
    assert_eq!(a["tool_calls"][0]["id"], "call_00_a");
    assert_eq!(
        a["tool_calls"][0]["function"]["arguments"],
        serde_json::to_string_pretty(&bash("cargo test")).unwrap(),
        "raw arguments replayed verbatim"
    );
    assert_eq!(
        m[4],
        json!({"content": "FAILED\n[exit_code=101]", "role": "tool", "tool_call_id": "call_00_a"})
    );
    assert_eq!(m[5]["tool_call_id"], "call_01_b");
    assert_eq!(m[6]["role"], "user", "signal note after the tool messages");
    assert!(
        m[6]["content"]
            .as_str()
            .unwrap()
            .starts_with("[init:signals]")
    );
    // Request 1's messages are a prefix of request 2's.
    assert_eq!(&messages(&r2.body)[..3], &messages(&r1.body)[..]);
    // Final answer: no signal follows it; the owner's next message starts a new group with no signal.
    respond(&mut ctx, &r2, &[], "Fixed.", 250_000);
    assert!(!ctx.has_pending_work());
    ctx.boundary_append(owner_in(&mut rng, "thanks")).unwrap();
    let r3 = ctx.next_request(&mut Sig, 8192).unwrap();
    let m = messages(&r3.body);
    assert_eq!(
        m.last().unwrap(),
        &json!({"content": "thanks", "role": "user"})
    );
    assert_eq!(m[7]["content"], "Fixed.");
    let signals = read_all(&home.session_file(SID))
        .unwrap()
        .iter()
        .filter(|e| e.kind() == Kind::Signal)
        .count();
    assert_eq!(signals, 1);
    // The assistant message records the request it answered.
    let evs = read_all(&home.session_file(SID)).unwrap();
    let a = evs.iter().find_map(|e| match &e.body {
        EventBody::Message(m) if m.is_assistant() => Some((**m).clone()),
        _ => None,
    });
    let req = a.unwrap().request.unwrap();
    assert_eq!(req.head_hash.as_deref(), Some(ctx.head_hash()));
    assert!(req.k_ppm.is_some() && req.est_tokens.is_some());
    assert_eq!(req.task_id.unwrap().as_str().unwrap(), r1.task_id.unwrap());
    assert_file_valid(&home.session_file(SID));
}

/// T-CTX-2: golden head bytes and `prefix_hash` (Chat Completions; deepseek-flash and mock).
/// `INIT_WP12_BLESS=1` rewrites the golden files. The SHA-256 is cross-checked with `shasum`.
#[test]
fn t_ctx_2_golden_head() {
    let golden = testdata().join("golden");
    let bless = std::env::var("INIT_WP12_BLESS").as_deref() == Ok("1");
    let head = test_head();
    assert_eq!(
        head.warnings.len(),
        1,
        "the skill without front matter is reported"
    );
    assert!(head.self_.ends_with("# Skills\n\n- ext-authoring: Build a WebAssembly extension from the template. (body/skills/ext-authoring/SKILL.md)\n"));
    for target in [Target::deepseek_flash(), Target::mock()] {
        let fields = render::head_fields(&head, &target).unwrap();
        let sys = icj::to_vec(&render::system_message(&head)).unwrap();
        let body = render::assemble(&fields, target.max_tokens, &[&sys]).unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let obj_bytes = icj::to_vec(&hash::head_object(&target.provider, target.api, &v)).unwrap();
        let h = hash::prefix_hash(&target.provider, target.api, &v).unwrap();
        let name = &target.model;
        let files = [
            (
                golden.join(format!("head-request-{name}.json")),
                body.clone(),
            ),
            (
                golden.join(format!("head-object-{name}.json")),
                obj_bytes.clone(),
            ),
            (
                golden.join(format!("prefix-hash-{name}.txt")),
                format!("{h}\n").into_bytes(),
            ),
        ];
        for (p, bytes) in &files {
            if bless {
                std::fs::create_dir_all(&golden).unwrap();
                std::fs::write(p, bytes).unwrap();
            }
            assert_eq!(
                &std::fs::read(p).unwrap_or_default(),
                bytes,
                "golden {} differs (bless with INIT_WP12_BLESS=1)",
                p.display()
            );
        }
        if let Ok(out) = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&files[1].0)
            .output()
        {
            let hex = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            assert_eq!(
                format!("sha256:{hex}"),
                h,
                "in-house SHA-256 agrees with shasum"
            );
        }
        // The context computes the same hash from its own head.
        let home = TempHome::new("golden");
        let (ctx, _) = new_ctx(&home, target.clone(), no_summarizer());
        assert_eq!(ctx.head_hash(), h);
    }
}

/// Byte-identical head across runs (two builds) and across a restart.
#[test]
fn head_deterministic_across_builds_and_restarts() {
    assert_eq!(test_head(), test_head());
    let home = TempHome::new("headdet");
    let mut rng = Rng(2);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "hi")).unwrap();
    let r1 = ctx.next_request(&mut Sig, 8192).unwrap();
    let h1 = ctx.head_hash().to_string();
    drop(ctx);
    let (mut ctx2, _) = reopen_ctx(&home, Target::mock(), no_summarizer());
    assert_eq!(ctx2.head_hash(), h1);
    let r2 = ctx2.next_request(&mut Sig, 8192).unwrap();
    assert_eq!(r2.body, r1.body, "same log, same bytes after a restart");
}

/// Child for T-CTX-1: renders the next request of the session in its own process.
#[test]
#[ignore]
fn child_render() {
    let Ok(home) = std::env::var("INIT_WP12_CHILD_HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let (log, rep) = open(&home, SID);
    let mut ctx =
        Context::new(log, test_head(), Target::mock(), no_summarizer(), &rep, SHA).unwrap();
    let r = ctx.next_request(&mut Sig, 8192).unwrap();
    std::fs::write(home.join("child-request.json"), &r.body).unwrap();
}

/// T-CTX-1: the same log projected in two processes (different `RandomState` seeds) gives
/// byte-identical request bodies.
#[test]
fn t_ctx_1_two_processes_identical() {
    let home = TempHome::new("tctx1");
    let mut rng = Rng(11);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "go")).unwrap();
    for i in 0..5 {
        let c1 = format!("call_{i}_a");
        let c2 = format!("call_{i}_b");
        step(
            &mut ctx,
            &[
                (&c1, "bash", bash("ls -la")),
                (&c2, "read", json!({"path": "a.rs", "offset": 3})),
            ],
            vec![
                res(&c1, "bash", &rng.text(300)),
                res(&c2, "read", &rng.text(200)),
            ],
            260_000,
        );
        ctx.boundary_append(BoundaryIn {
            steers: vec![OwnerIn {
                input_id: rng.hex16(),
                text: "also check b".into(),
                channel: None,
            }],
            ..Default::default()
        })
        .unwrap();
    }
    let mine = ctx.next_request(&mut Sig, 8192).unwrap();
    drop(ctx);
    let st = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "context::tests::child_render",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INIT_WP12_CHILD_HOME", &home.path)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
    let theirs = std::fs::read(home.path.join("child-request.json")).unwrap();
    assert_eq!(theirs, mine.body);
}

/// T-CTX-4: within an epoch, request N's messages are a byte prefix of request N+1's, and the cached
/// concatenation equals the full serialization. 200 random steps.
#[test]
fn t_ctx_4_append_only_prefix_200_steps() {
    let home = TempHome::new("tctx4");
    let mut rng = Rng(0xc0ffee);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "start")).unwrap();
    let mut prev: Option<Vec<u8>> = None;
    for i in 0..200 {
        let req = ctx.next_request(&mut Sig, 8192).unwrap();
        assert!(req.rebuilt.is_none());
        let full = ctx.render_uncached(&req.messages, 8192).unwrap();
        assert_eq!(full, req.body, "step {i}: cached != full serialization");
        let v: Value = serde_json::from_slice(&req.body).unwrap();
        let msgs = icj::to_vec(&v["messages"]).unwrap();
        if let Some(p) = &prev {
            assert!(
                msgs.starts_with(&p[..p.len() - 1]),
                "step {i}: not an append-only prefix"
            );
        }
        prev = Some(msgs);
        let n = rng.below(4);
        let calls: Vec<(String, String, Value)> = (0..n)
            .map(|j| (format!("c{i}_{j}"), "bash".to_string(), bash(&rng.text(20))))
            .collect();
        let call_refs: Vec<(&str, &str, Value)> = calls
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), c.clone()))
            .collect();
        respond(
            &mut ctx,
            &req,
            &call_refs,
            if n == 0 { "done" } else { "" },
            250_000,
        );
        let mut b = BoundaryIn {
            results: calls
                .iter()
                .map(|(c, _, _)| {
                    let n = rng_len(&mut rng);
                    res(c, "bash", &rng.text(n))
                })
                .collect(),
            ..Default::default()
        };
        if n == 0 || rng.below(5) == 0 {
            b.steers.push(OwnerIn {
                input_id: rng.hex16(),
                text: rng.text(30),
                channel: None,
            });
        }
        if rng.below(7) == 0 {
            b.notices.push(NoticeIn {
                id: rng.hex16(),
                kind: "lkg".into(),
                data: json!({}),
                text: "[init:notice] You are LKG.".into(),
            });
        }
        ctx.boundary_append(b).unwrap();
    }
    assert_file_valid(&home.session_file(SID));
}

fn rng_len(rng: &mut Rng) -> usize {
    20 + rng.below(200) as usize
}

/// T-CTX-5 (+ T-LOG-8 in context): a restart mid-round. The first post-restart request is the previous
/// request's messages + the assistant reply + synthetic results + the boot block; the head hash is equal.
#[test]
fn t_ctx_5_restart_mid_session() {
    let home = TempHome::new("tctx5");
    let mut rng = Rng(5);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "run it")).unwrap();
    step(
        &mut ctx,
        &[("c1", "bash", bash("make"))],
        vec![res("c1", "bash", "ok")],
        250_000,
    );
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    respond(
        &mut ctx,
        &req,
        &[
            ("c2", "bash", bash("make test")),
            ("c3", "bash", bash("make lint")),
        ],
        "",
        250_000,
    );
    let hash_before = ctx.head_hash().to_string();
    drop(ctx); // "kill": the tools of c2/c3 started (markers written) but no result was recorded
    let (mut ctx, rep) = reopen_ctx(&home, Target::mock(), no_summarizer());
    assert_eq!(rep.interrupted, 2);
    let mut boot = boot_ev("[init:boot] You restarted after abc1234 crashed (signal SIGKILL).");
    boot.reason = "crash".into();
    ctx.append_boot(boot, &rep).unwrap();
    assert!(ctx.has_pending_work());
    assert_eq!(ctx.head_hash(), hash_before);
    let after = ctx.next_request(&mut Sig, 8192).unwrap();
    let before_msgs = messages(&req.body);
    let m = messages(&after.body);
    assert_eq!(&m[..before_msgs.len()], &before_msgs[..]);
    let rest = &m[before_msgs.len()..];
    assert_eq!(rest.len(), 4, "{rest:#?}");
    assert_eq!(rest[0]["role"], "assistant");
    assert_eq!(
        rest[1],
        json!({"content": INTERRUPTED_TEXT, "role": "tool", "tool_call_id": "c2"})
    );
    assert_eq!(rest[2]["tool_call_id"], "c3");
    assert!(
        rest[3]["content"]
            .as_str()
            .unwrap()
            .starts_with("[init:boot] You restarted")
    );
    let evs = read_all(&home.session_file(SID)).unwrap();
    let boot = evs.iter().rev().find_map(|e| match &e.body {
        EventBody::Boot(b) => Some(b.clone()),
        _ => None,
    });
    let boot = boot.unwrap();
    assert_eq!(boot.prefix_hash, hash_before);
    assert_eq!(boot.extra["interrupted_calls"], 2);
    assert_file_valid(&home.session_file(SID));
}

/// T-CTX-6: a T1 reload leaves the head hash unchanged; a T0 reload with a changed `SELF.md` changes it.
#[test]
fn t_ctx_6_reload_prefix() {
    let home = TempHome::new("tctx6");
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    let h0 = ctx.head_hash().to_string();
    let out = ctx
        .boundary_append(BoundaryIn {
            reload: vec![ReloadIn {
                tier: "T1".into(),
                ok: true,
                components: vec!["ext/pty".into()],
                head: None,
                error: None,
                text: "[ext reload] generation 2".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(out.reload_prefix_changed, vec![false]);
    assert_eq!(ctx.head_hash(), h0);
    // T0 with an identical head: unchanged. With an edited SELF.md: changed.
    let mut h = test_head();
    ctx.boundary_append(BoundaryIn {
        reload: vec![ReloadIn {
            tier: "T0".into(),
            ok: true,
            components: vec![],
            head: Some(h.clone()),
            error: None,
            text: "same".into(),
        }],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(ctx.head_hash(), h0);
    h.self_
        .push_str("- new-skill: x (body/skills/new/SKILL.md)\n");
    let out = ctx
        .boundary_append(BoundaryIn {
            reload: vec![ReloadIn {
                tier: "T0".into(),
                ok: true,
                components: vec!["SELF.md".into()],
                head: Some(h),
                error: None,
                text: "SELF.md".into(),
            }],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(out.reload_prefix_changed, vec![true]);
    assert_ne!(ctx.head_hash(), h0);
    let evs = read_all(&home.session_file(SID)).unwrap();
    let reloads: Vec<ReloadEv> = evs
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Reload(r) => Some(r.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reloads.len(), 3);
    assert_eq!(reloads[0].text, "[init:reload] [ext reload] generation 2");
    assert!(!reloads[0].prefix_changed && reloads[2].prefix_changed);
    assert_eq!(reloads[2].head_hash_before, h0);
    assert_file_valid(&home.session_file(SID));
}

/// Builds a session that crosses the small target's trigger with maskable outputs.
fn mask_session(home: &TempHome, summ: Box<dyn Summarizer>) -> (Context, u64) {
    let mut rng = Rng(77);
    let (mut ctx, _) = new_ctx(home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "investigate"))
        .unwrap();
    // A big write input (masked as input), a spilled bash, an image, small and big errors.
    let big = "w".repeat(6_000);
    step(
        &mut ctx,
        &[(
            "w1",
            "write",
            json!({"path": "work/big.txt", "content": big}),
        )],
        vec![res(
            "w1",
            "write",
            "Wrote 6,000 bytes (1 lines) to work/big.txt (new file).",
        )],
        250_000,
    );
    let spilled = ResultIn {
        spill_path: Some("state/spill/s/b1.txt".into()),
        ..res("b1", "bash", &"o".repeat(8_000))
    };
    step(
        &mut ctx,
        &[("b1", "bash", bash("cargo build\n  --release"))],
        vec![spilled],
        250_000,
    );
    let img = ResultIn {
        output: vec![
            Block::text("[image: image/png, 1,000 bytes]"),
            Block::Image {
                media_type: "image/png".into(),
                blob: format!("sha256:{}", "c".repeat(64)),
                path: None,
                tokens_est: 1600,
            },
        ],
        ..res("i1", "read", "")
    };
    step(
        &mut ctx,
        &[("i1", "read", json!({"path": "shot.png"}))],
        vec![img],
        250_000,
    );
    step(
        &mut ctx,
        &[("e1", "bash", bash("false"))],
        vec![err_res("e1", &"e".repeat(1_200))],
        250_000,
    );
    step(
        &mut ctx,
        &[("e2", "bash", bash("bad"))],
        vec![err_res("e2", &"E".repeat(9_000))],
        250_000,
    );
    let mut i = 0;
    let mut first_read = 0;
    loop {
        let c = format!("r{i}");
        let text: String = (1..=300)
            .map(|n| format!("{n:>6}\tline {n} of the file\n"))
            .collect();
        step(
            &mut ctx,
            &[(&c, "read", json!({"path": format!("src/f{i}.rs")}))],
            vec![res(
                &c,
                "read",
                &format!("{text}\n[Showing lines 1-300 of 900. Use offset=301 to continue.]"),
            )],
            250_000,
        );
        if i == 0 {
            first_read = ctx
                .log
                .index
                .ents()
                .iter()
                .rev()
                .find(|e| e.kind == Kind::ToolResult)
                .unwrap()
                .id;
        }
        i += 1;
        if ctx.estimate(&ctx.project()) >= ctx.th.t_trigger {
            break;
        }
        assert!(i < 100);
    }
    (ctx, first_read)
}

/// T-CTX-7: a mask epoch: golden placeholders, protected tail untouched, error results < 2k kept,
/// thinking before the epoch stripped where the profile says so.
#[test]
fn t_ctx_7_mask_epoch() {
    let home = TempHome::new("tctx7");
    let (mut ctx, first_read) = mask_session(&home, no_summarizer());
    let tail_before: Vec<u64> = ctx
        .view
        .window
        .iter()
        .rev()
        .take(4)
        .map(|l| l.id())
        .collect();
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("mask"));
    let evs = read_all(&home.session_file(SID)).unwrap();
    let mask = evs.iter().find_map(|e| match &e.body {
        EventBody::Mask(m) => Some(m.clone()),
        _ => None,
    });
    let mask = mask.unwrap();
    assert_eq!(mask.n, 1);
    let by = |part: &str, pred: &dyn Fn(&MaskEntry) -> bool| {
        mask.masked
            .iter()
            .find(|e| e.part == part && pred(e))
            .cloned()
    };
    let id_of = |cid: &str| {
        evs.iter()
            .find_map(|e| match &e.body {
                EventBody::ToolResult(r) if r.call_id == cid => Some(e.id_hex()),
                _ => None,
            })
            .unwrap()
    };
    // Golden placeholder texts.
    let b1 = by("output", &|e| e.id == id_of("b1")).expect("spilled bash masked");
    assert_eq!(
        b1.placeholder,
        format!(
            "[init:masked] bash `cargo build⏎  --release` exit=0 → ~2000 tokens, see event {} (full output: state/spill/s/b1.txt)",
            id_of("b1")
        )
    );
    let r0 = by("output", &|e| e.id == id_hex(first_read)).expect("read masked");
    assert!(
        r0.placeholder
            .starts_with("[init:masked] read src/f0.rs lines 1-300 → ~"),
        "{}",
        r0.placeholder
    );
    assert!(
        by("output", &|e| e.id == id_of("e2")).is_some(),
        "error ≥ 2k masked"
    );
    assert!(
        by("output", &|e| e.id == id_of("e1")).is_none(),
        "error < 2k kept"
    );
    let img = by("image", &|e| e.id == id_of("i1")).expect("image masked");
    assert_eq!(
        img.placeholder,
        format!(
            "[init:masked] image image/png → ~1600 tokens, see event {}",
            id_of("i1")
        )
    );
    let inp = by("input", &|e| e.call_id.as_deref() == Some("w1")).expect("big write input masked");
    let masked_content = inp.input.as_ref().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        masked_content,
        format!("[init:masked ~1500 tokens, see event {}]", inp.id)
    );
    // Protected tail untouched.
    for id in &tail_before {
        assert!(
            !mask.masked.iter().any(|e| e.id == id_hex(*id)),
            "protected tail masked"
        );
    }
    // The request carries the placeholders, with raw arguments replaced by the masked input's ICJ.
    let m = messages(&req.body);
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("[init:masked] bash `cargo build⏎  --release`"));
    let w = m.iter().find(|x| x["tool_calls"][0]["id"] == "w1").unwrap();
    assert_eq!(
        w["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap(),
        icj::to_string(&Value::Object(inp.input.clone().unwrap())).unwrap()
    );
    assert!(req.est_tokens < ctx.th.t_target);
    assert_eq!(req.epoch, id_hex(ctx.view.epoch));
    // DeepSeek keeps thinking; a stripping profile drops it before the epoch only.
    assert!(
        m.iter()
            .filter(|x| x["role"] == "assistant")
            .all(|x| x.get("reasoning_content").is_some())
    );
    let mut prof = ctx.target.profile.clone();
    prof.strip_thinking_before_epoch = true;
    let p = project::project(&ctx.view, &prof, &ctx.calib);
    let thinking = |m: &Msg| matches!(m, Msg::Assistant { blocks, .. } if blocks.iter().any(|b| matches!(b, Block::Thinking { .. })));
    assert!(!p.iter().any(thinking));
    // The next step (after the epoch) is append-only again.
    respond(&mut ctx, &req, &[("z1", "bash", bash("ls"))], "", 250_000);
    ctx.boundary_append(BoundaryIn {
        results: vec![res("z1", "bash", "a\nb")],
        ..Default::default()
    })
    .unwrap();
    let req2 = ctx.next_request(&mut Sig, 4096).unwrap();
    assert!(req2.rebuilt.is_none());
    let p1 = icj::to_vec(&Value::Array(messages(&req.body))).unwrap();
    let p2 = icj::to_vec(&Value::Array(messages(&req2.body))).unwrap();
    assert!(p2.starts_with(&p1[..p1.len() - 1]));
    let p = project::project(&ctx.view, &prof, &ctx.calib);
    assert!(p.iter().any(thinking), "thinking after the epoch is kept");
    assert_file_valid(&home.session_file(SID));
}

/// T-CMP-1: random logs with parallel tool batches: `choose_cut` never keeps a result whose
/// `tool_use` is summarized.
#[test]
fn t_cmp_1_cut_never_splits_tool_rounds() {
    use super::project::Loaded;
    let mut rng = Rng(99);
    for _ in 0..500 {
        let mut window: Vec<Loaded> = Vec::new();
        let mut owner_of: Vec<Option<usize>> = Vec::new();
        let mut id = 1u64;
        let mut push = |w: &mut Vec<Loaded>, body: EventBody, tokens: u64| {
            id += 1;
            w.push(Loaded {
                ev: Event {
                    id,
                    parent: Some(id - 1),
                    ts: "2026-10-19T20:30:15.123Z".into(),
                    body,
                },
                ppos: w.len(),
                tokens,
            });
        };
        for _ in 0..rng.below(30) + 1 {
            match rng.below(4) {
                0 => {
                    push(
                        &mut window,
                        EventBody::from(MessageEv {
                            role: "user".into(),
                            content: vec![Block::text("o")],
                            source: Some("owner".into()),
                            input_id: Some(rng.hex16()),
                            ..Default::default()
                        }),
                        rng.below(500),
                    );
                    owner_of.push(None);
                }
                1 => {
                    push(
                        &mut window,
                        EventBody::Signal(SignalEv {
                            ctx_tokens: 0,
                            ctx_permille: 0,
                            text: "s".into(),
                            extra: Default::default(),
                        }),
                        30,
                    );
                    owner_of.push(None);
                }
                _ => {
                    let k = rng.below(5);
                    let calls: Vec<Block> = (0..k)
                        .map(|j| Block::ToolUse {
                            call_id: format!("c{j}"),
                            name: "bash".into(),
                            input: Default::default(),
                            raw_arguments: None,
                        })
                        .collect();
                    let a = window.len();
                    push(
                        &mut window,
                        EventBody::from(MessageEv {
                            role: "assistant".into(),
                            content: calls,
                            ..Default::default()
                        }),
                        rng.below(800),
                    );
                    owner_of.push(None);
                    for j in 0..k {
                        push(
                            &mut window,
                            EventBody::ToolCall(ToolCallEv {
                                call_id: format!("c{j}"),
                                name: "bash".into(),
                                message_id: "x".into(),
                                idempotency_key: None,
                            }),
                            0,
                        );
                        owner_of.push(Some(a));
                    }
                    for j in 0..k {
                        let r = ToolResultEv {
                            call_id: format!("c{j}"),
                            name: "bash".into(),
                            output: vec![],
                            is_error: false,
                            spill_path: None,
                            exit_code: None,
                            signal: None,
                            wall_ms: None,
                            tokens_est: rng.below(20_000),
                            synthetic: None,
                        };
                        let t = r.tokens_est;
                        push(&mut window, EventBody::ToolResult(r), t);
                        owner_of.push(Some(a));
                    }
                }
            }
        }
        let tokens: Vec<u64> = window.iter().map(|l| l.tokens).collect();
        let keep = rng.below(30_000);
        let max_tail = keep + rng.below(60_000);
        if let Some(cut) = compact::choose_cut(&window, &tokens, keep, max_tail) {
            assert!(cut >= 1 && cut < window.len());
            assert!(compact::is_boundary(&window[cut]));
            for (i, o) in owner_of.iter().enumerate().skip(cut) {
                if let Some(a) = o {
                    assert!(
                        *a >= cut,
                        "result {i} kept but its tool_use {a} summarized (cut {cut})"
                    );
                }
            }
        }
    }
}

fn valid_summary(extra: &str) -> String {
    format!(
        "<summary>\n## Intent\n- [I1] Make the tests pass\n## Constraints and preferences\n- [C1] std only\n## Progress\n### Done\n- [P1] read the files\n### In progress\n- none\n### Blocked\n- none\n## Decisions\n- [D1] mask first (because cheap)\n## Open errors\n- none\n## Next steps\n- [N1] rerun the tests\n## Critical details\n- [K1] path src/f0.rs{extra}\nRetired:\n</summary>"
    )
}

/// Fills the small target's window with unmaskable outputs (errors under 2k tokens), so the boundary
/// must compact.
fn fill_unmaskable(ctx: &mut Context, rng: &mut Rng, tag: &str) {
    let mut i = 0;
    loop {
        let c = format!("{tag}{i}");
        step(
            ctx,
            &[(
                &c,
                "edit",
                json!({"path": format!("src/m{i}.rs"), "edits": [{"old_text": "a", "new_text": "b"}]}),
            )],
            vec![err_res(&c, &rng.text(6_000))],
            250_000,
        );
        if i % 3 == 0 {
            ctx.boundary_append(BoundaryIn {
                steers: vec![OwnerIn {
                    input_id: rng.hex16(),
                    text: format!("steer {tag}{i}"),
                    channel: None,
                }],
                ..Default::default()
            })
            .unwrap();
        }
        i += 1;
        if ctx.estimate(&ctx.project()) >= ctx.th.t_trigger {
            return;
        }
        assert!(i < 200);
    }
}

fn compactions(home: &TempHome, sid: &str) -> Vec<CompactionEv> {
    read_all(&home.session_file(sid))
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::Compaction(c) => Some(*c),
            _ => None,
        })
        .collect()
}

/// Compaction end to end: the fork reuses the request prefix, the summary is anchored and merged,
/// and the projection becomes prelude + summary + kept tail. A second compaction keeps every id.
#[test]
fn compaction_epoch_end_to_end() {
    let home = TempHome::new("compact");
    let mut rng = Rng(123);
    let (summ, seen) = script(
        vec![
            Ok(reply(&valid_summary(""))),
            Ok(reply(&valid_summary("\n- [K2] second round"))),
        ],
        None,
    );
    let (mut ctx, _) = new_ctx(&home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "Make the tests pass. Keep it std-only."))
        .unwrap();
    ctx.boundary_append(BoundaryIn {
        results: vec![],
        ..Default::default()
    })
    .unwrap();
    fill_unmaskable(&mut ctx, &mut rng, "a");
    let before = ctx.project();
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("compaction"));
    // The fork request = exactly the current request + the summary prompt.
    let (attempt, fork_msgs, fork_body) = seen.borrow()[0].clone();
    assert_eq!(attempt, 0);
    assert_eq!(&fork_msgs[..before.len()], &before[..]);
    let fm = messages(&fork_body);
    assert!(
        fm.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .starts_with("[init:compact] Your context is about to be compacted.")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fork_body).unwrap()["max_tokens"],
        SUMMARY_MAX_OUTPUT
    );
    let c = &compactions(&home, SID)[0];
    assert_eq!((c.n, c.trigger.as_str(), c.retries), (1, "auto", 0));
    assert!(
        c.text
            .starts_with("[init:summary] Context compacted (compaction 1; events up to ")
    );
    assert!(
        c.text
            .contains(&format!("The full record is state/sessions/{SID}.jsonl"))
    );
    assert!(
        c.summary
            .contains("## Owner messages (verbatim, newest last)\n")
    );
    assert!(
        c.summary
            .contains("Make the tests pass. Keep it std-only.\"")
    );
    assert!(c.summary.contains("| src/m0.rs | e | 1 |"));
    assert!(c.tokens_after_est < c.tokens_before);
    // Projection: system, prelude boot, summary, kept tail starting at a boundary.
    let m = messages(&req.body);
    assert!(m[1]["content"].as_str().unwrap().starts_with("[init:boot]"));
    assert!(
        m[2]["content"]
            .as_str()
            .unwrap()
            .starts_with("[init:summary]")
    );
    assert!(m[3]["role"] == "assistant" || m[3]["content"].as_str().is_some());
    let fk = parse_id(&c.first_kept_id).unwrap();
    assert!(ctx.view.window.first().map(|l| l.id()) == Some(fk));
    assert!(req.est_tokens < ctx.th.t_trigger);
    // Second compaction: merged, ids kept, owner messages and artifacts accumulate.
    respond(&mut ctx, &req, &[], "ok", 250_000);
    ctx.boundary_append(owner_in(&mut rng, "second task"))
        .unwrap();
    fill_unmaskable(&mut ctx, &mut rng, "b");
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("compaction"));
    let cs = compactions(&home, SID);
    let c2 = &cs[1];
    assert_eq!(c2.n, 2);
    assert!(c2.summary.contains("- [K2] second round"));
    assert!(
        c2.summary
            .contains("Make the tests pass. Keep it std-only.\""),
        "older owner messages kept"
    );
    assert!(c2.summary.contains("second task"));
    assert!(c2.owner_msgs.len() > cs[0].owner_msgs.len());
    let a1 = compact::Artifacts::from_value(&cs[0].artifacts);
    let a2 = compact::Artifacts::from_value(&c2.artifacts);
    assert!(a2.files.len() > a1.files.len());
    // The second fork showed the model the previous summary.
    let (_, fork2, _) = seen.borrow()[1].clone();
    assert!(fork2.iter().any(|m| matches!(m, Msg::User { blocks, .. } if blocks.iter().any(|b| b.as_text().is_some_and(|t| t.starts_with("[init:summary]"))))));
    assert_file_valid(&home.session_file(SID));
}

/// T-CMP-2 (engine level): a reply that drops ids gets one retry, then deterministic repair; a reply
/// with a tool call gets the format retry.
#[test]
fn t_cmp_2_retry_then_repair() {
    let home = TempHome::new("tcmp2");
    let mut rng = Rng(321);
    let dropped = valid_summary("")
        .replace("- [P1] read the files\n", "")
        .replace("- [N1] rerun the tests\n", "- [N1] rerun\n- [N1] again\n");
    let mut tool_reply = reply("calling a tool");
    tool_reply.tool_use = true;
    let (summ, seen) = script(
        vec![
            Ok(reply(&valid_summary(""))),
            Ok(tool_reply),
            Ok(reply(&dropped)),
            Ok(reply(&dropped)),
        ],
        None,
    );
    let (mut ctx, _) = new_ctx(&home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "task")).unwrap();
    fill_unmaskable(&mut ctx, &mut rng, "a");
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    respond(&mut ctx, &req, &[], "ok", 250_000);
    ctx.boundary_append(owner_in(&mut rng, "more")).unwrap();
    fill_unmaskable(&mut ctx, &mut rng, "b");
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("compaction"));
    let c = compactions(&home, SID).pop().unwrap();
    assert_eq!(c.retries, 2);
    assert_eq!(c.repaired_ids, vec!["P1".to_string(), "N2".to_string()]);
    assert!(c.summary.contains("- [P1] read the files (restored)"));
    assert!(c.summary.contains("- [N1] rerun\n- [N2] again"));
    let calls = seen.borrow();
    assert_eq!(calls.len(), 4);
    let last_user = |i: usize| match calls[i].1.last().unwrap() {
        Msg::User { blocks, .. } => blocks[0].as_text().unwrap().to_string(),
        _ => panic!(),
    };
    assert_eq!(last_user(2), compact::RETRY_FORMAT_TEXT);
    assert!(last_user(3).contains("it dropped these ids without retiring them: P1"));
    assert!(last_user(3).contains("these ids appear more than once: N1"));
    // The retry replays the bad reply as an assistant turn (with its reasoning for DeepSeek).
    assert!(
        matches!(&calls[3].1[calls[3].1.len() - 2], Msg::Assistant { blocks, .. } if blocks.iter().any(|b| matches!(b, Block::Thinking { .. })))
    );
}

/// T-CMP-3: an inflated summary is discarded: mask-only epoch and `summary_inflated`.
#[test]
fn t_cmp_3_inflated_summary_rejected() {
    let home = TempHome::new("tcmp3");
    let mut rng = Rng(5150);
    let huge = valid_summary(&format!("\n- [K2] {}", "z".repeat(240_000)));
    let (summ, _) = script(vec![Ok(reply(&huge))], None);
    let (mut ctx, _) = new_ctx(&home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "task")).unwrap();
    for i in 0..6 {
        let c = format!("m{i}");
        step(
            &mut ctx,
            &[(&c, "bash", bash("cat log"))],
            vec![res(&c, "bash", &"l".repeat(4_000))],
            250_000,
        );
    }
    fill_unmaskable(&mut ctx, &mut rng, "a");
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(
        req.rebuilt,
        Some("mask"),
        "the compaction fell back to a mask-only epoch"
    );
    let evs = read_all(&home.session_file(SID)).unwrap();
    assert!(
        evs.iter()
            .any(|e| matches!(&e.body, EventBody::Error(x) if x.code == "summary_inflated"))
    );
    assert!(!evs.iter().any(|e| e.kind() == Kind::Compaction));
    let mask = evs.iter().find_map(|e| match &e.body {
        EventBody::Mask(m) => Some(m.clone()),
        _ => None,
    });
    assert_eq!(mask.unwrap().masked.len(), 6);
}

/// T-CMP-4: an output that refills the window each step: the thrash guard fires on the third rebuild
/// within 6 requests, and the request still runs while it fits the hard window.
#[test]
fn t_cmp_4_thrash_guard() {
    let home = TempHome::new("tcmp4");
    let mut rng = Rng(4);
    let (summ, _) = script(vec![], Some(&valid_summary("")));
    let (mut ctx, _) = new_ctx(&home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "dump the huge log"))
        .unwrap();
    let mut rebuilt = Vec::new();
    for i in 0..3 {
        let c = format!("h{i}");
        let req = ctx.next_request(&mut Sig, 4096).unwrap();
        rebuilt.push(req.rebuilt);
        respond(
            &mut ctx,
            &req,
            &[(&c, "bash", bash("cat huge.log"))],
            "",
            250_000,
        );
        ctx.boundary_append(BoundaryIn {
            results: vec![res(&c, "bash", &"h".repeat(200_000))],
            ..Default::default()
        })
        .unwrap();
    }
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    rebuilt.push(req.rebuilt);
    assert_eq!(
        rebuilt,
        vec![None, Some("compaction"), Some("compaction"), None]
    );
    let evs = read_all(&home.session_file(SID)).unwrap();
    assert!(
        evs.iter()
            .any(|e| matches!(&e.body, EventBody::Error(x) if x.code == "rebuild_thrash"))
    );
    assert!(evs.iter().any(
        |e| matches!(&e.body, EventBody::Display(d) if d.text.starts_with("Context refills faster"))
    ));
    assert!(req.est_tokens < ctx.th.hard_limit);
}

/// T-CMP-5: owner messages (verbatim, truncated at 2,000 characters) and the artifact index are
/// generated by code, deterministically for a fixed log.
#[test]
fn t_cmp_5_owner_msgs_and_artifacts_deterministic() {
    let a = TempHome::new("tcmp5a");
    let mut rng = Rng(55);
    {
        let (mut ctx, _) = new_ctx(&a, small_target(), no_summarizer());
        let long = "ü".repeat(2_500);
        ctx.boundary_append(owner_in(&mut rng, &long)).unwrap();
        step(
            &mut ctx,
            &[
                ("r1", "read", json!({"path": "work/abs.rs"})),
                ("x1", "ext", json!({"name": "pty", "input": {}})),
            ],
            vec![res("r1", "read", "     1\tx"), res("x1", "ext", "ok")],
            250_000,
        );
        step(
            &mut ctx,
            &[
                (
                    "w1",
                    "write",
                    json!({"path": "work/abs.rs", "content": "y"}),
                ),
                ("x2", "ext", json!({"name": "pty", "input": {}})),
            ],
            vec![res("w1", "write", "Wrote"), res("x2", "ext", "ok")],
            250_000,
        );
        let spilled = ResultIn {
            spill_path: Some("state/spill/s/c.txt".into()),
            ..res("b1", "bash", "out")
        };
        step(
            &mut ctx,
            &[("b1", "bash", bash("make"))],
            vec![spilled],
            250_000,
        );
        ctx.boundary_append(BoundaryIn {
            notices: vec![NoticeIn {
                id: "n-gate".into(),
                kind: "gate".into(),
                data: json!({"gate_id": "g7", "sha": SHA, "stage": "G2", "pass": true}),
                text: "[init:gate] G2 passed".into(),
            }],
            steers: vec![OwnerIn {
                input_id: rng.hex16(),
                text: "a steer".into(),
                channel: None,
            }],
            ..Default::default()
        })
        .unwrap();
        fill_unmaskable(&mut ctx, &mut rng, "f");
    }
    let b = TempHome::new("tcmp5b");
    std::fs::copy(a.session_file(SID), b.session_file(SID)).unwrap();
    let mut out = Vec::new();
    for home in [&a, &b] {
        let (summ, _) = script(vec![Ok(reply(&valid_summary("")))], None);
        let (mut ctx, _) = reopen_ctx(home, small_target(), summ);
        ctx.request_compaction(Some("files".into()));
        let r = ctx.next_request(&mut Sig, 4096).unwrap();
        assert_eq!(r.rebuilt, Some("compaction"));
        out.push(compactions(home, SID).pop().unwrap());
    }
    assert_eq!(out[0].summary, out[1].summary);
    assert_eq!(out[0].artifacts, out[1].artifacts);
    assert_eq!(out[0].owner_msgs, out[1].owner_msgs);
    let s = &out[0].summary;
    assert_eq!(out[0].trigger, "manual");
    assert_eq!(out[0].focus.as_deref(), Some("files"));
    let quoted = format!("\"{}…\"", "ü".repeat(2_000));
    assert!(
        s.contains(&quoted),
        "owner message truncated to 2,000 characters"
    );
    assert!(s.contains(": \"a steer\""));
    assert!(s.contains("| work/abs.rs | r,w | 2 |"), "{s}");
    assert!(s.contains("- state/spill/s/c.txt (event "));
    assert!(s.contains("- gate G2 3f2a1c0 pass (event "));
    assert!(s.contains("- ext/pty ×2"));
}

/// T-TOK-1: with a known k, the running estimate is within 5% after 3 requests, and the
/// from-scratch estimate after an epoch within 15%.
#[test]
fn t_tok_1_estimates() {
    let home = TempHome::new("ttok1");
    let mut rng = Rng(8);
    let k_true = 310_000;
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "go")).unwrap();
    for i in 0..8 {
        let req = ctx.next_request(&mut Sig, 8192).unwrap();
        let p = crate::log::tokens_for_bytes(k_true, req_bytes(&ctx, &req));
        let err = (req.est_tokens as f64 - p as f64).abs() / p as f64;
        if i >= 3 {
            assert!(
                err <= 0.05,
                "step {i}: running estimate off by {:.1}%",
                err * 100.0
            );
        }
        let c = format!("t{i}");
        respond(&mut ctx, &req, &[(&c, "bash", bash("x"))], "", k_true);
        ctx.boundary_append(BoundaryIn {
            results: vec![res(&c, "bash", &rng.text(3_000))],
            ..Default::default()
        })
        .unwrap();
    }
    // From scratch (as after an epoch or a restart): within 15%.
    drop(ctx);
    let (mut ctx, _) = reopen_ctx(&home, Target::mock(), no_summarizer());
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    let p = crate::log::tokens_for_bytes(k_true, req_bytes(&ctx, &req));
    let err = (req.est_tokens as f64 - p as f64).abs() / p as f64;
    assert!(
        err <= 0.15,
        "from-scratch estimate off by {:.1}%",
        err * 100.0
    );
    assert!(
        ctx.calib.k_ppm > 250_000,
        "k recovered from request.k_ppm: {}",
        ctx.calib.k_ppm
    );
}

/// T-LOG-5: pause → B1 boots (and dies mid-write) → resume: `update_failed` with parent = the old
/// leaf; B1's boot is off the path; a redelivered failure is not appended twice.
#[test]
fn t_log_5_handoff_branch_back() {
    let home = TempHome::new("tlog5");
    let mut rng = Rng(9);
    let (mut b0, _) = new_ctx(&home, Target::mock(), no_summarizer());
    b0.boundary_append(owner_in(&mut rng, "work")).unwrap();
    step(
        &mut b0,
        &[("c1", "bash", bash("ls"))],
        vec![res("c1", "bash", "x")],
        250_000,
    );
    let (len, leaf) = b0.pause().unwrap();
    {
        let (mut b1, _) = new_ctx(&home, Target::mock(), no_summarizer());
        b1.log.sync().unwrap();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(home.session_file(SID))
            .unwrap();
        f.write_all(br#"{"id":"00000000000000d1","parent_id":"#)
            .unwrap();
    } // B1 "killed": its lock goes with its file handle
    let failure = json!({"to_sha": "1111111111111111111111111111111111111111", "stage": "probation", "reason": "no ready in 90 s", "seq": 41, "stderr_tail": "panic"});
    b0.resume(len, &leaf, Some(failure.clone())).unwrap();
    let evs = read_all(&home.session_file(SID)).unwrap();
    let uf = evs.iter().find(|e| e.kind() == Kind::UpdateFailed).unwrap();
    assert_eq!(uf.parent, parse_id(&leaf));
    let path: Vec<u64> = b0
        .log
        .path()
        .iter()
        .map(|&p| b0.log.index.ents()[p].id)
        .collect();
    let boots: Vec<u64> = evs
        .iter()
        .filter(|e| e.kind() == Kind::Boot)
        .map(|e| e.id)
        .collect();
    assert_eq!(boots.len(), 2);
    assert!(
        path.contains(&boots[0]) && !path.contains(&boots[1]),
        "B1's boot is off the path"
    );
    assert!(
        evs.iter()
            .any(|e| matches!(&e.body, EventBody::Error(x) if x.code == "corrupt_line"))
    );
    // The model sees the failure as a note at the next request.
    let req = b0.next_request(&mut Sig, 8192).unwrap();
    let m = messages(&req.body);
    assert!(
        m.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("[init:update-failed]")
    );
    // Redelivery of the same failure (same seq) appends nothing.
    let (len, leaf) = b0.pause().unwrap();
    b0.resume(len, &leaf, Some(failure)).unwrap();
    let n = read_all(&home.session_file(SID))
        .unwrap()
        .iter()
        .filter(|e| e.kind() == Kind::UpdateFailed)
        .count();
    assert_eq!(n, 1);
    assert_file_valid(&home.session_file(SID));
}

/// T-LOG-10: the same input, notice and `update_failed{seq}` redelivered, also across a restart:
/// exactly one event each.
#[test]
fn t_log_10_redelivery_exactly_once() {
    let home = TempHome::new("tlog10");
    let input = OwnerIn {
        input_id: "00000000000000e1".into(),
        text: "hello".into(),
        channel: None,
    };
    let notice = NoticeIn {
        id: "notice-1".into(),
        kind: "promoted".into(),
        data: json!({"sha": SHA}),
        text: "[init:notice] promoted".into(),
    };
    let uf = |id: &str| NoticeIn {
        id: id.into(),
        kind: "update_failed".into(),
        data: json!({"to_sha": SHA, "stage": "G3", "reason": "r", "seq": 7}),
        text: "[init:update-failed] G3".into(),
    };
    let cmd = OwnerIn {
        input_id: "00000000000000e2".into(),
        text: "/label 00000000000000e1 x".into(),
        channel: None,
    };
    let batch = || BoundaryIn {
        owner: vec![input.clone()],
        commands: vec![cmd.clone()],
        notices: vec![notice.clone(), uf("uf-1")],
        ..Default::default()
    };
    {
        let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
        let o1 = ctx.boundary_append(batch()).unwrap();
        assert!(o1.duplicates.is_empty());
        let o2 = ctx.boundary_append(batch()).unwrap();
        assert_eq!(o2.duplicates.len(), 4);
        let o3 = ctx
            .boundary_append(BoundaryIn {
                notices: vec![uf("uf-2")],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            o3.duplicates,
            vec!["uf-2".to_string()],
            "same seq, new notice id"
        );
    }
    let (mut ctx, _) = reopen_ctx(&home, Target::mock(), no_summarizer());
    let o = ctx.boundary_append(batch()).unwrap();
    assert_eq!(o.duplicates.len(), 4);
    let evs = read_all(&home.session_file(SID)).unwrap();
    let count = |k: Kind| evs.iter().filter(|e| e.kind() == k).count();
    assert_eq!(
        (
            count(Kind::Message),
            count(Kind::Notice),
            count(Kind::UpdateFailed),
            count(Kind::Command)
        ),
        (1, 1, 1, 1)
    );
    assert_file_valid(&home.session_file(SID));
}

/// T-LOG-6 (projection side): unknown types and unknown fields are ignored by the projection.
#[test]
fn t_log_6_projection_ignores_unknown() {
    let home = TempHome::new("tlog6ctx");
    let mut rng = Rng(66);
    let r_before;
    {
        let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
        ctx.boundary_append(owner_in(&mut rng, "hi")).unwrap();
        r_before = ctx.next_request(&mut Sig, 8192).unwrap();
        let log = &mut ctx.log;
        log.writer
            .append_fields(
                "hologram",
                obj(json!({"text": "must not be projected"})),
                None,
                &mut log.index,
            )
            .unwrap();
        log.sync().unwrap();
    }
    let (mut ctx, _) = reopen_ctx(&home, Target::mock(), no_summarizer());
    let r = ctx.next_request(&mut Sig, 8192).unwrap();
    assert_eq!(r.body, r_before.body);
}

/// Owner commands: `/label`, `/branch` (refused inside an open tool round, then a real branch),
/// `/compact` under 10k tokens, `/new`.
#[test]
fn owner_commands() {
    let home = TempHome::new("cmds");
    let mut rng = Rng(10);
    let (mut ctx, _) = new_ctx(&home, Target::mock(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "first")).unwrap();
    let owner_id = ctx.leaf();
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    respond(
        &mut ctx,
        &req,
        &[("c1", "bash", bash("ls")), ("c2", "bash", bash("pwd"))],
        "",
        250_000,
    );
    ctx.boundary_append(BoundaryIn {
        results: vec![res("c1", "bash", "a")],
        ..Default::default()
    })
    .unwrap();
    let half = ctx.leaf(); // c1's result: the round is still open (c2 missing)
    ctx.boundary_append(BoundaryIn {
        results: vec![res("c2", "bash", "b")],
        ..Default::default()
    })
    .unwrap();
    let mut cmd = |ctx: &mut Context, text: &str| {
        let o = ctx
            .boundary_append(BoundaryIn {
                commands: vec![OwnerIn {
                    input_id: rng.hex16(),
                    text: text.into(),
                    channel: None,
                }],
                ..Default::default()
            })
            .unwrap();
        o.outcomes[0].1.clone()
    };
    assert_eq!(
        cmd(&mut ctx, &format!("/label {owner_id} the start")),
        format!("labeled {owner_id}")
    );
    assert_eq!(
        ctx.log.index.label(parse_id(&owner_id).unwrap()),
        Some("the start")
    );
    assert!(
        cmd(&mut ctx, &format!("/branch {half}"))
            .starts_with("refused: the target lies inside an open tool round")
    );
    assert!(cmd(&mut ctx, "/branch 0000000000000abc").starts_with("refused: unknown event id"));
    assert_eq!(
        cmd(&mut ctx, &format!("/branch {owner_id} try another way")),
        format!("branched to {owner_id}")
    );
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    let m = messages(&req.body);
    assert_eq!(m.len(), 4, "{m:#?}");
    assert_eq!(m[2]["content"], "first");
    assert_eq!(m[3]["content"], "[init:branch] try another way");
    assert!(ctx.has_pending_work());
    assert_eq!(
        cmd(&mut ctx, "/compact keep the plan"),
        "compaction at the next boundary"
    );
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    assert!(req.rebuilt.is_none());
    let evs = read_all(&home.session_file(SID)).unwrap();
    assert!(
        evs.iter()
            .any(|e| matches!(&e.body, EventBody::Display(d) if d.text == "nothing to compact"))
    );
    assert!(
        evs.iter()
            .any(|e| matches!(&e.body, EventBody::Error(x) if x.code == "branch_in_tool_round"))
    );
    assert_eq!(cmd(&mut ctx, "/frobnicate"), "unknown command");
    let out = cmd(&mut ctx, "/new");
    assert!(out.starts_with("new session "));
    let new_sid = ctx.log.sid().to_string();
    assert_ne!(new_sid, SID);
    let req = ctx.next_request(&mut Sig, 8192).unwrap();
    assert_eq!(
        messages(&req.body).len(),
        1,
        "a new session projects only the head"
    );
    assert!(req.task_id.is_none());
    assert!(!ctx.has_pending_work());
    assert_file_valid(&home.session_file(SID));
    assert_file_valid(&home.session_file(&new_sid));
}

/// An emergency compaction (provider overflow) runs at once with trigger `overflow` and KEEP/2.
#[test]
fn emergency_compaction() {
    let home = TempHome::new("emerg");
    let mut rng = Rng(12);
    let (summ, _) = script(vec![Ok(reply(&valid_summary("")))], None);
    let (mut ctx, _) = new_ctx(&home, small_target(), summ);
    ctx.boundary_append(owner_in(&mut rng, "task")).unwrap();
    for i in 0..12 {
        let c = format!("q{i}");
        step(
            &mut ctx,
            &[(&c, "bash", bash("x"))],
            vec![err_res(&c, &rng.text(4_000))],
            250_000,
        );
    }
    assert!(ctx.estimate(&ctx.project()) < ctx.th.t_trigger);
    ctx.request_emergency_compaction();
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("compaction"));
    assert_eq!(compactions(&home, SID)[0].trigger, "overflow");
}

/// The view as the log holds it: every window event reloaded from disk, unslimmed (§3.7 check).
fn full_view(ctx: &Context) -> project::View {
    let mut v = ctx.view.clone();
    for l in &mut v.window {
        l.ev = ctx.log.load_id(l.id()).unwrap().unwrap();
    }
    v
}

/// Text bytes the window holds for tool outputs (the part that masking replaces).
fn window_output_bytes(ctx: &Context) -> u64 {
    ctx.view
        .window
        .iter()
        .map(|l| match &l.ev.body {
            EventBody::ToolResult(r) => joined_text(&r.output, "").len() as u64,
            _ => 0,
        })
        .sum()
}

/// §3.7 bounded view (FIX8, M-5): after a mask epoch the window keeps no masked content and no
/// unread metadata, and the projection, the estimate and the rendered request are byte-identical
/// to those of the full events on disk, for both thinking profiles and after a restart.
#[test]
fn bounded_view_slims_without_changing_the_request() {
    let home = TempHome::new("slim");
    let (mut ctx, first_read) = mask_session(&home, no_summarizer());
    let req = ctx.next_request(&mut Sig, 4096).unwrap();
    assert_eq!(req.rebuilt, Some("mask"));
    let mut masked_outputs = 0;
    let mut tool_calls = 0;
    for l in &ctx.view.window {
        let id = l.id();
        match &l.ev.body {
            EventBody::ToolResult(r) => {
                if ctx
                    .view
                    .masked
                    .contains_key(&(id, "output".into(), String::new()))
                {
                    assert!(r.output.is_empty(), "masked output kept in memory: {id}");
                    masked_outputs += 1;
                }
            }
            EventBody::Message(m) if m.is_assistant() => {
                assert!(m.request.is_none() && m.stop_details.is_none());
                assert!(m.response_id.is_none() && m.usage.is_none());
                assert!(m.provider.is_none() && m.api.is_none() && m.model.is_none());
                assert!(m.stop_reason.is_none() && l.ev.ts.is_empty());
                for (cid, name, input) in m.tool_uses() {
                    if name == "write" {
                        let e = ctx
                            .view
                            .masked
                            .get(&(id, "input".into(), cid.to_string()))
                            .unwrap();
                        assert_eq!(Some(input), e.input.as_ref(), "masked input kept in memory");
                    }
                }
            }
            EventBody::Message(m) if m.is_owner() => assert!(!l.ev.ts.is_empty()),
            EventBody::Signal(s) => assert!(s.extra.is_empty()),
            EventBody::Mask(m) => assert!(m.masked.is_empty(), "entries live in View::masked"),
            EventBody::ToolCall(c) => {
                assert!(c.call_id.is_empty() && c.message_id.is_empty());
                tool_calls += 1;
            }
            _ => {}
        }
    }
    assert!(masked_outputs >= 3, "{masked_outputs}");
    assert!(
        tool_calls >= 3,
        "the markers stay in the window: {tool_calls}"
    );
    assert!(
        ctx.view
            .masked
            .contains_key(&(first_read, "output".into(), String::new()))
    );
    // The slimmed view projects exactly like the full events.
    let full = full_view(&ctx);
    assert_ne!(full, ctx.view, "the full events carry more than the view");
    let mut strip = ctx.target.profile.clone();
    strip.strip_thinking_before_epoch = true;
    for prof in [ctx.target.profile.clone(), strip] {
        assert_eq!(
            project::project(&ctx.view, &prof, &ctx.calib),
            project::project(&full, &prof, &ctx.calib)
        );
    }
    let slim_msgs = ctx.project();
    let full_msgs = project::project(&full, &ctx.target.profile, &ctx.calib);
    assert_eq!(ctx.estimate(&slim_msgs), ctx.estimate(&full_msgs));
    let a = ctx.render_uncached(&slim_msgs, 4096).unwrap();
    let b = ctx.render_uncached(&full_msgs, 4096).unwrap();
    assert_eq!(a, b);
    assert_eq!(a, req.body, "the request sent at the epoch");
    // A restarted body rebuilds the same slim view from disk.
    let leaf = ctx.leaf();
    drop(ctx);
    let (mut ctx2, _) = reopen_ctx(&home, small_target(), no_summarizer());
    assert_eq!(ctx2.leaf(), leaf);
    let m2 = ctx2.project();
    assert_eq!(ctx2.render_uncached(&m2, 4096).unwrap(), a);
}

/// Heap bytes of the view, estimated from its structure (the seed forbids `unsafe`, so no counting
/// allocator): every window event's inline size, its strings and vectors by capacity, maps as
/// their entries plus one B-tree leaf node per 11 entries, and `View::masked` the same way.
/// Exact for the inline part, close for the rest; the per-step test below uses differences.
mod heap_est {
    use std::collections::BTreeMap;
    use std::mem::size_of;

    use serde_json::{Map, Value};

    use crate::context::project::{Loaded, MaskKey, View};
    use crate::log::event::{Block, EventBody, MaskEntry};

    /// Heap behind a field that may be boxed (0 when the value is inline).
    pub trait Boxed {
        fn boxed(&self) -> usize;
    }
    impl<T> Boxed for Box<T> {
        fn boxed(&self) -> usize {
            size_of::<T>()
        }
    }
    macro_rules! inline {
        ($($t:ty),*) => {$(
            impl Boxed for $t {
                fn boxed(&self) -> usize {
                    0
                }
            }
        )*};
    }
    inline!(
        crate::log::event::MessageEv,
        crate::log::event::UsageEv,
        crate::log::event::RequestMeta
    );

    /// B-tree leaf node (`alloc::collections::btree`, CAPACITY = 11): parent pointer, two `u16`
    /// and the key and value arrays.
    fn node<K, V>(n: usize) -> usize {
        n.div_ceil(11) * (16 + 11 * (size_of::<K>() + size_of::<V>()))
    }

    fn s(x: &str) -> usize {
        x.len()
    }

    fn os(x: &Option<String>) -> usize {
        x.as_deref().map(s).unwrap_or(0)
    }

    pub fn value(v: &Value) -> usize {
        match v {
            Value::String(x) => s(x),
            Value::Array(a) => a.len() * size_of::<Value>() + a.iter().map(value).sum::<usize>(),
            Value::Object(m) => map(m),
            _ => 0,
        }
    }

    pub fn map(m: &Map<String, Value>) -> usize {
        node::<String, Value>(m.len()) + m.iter().map(|(k, v)| s(k) + value(v)).sum::<usize>()
    }

    fn blocks(bs: &[Block]) -> usize {
        std::mem::size_of_val(bs)
            + bs.iter()
                .map(|b| match b {
                    Block::Text { text } => s(text),
                    Block::Image {
                        media_type,
                        blob,
                        path,
                        ..
                    } => s(media_type) + s(blob) + os(path),
                    Block::Thinking {
                        provider,
                        model,
                        text,
                        native,
                    } => s(provider) + s(model) + s(text) + native.as_ref().map(value).unwrap_or(0),
                    Block::ToolUse {
                        call_id,
                        name,
                        input,
                        raw_arguments,
                    } => s(call_id) + s(name) + map(input) + os(raw_arguments),
                    Block::Opaque { api, raw } => s(api) + value(raw),
                    Block::Other(v) => value(v),
                })
                .sum::<usize>()
    }

    pub fn loaded(l: &Loaded) -> usize {
        let body = match &l.ev.body {
            EventBody::Message(m) => {
                m.boxed()
                    + s(&m.role)
                    + blocks(&m.content)
                    + [
                        &m.source,
                        &m.input_id,
                        &m.channel,
                        &m.provider,
                        &m.api,
                        &m.model,
                    ]
                    .into_iter()
                    .map(os)
                    .sum::<usize>()
                    + os(&m.stop_reason)
                    + m.response_id.as_ref().map(value).unwrap_or(0)
                    + m.stop_details.as_ref().map(value).unwrap_or(0)
                    + m.usage
                        .as_ref()
                        .map(|u| u.boxed() + u.native.as_ref().map(value).unwrap_or(0))
                        .unwrap_or(0)
                    + m.request
                        .as_ref()
                        .map(|r| {
                            r.boxed()
                                + os(&r.head_hash)
                                + os(&r.epoch)
                                + r.task_id.as_ref().map(value).unwrap_or(0)
                        })
                        .unwrap_or(0)
            }
            EventBody::ToolCall(c) => {
                s(&c.call_id) + s(&c.name) + s(&c.message_id) + os(&c.idempotency_key)
            }
            EventBody::ToolResult(r) => {
                s(&r.call_id)
                    + s(&r.name)
                    + blocks(&r.output)
                    + os(&r.spill_path)
                    + os(&r.synthetic)
            }
            EventBody::Signal(x) => s(&x.text) + map(&x.extra),
            EventBody::Mask(m) => m.masked.len() * size_of::<MaskEntry>(),
            b => b.stored_text().map(s).unwrap_or(0),
        };
        size_of::<Loaded>() + s(&l.ev.ts) + body
    }

    fn masked(m: &BTreeMap<MaskKey, MaskEntry>) -> usize {
        node::<MaskKey, MaskEntry>(m.len())
            + m.iter()
                .map(|((_, part, call), e)| {
                    s(part)
                        + s(call)
                        + s(&e.id)
                        + s(&e.part)
                        + s(&e.placeholder)
                        + os(&e.call_id)
                        + e.input.as_ref().map(map).unwrap_or(0)
                })
                .sum::<usize>()
    }

    /// (window events, masked entries) in bytes.
    pub fn view(v: &View) -> (usize, usize) {
        (v.window.iter().map(loaded).sum(), masked(&v.masked))
    }
}

/// Window heap per masked step (M5 lane): the view's estimated heap (`heap_est`) right after each
/// mask epoch, on M-5's step shape (a 2 KB bash output and a
/// signal per step, masking only). Between compactions the window keeps every masked step's
/// events (§3.7), so this number times the steps between compactions is the sawtooth M-5 sees.
fn heap_per_masked_step() -> (i64, Vec<(u64, i64)>) {
    let home = TempHome::new("perstep");
    let mut rng = Rng(9);
    let (mut ctx, _) = new_ctx(&home, small_target(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "run the steps"))
        .unwrap();
    let out = " ".repeat(2_048);
    let mut at_epochs: Vec<(u64, i64)> = Vec::new();
    for i in 0..260u64 {
        let c = format!("s{i}");
        let req = step(
            &mut ctx,
            &[(&c, "bash", bash("printf '%2048s\\n' x"))],
            vec![res(&c, "bash", &out)],
            250_000,
        );
        if req.rebuilt.is_some() {
            assert_eq!(req.rebuilt, Some("mask"), "masking alone suffices here");
            let (w, m) = heap_est::view(&ctx.view);
            at_epochs.push((i, (w + m) as i64));
        }
    }
    assert!(at_epochs.len() >= 4, "{at_epochs:?}");
    let (s0, b0) = at_epochs[1];
    let (s1, b1) = *at_epochs.last().unwrap();
    ((b1 - b0) / (s1 - s0) as i64, at_epochs)
}

/// M-5 margin (M5 lane): what one masked step costs the window. Before the shrink (an inline
/// `MessageEv` in every event, the per-response metadata and the `tool_call` fields kept) it was
/// 3,800 B by this estimate; boxing and slimming bring it to about 2,600 B.
#[test]
fn bounded_view_heap_per_masked_step() {
    let (per_step, at) = heap_per_masked_step();
    eprintln!("window heap per masked step: {per_step} B; at mask epochs (step, bytes): {at:?}");
    assert!(per_step > 0, "{at:?}");
    assert!(
        per_step <= 3_000,
        "the window keeps {per_step} B per masked step (limit 3,000 B): {at:?}"
    );
}

/// The window holds one `Loaded` per event until the next compaction, so its inline size is paid
/// per event (about 4 per step in M-5). It was 568 B with `MessageEv` inline (`EventBody` 504 B).
#[test]
fn window_event_inline_size_is_pinned() {
    use std::mem::size_of;
    assert!(
        size_of::<EventBody>() <= 192,
        "EventBody is {} B: box the large variant",
        size_of::<EventBody>()
    );
    assert!(
        size_of::<project::Loaded>() <= 256,
        "Loaded is {} B",
        size_of::<project::Loaded>()
    );
    assert!(size_of::<MessageEv>() <= 304, "{}", size_of::<MessageEv>());
}

/// §3.7 memory bound (FIX8, M-5): over a long masked session the window's tool-output bytes stay
/// under `T_trigger` tokens' worth, however many masked steps the window spans. Before the fix the
/// window kept every masked output until the next compaction (8 KB per step here).
#[test]
fn bounded_view_memory_does_not_grow_with_masked_steps() {
    let home = TempHome::new("bounded");
    let mut rng = Rng(5);
    let (mut ctx, _) = new_ctx(&home, small_target(), no_summarizer());
    ctx.boundary_append(owner_in(&mut rng, "run the steps"))
        .unwrap();
    let out = " ".repeat(8_000);
    // Bytes of `T_trigger` tokens at k = 0.25 tokens/byte, plus one step.
    let bound = ctx.th.t_trigger * 4 + 8_000;
    let mut epochs = 0;
    let mut peak = 0;
    for i in 0..150 {
        let c = format!("p{i}");
        let req = step(
            &mut ctx,
            &[(&c, "bash", bash("printf '%8000s' x"))],
            vec![res(&c, "bash", &out)],
            250_000,
        );
        if req.rebuilt.is_some() {
            assert_eq!(req.rebuilt, Some("mask"), "masking alone suffices here");
            epochs += 1;
        }
        peak = peak.max(window_output_bytes(&ctx));
        assert!(
            window_output_bytes(&ctx) <= bound,
            "step {i}: {} output bytes held, bound {bound}",
            window_output_bytes(&ctx)
        );
    }
    let results_in_window = ctx
        .view
        .window
        .iter()
        .filter(|l| l.kind() == Kind::ToolResult)
        .count();
    assert!(epochs >= 5, "{epochs} mask epochs");
    assert!(
        results_in_window >= 140,
        "the window spans the session: {results_in_window}"
    );
    // Without the bound the window would hold every output: 150 × 8,000 bytes.
    assert!(peak * 4 < 150 * 8_000, "peak {peak}");
}

/// Production Context must render native API requests, not merely pass adapter unit tests.
#[test]
fn native_api_context_requests_and_head_hashes() {
    for (provider, model) in [("anthropic", "claude-opus-5-5"), ("openai", "gpt-6.1-sol")] {
        let home = TempHome::new(&format!("native-{provider}"));
        let info = crate::provider::models::lookup(provider, model).unwrap();
        let target = crate::provider::models::target(info, None);
        let (mut ctx, _) = new_ctx(&home, target.clone(), no_summarizer());
        ctx.boundary_append(owner_in(&mut Rng(1), "Use a tool to inspect the repo.")).unwrap();
        let req = ctx.next_request(&mut Sig, 4096).unwrap();
        let v: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(v["model"], model);
        assert!(v.get("reasoning_effort").is_none());
        assert_eq!(req.head_hash, hash::prefix_hash(provider, target.api, &v).unwrap());
        assert_eq!(req.body, ctx.render_uncached(&req.messages, 4096).unwrap());
        if target.api == Api::Messages {
            assert!(v["system"].is_array());
            assert_eq!(v["max_tokens"], 4096);
            assert!(v["messages"].is_array());
            assert!(v["tools"][0].get("input_schema").is_some());
            assert!(v.get("input").is_none());
        } else {
            assert_eq!(v["input"][0]["role"], "developer");
            assert_eq!(v["max_output_tokens"], 4096);
            assert_eq!(v["prompt_cache_key"], SID);
            assert!(v["tools"][0].get("parameters").is_some());
            assert!(v.get("messages").is_none());
        }
        // The compaction path uses the same renderer with a different output budget.
        let fork = ctx.render_uncached(&req.messages, SUMMARY_MAX_OUTPUT).unwrap();
        let v: Value = serde_json::from_slice(&fork).unwrap();
        assert_eq!(v[if target.api == Api::Messages { "max_tokens" } else { "max_output_tokens" }], SUMMARY_MAX_OUTPUT);
    }
}

#[test]
fn generic_chat_context_omits_deepseek_specific_fields() {
    let home = TempHome::new("generic-chat");
    let mut target = Target::mock();
    target.provider = "local".into();
    target.model = "custom-model".into();
    let (mut ctx, _) = new_ctx(&home, target, no_summarizer());
    ctx.boundary_append(owner_in(&mut Rng(1), "Hello")).unwrap();
    let req = ctx.next_request(&mut Sig, 256).unwrap();
    let v: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(v["model"], "custom-model");
    assert!(v.get("thinking").is_none());
    assert!(v.get("reasoning_effort").is_none());
    assert!(v["messages"].is_array());
    assert_eq!(req.head_hash, hash::prefix_hash("local", Api::ChatCompletions, &v).unwrap());
}
