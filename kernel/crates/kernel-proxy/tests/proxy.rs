//! kernel/proxy.md §8 through a real socket: body → kernel proxy → (fake) Root egress → init-mockllm.

mod common;

use common::*;
use init_testkit::mockllm::assemble;
use init_testkit::mockllm::sse::parse_all;
use kernel_proxy::hash;
use kernel_proxy::tap::Family;
use kernel_proxy::{GateKind, Label, Level, OpenScope, ScopeKind, SoftBudgets, Visibility};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

const META: &str = r#"{"v":1,"kind":"meta","provider":"deepseek","model":"mock-frontier-1","name":"wp5","chunk":8}"#;

fn script(turns: &[&str]) -> String {
    let mut s = String::from(META);
    for t in turns {
        s.push('\n');
        s.push_str(t);
    }
    s.push('\n');
    s
}

/// Slice-1 path: forwarding, header filtering (P-02), usage and cost (DeepSeek cache hit/miss), the
/// head hash, `/_init/stats` (integers only), traces and the agent's view, tool round trip (P-14),
/// no key material anywhere (P-16).
#[test]
fn slice1_forward_meter_trace() {
    let _wd = init_testkit::watchdog::arm("slice1_forward_meter_trace", WD);
    let s = script(&[
        r#"{"v":1,"kind":"turn","reply":{"think":"Plan.","calls":[{"name":"bash","input":{"command":"ls"}}],"stop":"tool_use","usage":{"input":1000,"cache_read":800,"output":50}}}"#,
        r#"{"v":1,"kind":"turn","reply":{"think":"Done.","text":"Listed.","stop":"end_turn","usage":{"input":1100,"cache_read":1000,"output":10}}}"#,
    ]);
    let rig = Rig::new(
        "slice1",
        RigOpts {
            script: &s,
            strict: true,
            ..RigOpts::default()
        },
    );
    let body = chat("mock-frontier-1", "list files", 1000);
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &body,
        &[
            ("authorization", "Bearer sk-SECRET-should-never-pass"),
            ("x-api-key", "sk-SECRET-2"),
            ("cookie", "a=b"),
            ("accept-encoding", "gzip"),
            ("x-init-session", "20261020-010203-abcdef"),
            ("x-init-purpose", "turn"),
            ("x-init-task", "t-1"),
            ("x-init-subtask", "plan"),
            ("x-init-attempt", "1"),
        ],
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.complete);
    let req_id = r.header("x-init-req-id").unwrap().to_string();
    assert_eq!(req_id.len(), 16);
    assert_eq!(r.header("x-init-retries"), Some("0"));
    assert_eq!(r.header("x-init-routed"), Some("0"));
    let head_hash = r.header("x-init-head-hash").unwrap().to_string();
    assert_eq!(
        head_hash,
        hash::prefix_hash("mock", Family::OpenAiChat, &body)
    );
    // The stream reaches the body unchanged.
    let a = assemble::deepseek(&parse_all(&r.body).unwrap()).unwrap();
    assert_eq!(a.calls[0].name, "bash");
    let call_id = a.calls[0].id.clone();

    // P-02: what the egress saw.
    let seen = rig.eg().seen_posts();
    assert_eq!(seen.len(), 1);
    let e = &seen[0];
    assert_eq!(e.path, "/mock/chat/completions");
    for k in ["authorization", "x-api-key", "api-key", "cookie"] {
        assert!(e.header(k).is_none(), "{k} leaked");
    }
    assert!(
        !e.headers.iter().any(|(k, _)| k.starts_with("x-init-")),
        "x-init-* leaked: {:?}",
        e.headers
    );
    assert_eq!(e.header("accept-encoding"), Some("identity"));
    assert_eq!(e.header("x-egress-scope"), Some("live"));
    assert_eq!(e.header("x-egress-reservation"), Some(req_id.as_str()));
    assert!(e.header("x-egress-body").is_some());
    assert!(e.header("x-egress-guard").unwrap().starts_with("epoch="));
    assert!(e.header("user-agent").unwrap().starts_with("init-kernel/"));
    assert_eq!(
        e.body,
        serde_json::to_vec(&body).unwrap(),
        "the request body is forwarded byte-for-byte"
    );

    // Accounting: hit 800 × 6,000 + miss 200 × 300,000 + 50 × 1,200,000 pusd = 124,800,000 pusd.
    wait_for(
        || rig.proxy.stats("live").unwrap()["requests_today"] == 1,
        "settlement",
    );
    let st = get_json(&rig.sock, "/_init/stats");
    assert_integers_only(&st, "");
    assert_eq!(st["spend"]["today_musd"], 124);
    assert_eq!(st["cache"]["last_permille"], 800);
    assert_eq!(st["last_head_hash"], json!(head_hash));
    assert_eq!(st["last"]["finish"], "tool_use");
    assert_eq!(st["last"]["usage"]["input_cache_read"], 800);
    assert_eq!(st["last"]["usage"]["input_uncached"], 200);
    assert_eq!(
        st["spend"]["by_model"]["mock/mock-frontier-1"]["requests"],
        1
    );
    assert_eq!(st["spend"]["task"]["id"], "t-1");
    assert_eq!(st["spend"]["task"]["today_musd"], 124);
    assert_eq!(st["routing"]["level"], "L0");
    assert!(st["root"]["money"]["day_limit_musd"].is_u64());
    let health = get_json(&rig.sock, "/_init/health");
    assert_eq!(health["ok"], true);
    assert_eq!(health["providers"]["mock"]["forwarded"], true);
    let models = get_json(&rig.sock, "/_init/models");
    assert_eq!(
        models["routable"].as_array().unwrap().len(),
        1,
        "L0 routes the default model only"
    );

    // P-14: a fabricated tool result id does not count; the emitted one does.
    let mut fake = chat("mock-frontier-1", "x", 1000);
    fake["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "tool", "tool_call_id": "call_fabricated", "content": "x"}));
    let _ = fake;
    let mut next = body.clone();
    let msgs = next["messages"].as_array_mut().unwrap();
    msgs.push(json!({"role": "assistant", "content": "", "reasoning_content": "Plan.",
                     "tool_calls": [{"id": call_id, "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}}]}));
    msgs.push(json!({"role": "tool", "tool_call_id": call_id, "content": "a.txt"}));
    let r2 = post(
        &rig.sock,
        "/mock/chat/completions",
        &next,
        &[
            ("x-init-session", "20261020-010203-abcdef"),
            ("x-init-task", "t-1"),
        ],
    );
    assert_eq!(r2.status, 200, "{}", r2.text());
    assert_eq!(
        r2.header("x-init-head-hash"),
        Some(head_hash.as_str()),
        "same head, same prefix_hash"
    );
    wait_for(
        || rig.proxy.stats("live").unwrap()["requests_today"] == 2,
        "second settlement",
    );
    let sum = rig.proxy.scope_summary("live").unwrap();
    assert_eq!(sum["tool_round_trip"], true);
    assert_eq!(sum["tool_calls"]["tool/bash"], 1);
    assert_eq!(sum["prefix_hashes"][0][1], 2);
    assert_eq!(sum["first"]["tools"][0]["name"], "bash");
    let st = get_json(&rig.sock, "/_init/stats");
    assert!(st["latency_1h"]["requests"].as_u64().unwrap() >= 1);
    assert!(
        st["last"]["gap_ms"].is_u64(),
        "the previous response ended with tool calls: gap_ms is measured"
    );

    // Traces: the agent view lists both, reconstructs the request, returns the raw response.
    rig.proxy.flush_traces();
    let list = get(&rig.sock, "/_init/traces?limit=10");
    assert_eq!(list.status, 200);
    let lines: Vec<Value> = list
        .text()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["req_id"], json!(req_id));
    assert_eq!(lines[0]["cost_musd"], 124);
    assert_eq!(lines[0]["subtask"], "plan");
    assert_eq!(lines[0]["outcome"], "ok");
    assert!(lines[0].get("cost_pusd").is_none() && lines[0].get("head").is_none());
    for l in &lines {
        assert_integers_only(l, "trace");
    }
    let rq = get(&rig.sock, &format!("/_init/traces/{req_id}/request"));
    assert_eq!(rq.status, 200);
    assert_eq!(serde_json::from_slice::<Value>(&rq.body).unwrap(), body);
    let rs = get(&rig.sock, &format!("/_init/traces/{req_id}/response"));
    assert_eq!(rs.status, 200);
    assert_eq!(rs.body, r.body, "the raw response bytes are stored");
    assert!(
        rs.header("content-type")
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(
        get(&rig.sock, "/_init/traces/0123456789abcdef/request").status,
        404
    );

    // P-16: no credential anywhere in the trace store.
    for f in files_under(&rig.home.join("vault")) {
        let b = std::fs::read(&f).unwrap();
        let t = String::from_utf8_lossy(&b);
        assert!(
            !t.contains("sk-SECRET") && !t.contains("Bearer "),
            "{} holds key material",
            f.display()
        );
    }
    // The mock (strict deepseek, upstream mode) found no protocol error: no x-init-* or credential headers.
    assert!(
        rig.mock
            .mock()
            .log()
            .iter()
            .all(|l| l.protocol_error.is_none()),
        "{:?}",
        rig.mock.mock().log()
    );
}

/// P-03 + P-23: keep-alive to the egress; the egress going away and coming back.
#[test]
fn p03_p23_keepalive_and_egress_down() {
    let _wd = init_testkit::watchdog::arm("p03_p23_keepalive_and_egress_down", WD);
    let rig = Rig::new("p03", RigOpts::default());
    let probe = chat("mock-frontier-1", "[init:probe] ping", 100);
    for _ in 0..100 {
        let r = post(&rig.sock, "/mock/chat/completions", &probe, &[]);
        assert_eq!(r.status, 200, "{}", r.text());
    }
    rig.proxy.quiesce(Duration::from_secs(5));
    let st = get_json(&rig.sock, "/_init/stats");
    let connects = st["egress"]["connects"].as_u64().unwrap();
    assert!(
        connects <= 4,
        "connections are reused: {connects} connects for 100 requests"
    );
    assert!(st["egress"]["reuse_permille"].as_u64().unwrap() >= 960);

    // The egress goes away: 503 egress.unavailable after the connect retries.
    let mut rig = rig;
    let eg = rig.egress.take().unwrap();
    let path = eg.path.clone();
    let state = eg.stop();
    let t = Instant::now();
    let r = post(&rig.sock, "/mock/chat/completions", &probe, &[]);
    assert_eq!(r.status, 503, "{}", r.text());
    assert_eq!(r.header("x-init-error"), Some("egress.unavailable"));
    assert!(t.elapsed() < Duration::from_secs(10));
    // …and comes back: the pool's dead connections are replaced.
    rig.egress = Some(FakeEgress::start_with(&path, state));
    let r = post(&rig.sock, "/mock/chat/completions", &probe, &[]);
    assert_eq!(r.status, 200, "{}", r.text());
}

/// RT-C1: soft layer intact. Charter day $0.10, request_max $0.02 → the soft day clamps to $0.08;
/// the kernel refuses with a typed 429 `budget.day`; the Root never refuses.
#[test]
fn rt_c1_soft_cap_typed_429() {
    let _wd = init_testkit::watchdog::arm("rt_c1_soft_cap_typed_429", WD);
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":10000,"cache_read":0,"output":14000}}}"#;
    let s = script(&[turn; 8]);
    let cfg = CONFIG
        .replace("request_max_usd = \"1.00\"", "request_max_usd = \"0.02\"")
        .replace("task_usd = \"0.50\"", "task_usd = \"1.00\"");
    let rig = Rig::new(
        "rtc1",
        RigOpts {
            script: &s,
            config: &cfg,
            day_limit_musd: 100_000,
            ..RigOpts::default()
        },
    );
    assert!(rig.alert_codes().contains(&"soft_day_clamped".to_string()));
    let st = get_json(&rig.sock, "/_init/stats");
    assert_eq!(st["spend"]["cap_today_musd"], 80_000);
    // Each request costs 10,000 × 0.30 + 14,000 × 1.20 µUSD = 19,800 µUSD; reservation ≤ $0.02.
    let body = chat("mock-frontier-1", "go", 15_000);
    let mut ok = 0;
    let refused = loop {
        let r = post(&rig.sock, "/mock/chat/completions", &body, &[]);
        if r.status != 200 {
            break r;
        }
        ok += 1;
        wait_for(
            || rig.proxy.stats("live").unwrap()["requests_today"] == ok,
            "settle",
        );
        assert!(ok < 8, "never refused");
    };
    assert_eq!(ok, 4);
    assert_eq!(refused.status, 429);
    assert_eq!(refused.header("x-init-error"), Some("budget.day"));
    assert_eq!(refused.header("x-init-cap"), Some("day"));
    assert!(
        refused
            .header("retry-after")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    let v = json_of(&refused);
    assert_eq!(v["error"]["type"], "init_cap_exceeded");
    assert_eq!(v["error"]["code"], "budget.day");
    assert_eq!(v["init"]["limit_musd"], 80_000);
    assert_eq!(v["init"]["spent_musd"], 79_200);
    assert_eq!(v["init"]["retryable"], true);
    assert_integers_only(&v, "");
    assert_eq!(
        rig.eg().ceiling_refusals.load(Ordering::SeqCst),
        0,
        "the Root never refused"
    );
    assert_eq!(
        rig.eg().seen_posts().len(),
        4,
        "the refused request never reached the Root"
    );
    assert!(
        rig.alert_codes()
            .contains(&"budget_day_reached".to_string())
    );
}

/// P-08 (b): the guard reads the Root's own (peak-priced) figure; every forwarded request carries a
/// true `x-egress-guard`; the egress never sees a request that would cross its ceiling.
#[test]
fn p08b_root_guard_and_epoch_header() {
    let _wd = init_testkit::watchdog::arm("p08b_root_guard_and_epoch_header", WD);
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":100,"cache_read":0,"output":10}}}"#;
    let s = script(&[turn; 6]);
    let rig = Rig::new(
        "p08b",
        RigOpts {
            script: &s,
            ..RigOpts::default()
        },
    );
    rig.eg().set_spent_musd(9_700_000, 9_700_000);
    rig.proxy.reload(kernel_proxy::Reload::RootState).unwrap();
    let small = chat("mock-frontier-1", "small", 50_000); // peak reservation ≈ $0.06: fits under $9.80
    let big = chat("mock-frontier-1", "big", 100_000); // ≈ $0.12: would pass $9.80
    let r = post(&rig.sock, "/mock/chat/completions", &small, &[]);
    assert_eq!(r.status, 200, "{}", r.text());
    let r = post(&rig.sock, "/mock/chat/completions", &big, &[]);
    assert_eq!(r.status, 429, "{}", r.text());
    assert_eq!(r.header("x-init-error"), Some("budget.root_day"));
    assert_eq!(r.header("x-init-cap"), Some("root_day"));
    let v = json_of(&r);
    assert_eq!(v["init"]["ceiling_musd"], 10_000_000);
    assert_eq!(v["init"]["limit_musd"], 9_800_000);
    assert!(
        v["init"]["spent_musd"].as_u64().unwrap() >= 9_700_000,
        "the Root's own figure"
    );
    assert!(r.header("retry-after").is_some());
    assert_eq!(rig.eg().ceiling_refusals.load(Ordering::SeqCst), 0);
    // The guard header names the epoch of a figure the kernel actually read.
    for s in rig.eg().seen_posts() {
        let e: u64 = s
            .header("x-egress-guard")
            .unwrap()
            .strip_prefix("epoch=")
            .unwrap()
            .parse()
            .unwrap();
        assert!(e >= 2 && e <= rig.eg().epoch.load(Ordering::SeqCst));
    }
    assert!(rig.alert_codes().contains(&"budget_root_guard".to_string()));
}

/// P-09: the retry matrix.
#[test]
fn p09_retry_matrix() {
    let _wd = init_testkit::watchdog::arm("p09_retry_matrix", WD);
    let ok =
        r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":10,"output":2}}}"#;
    let s = script(&[
        // 1: 429 without retry-after → retried → ok
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"body":{"error":{"message":"busy","type":"rate_limit_error"}}}}"#,
        ok,
        // 2: 503 three times → passed through after 2 retries
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":503,"body":{"error":{"message":"overloaded"}}}}"#,
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":503,"body":{"error":{"message":"overloaded"}}}}"#,
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":503,"body":{"error":{"message":"overloaded"}}}}"#,
        // 3: 529 with retry-after: 1 → retried → ok
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":529,"headers":{"retry-after":"1"},"body":{"type":"error","error":{"type":"overloaded_error"}}}}"#,
        ok,
        // 4: retry-after 3600 → passed through at once
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"headers":{"retry-after":"3600"},"body":{"error":{"message":"later"}}}}"#,
        // 5: 401 with a key fragment → 401 upstream.auth, body withheld
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":401,"body":{"error":{"message":"Your api key: ****abcd is invalid","type":"authentication_error"}}}}"#,
        // 6: 402 → passed through, alert
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":402,"body":{"error":{"message":"Insufficient Balance"}}}}"#,
        // 7: 500 with x-should-retry: false → passed through
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":500,"headers":{"x-should-retry":"false"},"body":{"error":{"message":"no"}}}}"#,
        // 8: 400 → passed through, no retry
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":400,"body":{"error":{"message":"bad"}}}}"#,
        // 9: an OpenAI-style spend-limit 429 → never retried
        r#"{"v":1,"kind":"turn","fault":{"kind":"http","status":429,"body":{"error":{"code":"insufficient_quota","message":"quota"}}}}"#,
        // 10: a cut mid-stream → forwarded truncated, outcome stream_error
        r#"{"v":1,"kind":"turn","fault":{"kind":"cut","after_events":3}}"#,
    ]);
    let rig = Rig::new(
        "p09",
        RigOpts {
            script: &s,
            ..RigOpts::default()
        },
    );
    let b = chat("mock-frontier-1", "x", 100);
    let p = |rig: &Rig| post(&rig.sock, "/mock/chat/completions", &b, &[]);

    let r = p(&rig);
    assert_eq!(
        (r.status, r.header("x-init-retries")),
        (200, Some("1")),
        "{}",
        r.text()
    );
    let r = p(&rig);
    assert_eq!((r.status, r.header("x-init-retries")), (503, Some("2")));
    let t = Instant::now();
    let r = p(&rig);
    assert_eq!((r.status, r.header("x-init-retries")), (200, Some("1")));
    assert!(
        t.elapsed() >= Duration::from_millis(900),
        "retry-after: 1 honoured"
    );
    let t = Instant::now();
    let r = p(&rig);
    assert_eq!(
        (
            r.status,
            r.header("x-init-retries"),
            r.header("retry-after")
        ),
        (429, Some("0"), Some("3600"))
    );
    assert!(t.elapsed() < Duration::from_secs(3));
    let r = p(&rig);
    assert_eq!(r.status, 401);
    assert_eq!(r.header("x-init-error"), Some("upstream.auth"));
    assert!(!r.text().contains("abcd"), "the upstream body is withheld");
    let r = p(&rig);
    assert_eq!(r.status, 402);
    assert!(r.text().contains("Insufficient Balance"));
    let r = p(&rig);
    assert_eq!((r.status, r.header("x-init-retries")), (500, Some("0")));
    let r = p(&rig);
    assert_eq!((r.status, r.header("x-init-retries")), (400, Some("0")));
    let r = p(&rig);
    assert_eq!((r.status, r.header("x-init-retries")), (429, Some("0")));
    let r = p(&rig);
    assert_eq!(r.status, 200);
    assert!(!r.complete, "a cut stream reaches the body truncated");
    let codes = rig.alert_codes();
    assert!(
        codes.contains(&"upstream_auth".to_string())
            && codes.contains(&"provider_balance".to_string()),
        "{codes:?}"
    );
    // 1+1, 3, 1+1, 1, 1, 1, 1, 1, 1, 1 = 14 upstream attempts.
    assert_eq!(rig.mock.mock().log().len(), 14);

    // Root refusals: mapped, never retried, `init.root` kept.
    let before = rig.eg().seen_posts().len();
    for (status, code, kstatus, kcode) in [
        (429, "ceiling.day", 429, "budget.root_day"),
        (429, "ceiling.total", 429, "budget.root_total"),
        (
            429,
            "quota.parallel_bodies",
            429,
            "budget.root_parallel_bodies",
        ),
        (403, "class.path", 403, "egress.path"),
    ] {
        rig.eg()
            .refusals
            .lock()
            .unwrap()
            .push_back((status, code.to_string()));
        let r = p(&rig);
        assert_eq!(r.status, kstatus, "{code}: {}", r.text());
        assert_eq!(r.header("x-init-error"), Some(kcode));
        let v = json_of(&r);
        assert_eq!(v["init"]["root"]["code"], code);
        if kstatus == 429 {
            assert_eq!(v["error"]["type"], "init_cap_exceeded");
        }
    }
    assert_eq!(
        rig.eg().seen_posts().len(),
        before + 4,
        "each Root refusal was sent once"
    );
    rig.proxy.quiesce(Duration::from_secs(5));
    rig.proxy.flush_traces();
    let lines = get(&rig.sock, "/_init/traces?limit=100").text();
    let outcomes: Vec<String> = lines
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["outcome"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(
        outcomes.iter().filter(|o| *o == "root_refused").count(),
        4,
        "{outcomes:?}"
    );
    assert!(
        outcomes.contains(&"stream_error".to_string()) || outcomes.contains(&"abort".to_string()),
        "{outcomes:?}"
    );
    assert!(outcomes.contains(&"upstream_auth".to_string()));
}

/// P-18, P-19, P-20, P-21, P-15: routing by level, attribution, the routable set, gate snapshots,
/// and the redacted view.
#[test]
fn routing_levels_gates_and_redaction() {
    let _wd = init_testkit::watchdog::arm("routing_levels_gates_and_redaction", WD);
    let ok = r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":100,"output":10}}}"#;
    let s = script(&[ok; 12]);
    let cfg = format!(
        "{CONFIG}\n[providers.mock.models.\"mock-unforwarded\"]\nmax_output = 1000\nprices.standard = {{ input = \"0.1\", output = \"0.1\" }}\n"
    );
    let mut models = default_root_models();
    models.push(("mock", "mock-unpriced", 0, 1, 1));
    let rig = Rig::new(
        "r",
        RigOpts {
            script: &s,
            config: &cfg,
            level: 0,
            models,
            ..RigOpts::default()
        },
    );
    let pro = chat("mock-pro", "hard", 100);
    let flash = chat("mock-frontier-1", "easy", 100);

    // P-18: L0 refuses a routed model; nothing reaches the egress.
    let r = post(&rig.sock, "/mock/chat/completions", &pro, &[]);
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (400, Some("request.model_not_routed"))
    );
    assert_eq!(rig.eg().seen_posts().len(), 0);

    // L1 (the Root announces it).
    rig.eg().level.store(1, Ordering::SeqCst);
    rig.eg().epoch.fetch_add(1, Ordering::SeqCst);
    rig.proxy.reload(kernel_proxy::Reload::RootState).unwrap();
    assert_eq!(rig.proxy.status().level, Level::L1);
    // P-20: priced but not forwarded; forwarded but unpriced.
    for m in ["mock-unforwarded", "mock-unpriced"] {
        let r = post(&rig.sock, "/mock/chat/completions", &chat(m, "x", 100), &[]);
        assert_eq!(
            (r.status, r.header("x-init-error")),
            (400, Some("request.model_not_allowed")),
            "{m}"
        );
    }
    // P-19: three requests, one task, two subtasks, mixed models.
    for (b, sub) in [(&flash, "a"), (&pro, "b"), (&flash, "b")] {
        let r = post(
            &rig.sock,
            "/mock/chat/completions",
            b,
            &[("x-init-task", "t-9"), ("x-init-subtask", sub)],
        );
        assert_eq!(r.status, 200, "{}", r.text());
    }
    wait_for(
        || rig.proxy.stats("live").unwrap()["requests_today"] == 3,
        "settle",
    );
    let st = get_json(&rig.sock, "/_init/stats");
    let today = st["spend"]["today_musd"].as_u64().unwrap();
    let by: u64 = st["spend"]["by_model"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v["today_musd"].as_u64().unwrap())
        .sum();
    let by_task: u64 = st["spend"]["task"]["by_model"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_u64().unwrap())
        .sum();
    // Truncation to µUSD happens per entry, so the sums may differ from the total by < 1 µUSD each.
    assert!(
        today.abs_diff(by) <= 2 && today.abs_diff(by_task) <= 2,
        "{today} {by} {by_task}"
    );
    assert_eq!(st["routing"]["routed_today"], 1);
    rig.proxy.flush_traces();
    let lines: Vec<Value> = get(&rig.sock, "/_init/traces?task=t-9")
        .text()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1]["model"], "mock-pro");
    assert_eq!(lines[1]["default_model"], "mock-frontier-1");
    assert_eq!(lines[1]["routed"], true);
    assert_eq!(lines[1]["subtask"], "b");
    assert_eq!(lines[1]["level"], "L1");
    assert!(
        lines[1]["reserved_musd"].as_u64().unwrap() > lines[0]["reserved_musd"].as_u64().unwrap(),
        "priced per model"
    );

    // P-21: a gate opened at L1 keeps L1 routing after a demotion to L0; live does not.
    rig.proxy
        .open_gate("j-000001", GateKind::Promote, 3_000_000)
        .unwrap();
    let probe_sock = rig.home.join("probe/0123456789ab/run/proxy.sock");
    rig.proxy
        .open_scope(OpenScope {
            scope_id: "j-000001/run/r1".into(),
            kind: ScopeKind::Run,
            job_id: Some("j-000001".into()),
            visibility: Visibility::Kernel,
            cap_musd: 250_000,
            label: Label {
                sha: "c".into(),
                boot_reason: "gate".into(),
                model: "mock-frontier-1".into(),
                level: Level::L1,
            },
            socket: probe_sock.clone(),
        })
        .unwrap();
    // The Root demotes to L0 (its figure says so, and supervise relays the level).
    rig.eg().level.store(0, Ordering::SeqCst);
    rig.eg().epoch.fetch_add(1, Ordering::SeqCst);
    rig.proxy.set_ceilings(rig.eg().ceilings());
    rig.proxy.set_level(Level::L0);
    let r = post(
        &probe_sock,
        "/mock/chat/completions",
        &pro,
        &[("x-init-task", "ignored")],
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let gate_req = r.header("x-init-req-id").unwrap().to_string();
    let seen = rig.eg().seen_posts();
    assert_eq!(seen.last().unwrap().header("x-egress-scope"), Some("gate"));
    assert_eq!(
        seen.last().unwrap().header("x-egress-body"),
        Some("0123456789ab")
    );
    let r = post(&rig.sock, "/mock/chat/completions", &pro, &[]);
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (400, Some("request.model_not_routed"))
    );

    // P-15: the kernel-visibility run is not in the agent's view; traces are 404 on gate sockets.
    rig.proxy.quiesce(Duration::from_secs(5));
    rig.proxy.flush_traces();
    let all = get(&rig.sock, "/_init/traces?limit=500").text();
    assert!(!all.contains(&gate_req));
    assert_eq!(
        get(&rig.sock, &format!("/_init/traces/{gate_req}/request")).status,
        404
    );
    assert_eq!(get(&probe_sock, "/_init/traces").status, 404);
    let gs = get_json(&probe_sock, "/_init/stats");
    assert_eq!(gs["scope"]["kind"], "run");
    let sum = rig.proxy.close_scope("j-000001/run/r1").unwrap();
    assert_eq!(sum["by_model"]["mock/mock-pro"]["routed"], 1);
    assert!(
        !probe_sock.exists(),
        "the lane socket is removed by exact path"
    );
    let spend = rig.proxy.close_gate("j-000001").unwrap();
    assert!(spend.spent_pusd > 0 && spend.cost_ref_pusd > 0);
}

/// P-22: soft budgets set by the agent at L1, the bounds, L0, and a lowered ceiling.
#[test]
fn p22_set_budgets_and_clamp() {
    let _wd = init_testkit::watchdog::arm("p22_set_budgets_and_clamp", WD);
    let rig = Rig::new(
        "p22",
        RigOpts {
            day_limit_musd: 5_000_000,
            ..RigOpts::default()
        },
    );
    let b = SoftBudgets {
        day_musd: 4_000_000,
        task_musd: 400_000,
        gate_musd: 2_000_000,
        quick_gate_musd: 500_000,
        probe_musd: 250_000,
        request_max_musd: 1_000_000,
        evolution_share_permille: 600,
    };
    let e = rig.proxy.set_budgets(b).unwrap_err();
    assert_eq!(e.code, "budget.invalid", "refused at L0");
    rig.eg().level.store(1, Ordering::SeqCst);
    rig.eg().epoch.fetch_add(1, Ordering::SeqCst);
    rig.proxy.reload(kernel_proxy::Reload::RootState).unwrap();
    rig.proxy.set_budgets(b).unwrap();
    let j: Value =
        serde_json::from_slice(&std::fs::read(rig.home.join("state/kernel/budget.json")).unwrap())
            .unwrap();
    assert_eq!(j["source"], "agent");
    assert_eq!(j["budgets"]["day_musd"], 4_000_000);
    for bad in [
        SoftBudgets {
            evolution_share_permille: 850,
            ..b
        },
        SoftBudgets {
            gate_musd: 3_000_000,
            ..b
        },
        SoftBudgets {
            day_musd: 4_500_000,
            ..b
        },
    ] {
        assert_eq!(
            rig.proxy.set_budgets(bad).unwrap_err().code,
            "budget.invalid"
        );
    }
    let st = get_json(&rig.sock, "/_init/stats");
    assert_eq!(st["spend"]["budgets"]["source"], "agent");
    // The Root lowers its day ceiling: the soft day clamps at once, an alert, nothing stops.
    rig.alerts.lock().unwrap().clear();
    rig.eg().day_limit_musd.store(2_000_000, Ordering::SeqCst);
    rig.eg().epoch.fetch_add(1, Ordering::SeqCst);
    rig.proxy.set_ceilings(rig.eg().ceilings());
    assert_eq!(rig.proxy.status().day_cap_musd, 1_000_000);
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &chat("mock-frontier-1", "[init:probe] p", 100),
        &[],
    );
    assert_eq!(r.status, 200);
    // A demotion resets agent-set budgets to the config defaults.
    rig.proxy.set_level(Level::L0);
    let j: Value =
        serde_json::from_slice(&std::fs::read(rig.home.join("state/kernel/budget.json")).unwrap())
            .unwrap();
    assert_eq!(j["source"], "config");
}

/// P-24: fleet member lanes with daily shares (L2).
#[test]
fn p24_member_lanes() {
    let _wd = init_testkit::watchdog::arm("p24_member_lanes", WD);
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":10000,"output":100000}}}"#;
    let s = script(&[turn; 6]);
    let rig = Rig::new(
        "p24",
        RigOpts {
            script: &s,
            level: 2,
            ..RigOpts::default()
        },
    );
    let lane = |bid: &str| rig.home.join(format!("run/fleet/{bid}/proxy.sock"));
    let open = |bid: &str| {
        rig.proxy.open_scope(OpenScope {
            scope_id: format!("member:{bid}"),
            kind: ScopeKind::Member(bid.into()),
            job_id: None,
            visibility: Visibility::Agent,
            cap_musd: 500_000,
            label: Label {
                sha: "m".into(),
                boot_reason: "fleet".into(),
                model: "mock-frontier-1".into(),
                level: Level::L2,
            },
            socket: lane(bid),
        })
    };
    open("b1").unwrap();
    open("b2").unwrap();
    // Soft day $5.00 − evolution share $3.00 = $2.00: four $0.50 shares fit, a fifth does not.
    open("b3").unwrap();
    open("b4").unwrap();
    assert_eq!(open("b5").unwrap_err().code, "budget.member");
    // Each request costs 10,000 × 0.30 + 100,000 × 1.20 µUSD = $0.123; the reservation ≤ $0.50 share.
    let body = chat("mock-frontier-1", "work", 200_000);
    let mut n = 0;
    let refused = loop {
        let task = format!("big-{n}"); // one task each: only the member share binds
        let r = post(
            &lane("b1"),
            "/mock/chat/completions",
            &body,
            &[("x-init-task", task.as_str())],
        );
        if r.status != 200 {
            break r;
        }
        n += 1;
        rig.proxy.quiesce(Duration::from_secs(5));
        assert!(n < 6);
    };
    assert_eq!(
        refused.header("x-init-error"),
        Some("budget.member"),
        "{}",
        refused.text()
    );
    assert!(refused.header("retry-after").is_some());
    let r = post(
        &lane("b2"),
        "/mock/chat/completions",
        &chat("mock-frontier-1", "small", 100),
        &[],
    );
    assert_eq!(
        r.status,
        200,
        "the other member is unaffected: {}",
        r.text()
    );
    let seen = rig.eg().seen_posts();
    assert_eq!(
        seen.last().unwrap().header("x-egress-scope"),
        Some("member")
    );
}

/// P-25: `judge` mode keeps no trace store; observations stay in memory.
#[test]
fn p25_judge_mode_writes_nothing() {
    let _wd = init_testkit::watchdog::arm("p25_judge_mode_writes_nothing", WD);
    let rig = Rig::new(
        "p25",
        RigOpts {
            judge: true,
            ..RigOpts::default()
        },
    );
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &chat("mock-frontier-1", "[init:probe] p", 100),
        &[],
    );
    assert_eq!(r.status, 200);
    rig.proxy.quiesce(Duration::from_secs(5));
    assert!(files_under(&rig.home.join("vault/traces")).is_empty());
    assert_eq!(rig.proxy.scope_summary("live").unwrap()["requests"], 1);
}

/// Pre-flight rejections and proxy errors reach the body in the family's shape.
#[test]
fn preflight_and_error_shapes() {
    let _wd = init_testkit::watchdog::arm("preflight_and_error_shapes", WD);
    let rig = Rig::new("preflight", RigOpts::default());
    let r = post(&rig.sock, "/mock/v1/files", &json!({}), &[]);
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (403, Some("route.not_allowed"))
    );
    let r = post(&rig.sock, "/nope/chat/completions", &json!({}), &[]);
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (404, Some("route.unknown_provider"))
    );
    let r = init_testkit::mockllm::client::request_unix(
        &rig.sock,
        &init_testkit::mockllm::MockRequest::post(
            "/mock/chat/completions",
            &[("content-type", "application/json")],
            b"{nope".to_vec(),
        ),
    )
    .unwrap();
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (400, Some("request.bad_json"))
    );
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &json!({"messages": []}),
        &[],
    );
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (400, Some("request.model_not_allowed"))
    );
    let mut b = chat("mock-frontier-1", "x", 100);
    b["service_tier"] = json!("flex");
    let r = post(&rig.sock, "/mock/chat/completions", &b, &[]);
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (400, Some("request.tier_not_allowed"))
    );
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &chat("mock-frontier-1", "x", 2_000_000),
        &[],
    );
    assert_eq!(
        (r.status, r.header("x-init-error")),
        (429, Some("budget.request"))
    );
    assert!(r.header("retry-after").is_none());
    assert_eq!(json_of(&r)["init"]["retryable"], false);
    let r = post(
        &rig.sock,
        "/mock/anthropic/v1/messages",
        &json!({"model": "mock-frontier-1", "max_tokens": 2_000_000, "messages": []}),
        &[],
    );
    assert_eq!(r.status, 429);
    let v = json_of(&r);
    assert_eq!(
        (v["type"].as_str(), v["error"]["type"].as_str()),
        (Some("error"), Some("init_cap_exceeded")),
        "Anthropic shape"
    );
    assert_eq!(rig.eg().seen_posts().len(), 0);
    assert_eq!(get(&rig.sock, "/_init/nope").status, 404);
}

/// A non-streaming (content-length) response is settled from its JSON usage; after a restart the
/// soft-budget totals are rebuilt from the ledger (§3.10.4, §5 "Proxy restarts mid-day").
#[test]
fn non_streaming_and_restart_rebuild() {
    let _wd = init_testkit::watchdog::arm("non_streaming_and_restart_rebuild", WD);
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"ok","think":"","usage":{"input":1000,"cache_read":0,"output":100}}}"#;
    let s = script(&[turn; 4]);
    let mut rig = Rig::new(
        "rebuild",
        RigOpts {
            script: &s,
            ..RigOpts::default()
        },
    );
    let mut b = chat("mock-frontier-1", "x", 500);
    b["stream"] = json!(false);
    let r = post(
        &rig.sock,
        "/mock/chat/completions",
        &b,
        &[("x-init-task", "t-r")],
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.header("content-length").is_some() || r.header("transfer-encoding").is_some());
    // 1,000 × 0.30 + 100 × 1.20 µUSD = 420 µUSD.
    wait_for(
        || rig.proxy.stats("live").unwrap()["spend"]["today_musd"] == 420,
        "non-streaming settlement",
    );
    let st = get_json(&rig.sock, "/_init/stats");
    assert_eq!(st["last"]["outcome"], "ok");
    assert_eq!(st["last"]["finish"], "stop");
    let reserved_before = st["spend"]["reserved_musd"].clone();
    assert_eq!(reserved_before, 0);
    // Restart on the same home: today's totals come back from vault/traces/v1/req/.
    rig.proxy.shutdown(Duration::from_secs(2));
    let cfg: kernel_proxy::ProxyConfig = toml::from_str(CONFIG).unwrap();
    let proxy = kernel_proxy::Proxy::start(
        &cfg,
        kernel_proxy::StartOptions {
            home: rig.home.clone(),
            egress_sock: rig.home.join("root/run/egress.sock"),
            live_socket: None,
            label: Label::default(),
            judge: false,
        },
        std::sync::Arc::new(|_a: kernel_proxy::Alert| {}),
    )
    .unwrap();
    rig.proxy = proxy;
    let st = get_json(&rig.sock, "/_init/stats");
    assert_eq!(st["spend"]["today_musd"], 420);
    assert_eq!(st["requests_today"], 1);
    assert_eq!(
        st["spend"]["by_model"]["mock/mock-frontier-1"]["today_musd"],
        420
    );
}

/// M7-G07 (GATE-V2b): per-run caps on actuals on a gate `run` lane (m7-design.md §3.1.3, §3.2.8):
/// the turn after `max_turns` is refused, `compaction`/`probe` requests are not turns, the output and
/// input token caps refuse the request after the one that reached them, a first request that
/// reserves far more than the token cap is never refused, and the lane summary names the cap and
/// the served model.
#[test]
fn m7_g07_run_caps_on_actuals() {
    let _wd = init_testkit::watchdog::arm("m7_g07_run_caps_on_actuals", WD);
    let turn = r#"{"v":1,"kind":"turn","reply":{"text":"ok","stop":"end_turn","usage":{"input":1000,"cache_read":800,"output":50}}}"#;
    let turns: Vec<&str> = std::iter::repeat_n(turn, 16).collect();
    let s = script(&turns);
    let rig = Rig::new("g07", RigOpts { script: &s, ..RigOpts::default() });
    rig.proxy.open_gate("j-000007", GateKind::Promote, 3_000_000).unwrap();
    let open = |name: &str, probe: &str| -> std::path::PathBuf {
        let sock = rig.home.join(format!("probe/{probe}/run/proxy.sock"));
        rig.proxy
            .open_scope(OpenScope {
                scope_id: format!("j-000007/run/{name}"),
                kind: ScopeKind::Run,
                job_id: Some("j-000007".into()),
                visibility: Visibility::Agent,
                cap_musd: 500_000,
                label: Label { sha: "c".into(), boot_reason: "cold".into(), model: "mock-frontier-1".into(), level: Level::L0 },
                socket: sock.clone(),
            })
            .unwrap();
        sock
    };
    let turn_h = [("x-init-purpose", "turn")];
    let small = chat("mock-frontier-1", "go", 1000);

    // Turns: 2 turns + 1 compaction + 1 turn reach max_turns 3; the next turn is refused.
    let a = open("c-t1-r1", "a");
    rig.proxy
        .set_run_caps("j-000007/run/c-t1-r1", kernel_proxy::RunCaps { max_turns: Some(3), ..Default::default() })
        .unwrap();
    for h in [&turn_h[..], &turn_h[..], &[("x-init-purpose", "compaction")][..], &turn_h[..]] {
        let r = post(&a, "/mock/chat/completions", &small, h);
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let r = post(&a, "/mock/chat/completions", &small, &turn_h);
    assert_eq!((r.status, r.header("x-init-cap"), r.header("x-init-error")), (429, Some("turns"), Some("budget.turns")));
    let j = json_of(&r);
    assert_eq!(j["error"]["type"], "init_cap_exceeded");
    assert_eq!((j["init"]["limit"].as_u64(), j["init"]["value"].as_u64(), j["init"]["retryable"].as_bool()), (Some(3), Some(3), Some(false)));
    rig.proxy.quiesce(Duration::from_secs(5));
    let sum = rig.proxy.close_scope("j-000007/run/c-t1-r1").unwrap();
    assert_eq!((sum["requests"].as_u64(), sum["turns"].as_u64()), (Some(4), Some(3)));
    assert_eq!((sum["capped"].as_bool(), sum["caps_hit"].as_str()), (Some(true), Some("turns")));
    assert!(sum["model_served"].as_str().is_some_and(|m| !m.is_empty()), "{sum}");
    assert_integers_only(&sum, "summary");

    // Output tokens: the first request reserves 65,536 output tokens against a 120-token cap and is
    // admitted; 50 + 50 < 120 admits a third; 150 ≥ 120 refuses the fourth.
    let b = open("c-t2-r1", "b");
    rig.proxy
        .set_run_caps("j-000007/run/c-t2-r1", kernel_proxy::RunCaps { max_output_tokens: Some(120), ..Default::default() })
        .unwrap();
    let big = chat("mock-frontier-1", "go", 65_536);
    for body in [&big, &small, &small] {
        let r = post(&b, "/mock/chat/completions", body, &turn_h);
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let r = post(&b, "/mock/chat/completions", &small, &turn_h);
    assert_eq!((r.status, r.header("x-init-cap")), (429, Some("tokens_out")));
    assert_eq!(json_of(&r)["init"]["value"].as_u64(), Some(150));
    rig.proxy.quiesce(Duration::from_secs(5));
    let sum = rig.proxy.close_scope("j-000007/run/c-t2-r1").unwrap();
    assert_eq!((sum["requests"].as_u64(), sum["caps_hit"].as_str()), (Some(3), Some("tokens_out")));

    // Input tokens, cached included: 1,000 per request; 3,000 ≥ 2,500 refuses the fourth.
    let c = open("c-t3-r1", "c");
    rig.proxy
        .set_run_caps("j-000007/run/c-t3-r1", kernel_proxy::RunCaps { max_input_tokens: Some(2_500), ..Default::default() })
        .unwrap();
    for _ in 0..3 {
        let r = post(&c, "/mock/chat/completions", &small, &turn_h);
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let r = post(&c, "/mock/chat/completions", &small, &turn_h);
    assert_eq!((r.status, r.header("x-init-cap")), (429, Some("tokens_in")));
    rig.proxy.quiesce(Duration::from_secs(5));
    assert_eq!(rig.proxy.close_scope("j-000007/run/c-t3-r1").unwrap()["caps_hit"], "tokens_in");

    // A lane without caps is not refused by them and names no cap; caps apply to run lanes only.
    let d = open("c-t4-r1", "d");
    for _ in 0..4 {
        assert_eq!(post(&d, "/mock/chat/completions", &small, &turn_h).status, 200);
    }
    rig.proxy.quiesce(Duration::from_secs(5));
    let sum = rig.proxy.close_scope("j-000007/run/c-t4-r1").unwrap();
    assert_eq!((sum["turns"].clone(), sum["caps_hit"].clone(), sum["capped"].as_bool()), (Value::Null, Value::Null, Some(false)));
    let e = rig.proxy.set_run_caps("live", kernel_proxy::RunCaps { max_turns: Some(1), ..Default::default() }).unwrap_err();
    assert_eq!(e.code, "scope.bad_kind");
    let e = rig.proxy.set_run_caps("nope", kernel_proxy::RunCaps::default()).unwrap_err();
    assert_eq!(e.code, "scope.unknown");
    rig.proxy.close_gate("j-000007").unwrap();
    let home = rig.home.clone();
    drop(rig);
    std::fs::remove_dir_all(&home).unwrap();
}

/// X5 (ADR-011 §5.2): the body's `x-init-caps` reaches the Root egress as `x-egress-caps`, verbatim
/// (the Root validates the ids); the `x-init-caps` original never leaves the proxy; a header over
/// 1 KiB, or none, forwards nothing.
#[test]
fn x5_init_caps_forwarded_as_egress_caps() {
    let _wd = init_testkit::watchdog::arm("x5_init_caps_forwarded_as_egress_caps", WD);
    let t = r#"{"v":1,"kind":"turn","reply":{"text":"ok","stop":"end_turn","usage":{"input":10,"output":2}}}"#;
    let s = script(&[t, t, t]);
    let rig = Rig::new("x5caps", RigOpts { script: &s, ..RigOpts::default() });
    let body = chat("mock-frontier-1", "hi", 100);
    let long = format!("mem/{}", "a".repeat(kernel_proxy::server::MAX_CAPS_HEADER));
    for caps in [Some("mem/notes,loop/plan2"), Some(long.as_str()), None] {
        let mut h = vec![("x-init-session", "20261020-010203-abcdef"), ("x-init-purpose", "turn")];
        if let Some(c) = caps {
            h.push(("x-init-caps", c));
        }
        let r = post(&rig.sock, "/mock/chat/completions", &body, &h);
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let seen = rig.eg().seen_posts();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].header("x-egress-caps"), Some("mem/notes,loop/plan2"));
    assert_eq!(seen[1].header("x-egress-caps"), None, "over 1 KiB: not forwarded");
    assert_eq!(seen[2].header("x-egress-caps"), None);
    for e in &seen {
        assert!(!e.headers.iter().any(|(k, _)| k.starts_with("x-init-")), "x-init-* leaked: {:?}", e.headers);
    }
    let home = rig.home.clone();
    drop(rig);
    std::fs::remove_dir_all(&home).unwrap();
}
