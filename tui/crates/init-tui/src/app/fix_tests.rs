//! TUI-FIX (v0.5.1): the defects the two v0.5.0 user sessions found (docs/user-sessions/), each
//! driven through the real `App` on one end of a socket pair: the frames it sends are read back from
//! the other end, and nothing reaches a Root (the Root home does not exist).

use crate::app::{App, Opts, PANIC_ASK, PANIC_HINT};
use crate::app::pick::DISMISSED_HINT;
use crate::conn::Ev;
use crate::model::Kind;
use crate::render::live::{self, Ui};
use crate::setup::Op;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::num::NonZeroU16;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// An App attached to one end of a socket pair, before `replay_end`.
fn attached() -> (App, BufReader<UnixStream>, mpsc::Receiver<Ev>) {
    let (tx, rx) = mpsc::channel();
    let (ptx, _prx) = mpsc::channel();
    let o = Opts { root_home: PathBuf::from("/nonexistent/i5-tui-fix"), replay: true, verbose: false, latency: false, no_color: false,
        cwd: None, full: false, workspace: false };
    let mut a = App::new(o, tx, ptx, 100, 30);
    let (w, r) = UnixStream::pair().unwrap();
    r.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    a.handle(Ev::Attached { cn: 0, writer: w, epoch: None, started_at: None });
    // the first providers.list said "ready": no first-run setup
    a.handle(Ev::Setup { op: Op::List, result: Ok(json!({"ready": true, "selected": {"provider": "deepseek", "model": "deepseek-flash"},
        "providers": [{"id": "deepseek", "key": "present", "tested": "live"}],
        "models": [{"provider": "deepseek", "model": "deepseek-flash", "selected": true, "tested": "live", "key": "present", "input_usd": "0.30", "output_usd": "1.20"}],
        "catalog": []})) });
    assert!(!a.setup.busy());
    (a, BufReader::new(r), rx)
}

/// Attached, the replay over, the greeting seen: the attach connection is live.
fn live() -> (App, BufReader<UnixStream>, mpsc::Receiver<Ev>) {
    let (mut a, r, rx) = attached();
    frame(&mut a, json!({"t": "replay_end"}));
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "root", "text": "[root] attached: slot running, level L0, 0 queued.", "ts": "x"}));
    assert!(a.feed.is_live());
    (a, r, rx)
}

fn frame(a: &mut App, v: Value) {
    a.handle(Ev::Frame { cn: 0, raw: v.to_string(), v, at: Instant::now() });
}
fn key(a: &mut App, c: KeyCode) {
    a.handle(Ev::Key(KeyEvent::new(c, KeyModifiers::NONE), Instant::now()));
}
fn typed(a: &mut App, s: &str) {
    for c in s.chars() {
        key(a, KeyCode::Char(c));
    }
}
fn enter(a: &mut App, s: &str) {
    typed(a, s);
    key(a, KeyCode::Enter);
}
/// The frames the App wrote on the attach connection since the last call.
fn sent(r: &mut BufReader<UnixStream>) -> Vec<Value> {
    let mut v = Vec::new();
    let mut l = String::new();
    while r.read_line(&mut l).is_ok_and(|n| n > 0) {
        v.push(serde_json::from_str(&l).unwrap());
        l.clear();
    }
    v
}
fn blocks(a: &App) -> Vec<String> {
    a.t.blocks.iter().map(|b| match &b.kind {
        Kind::Owner { text, .. } => format!("› {text}"),
        Kind::Info(t) => format!("· {t}"),
        Kind::Notice { text, .. } => format!("! {text}"),
        k => format!("{k:?}").chars().take(40).collect(),
    }).collect()
}
fn screen(a: &App) -> String {
    let now = Instant::now();
    let ui = Ui { t: &a.t, tray: &a.tray, ed: &a.ed, info: &a.info, verbose: false, overlay: false, hint: None, now, now_ms: 0, no_color: false, spin: 0 };
    live::region(&ui, 100, 30).rows.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
}
/// What a terminal shows after the App's bytes (vt100, 100×30).
fn terminal(bytes: &[u8]) -> String {
    let mut p = vt100::Parser::new(NonZeroU16::new(30).unwrap(), NonZeroU16::new(100).unwrap(), 0);
    p.process(bytes);
    p.screen().contents()
}

/// User session v0.5.0-new, blocker 3: `/` then Tab completed to `/panic`, which then ran with no
/// confirmation. Tab now lands on a safe command, and `/panic` says what it does and waits for the
/// word `panic`; Esc, or any other line, cancels and sends nothing.
#[test]
fn panic_asks_first_and_tab_never_lands_on_it() {
    let (mut a, mut r, _rx) = live();
    typed(&mut a, "/");
    key(&mut a, KeyCode::Tab);
    assert_eq!(a.ed.text(), "/usage", "Tab on a bare / is a safe command");
    a.ed.clear();
    // /panic: nothing is sent yet; the screen says what it does, how to confirm and how to cancel
    enter(&mut a, "/panic");
    assert!(sent(&mut r).is_empty(), "no panic frame before the confirmation");
    assert!(a.panic_ask);
    let b = blocks(&a);
    assert_eq!(b[b.len() - 2..], ["› /panic".to_string(), format!("! {PANIC_ASK}")]);
    for want in ["kills every agent process", "autonomy level one step", "audited-good", "Type panic and press Enter", "Esc or any other line cancels"] {
        assert!(PANIC_ASK.contains(want), "{want}");
    }
    let mut out = Vec::new();
    a.render(&mut out).unwrap();
    assert!(terminal(&out).contains(PANIC_HINT), "the hint line asks: {}", terminal(&out));
    // Esc cancels
    key(&mut a, KeyCode::Esc);
    assert!(!a.panic_ask && sent(&mut r).is_empty());
    assert_eq!(blocks(&a).last().unwrap(), "· /panic cancelled: nothing was stopped");
    // any other line cancels, is not sent, and comes back to the box
    enter(&mut a, "/panic");
    enter(&mut a, "hello");
    assert!(!a.panic_ask && sent(&mut r).is_empty(), "neither a panic nor the line");
    assert_eq!(a.ed.text(), "hello");
    a.ed.clear();
    // Enter twice is not a confirmation: an empty box sends nothing and keeps asking
    enter(&mut a, "/panic");
    key(&mut a, KeyCode::Enter);
    assert!(a.panic_ask && sent(&mut r).is_empty());
    // the word confirms: exactly one panic frame, never a line
    enter(&mut a, "panic");
    assert_eq!(sent(&mut r), vec![json!({"t": "panic"})]);
    assert!(!a.panic_ask);
}

/// Pre-release review of v0.5.1 (HIGH): an agent `ask` that arrived while `/panic` waited for its word
/// cancelled the confirmation and took the keys, so the owner typing `panic` ended up in the agent's
/// picker and nothing stopped. The agent can ask at any time; its question now waits until the
/// confirmation is over, sent or cancelled, then shows. Setup's answers (a picker, the hidden key
/// field) and a reconnect do not take the box from it either.
#[test]
fn nothing_preempts_a_pending_panic_confirmation() {
    let (mut a, mut r, _rx) = live();
    enter(&mut a, "/panic");
    typed(&mut a, "pan");
    frame(&mut a, ask()); // the agent asks while the owner types the word
    assert!(a.panic_ask, "the confirmation still waits");
    assert!(a.ed.picker.is_none(), "the question waits for it");
    assert_eq!(a.ed.text(), "pan");
    let mut out = Vec::new();
    a.render(&mut out).unwrap();
    assert!(terminal(&out).contains(PANIC_HINT), "{}", terminal(&out));
    typed(&mut a, "ic");
    key(&mut a, KeyCode::Enter);
    assert_eq!(sent(&mut r), vec![json!({"t": "panic"})], "the stop goes out");
    assert!(!a.panic_ask);
    assert_eq!(a.ed.picker.as_ref().map(|p| p.key.as_str()), Some("Run it?"), "then the question shows");
    key(&mut a, KeyCode::Esc);
    // a setup answer that arrives meanwhile (`/model`'s list) waits too; then its picker, then the question
    enter(&mut a, "/model");
    assert!(a.setup.busy());
    enter(&mut a, "/panic");
    frame(&mut a, ask());
    a.handle(Ev::Setup { op: Op::List, result: Ok(json!({"ready": true, "selected": {"provider": "deepseek", "model": "deepseek-flash"},
        "providers": [{"id": "deepseek", "key": "present", "tested": "live"}],
        "models": [{"provider": "deepseek", "model": "deepseek-flash", "selected": true, "tested": "live", "key": "present", "input_usd": "0.30", "output_usd": "1.20"},
                   {"provider": "deepseek", "model": "deepseek-v4-pro", "selected": false, "tested": "live", "key": "present", "input_usd": "1.32", "output_usd": "3.96"}],
        "catalog": []})) });
    assert!(a.panic_ask && a.ed.picker.is_none() && a.secret.is_none());
    enter(&mut a, "panic");
    assert_eq!(sent(&mut r), vec![json!({"t": "panic"})]);
    assert_eq!(a.ed.picker.as_ref().map(|p| p.key.as_str()), Some("model"), "setup's picker first");
    key(&mut a, KeyCode::Esc);
    assert_eq!(a.ed.picker.as_ref().map(|p| p.key.as_str()), Some("Run it?"), "then the agent's question");
    key(&mut a, KeyCode::Esc);
    // cancelled: the question shows then; a reconnect meanwhile does not open it
    enter(&mut a, "/panic");
    frame(&mut a, ask());
    let (w, r2) = UnixStream::pair().unwrap();
    r2.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let cn = a.cn;
    a.handle(Ev::Attached { cn, writer: w, epoch: None, started_at: None });
    assert!(a.panic_ask && a.ed.picker.is_none(), "a reconnect does not pre-empt it");
    key(&mut a, KeyCode::Esc);
    assert!(!a.panic_ask);
    assert_eq!(a.ed.picker.as_ref().map(|p| p.key.as_str()), Some("Run it?"));
    let mut r2 = BufReader::new(r2);
    assert!(sent(&mut r2).is_empty() && sent(&mut r).is_empty(), "nothing was stopped or sent");
}

/// Pre-release review of v0.5.1 (MEDIUM): only `panic` confirmed. `/panic` again, or Up+Enter from
/// the history, cancelled and put `/panic` back in the box, where Enter asked again, without end.
/// `panic` and `/panic`, trimmed and in any case, confirm; any other line still cancels.
#[test]
fn panic_again_confirms_the_panic() {
    let (mut a, mut r, _rx) = live();
    enter(&mut a, "/panic");
    key(&mut a, KeyCode::Up); // the history recalls the /panic just typed
    assert_eq!(a.ed.text(), "/panic");
    key(&mut a, KeyCode::Enter);
    assert_eq!(sent(&mut r), vec![json!({"t": "panic"})]);
    assert!(!a.panic_ask && a.ed.is_empty());
    for word in ["/panic", " /PANIC ", "PANIC", "  Panic "] {
        enter(&mut a, "/panic");
        enter(&mut a, word);
        assert_eq!(sent(&mut r), vec![json!({"t": "panic"})], "{word:?}");
        assert!(!a.panic_ask && a.ed.is_empty(), "{word:?}");
    }
    for other in ["panic now", "/panic!", "pan"] {
        enter(&mut a, "/panic");
        enter(&mut a, other);
        assert!(sent(&mut r).is_empty() && !a.panic_ask, "{other:?}");
        assert_eq!(a.ed.text(), other);
        a.ed.clear();
    }
    assert!(crate::app::panic_confirms("panic\n") && !crate::app::panic_confirms("/panic panic"));
}

fn ask() -> Value {
    json!({"t": "display", "kind": "info", "src": "kernel", "boot_seq": 1, "text": "The agent asks: Run it?",
        "ask": {"question": "Write hello.py and run it?", "header": "Run it?", "multi": false,
            "options": [{"label": "Write it and run it", "recommended": true}, {"label": "Just write it"}]}})
}

/// User session v0.5.0-new: after a relaunch the replay re-opened the already-answered "agent asks ·
/// Run it?" picker, and "waiting on you" stayed after Esc. A question answered in the replay stays
/// answered; one still unanswered still opens; Esc ends the waiting state and says what it did.
#[test]
fn an_answered_question_stays_answered_after_a_relaunch() {
    // the relaunch: the attach replay holds the question, its answer (the kernel's echo) and the work after it
    let (mut a, _r, _rx) = attached();
    frame(&mut a, ask());
    frame(&mut a, json!({"t": "display", "kind": "turn_end", "src": "kernel", "boot_seq": 1, "text": "tokens: in 900 (cached 800), out 20"}));
    frame(&mut a, json!({"t": "echo", "src": "kernel", "text": "Write it and run it", "seq": 3}));
    frame(&mut a, json!({"t": "display", "kind": "text", "src": "kernel", "boot_seq": 1, "text": "Done: hello.py is written."}));
    frame(&mut a, json!({"t": "replay_end"}));
    assert!(a.ed.picker.is_none(), "the answered question does not come back");
    assert!(a.ed.waiting.is_none());
    assert!(!screen(&a).contains("waiting on you"), "{}", screen(&a));
    let b = blocks(&a);
    let q = b.iter().position(|x| x == "· agent asked · Run it?: Write hello.py and run it? · answered").unwrap_or_else(|| panic!("{b:#?}"));
    assert_eq!(b[q + 1], "› Write it and run it");
    // a question with no answer in the replay is still waiting, so it opens
    let (mut a, _r, _rx) = attached();
    frame(&mut a, ask());
    frame(&mut a, json!({"t": "replay_end"}));
    assert!(a.ed.picker.is_some() && screen(&a).contains("waiting on you"));
    // Esc dismisses it, and the waiting state with it; the hint says once what that means
    key(&mut a, KeyCode::Esc);
    assert!(a.ed.picker.is_none() && a.ed.waiting.is_none());
    assert!(!screen(&a).contains("waiting on you"), "{}", screen(&a));
    assert_eq!(a.hint.as_ref().map(|h| h.0.as_str()), Some(DISMISSED_HINT));
    assert!(blocks(&a).last().unwrap().ends_with("· dismissed"));
}

/// User session v0.5.0-new: "Ctrl-D quit silently and left half an input box drawn in my terminal". On
/// the way out the live region is erased and one line says what keeps running and how to come back.
#[test]
fn quitting_leaves_no_input_box_and_says_what_keeps_running() {
    let (mut a, mut r, _rx) = live();
    a.info.on_egress(json!({"money": {"enforced": false, "policy": "observe", "total_spent_musd": 45_850, "day_spent_musd": 45_850}}));
    a.info.on_status(json!({"slot": "running", "mode": "normal", "level": 0, "kernel": {"ready": true}}), Instant::now());
    frame(&mut a, json!({"t": "display", "kind": "text", "src": "kernel", "boot_seq": 1, "text": "Hello from the agent.\n"}));
    typed(&mut a, "half a thought");
    let mut out = Vec::new();
    a.render(&mut out).unwrap();
    let before = terminal(&out);
    assert!(before.contains('┌') && before.contains("half a thought"), "{before}");
    a.ed.clear();
    a.handle(Ev::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), Instant::now()));
    assert_eq!(a.exit, Some(0));
    assert_eq!(sent(&mut r), vec![json!({"t": "detach"})], "quitting only detaches");
    a.leave(&mut out).unwrap();
    let after = terminal(&out);
    for gone in ['┌', '└', '│'] {
        assert!(!after.contains(gone), "no part of the input box is left: {after}");
    }
    assert!(!after.contains("live total est") && !after.contains("Enter send"), "no status bar or hint line left: {after}");
    assert!(after.contains("Hello from the agent."), "the conversation stays: {after}");
    assert!(after.contains("init-agent closed. The agent keeps running under the Root"), "{after}");
    assert!(after.contains("to come back") && after.contains("$0.046 in all"), "{after}");
}

/// User session v0.5.0-returning: the Root-unreachable line said to run `init-root start`, which is not
/// on the owner's PATH ("zsh: command not found: init-root"). It names the binary where it is.
#[test]
fn the_start_hint_names_the_root_binary_by_its_path() {
    let h = crate::app::start_hint(std::path::Path::new("/Users/o/.init-root"), Some("/Users/o"));
    assert!(h.contains("`~/.init-root/bin/init-root start`"), "{h}");
    let h = crate::app::start_hint(std::path::Path::new("/private/tmp/i5 us2/r"), Some("/Users/o"));
    assert!(h.contains("`INIT_ROOT_HOME='/private/tmp/i5 us2/r' '/private/tmp/i5 us2/r/bin/init-root' start`"), "{h}");
    assert!(!h.contains("`init-root start`"));
}

/// TUI-FIX2 (v0.5.1 review): the start hint said "its launchd job" on Linux too, where the Root's
/// service is a systemd user unit (init-root's install.rs `service_path`).
#[test]
fn the_start_hint_names_this_platforms_service() {
    use crate::app::start_hint_on;
    let rh = std::path::Path::new("/home/o/.init-root");
    let linux = start_hint_on(rh, Some("/home/o"), "linux");
    assert!(linux.contains("through its systemd user unit init-root.service, if one is installed") && !linux.contains("launchd"), "{linux}");
    let mac = start_hint_on(rh, Some("/home/o"), "macos");
    assert!(mac.contains("through its launchd job, if one is installed") && !mac.contains("systemd"), "{mac}");
    assert_eq!(crate::app::start_hint(rh, Some("/home/o")), start_hint_on(rh, Some("/home/o"), std::env::consts::OS));
}

/// v0.5.1: the App's key-setup texts name this App's own ROOT_HOME (here not the default one, so with
/// `INIT_ROOT_HOME=` in front), never a bare `init-root`.
#[test]
fn the_apps_setup_names_its_own_root_home() {
    let (mut a, _r, _rx) = attached();
    a.setup.on_line("/model");
    let o = a.setup.on_answer(Op::List, Err(crate::conn::ReqErr::Refused("root.unknown_cmd".into(), "x".into(), Value::Null)));
    let t = format!("{o:?}");
    let exe = "INIT_ROOT_HOME=/nonexistent/i5-tui-fix /nonexistent/i5-tui-fix/bin/init-root";
    assert!(t.contains(&format!("`{exe} providers add`")) && t.contains(&format!("`{exe} keys set`")), "{t}");
}

/// v0.5.1: the TUI prints `init-root` in the Root's own form: these are the cases of the Root's
/// util.rs `root_cmd_runs_as_printed`, so the two cannot drift apart unnoticed.
#[test]
fn root_cmd_is_the_roots_form() {
    use crate::app::root_cmd;
    use std::path::Path;
    let h = Some("/Users/o");
    assert_eq!(root_cmd(Path::new("/Users/o/.init-root"), h, "resume"), "~/.init-root/bin/init-root resume");
    assert_eq!(root_cmd(Path::new("/Users/o/.init-root"), h, ""), "~/.init-root/bin/init-root");
    assert_eq!(root_cmd(Path::new("/private/tmp/i5/r"), h, "start --foreground"),
        "INIT_ROOT_HOME=/private/tmp/i5/r /private/tmp/i5/r/bin/init-root start --foreground");
    assert_eq!(root_cmd(Path::new("/Users/o/.init-root"), None, "stop"),
        "INIT_ROOT_HOME=/Users/o/.init-root /Users/o/.init-root/bin/init-root stop", "no HOME: the absolute form");
    assert_eq!(root_cmd(Path::new("/Users/o/My Root"), h, "status"), "INIT_ROOT_HOME='/Users/o/My Root' '/Users/o/My Root/bin/init-root' status");
    assert_eq!(root_cmd(Path::new("/Users/o/it's"), h, ""), "INIT_ROOT_HOME='/Users/o/it'\\''s' '/Users/o/it'\\''s/bin/init-root'");
}
