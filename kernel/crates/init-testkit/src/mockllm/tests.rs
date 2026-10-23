//! Self-tests of the in-process mock: routing, cursors, expectations, faults, strict mode,
//! responders, cache simulation, recording, determinism and split-fuzz of its own streams.

use super::assemble::{self, Assembled};
use super::http::encode_response;
use super::sse::{ChunkedDecoder, SseParser, parse_all};
use super::*;
use serde_json::json;
use std::path::Path;

pub(crate) fn test_home(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp2");
    std::fs::create_dir_all(&base).unwrap();
    let d = base.canonicalize().unwrap().join(name);
    if d.exists() {
        // Exact path, owned by this test.
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tool(name: &str) -> Value {
    json!({"function": {"description": "d", "name": name, "parameters": {"properties": {}, "type": "object"}}, "type": "function"})
}

fn ds_body(messages: Value) -> Value {
    json!({"max_tokens": 1000, "messages": messages, "model": "deepseek-flash", "reasoning_effort": "high",
           "stream": true, "thinking": {"type": "enabled"}, "tools": [tool("bash"), tool("read")]})
}

fn sys_user(text: &str) -> Value {
    json!([{"content": "SYSTEM", "role": "system"}, {"content": text, "role": "user"}])
}

fn script(text: &str) -> Script {
    Script::parse(text, Path::new(".")).unwrap()
}

fn mock(mode: Mode, text: &str) -> Arc<Mock> {
    Mock::new(MockConfig::new(mode, script(text))).unwrap()
}

fn strict_mock(text: &str) -> Arc<Mock> {
    let mut c = MockConfig::new(Mode::Socket, script(text));
    c.strict_deepseek = true;
    Mock::new(c).unwrap()
}

fn events(r: &MockResponse) -> Vec<sse::SseEvent> {
    parse_all(&r.body_bytes()).unwrap()
}

fn ds(r: &MockResponse) -> Assembled {
    assert_eq!(r.status, 200, "{}", r.text());
    assemble::deepseek(&events(r)).unwrap()
}

/// The assistant message the body would replay for a DeepSeek reply (provider.md §3.8.1).
fn replay(a: &Assembled) -> Value {
    let mut m = json!({"content": a.text, "role": "assistant"});
    if !a.reasoning.is_empty() {
        m["reasoning_content"] = json!(a.reasoning);
    }
    if !a.calls.is_empty() {
        m["tool_calls"] = Value::Array(
            a.calls
                .iter()
                .map(|c| json!({"function": {"arguments": c.args, "name": c.name}, "id": c.id, "type": "function"}))
                .collect(),
        );
    }
    m
}

fn tool_results(a: &Assembled, out: &str) -> Vec<Value> {
    a.calls
        .iter()
        .map(|c| json!({"content": out, "role": "tool", "tool_call_id": c.id}))
        .collect()
}

const S1: &str = r#"{"v":1,"kind":"meta","provider":"deepseek","model":"deepseek-flash","name":"s1","chunk":5}
{"v":1,"kind":"turn","expect":{"last_role":"user","last_text_contains":"Fix","tools":["bash","read"]},"reply":{"think":"Run the tests.","calls":[{"input":{"command":"cargo test","timeout":30},"name":"bash"},{"input":{"path":"src/lib.rs"},"name":"read"}],"stop":"tool_use"}}
{"v":1,"kind":"turn","expect":{"last_role":"tool","prefix":"same"},"reply":{"text":"Done: all tests pass.","stop":"end_turn"}}
"#;

#[test]
fn scripted_conversation_renders_and_reassembles() {
    let m = strict_mock(S1);
    let mut msgs = sys_user("Fix the failing test").as_array().unwrap().clone();
    let r1 = m.respond(&MockRequest::deepseek(
        "s1",
        "turn",
        &ds_body(Value::Array(msgs.clone())),
    ));
    let a1 = ds(&r1);
    assert_eq!(a1.reasoning, "Run the tests.");
    assert_eq!(a1.stop, "tool_use");
    assert_eq!(a1.calls.len(), 2);
    assert_eq!(a1.calls[0].id, "call_00_mock0");
    assert_eq!(a1.calls[1].id, "call_01_mock0");
    assert_eq!(a1.calls[0].args, r#"{"command":"cargo test","timeout":30}"#);
    assert_eq!(a1.id.as_deref(), Some("mock-turn-0"));
    assert_eq!(a1.model.as_deref(), Some("deepseek-flash"));
    let u = a1.usage.clone().unwrap();
    assert_eq!(
        u["prompt_tokens"],
        u["prompt_cache_hit_tokens"].as_u64().unwrap()
            + u["prompt_cache_miss_tokens"].as_u64().unwrap()
    );
    msgs.push(replay(&a1));
    msgs.extend(tool_results(&a1, "ok"));
    let r2 = m.respond(&MockRequest::deepseek(
        "s1",
        "turn",
        &ds_body(Value::Array(msgs.clone())),
    ));
    let a2 = ds(&r2);
    // No `think` in the script: DeepSeek in thinking mode still reasons (render::DEFAULT_REASONING).
    assert_eq!(a2.reasoning, render::DEFAULT_REASONING);
    assert_eq!(
        (a2.text.as_str(), a2.stop.as_str()),
        ("Done: all tests pass.", "end_turn")
    );
    // The replayed history keeps reasoning_content, so a third strict request is accepted.
    msgs.push(replay(&a2));
    msgs.push(json!({"content": "thanks", "role": "user"}));
    let r3 = m.respond(&MockRequest::deepseek(
        "s1",
        "turn",
        &ds_body(Value::Array(msgs)),
    ));
    assert_eq!(r3.status, 500);
    assert_eq!(r3.json_body().unwrap()["mock_error"], "script exhausted");
    let s = m.summary();
    assert_eq!(
        s,
        Summary {
            requests: 3,
            expect_failures: 0,
            protocol_errors: 0,
            exhausted: true
        }
    );
    assert!(prefix_constant(&m.log(), "s1"));
    let log = m.log();
    assert_eq!(log[0].expect_ok, Some(true));
    assert_eq!(log[0].responder.as_deref(), Some("script"));
}

#[test]
fn cursors_are_per_session_and_purpose() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"A"}}
{"v":1,"kind":"turn","reply":{"text":"B"}}
{"v":1,"kind":"turn","purpose":"compaction","reply":{"text":"SUMMARY"}}"#,
    );
    let body = ds_body(sys_user("hi"));
    let text = |s: &str, p: &str| ds(&m.respond(&MockRequest::deepseek(s, p, &body))).text;
    assert_eq!(text("x", "turn"), "A");
    assert_eq!(text("y", "turn"), "A");
    assert_eq!(text("x", "compaction"), "SUMMARY");
    assert_eq!(text("x", "turn"), "B");
    assert_eq!(text("y", "turn"), "B");
}

#[test]
fn expect_failure_is_500_and_does_not_consume_the_turn() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","expect":{"last_role":"tool"},"reply":{"text":"ok"}}"#,
    );
    let r = m.respond(&MockRequest::deepseek(
        "s",
        "turn",
        &ds_body(sys_user("hi")),
    ));
    assert_eq!(r.status, 500);
    let err = r.json_body().unwrap()["mock_error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        err.starts_with("expect failed (line 1): last_role: want tool, got user"),
        "{err}"
    );
    // A retry meets the same expectation; a request that satisfies it gets the turn.
    assert_eq!(
        m.respond(&MockRequest::deepseek(
            "s",
            "turn",
            &ds_body(sys_user("hi"))
        ))
        .status,
        500
    );
    let mut msgs = sys_user("hi").as_array().unwrap().clone();
    msgs.push(json!({"content": "", "reasoning_content": "r", "role": "assistant",
                     "tool_calls": [{"function": {"arguments": "{}", "name": "bash"}, "id": "c", "type": "function"}]}));
    msgs.push(json!({"content": "out", "role": "tool", "tool_call_id": "c"}));
    assert_eq!(
        ds(&m.respond(&MockRequest::deepseek(
            "s",
            "turn",
            &ds_body(Value::Array(msgs))
        )))
        .text,
        "ok"
    );
    let s = m.summary();
    assert_eq!((s.expect_failures, s.exhausted), (2, false));
    assert_eq!(m.log()[2].expect_ok, Some(true));
}

#[test]
fn prefix_expectations() {
    let body = ds_body(sys_user("hi"));
    let view = wire::parse(Family::Chat, body.to_string().as_bytes()).unwrap();
    let h = sha256_hex(&view.prefix_bytes);
    let m = mock(
        Mode::Socket,
        &format!(
            r#"{{"v":1,"kind":"turn","expect":{{"prefix":"sha256:{h}"}},"reply":{{"text":"1"}}}}
{{"v":1,"kind":"turn","expect":{{"prefix":"same"}},"reply":{{"text":"2"}}}}"#
        ),
    );
    assert_eq!(
        ds(&m.respond(&MockRequest::deepseek("s", "turn", &body))).text,
        "1"
    );
    // Changing the tools changes the prefix: "same" fails.
    let mut changed = body.clone();
    changed["tools"] = json!([tool("bash")]);
    let r = m.respond(&MockRequest::deepseek("s", "turn", &changed));
    assert_eq!(r.status, 500);
    assert!(r.text().contains("prefix: changed"), "{}", r.text());
    assert_eq!(
        ds(&m.respond(&MockRequest::deepseek("s", "turn", &body))).text,
        "2"
    );
    assert_eq!(m.log()[0].prefix_sha256.as_deref(), Some(h.as_str()));
}

#[test]
fn http_budget_and_raw_turns() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"headers":{"retry-after":"1"},"body":{"error":{"message":"rate limited","type":"rate_limit_error"}}}}
{"v":1,"kind":"turn","fault":{"kind":"budget"}}
{"v":1,"kind":"turn","raw":{"status":200,"headers":{"content-type":"text/event-stream"},"body":": keep-alive\n\ndata: [DONE]\n\n"}}"#,
    );
    let body = ds_body(sys_user("hi"));
    let r = m.respond(&MockRequest::deepseek("s", "turn", &body));
    assert_eq!(r.status, 429);
    assert_eq!(r.header("retry-after"), Some("1"));
    assert_eq!(
        r.text(),
        r#"{"error":{"message":"rate limited","type":"rate_limit_error"}}"#
    );
    let b = m.respond(&MockRequest::deepseek("s", "turn", &body));
    assert_eq!(
        (b.status, b.header("x-init-cap"), b.header("x-init-error")),
        (429, Some("day"), Some("budget.day"))
    );
    let bj = b.json_body().unwrap();
    assert_eq!(bj["error"]["type"], "init_cap_exceeded");
    assert_eq!(bj["init"]["cap"], "day");
    let raw = m.respond(&MockRequest::deepseek("s", "turn", &body));
    assert_eq!(raw.body_bytes(), b": keep-alive\n\ndata: [DONE]\n\n");
    let log = m.log();
    assert_eq!(log[0].fault.as_deref(), Some("http"));
    assert_eq!(log[1].fault.as_deref(), Some("budget"));
    assert_eq!(log[2].fault, None);
}

#[test]
fn stream_faults() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","fault":{"kind":"cut","after_events":2}}
{"v":1,"kind":"turn","fault":{"kind":"stall","after_events":1,"ms":7},"reply":{"text":"slow"}}
{"v":1,"kind":"turn","fault":{"kind":"stream_error","after_events":2,"error":"overloaded_error"}}"#,
    );
    let body = ds_body(sys_user("hi"));
    let cut = m.respond(&MockRequest::deepseek("s", "turn", &body));
    assert!(cut.is_cut());
    assert_eq!(events(&cut).len(), 2);
    assert!(
        assemble::deepseek(&events(&cut)).is_err(),
        "a cut stream has no [DONE]"
    );
    let stall = m.respond(&MockRequest::deepseek("s", "turn", &body));
    match &stall.body {
        Body::Stream(p) => assert_eq!(p[1], Part::Stall(7)),
        b => panic!("{b:?}"),
    }
    assert_eq!(ds(&stall).text, "slow");
    let se = m.respond(&MockRequest::deepseek("s", "turn", &body));
    let a = assemble::deepseek(&events(&se)).unwrap();
    assert_eq!(
        (a.stop.as_str(), a.finish_raw.as_str()),
        ("stream:overloaded", "insufficient_system_resource")
    );
    assert_eq!(a.usage, None);
    let kinds: Vec<_> = m.log().iter().map(|l| l.fault.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            Some("cut".into()),
            Some("stall".into()),
            Some("stream_error".into())
        ]
    );
}

#[test]
fn strict_mode_rejects_missing_reasoning_with_400() {
    let m = strict_mock(r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#);
    let msgs = json!([{"content": "SYSTEM", "role": "system"}, {"content": "a", "role": "user"},
                      {"content": "earlier", "role": "assistant"}, {"content": "b", "role": "user"}]);
    let r = m.respond(&MockRequest::deepseek("s", "turn", &ds_body(msgs.clone())));
    assert_eq!(r.status, 400);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(
        r.text(),
        r#"{"error":{"message":"reasoning_content missing in assistant message 2","type":"invalid_request_error"}}"#
    );
    let log = m.log();
    assert_eq!(
        log[0].protocol_error.as_deref(),
        Some("reasoning_content missing in assistant message 2")
    );
    assert_eq!(log[0].responder, None);
    // The rejected request consumed no turn.
    assert_eq!(
        ds(&m.respond(&MockRequest::deepseek("s", "turn", &ds_body(sys_user("a"))))).text,
        "ok"
    );
    // Without strict mode the same request is served.
    let lax = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#,
    );
    assert_eq!(
        lax.respond(&MockRequest::deepseek("s", "turn", &ds_body(msgs.clone())))
            .status,
        200
    );
    // Thinking disabled: no pass-back rule.
    let m2 = strict_mock(r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#);
    let mut off = ds_body(msgs);
    off["thinking"] = json!({"type": "disabled"});
    let a = ds(&m2.respond(&MockRequest::deepseek("s", "turn", &off)));
    assert_eq!(
        a.reasoning, "",
        "no default reasoning when thinking is disabled"
    );
    assert_eq!(m.summary().protocol_errors, 1);
}

#[test]
fn strict_mode_header_checks_per_mode() {
    let m = strict_mock(r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#);
    let body = ds_body(sys_user("a")).to_string();
    let no_headers = MockRequest::post("/deepseek/chat/completions", &[], body.clone());
    assert_eq!(m.respond(&no_headers).status, 400);
    let auth = MockRequest::post(
        "/deepseek/chat/completions",
        &[
            ("x-init-session", "s"),
            ("x-init-purpose", "turn"),
            ("Authorization", "Bearer sk-x"),
        ],
        body.clone(),
    );
    let r = m.respond(&auth);
    assert_eq!(r.status, 400);
    assert!(r.text().contains("credential header authorization"));
    let mut c = MockConfig::new(
        Mode::Upstream,
        script(r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#),
    );
    c.strict_deepseek = true;
    let up = Mock::new(c).unwrap();
    let leaked = MockRequest::post(
        "/chat/completions",
        &[("x-init-session", "s")],
        body.clone(),
    );
    assert!(
        up.respond(&leaked)
            .text()
            .contains("x-init-session header reached the upstream")
    );
    assert_eq!(
        ds(&up.respond(&MockRequest::post("/chat/completions", &[], body))).text,
        "ok"
    );
}

#[test]
fn invalid_json_is_a_protocol_error() {
    let m = mock(Mode::Socket, "");
    let r = m.respond(&MockRequest::post(
        "/deepseek/chat/completions",
        &[("x-init-session", "s")],
        "{nope",
    ));
    assert_eq!(r.status, 400);
    assert_eq!(m.summary().protocol_errors, 1);
}

#[test]
fn probe_responder_is_stateless_across_parallel_arms() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"live"}}"#,
    );
    let probe1 = ds_body(sys_user("[init:probe] Gate probe: call the bash tool once"));
    // Two arms on the same session id, interleaved.
    let a = ds(&m.respond(&MockRequest::deepseek("live-1", "probe", &probe1)));
    let b = ds(&m.respond(&MockRequest::deepseek("live-1", "probe", &probe1)));
    assert_eq!(a, b);
    assert_eq!(
        (a.calls[0].name.as_str(), a.calls[0].args.as_str()),
        ("bash", r#"{"command":"true"}"#)
    );
    let mut msgs = sys_user("[init:probe] Gate probe")
        .as_array()
        .unwrap()
        .clone();
    msgs.push(replay(&a));
    msgs.extend(tool_results(&a, "[exit_code=0]"));
    let done = ds(&m.respond(&MockRequest::deepseek(
        "live-1",
        "probe",
        &ds_body(Value::Array(msgs)),
    )));
    assert_eq!(
        (done.text.as_str(), done.stop.as_str()),
        ("ready", "end_turn")
    );
    assert_eq!(
        ds(&m.respond(&MockRequest::deepseek(
            "live-1",
            "turn",
            &ds_body(sys_user("hi"))
        )))
        .text,
        "live"
    );
    assert!(
        m.log()[..3]
            .iter()
            .all(|l| l.responder.as_deref() == Some("probe"))
    );

    // --no-probe-responder hands probe requests to the script's probe cursor.
    let mut c = MockConfig::new(
        Mode::Socket,
        script(r#"{"v":1,"kind":"turn","purpose":"probe","reply":{"text":"scripted"}}"#),
    );
    c.probe_responder = false;
    let m2 = Mock::new(c).unwrap();
    assert_eq!(
        ds(&m2.respond(&MockRequest::deepseek("s", "probe", &probe1))).text,
        "scripted"
    );
}

#[test]
fn upstream_mode_detects_probes_by_content() {
    let m = mock(
        Mode::Upstream,
        r#"{"v":1,"kind":"turn","reply":{"text":"live"}}"#,
    );
    let probe = ds_body(sys_user("boot block\n[init:probe] Gate probe"));
    let r = m.respond(&MockRequest::post(
        "/chat/completions",
        &[],
        probe.to_string(),
    ));
    assert_eq!(ds(&r).calls[0].name, "bash");
    let log = m.log();
    assert_eq!(
        (log[0].purpose.as_str(), log[0].session.as_str()),
        ("probe", "_none")
    );
    let live = m.respond(&MockRequest::post(
        "/chat/completions",
        &[],
        ds_body(sys_user("hi")).to_string(),
    ));
    assert_eq!(ds(&live).text, "live");
}

#[test]
fn g3_responder_drives_the_gate_steps() {
    let m = mock(Mode::Upstream, "");
    let h = "/x/probe/p 1";
    let mut msgs = vec![
        json!({"content": "SYSTEM", "role": "system"}),
        json!({"content": responders::tests::g3_instruction(h, "abc123"), "role": "user"}),
    ];
    let mut commands = Vec::new();
    for _ in 0..4 {
        let a = ds(&m.respond(&MockRequest::post(
            "/chat/completions",
            &[],
            ds_body(Value::Array(msgs.clone())).to_string(),
        )));
        let args: Value = serde_json::from_str(&a.calls[0].args).unwrap();
        commands.push(args["command"].as_str().unwrap().to_string());
        msgs.push(replay(&a));
        msgs.extend(tool_results(
            &a,
            "0123456789abcdef0123456789abcdef01234567\n",
        ));
    }
    assert!(commands[0].starts_with("git -C '/x/probe/p 1/body' worktree add"));
    assert!(commands[1].contains("mod g3_abc123 {"));
    assert!(commands[2].contains("selfmap --root"));
    assert!(commands[3].contains("refactor(g3): evolvability probe abc123"));
    let done = ds(&m.respond(&MockRequest::post(
        "/chat/completions",
        &[],
        ds_body(Value::Array(msgs)).to_string(),
    )));
    assert_eq!(
        done.text,
        "G3 DONE 0123456789abcdef0123456789abcdef01234567"
    );
    assert!(m.log().iter().all(|l| l.responder.as_deref() == Some("g3")));
}

/// REAL-M7: `_default.mock.jsonl` in the tasks dir serves task sessions without their own script (the Root's
/// random instance ids), each session from its first turn; an own script still wins.
#[test]
fn default_task_script_serves_unscripted_tasks() {
    let home = test_home("task-default");
    let tasks = home.join("tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::write(
        tasks.join(DEFAULT_TASK_SCRIPT),
        r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"default","chunk":8}
{"v":1,"kind":"turn","reply":{"calls":[{"name":"write","input":{"path":"ok.txt","content":"OK\n"}}]}}
{"v":1,"kind":"turn","reply":{"text":"done"}}
"#,
    )
    .unwrap();
    std::fs::write(tasks.join("t-01.mock.jsonl"), "{\"v\":1,\"kind\":\"meta\",\"provider\":\"deepseek\",\"model\":\"mock-frontier-1\",\"name\":\"t-01\",\"chunk\":8}\n{\"v\":1,\"kind\":\"turn\",\"reply\":{\"text\":\"own\"}}\n").unwrap();
    let mut c = MockConfig::new(Mode::Socket, Script::empty());
    c.tasks_dir = Some(tasks);
    let m = Mock::new(c).unwrap();
    for (session, id) in [("a", "0a1b2c3d4e5f"), ("b", "ffeeddccbbaa")] {
        let first = ds(&m.respond(&MockRequest::deepseek(session, "turn", &ds_body(sys_user(&format!("[task {id}] Workspace: /r\n\nx"))))));
        assert_eq!(first.calls[0].name, "write", "{session}");
    }
    let own = ds(&m.respond(&MockRequest::deepseek("c", "turn", &ds_body(sys_user("[task t-01] Workspace: /r\n\nx")))));
    assert_eq!(own.text, "own");
}

#[test]
fn task_sessions_get_their_own_cursors_and_give_up_without_a_script() {
    let home = test_home("task-sessions");
    let tasks = home.join("tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::write(
        tasks.join("t-01.mock.jsonl"),
        r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"t-01","chunk":8}
{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"calls":[{"name":"bash","input":{"command":"make"}}]}}
{"v":1,"kind":"turn","reply":{"text":"Task done."}}
"#,
    )
    .unwrap();
    let mut c = MockConfig::new(Mode::Socket, Script::empty());
    c.tasks_dir = Some(tasks);
    let m = Mock::new(c).unwrap();
    let prompt = |root: &str| {
        sys_user(&format!(
            "[task t-01] Workspace: {root}/work/task\n\nBuild it."
        ))
    };
    for session in ["arm-a", "arm-b"] {
        let a = ds(&m.respond(&MockRequest::deepseek(
            session,
            "turn",
            &ds_body(prompt("/r1")),
        )));
        assert_eq!(a.calls[0].name, "bash");
        let mut msgs = prompt("/r1").as_array().unwrap().clone();
        msgs.push(replay(&a));
        msgs.extend(tool_results(&a, "ok"));
        assert_eq!(
            ds(&m.respond(&MockRequest::deepseek(
                session,
                "turn",
                &ds_body(Value::Array(msgs))
            )))
            .text,
            "Task done."
        );
    }
    let g = ds(&m.respond(&MockRequest::deepseek(
        "arm-c",
        "turn",
        &ds_body(sys_user("[task t-99] Workspace: /r\n\nx")),
    )));
    assert_eq!(g.text, responders::GIVE_UP_TEXT);
    let responders: Vec<_> = m
        .log()
        .iter()
        .map(|l| l.responder.clone().unwrap())
        .collect();
    assert_eq!(
        responders,
        vec![
            "task:t-01",
            "task:t-01",
            "task:t-01",
            "task:t-01",
            "give_up"
        ]
    );
    assert_eq!(m.summary().expect_failures, 0);

    // Upstream mode (no session header): sessions are told apart by the task message itself.
    let mut c = MockConfig::new(Mode::Upstream, Script::empty());
    c.tasks_dir = Some(home.join("tasks"));
    let up = Mock::new(c).unwrap();
    for root in ["/r1", "/r2"] {
        let r = up.respond(&MockRequest::post(
            "/chat/completions",
            &[],
            ds_body(prompt(root)).to_string(),
        ));
        assert_eq!(
            ds(&r).calls[0].name,
            "bash",
            "each probe root starts its own cursor"
        );
    }
}

#[test]
fn cache_simulation() {
    let mut c = MockConfig::new(
        Mode::Socket,
        script(
            &r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}
"#
            .repeat(4),
        ),
    );
    c.simulate_cache = true;
    c.cache_unit_tokens = 16;
    let m = Mock::new(c).unwrap();
    let long = "x".repeat(4000);
    let first = ds_body(sys_user(&long));
    let u1 = ds(&m.respond(&MockRequest::deepseek("s", "turn", &first)))
        .usage
        .unwrap();
    assert_eq!(u1["prompt_cache_hit_tokens"], 0);
    let mut grown = first.clone();
    grown["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"content": "more", "role": "user"}));
    let u2 = ds(&m.respond(&MockRequest::deepseek("s", "turn", &grown)))
        .usage
        .unwrap();
    let hit = u2["prompt_cache_hit_tokens"].as_u64().unwrap();
    assert!(hit > 1000 && hit.is_multiple_of(16), "hit {hit}");
    // A restarted body re-sends the same prompt: a hit again (cache_hit_after_restart).
    let u3 = ds(&m.respond(&MockRequest::deepseek("s2", "turn", &grown)))
        .usage
        .unwrap();
    assert!(u3["prompt_cache_hit_tokens"].as_u64().unwrap() >= hit);
    let log = m.log();
    assert_eq!(log[0].cache_read, Some(0));
    assert_eq!(log[1].cache_read, Some(hit));
    // Without the flag cache_read is 0 and not logged.
    let plain = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"ok"}}"#,
    );
    let u = ds(&plain.respond(&MockRequest::deepseek("s", "turn", &grown)))
        .usage
        .unwrap();
    assert_eq!(u["prompt_cache_hit_tokens"], 0);
    assert_eq!(plain.log()[0].cache_read, None);
}

#[test]
fn explicit_usage_wins() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"x","usage":{"cache_read":11776,"input":12034,"output":31,"reasoning":5}}}"#,
    );
    let u = ds(&m.respond(&MockRequest::deepseek("s", "turn", &ds_body(sys_user("a")))))
        .usage
        .unwrap();
    assert_eq!(
        canon::to_string(&u),
        r#"{"completion_tokens":31,"completion_tokens_details":{"reasoning_tokens":5},"prompt_cache_hit_tokens":11776,"prompt_cache_miss_tokens":258,"prompt_tokens":12034,"prompt_tokens_details":{"cached_tokens":11776},"total_tokens":12065}"#
    );
}

#[test]
fn recording_writes_bodies_and_log_lines() {
    let home = test_home("recording");
    let mut c = MockConfig::new(
        Mode::Socket,
        script(r#"{"v":1,"kind":"turn","expect":{"last_role":"user"},"reply":{"text":"ok"}}"#),
    );
    c.record_dir = Some(home.join("rec"));
    let m = Mock::new(c).unwrap();
    let body = ds_body(sys_user("a")).to_string();
    m.respond(&MockRequest::post(
        "/deepseek/chat/completions",
        &[("x-init-session", "s"), ("x-init-purpose", "turn")],
        body.clone(),
    ));
    m.respond(&MockRequest::post(
        "/deepseek/chat/completions",
        &[("x-init-session", "s"), ("x-init-purpose", "turn")],
        body.clone(),
    ));
    assert_eq!(
        std::fs::read(home.join("rec/000001-turn.json")).unwrap(),
        body.as_bytes()
    );
    assert!(home.join("rec/000002-turn.json").exists());
    let log = read_log(&home.join("rec/mock.log.jsonl")).unwrap();
    assert_eq!(log, m.log());
    assert_eq!(log.len(), 2);
    assert_eq!(
        (log[0].seq, log[0].status, log[0].expect_ok),
        (1, 200, Some(true))
    );
    assert!(log[0].t_done_ms >= log[0].t_recv_ms);
    assert!(log[1].exhausted);
    // Lines are canonical JSON with sorted keys.
    let raw = std::fs::read_to_string(home.join("rec/mock.log.jsonl")).unwrap();
    let first = raw.lines().next().unwrap();
    assert!(first.starts_with(r#"{"cache_read":null,"exhausted":false,"expect_ok":true,"fault":null,"mock_error":null,"path":"/deepseek/chat/completions","prefix_sha256":""#), "{first}");

    let mut c = MockConfig::new(Mode::Socket, Script::empty());
    c.record_dir = Some(home.join("nobodies"));
    c.record_bodies = false;
    let m = Mock::new(c).unwrap();
    m.respond(&MockRequest::deepseek("s", "turn", &ds_body(sys_user("a"))));
    let names: Vec<_> = std::fs::read_dir(home.join("nobodies"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("mock.log.jsonl")]);
}

#[test]
fn routes_and_stats() {
    let m = mock(
        Mode::Socket,
        r#"{"v":1,"kind":"turn","reply":{"text":"ok","usage":{"input":1000,"cache_read":900}}}"#,
    );
    assert_eq!(
        m.respond(&MockRequest::post("/nope/chat/completions", &[], "{}"))
            .status,
        404
    );
    assert_eq!(
        m.respond(&MockRequest::post("/deepseek/v1/models", &[], "{}"))
            .status,
        404
    );
    assert_eq!(m.respond(&MockRequest::get("/_init/health")).status, 200);
    ds(&m.respond(&MockRequest::post(
        "/mock/chat/completions",
        &[("x-init-session", "s"), ("x-init-purpose", "turn")],
        ds_body(sys_user("a")).to_string(),
    )));
    let st = m
        .respond(&MockRequest::get("/_init/stats"))
        .json_body()
        .unwrap();
    assert_eq!(st["requests_today"], 1);
    assert_eq!(st["cache"]["last_permille"], 900);
    assert_eq!(st["v"], 1);
    assert!(m.log().len() == 1, "GET and 404 are not logged");
    let up = mock(Mode::Upstream, "");
    assert_eq!(
        up.respond(&MockRequest::post("/v1/models", &[], "{}"))
            .status,
        404
    );
    assert_eq!(
        up.respond(&MockRequest::get("/_init/stats")).status,
        404,
        "stats only in socket mode"
    );
    assert_eq!(
        up.respond(&MockRequest::post("/v1/messages", &[], "{}"))
            .status,
        500,
        "anthropic route, empty script"
    );
}

#[test]
fn same_script_same_bytes() {
    let run = || {
        let m = mock(Mode::Socket, S1);
        let mut msgs = sys_user("Fix it").as_array().unwrap().clone();
        let r1 = m.respond(&MockRequest::deepseek(
            "s",
            "turn",
            &ds_body(Value::Array(msgs.clone())),
        ));
        let a1 = ds(&r1);
        msgs.push(replay(&a1));
        msgs.extend(tool_results(&a1, "ok"));
        let r2 = m.respond(&MockRequest::deepseek(
            "s",
            "turn",
            &ds_body(Value::Array(msgs)),
        ));
        (r1.body_bytes(), r2.body_bytes())
    };
    assert_eq!(run(), run());
}

// ------------------------------------------------------------------ split-fuzz of the mock's streams

/// A deterministic LCG (no rand crate; fixed seed).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn fuzz_replies() -> Vec<Reply> {
    let calls = vec![
        Call {
            name: "bash".into(),
            input: json!({"command": "echo 'héllo' && printf \"\\n\"", "timeout": 120}),
        },
        Call {
            name: "write".into(),
            input: json!({"content": "fn main() {}\n// 😀 ✓\n", "path": "src/main.rs"}),
        },
        Call {
            name: "read".into(),
            input: json!({}),
        },
    ];
    vec![
        Reply::text(
            "Hello, world. Ünïcödé text with emoji 😀 and CJK 漢字, plus \"quotes\" and \\ backslashes\n.",
        ),
        Reply {
            think: Some("Thinking… step 1\nstep 2 → done".into()),
            text: String::new(),
            calls: calls.clone(),
            stop: Stop::ToolUse,
            usage: UsageSpec::default(),
        },
        Reply {
            think: Some(String::new()),
            text: "partial".into(),
            calls: calls[..1].to_vec(),
            stop: Stop::MaxTokens,
            usage: UsageSpec::default(),
        },
        Reply {
            think: None,
            text: "no.".into(),
            calls: vec![],
            stop: Stop::Refusal,
            usage: UsageSpec::default(),
        },
        Reply {
            think: Some("x".into()),
            text: String::new(),
            calls: vec![],
            stop: Stop::EndTurn,
            usage: UsageSpec::default(),
        },
    ]
}

fn expected_stop(r: &Reply, family: Family) -> String {
    match (r.stop, family) {
        (Stop::EndTurn, Family::Chat) if !r.calls.is_empty() => "tool_use".into(),
        (Stop::Refusal, Family::Messages) => "refusal".into(),
        (s, _) => s.as_str().into(),
    }
}

#[test]
fn split_fuzz_every_byte_boundary_reassembles_identically() {
    let mut rng = Lcg(0x1d17_a6e7);
    let mut checked = 0usize;
    for family in [Family::Chat, Family::Messages, Family::Responses] {
        for reply in fuzz_replies() {
            for chunk in [1usize, 2, 3, 5, 7, 16, 64] {
                let cx = render::RenderCx {
                    family,
                    model: "m",
                    label: "turn",
                    n: 2,
                    chunk,
                    thinking_disabled: false,
                };
                let think = render::effective_think(&cx, &reply);
                let usage = render::compute_usage(&reply.usage, 4000, None, &think, &reply);
                let events = render::render(&cx, &reply, &usage);
                let bytes: Vec<u8> = events.concat();
                let whole = parse_all(&bytes).unwrap();
                assert_eq!(
                    whole.len(),
                    events.len(),
                    "one SSE event per rendered event"
                );

                // The stream reassembles to the reply.
                let a = assemble::assemble(family, &whole)
                    .unwrap_or_else(|e| panic!("{family:?} chunk {chunk}: {e}"));
                assert_eq!(a.text, reply.text, "{family:?} chunk {chunk}");
                if family == Family::Chat {
                    assert_eq!(a.reasoning, think);
                }
                assert_eq!(
                    a.stop,
                    expected_stop(&reply, family),
                    "{family:?} {reply:?}"
                );
                let want: Vec<(String, String)> = reply
                    .calls
                    .iter()
                    .map(|c| (c.name.clone(), render::call_arguments(&c.input)))
                    .collect();
                let got: Vec<(String, String)> = a
                    .calls
                    .iter()
                    .map(|c| (c.name.clone(), c.args.clone()))
                    .collect();
                assert_eq!(got, want);
                let ids: std::collections::BTreeSet<_> =
                    a.calls.iter().map(|c| c.id.clone()).collect();
                assert_eq!(ids.len(), a.calls.len(), "call ids are unique");

                // Every-byte splits are O(n²); they run for the realistic chunk sizes (default 16),
                // the small sizes get byte-at-a-time and random multi-splits below.
                let every_byte = chunk >= 16;
                // SSE layer: a boundary at every byte gives identical events.
                for p in (0..=bytes.len()).filter(|_| every_byte) {
                    let mut parser = SseParser::new();
                    let mut ev = parser.feed(&bytes[..p]).unwrap();
                    ev.extend(parser.feed(&bytes[p..]).unwrap());
                    assert_eq!(ev, whole, "split at {p}");
                    checked += 1;
                }
                // Byte-at-a-time and random multi-splits.
                let mut parser = SseParser::new();
                let mut ev = Vec::new();
                for b in &bytes {
                    ev.extend(parser.feed(std::slice::from_ref(b)).unwrap());
                }
                assert_eq!(ev, whole);
                for _ in 0..8 {
                    let mut parser = SseParser::new();
                    let mut ev = Vec::new();
                    let mut i = 0;
                    while i < bytes.len() {
                        let n = 1 + (rng.next() as usize % 97);
                        let j = (i + n).min(bytes.len());
                        ev.extend(parser.feed(&bytes[i..j]).unwrap());
                        i = j;
                    }
                    assert_eq!(ev, whole);
                }

                // HTTP layer: the chunked framing the server writes, split at every byte.
                let resp = MockResponse::sse(events.iter().cloned().map(Part::Event).collect());
                let wire = encode_response(&resp, true);
                let body_at = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let framed = &wire[body_at..];
                for p in (0..=framed.len()).filter(|p| every_byte || p % 7 == 0) {
                    let mut d = ChunkedDecoder::new();
                    let mut out = d.feed(&framed[..p]).unwrap();
                    out.extend(d.feed(&framed[p..]).unwrap());
                    assert!(d.is_done());
                    assert_eq!(out, bytes, "chunk split at {p}");
                }
            }
        }
    }
    assert!(checked > 20_000, "checked {checked} split points");
}

#[test]
fn stream_faults_in_every_family_are_detectable() {
    for family in [Family::Chat, Family::Messages, Family::Responses] {
        let cx = render::RenderCx {
            family,
            model: "m",
            label: "turn",
            n: 0,
            chunk: 4,
            thinking_disabled: false,
        };
        let reply = render::filler_reply();
        let events = render::render(&cx, &reply, &Usage::default());
        let parts = render::apply_stream_fault(
            &cx,
            events.clone(),
            &Fault::StreamError {
                after_events: 3,
                error: "overloaded_error".into(),
            },
        );
        let bytes: Vec<u8> = parts
            .iter()
            .flat_map(|p| {
                if let Part::Event(e) = p {
                    e.clone()
                } else {
                    vec![]
                }
            })
            .collect();
        let a = assemble::assemble(family, &parse_all(&bytes).unwrap()).unwrap();
        assert!(a.stop.starts_with("stream:"), "{family:?}: {}", a.stop);
        for k in [0, 1, 5, 1000] {
            let parts =
                render::apply_stream_fault(&cx, events.clone(), &Fault::Cut { after_events: k });
            let bytes: Vec<u8> = parts
                .iter()
                .flat_map(|p| {
                    if let Part::Event(e) = p {
                        e.clone()
                    } else {
                        vec![]
                    }
                })
                .collect();
            assert!(
                assemble::assemble(family, &parse_all(&bytes).unwrap()).is_err(),
                "{family:?} cut after {k}"
            );
        }
    }
}
