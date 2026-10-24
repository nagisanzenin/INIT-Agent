//! WP26 (tui.md §9 Q2; supervise.md §4.6): the owner `state` frame carries `model` and
//! `task_musd`, and the kernel also sends it after each `turn_end` of the primary. The fake Root
//! is in-test (tests/common); the body is `sv-testbody`; no proxy runs, so `task_musd` is null.

mod common;

use common::*;
use init_proto::root_link::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

const T: Duration = Duration::from_secs(30);
/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(30);

fn homes_root() -> PathBuf {
    let r = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp26-kernel");
    std::fs::create_dir_all(&r).unwrap();
    std::fs::canonicalize(r).unwrap()
}

/// `make_home`'s layout under `target/test-homes/wp26-kernel/`, with `[model] id = "wp26-model"`.
fn home(name: &str) -> PathBuf {
    let h = homes_root().join(format!("{name}-{}", kernel_supervise::sys::random_hex(3)));
    for d in ["bin", "kernel", "state/kernel", "state/run", "state/sessions", "work", "vault", "cache", "run", "root", "lineage"] {
        std::fs::create_dir_all(h.join(d)).unwrap();
    }
    std::fs::copy(testbody(), h.join("bin/init-body")).unwrap();
    std::fs::hard_link(h.join("bin/init-body"), h.join("bin/init-body.lkg")).unwrap();
    let cfg = "[proxy]\ntest_mode = true\n\n[supervise]\ngit = \"/usr/bin/git\"\nready_timeout_s = 10\nsettle_s = 1\nbackoff_initial_ms = 50\nbackoff_max_ms = 400\nexit_grace_s = 3\n\n[model]\nprovider = \"mock\"\nid = \"wp26-model\"\n";
    std::fs::write(h.join("kernel/config.toml"), cfg).unwrap();
    h
}

#[test]
fn state_follows_turn_end_with_model_and_task_musd() {
    let _wd = init_testkit::watchdog::arm("state_follows_turn_end_with_model_and_task_musd", WD);
    let home = home("state-turn-end");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    assert!(fr.owner_line("hi there").is_ok());

    // The body sends text, turn_end, then state{busy:false}; the kernel relays the turn_end and
    // follows it at once with a state frame.
    let frames = fr.wait_for("state after turn_end", T, |s| {
        let out: Vec<_> = s.events.iter().filter(|e| e.ev == "owner.out").map(|e| e.data["frame"].clone()).collect();
        let reply = out.iter().position(|f| f["t"] == "display" && f["text"] == "echo: hi there")?;
        let end = reply + 1 + out[reply + 1..].iter().position(|f| f["t"] == "display" && f["kind"] == "turn_end")?;
        (out.len() > end + 1).then_some((out, end))
    });
    let (out, end) = frames;
    let st = &out[end + 1];
    assert_eq!(st["t"], "state", "{out:?}");
    assert_eq!(st["model"], "wp26-model");
    assert!(st.as_object().unwrap().contains_key("task_musd") && st["task_musd"].is_null(), "no proxy: null");
    assert!(st.get("evolution_checks_today_musd").is_none(), "no proxy must not become zero evolution cost");
    assert_eq!((st["slot"].is_string(), st["busy"].is_boolean(), st["queued"].is_u64()), (true, true, true));
    // Every state frame has the two fields, null or not.
    assert!(out.iter().filter(|f| f["t"] == "state").all(|f| f["model"] == "wp26-model" && f.get("task_musd").is_some()));

    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 3000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    assert!(home.starts_with(homes_root()) && home != homes_root());
    let _ = std::fs::remove_dir_all(&home);
}

/// The newest `state` frame among `owner.out` so far.
fn last_state(out: &[serde_json::Value]) -> Option<serde_json::Value> {
    out.iter().rev().find(|f| f["t"] == "state").cloned()
}

fn seq_list(v: &serde_json::Value) -> Vec<u64> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default()
}

/// STEER (supervise.md §4.6): after each body ack the kernel publishes a `state` frame whose
/// `steer_seen` names the input just logged and whose `steer_pending` no longer has it; a held input
/// stays pending until the owner retracts it, when the body answers `retracted{ok}` and the input
/// moves to `steer_retracted` (never redelivered); retracting a line already read says so.
#[test]
fn state_reports_steers_pending_seen_and_retracted() {
    let _wd = init_testkit::watchdog::arm("state_reports_steers_pending_seen_and_retracted", WD);
    let home = home("steer-state");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    let echo_seq = |text: &str| {
        fr.wait_for(&format!("echo {text}"), T, |s| {
            s.events.iter().filter(|e| e.ev == "owner.out").map(|e| &e.data["frame"]).find(|f| f["t"] == "echo" && f["text"] == text).and_then(|f| f["seq"].as_u64())
        })
    };
    let state_where = |what: &str, f: &dyn Fn(&serde_json::Value) -> bool| {
        fr.wait_for(what, T, |s| {
            let out: Vec<_> = s.events.iter().filter(|e| e.ev == "owner.out").map(|e| e.data["frame"].clone()).collect();
            last_state(&out).filter(|st| f(st))
        })
    };
    let notice = |needle: String| {
        fr.wait_for(&needle.clone(), T, |s| {
            s.events.iter().any(|e| e.ev == "owner.out" && e.data["frame"]["text"].as_str().is_some_and(|t| t.contains(&needle))).then_some(())
        })
    };

    // an ordinary line: acked at once by sv-testbody, so the next state shows it seen
    assert!(fr.owner_line("hello").is_ok());
    let a = echo_seq("hello");
    let st = state_where("state with steer_seen", &|st| seq_list(&st["steer_seen"]).contains(&a));
    assert!(!seq_list(&st["steer_pending"]).contains(&a), "{st}");

    // a held line stays pending (the body neither logs nor acks it) ...
    assert!(fr.owner_line("!hold").is_ok());
    let b = echo_seq("!hold");
    assert!(fr.owner_line("again").is_ok());
    let c = echo_seq("again");
    let st = state_where("state after the next ack", &|st| seq_list(&st["steer_seen"]).contains(&c));
    assert_eq!(seq_list(&st["steer_pending"]), vec![b], "{st}");
    // ... until it is taken back: the body answers ok, the kernel closes it for good
    let retract = |seq: u64| {
        let qid = format!("r-{seq}-{}", kernel_supervise::sys::random_hex(2));
        fr.request(&RootRequest::OwnerIn(OwnerInArgs { qid, frame: OwnerInFrame::Retract { seq } }), T).expect("answered")
    };
    assert!(retract(b).is_ok());
    let st = state_where("state with steer_retracted", &|st| seq_list(&st["steer_retracted"]).contains(&b));
    assert!(seq_list(&st["steer_pending"]).is_empty() && !seq_list(&st["steer_seen"]).contains(&b), "{st}");
    notice(format!("taken back: message {b}"));
    // a line the agent already read cannot be taken back, and the owner is told
    assert!(retract(a).is_ok());
    notice(format!("not taken back: message {a}"));
    // the inbox on disk agrees: nothing is left to redeliver
    let inbox = std::fs::read_to_string(home.join("state/kernel/inbox.jsonl")).unwrap();
    assert!(inbox.contains(&format!("\"seq\":{b},\"t\":\"ack\"")), "{inbox}");

    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 3000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    assert!(home.starts_with(homes_root()) && home != homes_root());
    let _ = std::fs::remove_dir_all(&home);
}

/// K-LINE: the text sv-testbody's `!big` sends (tests/fixtures/testbody.rs `big_text`).
fn big_text(n: usize) -> String {
    const P: &str = "lorem \"q\" \\ \t\n\u{1} é 😀 ipsum ";
    let mut s = P.repeat(n / P.len() + 1);
    let mut cut = n.min(s.len());
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s
}

/// The `owner.out` frames between the echo of `line` and the `turn_end` of its turn.
fn turn_frames(fr: &FakeRoot, line: &str) -> Vec<serde_json::Value> {
    fr.wait_for(&format!("the turn of {}", &line[..line.len().min(40)]), T, |s| {
        let out: Vec<_> = s.events.iter().filter(|e| e.ev == "owner.out").map(|e| &e.data["frame"]).collect();
        let start = out.iter().position(|f| f["t"] == "echo" && f["text"] == line)?;
        let end = start + out[start..].iter().position(|f| f["t"] == "display" && f["kind"] == "turn_end")?;
        Some(out[start + 1..end].iter().map(|f| (*f).clone()).collect())
    })
}

/// K-LINE: display frames as large as a body line (1 MiB) would make the kernel's `owner.out`
/// line pass the Root's 1 MiB limit, and the Root would end the link, SIGKILL the kernel and count
/// a crash. The kernel splits long text (the owner gets every byte) and cuts a long tool result
/// with a marker, so no line passes the limit, the link stays up and nothing restarts.
#[test]
fn oversized_displays_keep_the_link_up() {
    let _wd = init_testkit::watchdog::arm("oversized_displays_keep_the_link_up", Duration::from_secs(90));
    let home = home("long-lines");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    let spawns = fr.spawn_count();

    // 2 MiB of text, sent by the body in frames of about 1 MiB each.
    let n = 2 * 1024 * 1024;
    let line = format!("!big text {n}");
    assert!(fr.owner_line(&line).is_ok());
    let frames = turn_frames(&fr, &line);
    let texts: Vec<&str> = frames.iter().filter(|f| f["t"] == "display" && f["kind"] == "text").map(|f| f["text"].as_str().unwrap()).collect();
    assert!(texts.len() > 8, "split into pieces: {}", texts.len());
    assert!(texts.concat() == big_text(n), "the owner gets every byte, in order");

    // A tool result as large as a body line: its start is kept, with a marker naming the size and
    // the session log.
    let line = format!("!big tool_end {n}");
    assert!(fr.owner_line(&line).is_ok());
    let frames = turn_frames(&fr, &line);
    let end = frames.iter().find(|f| f["kind"] == "tool_end").expect("tool_end relayed");
    assert_eq!((end["call_id"].as_str(), end["name"].as_str(), end["ok"].as_bool()), (Some("c-big"), Some("bash"), Some(true)));
    let shown = end["text"].as_str().unwrap();
    let at = shown.find(" [kernel: showing ").expect("a visible marker");
    assert!(at + 4 >= kernel_supervise::ownerline::KEEP && big_text(n).starts_with(&shown[..at]), "the start of the result is kept");
    assert!(shown[at..].contains("session log"), "{}", &shown[at..]);

    // A `detail` field that alone passes the line: dropped, and the summary says so.
    let line = "!bigdetail 1048000";
    assert!(fr.owner_line(line).is_ok());
    let frames = turn_frames(&fr, line);
    let end = frames.iter().find(|f| f["call_id"] == "c-detail").expect("tool_end relayed");
    assert!(end.get("detail").is_none());
    assert!(end["text"].as_str().unwrap().starts_with("exit 0 [kernel: dropped the fields detail ("), "{end}");

    // The link is up, nothing restarted, and no line came near the Root's limit.
    assert!(fr.owner_line("after").is_ok());
    assert!(turn_frames(&fr, "after").iter().any(|f| f["text"] == "echo: after"));
    {
        let g = fr.seen.0.lock().unwrap();
        assert!(g.max_line <= kernel_supervise::ownerline::LINE_BUDGET, "longest line {} bytes", g.max_line);
        assert!(g.exits.is_empty() && g.kills.is_empty(), "exits {:?} kills {:?}", g.exits, g.kills);
    }
    assert_eq!(fr.spawn_count(), spawns, "no body restart");

    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 3000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    assert!(home.starts_with(homes_root()) && home != homes_root());
    let _ = std::fs::remove_dir_all(&home);
}

/// K-LINE: the Root also ends the link past 10,000 control frames a minute, and a body streams one
/// display frame per model delta. 20,000 deltas at once reach the owner whole, in far fewer frames
/// than the limit, and the link stays up.
#[test]
fn a_delta_flood_is_paced_without_losing_text() {
    let _wd = init_testkit::watchdog::arm("a_delta_flood_is_paced_without_losing_text", Duration::from_secs(90));
    let home = home("delta-flood");
    let fr = FakeRoot::new(&home, Opts::default());
    let (_h, code) = fr.start_in_process();
    fr.wait_for("ready", T, |s| s.ready.then_some(()));
    let before = fr.owner_out().len();
    let n = 20_000;
    let line = format!("!flood {n}");
    assert!(fr.owner_line(&line).is_ok());
    let frames = turn_frames(&fr, &line);
    let got: String = frames.iter().filter(|f| f["t"] == "display" && f["kind"] == "text").map(|f| f["text"].as_str().unwrap()).collect();
    let want: String = (0..n).map(|i| format!("<{i}>")).collect();
    assert!(got == want, "every delta, in order ({} of {} bytes)", got.len(), want.len());
    let sent = fr.owner_out().len() - before;
    let limit = (kernel_supervise::ownerline::BURST + 60 * kernel_supervise::ownerline::RATE) as usize;
    assert!(sent < limit, "{sent} owner.out frames for {n} deltas");

    assert!(fr.owner_line("after").is_ok());
    assert!(turn_frames(&fr, "after").iter().any(|f| f["text"] == "echo: after"));
    let r = fr.request(&RootRequest::Stop(StopArgs { reason: "test".into(), grace_ms: 3000 }), T).unwrap();
    assert!(r.is_ok());
    assert_eq!(code.recv_timeout(T).unwrap(), 0);
    assert!(home.starts_with(homes_root()) && home != homes_root());
    let _ = std::fs::remove_dir_all(&home);
}
