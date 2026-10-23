//! Golden SSE files under `fixtures/`: the mock's rendering of fixed script turns for a fixed
//! request, byte for byte. Regenerate with `INIT_TESTKIT_BLESS=1` after an intended change, and
//! review the diff. The hand-made edge-case fixtures (`usage-separate-chunk`, `split-args`,
//! `keepalive-crlf`) are checked by reassembly and by byte-exact `raw` replay.

use init_testkit::mockllm::assemble;
use init_testkit::mockllm::sse::{SseParser, parse_all};
use init_testkit::mockllm::wire::Family;
use init_testkit::mockllm::{Mock, MockConfig, MockRequest, Mode, Script};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn request(family: Family) -> MockRequest {
    let tools_chat = json!([{"function": {"description": "Run a shell command.", "name": "bash",
                              "parameters": {"properties": {"command": {"type": "string"}}, "required": ["command"], "type": "object"}},
                             "type": "function"}]);
    match family {
        Family::Chat => MockRequest::deepseek(
            "golden",
            "turn",
            &json!({"max_tokens": 65536, "messages": [{"content": "You are INIT.", "role": "system"},
                    {"content": "List the files.", "role": "user"}], "model": "deepseek-flash",
                    "reasoning_effort": "high", "stream": true, "thinking": {"type": "enabled"}, "tools": tools_chat}),
        ),
        Family::Messages => MockRequest::post(
            "/anthropic/v1/messages",
            &[("x-init-session", "golden"), ("x-init-purpose", "turn")],
            json!({"max_tokens": 32000, "messages": [{"content": "List the files.", "role": "user"}],
                   "model": "claude-mock", "stream": true, "system": "You are INIT.",
                   "tools": [{"description": "Run a shell command.", "input_schema": {"type": "object"}, "name": "bash"}]})
            .to_string(),
        ),
        Family::Responses => MockRequest::post(
            "/openai/v1/responses",
            &[("x-init-session", "golden"), ("x-init-purpose", "turn")],
            json!({"input": [{"content": "You are INIT.", "role": "developer"}, {"content": "List the files.", "role": "user"}],
                   "model": "gpt-mock", "stream": true,
                   "tools": [{"description": "Run a shell command.", "name": "bash", "parameters": {"type": "object"}, "type": "function"}]})
            .to_string(),
        ),
    }
}

/// (fixture path, family, script turn).
const CASES: &[(&str, Family, &str)] = &[
    (
        "deepseek/text.sse",
        Family::Chat,
        r#"{"think":"The user wants a listing.","text":"Here are the files: a.rs, b.rs.","stop":"end_turn"}"#,
    ),
    (
        "deepseek/default-reasoning.sse",
        Family::Chat,
        r#"{"text":"ok"}"#,
    ),
    (
        "deepseek/tool-calls.sse",
        Family::Chat,
        r#"{"think":"Use ls, then read.","calls":[{"name":"bash","input":{"command":"ls -la","timeout":30}},{"name":"read","input":{"path":"src/main.rs"}}],"stop":"tool_use"}"#,
    ),
    (
        "deepseek/max-tokens.sse",
        Family::Chat,
        r#"{"think":"","text":"Partial answer","calls":[{"name":"bash","input":{"command":"echo cut"}}],"stop":"max_tokens"}"#,
    ),
    (
        "deepseek/refusal.sse",
        Family::Chat,
        r#"{"text":"I can't help with that.","stop":"refusal"}"#,
    ),
    (
        "deepseek/usage-explicit.sse",
        Family::Chat,
        r#"{"text":"cached","usage":{"cache_read":11776,"input":12034,"output":31,"reasoning":12}}"#,
    ),
    (
        "anthropic/text-tool.sse",
        Family::Messages,
        r#"{"think":"hidden","text":"Listing now.","calls":[{"name":"bash","input":{"command":"ls"}}],"stop":"tool_use"}"#,
    ),
    (
        "anthropic/text.sse",
        Family::Messages,
        r#"{"text":"Hello from the mock.","stop":"end_turn"}"#,
    ),
    (
        "openai/text-tool.sse",
        Family::Responses,
        r#"{"think":"plan","text":"Listing now.","calls":[{"name":"bash","input":{"command":"ls"}}],"stop":"tool_use"}"#,
    ),
    (
        "openai/max-tokens.sse",
        Family::Responses,
        r#"{"text":"Partial","stop":"max_tokens"}"#,
    ),
];

/// Stream-fault goldens: (fixture path, fault JSON).
const FAULTS: &[(&str, &str)] = &[
    (
        "deepseek/fault-stream-error.sse",
        r#"{"kind":"stream_error","after_events":3,"error":"overloaded_error"}"#,
    ),
    (
        "deepseek/fault-cut.sse",
        r#"{"kind":"cut","after_events":4}"#,
    ),
];

fn one_turn_mock(turn_kind: &str, body: &str) -> std::sync::Arc<Mock> {
    let text = format!(
        "{{\"v\":1,\"kind\":\"meta\",\"provider\":\"deepseek\",\"model\":\"mock-frontier-1\",\"name\":\"golden\",\"chunk\":8}}\n\
         {{\"v\":1,\"kind\":\"turn\",\"{turn_kind}\":{body}}}\n"
    );
    let script = Script::parse(&text, &fixtures()).unwrap();
    Mock::new(MockConfig::new(Mode::Socket, script)).unwrap()
}

fn check_or_bless(rel: &str, got: &[u8]) {
    let path = fixtures().join(rel);
    if std::env::var_os("INIT_TESTKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with INIT_TESTKIT_BLESS=1)", path.display()));
    assert!(
        want == got,
        "{rel} differs from the golden file.\n--- want\n{}\n--- got\n{}",
        String::from_utf8_lossy(&want),
        String::from_utf8_lossy(got)
    );
}

#[test]
fn golden_streams_are_byte_exact() {
    for (rel, family, turn) in CASES {
        let m = one_turn_mock("reply", turn);
        let r = m.respond(&request(*family));
        assert_eq!(r.status, 200, "{rel}: {}", r.text());
        let bytes = r.body_bytes();
        check_or_bless(rel, &bytes);
        // Every golden also reassembles cleanly with the reference decoder for its family.
        let a = assemble::assemble(*family, &parse_all(&bytes).unwrap())
            .unwrap_or_else(|e| panic!("{rel}: {e}"));
        let reply: Value = serde_json::from_str(turn).unwrap();
        assert_eq!(a.text, reply["text"].as_str().unwrap_or(""), "{rel}");
    }
    for (rel, fault) in FAULTS {
        let m = one_turn_mock("fault", fault);
        let r = m.respond(&request(Family::Chat));
        check_or_bless(rel, &r.body_bytes());
    }
}

#[test]
fn golden_files_never_carry_clocks_or_secrets() {
    for (rel, _, _) in CASES {
        let text = std::fs::read_to_string(fixtures().join(rel)).unwrap();
        for ev in parse_all(text.as_bytes()).unwrap() {
            if let Ok(v) = serde_json::from_str::<Value>(&ev.data)
                && let Some(c) = v.get("created")
            {
                assert_eq!(c, 0, "{rel}: created must be 0");
            }
        }
        assert!(!text.contains("sk-"), "{rel}");
    }
}

#[test]
fn handmade_usage_separate_chunk() {
    let bytes = std::fs::read(fixtures().join("deepseek/usage-separate-chunk.sse")).unwrap();
    let a = assemble::deepseek(&parse_all(&bytes).unwrap()).unwrap();
    assert_eq!(
        (a.reasoning.as_str(), a.text.as_str(), a.stop.as_str()),
        ("Say hi.", "Hi there.", "end_turn")
    );
    let u = a.usage.unwrap();
    assert_eq!(
        (
            u["prompt_tokens"].as_u64(),
            u["prompt_cache_miss_tokens"].as_u64()
        ),
        (Some(20), Some(20))
    );
}

#[test]
fn handmade_split_args_interleave_by_index() {
    let bytes = std::fs::read(fixtures().join("deepseek/split-args.sse")).unwrap();
    let a = assemble::deepseek(&parse_all(&bytes).unwrap()).unwrap();
    assert_eq!(a.stop, "tool_use");
    let calls: Vec<(&str, &str, &str)> = a
        .calls
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str(), c.args.as_str()))
        .collect();
    assert_eq!(
        calls,
        vec![
            ("call_00_hAnDmAdE0", "read", r#"{"path":"a.rs"}"#),
            ("call_01_hAnDmAdE1", "read", r#"{"path":"b.rs"}"#)
        ]
    );
}

#[test]
fn handmade_keepalive_comments_and_crlf() {
    let bytes = std::fs::read(fixtures().join("deepseek/keepalive-crlf.sse")).unwrap();
    let mut p = SseParser::new();
    let ev = p.feed(&bytes).unwrap();
    assert_eq!(p.comments, 2);
    let a = assemble::deepseek(&ev).unwrap();
    assert_eq!((a.text.as_str(), a.stop.as_str()), ("ok", "end_turn"));
}

#[test]
fn raw_turns_replay_fixtures_byte_exact() {
    // body_file resolves against the script's directory (here: fixtures/).
    for rel in [
        "deepseek/usage-separate-chunk.sse",
        "deepseek/split-args.sse",
        "deepseek/keepalive-crlf.sse",
    ] {
        let m = one_turn_mock(
            "raw",
            &format!(
                r#"{{"status":200,"headers":{{"content-type":"text/event-stream"}},"body_file":"{rel}"}}"#
            ),
        );
        let r = m.respond(&request(Family::Chat));
        assert_eq!(
            r.body_bytes(),
            std::fs::read(fixtures().join(rel)).unwrap(),
            "{rel}"
        );
        assert_eq!(r.header("content-type"), Some("text/event-stream"));
    }
}
