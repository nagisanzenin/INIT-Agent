//! TUI-FIX (v0.5.1): the defects the two v0.5.0 user sessions found (docs/user-sessions/), each
//! driven through the real `App` on one end of a socket pair: the frames it sends are read back from
//! the other end, and nothing reaches a Root (the Root home does not exist).

use crate::app::{App, Opts, PANIC_ASK, PANIC_HINT, panic_ask_text};
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

// ---------------------------------------------------------------------------------------------
// v0.5.2 (R-TUI): docs/user-sessions/v0.5.1-new.md and v0.5.1-returning.md
// ---------------------------------------------------------------------------------------------

fn bar_text(a: &App) -> String {
    crate::render::status::bar(&a.info, 240, Instant::now(), 0, 0, false).text()
}
fn status(slot: &str, inst: &str, current: &str, audited: &str) -> Value {
    json!({"slot": slot, "mode": "normal", "level": 0, "cap": 0, "queued": 0, "kernel": {"instance": inst, "ready": slot == "running", "boot_seq": 1},
        "tuples": {"current": current, "lkg": "t06f78dd675f4", "audited": audited}, "panic": if slot == "panicked" { json!({"at": "2026-10-28T11:20:00Z", "channel": "owner_socket"}) } else { Value::Null }})
}

/// v0.5.1-new, item 3: `/undo list` went to the model as a chat message and was billed. An unknown
/// `/command` is refused locally and stays in the box; `/help` lists the commands; `//` sends a line
/// beginning with `/` as a message; R-UNDO's `/undo list` and `/undo <id>` go to the Root as lines.
#[test]
fn unknown_commands_are_refused_locally() {
    let (mut a, mut r, _rx) = live();
    enter(&mut a, "/undo-list");
    assert!(sent(&mut r).is_empty(), "nothing reaches the model");
    assert_eq!(a.ed.text(), "/undo-list", "the line stays in the box");
    assert_eq!(blocks(&a).last().unwrap(), "· /undo-list: did you mean /undo list? — /help lists the commands; start the line with // to send \
        it as a message (nothing was sent)");
    a.ed.clear();
    enter(&mut a, "/help");
    assert!(sent(&mut r).is_empty());
    assert!(matches!(&a.t.blocks.back().unwrap().kind, Kind::Table { rows, .. } if rows.iter().any(|(c, _)| c == "/undo list")));
    enter(&mut a, "//undo list is what I typed");
    assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "//undo list is what I typed"})]);
    for l in ["/undo list", "/undo 20261028T111910.728Z-project", "/undo 20261028T111910.728Z-project confirm", "/new", "/usr/bin/env is missing"] {
        enter(&mut a, l);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": l})], "{l}");
    }
    // completion offers R-UNDO's lines
    typed(&mut a, "/undo");
    let c: Vec<&str> = a.ed.completions().iter().map(|c| c.0).collect();
    assert!(c.contains(&"/undo list") && c.contains(&"/undo "), "{c:?}");
}

/// v0.5.1-new, item 2: after `/panic` then `/resume` the status bar still said "body 9aa7d39624ce"
/// while `/status` said "sha 4c332dd485ce"; the resume printed `✕`; and `/panic` never said that the
/// kept change would stop running.
#[test]
fn panic_and_resume_tell_the_truth() {
    let (mut a, mut r, _rx) = live();
    a.handle(Ev::Status(status("running", "k1", "t06f78dd675f4", "tb75a7fb288b4")));
    frame(&mut a, json!({"t": "state", "busy": false, "mode": "normal", "sha12": "9aa7d39624ce", "src": "kernel"}));
    assert!(bar_text(&a).contains("body 9aa7d39624ce"), "{}", bar_text(&a));
    // /panic says what /resume will drop
    enter(&mut a, "/panic");
    let ask = blocks(&a).last().unwrap().clone();
    assert!(ask.contains("What you lose: this agent runs version t06f78dd675f4, newer than the audited-good tb75a7fb288b4"), "{ask}");
    assert!(ask.ends_with("Type panic and press Enter to stop everything now; Esc or any other line cancels."), "{ask}");
    enter(&mut a, "panic");
    assert_eq!(sent(&mut r), vec![json!({"t": "panic"})]);
    frame(&mut a, json!({"t": "notice", "level": "alert", "code": "panic", "src": "root", "text": "PANIC at 2026-10-28T11:20:00Z via owner_socket: killed 2 processes in 2 instances; egress closed; level 0→0", "ts": "x"}));
    a.handle(Ev::Status(status("panicked", "k1", "t06f78dd675f4", "tb75a7fb288b4")));
    assert!(!bar_text(&a).contains("9aa7d39624ce"), "no body runs while panicked: {}", bar_text(&a));
    // the resume: a success, never ✕
    frame(&mut a, json!({"t": "notice", "level": "alert", "code": "resume", "src": "root", "text": "[root] resumed after panic at level L0; the zone restarts on the audited-good tuple", "ts": "x"}));
    let rows: Vec<String> = live::block_rows(&a.t.blocks.back().unwrap().kind, 100, Instant::now(), &live::RowOpts { verbose: false, expanded: false, spin: 0, first: false })
        .iter().map(|l| l.text()).collect();
    assert!(rows[0].starts_with("✓ [root] resumed after panic") && !rows.iter().any(|l| l.contains('✕')), "{rows:#?}");
    a.handle(Ev::Status(status("running", "k2", "tb75a7fb288b4", "tb75a7fb288b4")));
    assert!(!bar_text(&a).contains("9aa7d39624ce"), "the dropped body is not shown: {}", bar_text(&a));
    // the kernel's /status line names the body that runs: the bar follows it
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "kernel", "ts": "x",
        "text": "slot running · mode normal · busy false · sha 4c332dd485ce · lkg 9aa7d39624ce · level L0 · queued 0"}));
    assert!(bar_text(&a).contains("body 4c332dd485ce"), "{}", bar_text(&a));
    // and a fresh state frame from the new kernel too
    frame(&mut a, json!({"t": "state", "busy": false, "mode": "normal", "sha12": "4c332dd485ce", "src": "kernel"}));
    assert!(bar_text(&a).contains("body 4c332dd485ce"), "{}", bar_text(&a));
}

/// v0.5.1-new and -returning, item 4: after a relaunch the replayed turn read "$0.000" (and once
/// "$0.000 incl. checks"); a T1 turn read "incl. checks" though no check had spent yet.
#[test]
fn a_replayed_turn_is_never_free_and_checks_are_only_named_when_they_ran() {
    let money = |m: i64| Ev::Egress(json!({"money": {"enforced": false, "policy": "observe", "total_spent_musd": m, "day_spent_musd": m}}));
    let (mut a, _r, _rx) = attached();
    a.handle(money(48_518));
    // the replay: a kept self-change, then the owner's turn after it (1,342 µUSD at the time)
    let sha = "9aa7d39624ce0011";
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "kernel", "ts": "2026-10-28T10:43:24Z", "text": "[kernel] gate started",
        "data": {"kind": "gate_start", "gate_id": "j-1", "sha": sha, "scope": "t1", "steps": ["G0", "G1", "G2", "G3", "live", "G4"]}}));
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "kernel", "ts": "2026-10-28T11:16:21Z", "text": "[kernel] lkg", "data": {"kind": "lkg", "sha": sha}}));
    frame(&mut a, json!({"t": "echo", "text": "use your new tool", "seq": 9, "src": "kernel"}));
    frame(&mut a, json!({"t": "state", "busy": true, "src": "kernel"}));
    frame(&mut a, json!({"t": "display", "kind": "turn_end", "text": "tokens: in 16000 (cached 15000), out 110", "boot_seq": 1, "src": "kernel"}));
    frame(&mut a, json!({"t": "state", "busy": false, "src": "kernel"}));
    frame(&mut a, json!({"t": "replay_end"}));
    a.handle(money(48_518)); // the poll after the run's end: the ledger did not move while it replayed
    let b = bar_text(&a);
    assert!(b.contains("$? before this launch") && !b.contains("$0.000 ") && !b.contains("incl. checks"), "{b}");
    let (_, rows) = crate::render::status::usage_rows(&a.info);
    let last = &rows.iter().find(|(k, _)| k == "last turn").unwrap().1;
    assert!(last.contains("$? before this launch") && last.contains("before this TUI attached"), "{last}");
    let sc = &rows.iter().find(|(k, _)| k == "self-changes").unwrap().1;
    assert!(sc.starts_with("a self-change ran before this TUI attached") && !sc.contains("$0.000"), "{sc}");
    // live: a turn during which a self-change only builds (G0) is not "incl. checks"…
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "root", "text": "[root] attached: slot running, level L0, 0 queued.", "ts": "x"}));
    frame(&mut a, json!({"t": "state", "busy": true, "src": "kernel"}));
    let sha2 = "1111111111112222";
    frame(&mut a, json!({"t": "notice", "level": "info", "src": "kernel", "ts": "2026-10-28T12:00:00Z", "text": "[kernel] gate started",
        "data": {"kind": "gate_start", "gate_id": "j-2", "sha": sha2, "scope": "t1", "steps": ["G0", "G1", "G2", "G3", "live", "G4"]}}));
    frame(&mut a, json!({"t": "display", "kind": "turn_end", "text": "tokens: in 1000 (cached 0), out 10", "boot_seq": 1, "src": "kernel"}));
    a.handle(money(53_874));
    frame(&mut a, json!({"t": "state", "busy": false, "src": "kernel"}));
    a.handle(money(53_874));
    let b = bar_text(&a);
    assert!(b.contains("· $0.005 ") && !b.contains("incl. checks") && !b.contains("before this launch"), "{b}");
    // …one during which its probes run (G2) is
    let gate = |stage: &str| json!({"t": "notice", "level": "info", "src": "kernel", "ts": "2026-10-28T12:20:00Z", "text": "[kernel] gate",
        "data": {"kind": "gate", "gate_kind": "promote", "gate_id": "j-2", "sha": sha2, "stage": stage, "pass": true, "reason": null, "score": null, "spent_musd": 0, "wall_ms": 1000, "cache_permille": null}});
    frame(&mut a, gate("G0"));
    frame(&mut a, gate("G1"));
    frame(&mut a, json!({"t": "state", "busy": true, "src": "kernel"}));
    frame(&mut a, json!({"t": "display", "kind": "turn_end", "text": "tokens: in 1000 (cached 0), out 10", "boot_seq": 1, "src": "kernel"}));
    a.handle(money(60_000));
    assert!(bar_text(&a).contains("$0.006 incl. checks"), "{}", bar_text(&a));
}

// ---------------------------------------------------------------------------------------------
// v0.5.3 (lane B): build/status/REL-052.md §7.5, the v0.5.2 post-release review, findings 4 and 10
// ---------------------------------------------------------------------------------------------

/// Finding 10: the fixed command list refused `/tmp is full` (a one-segment path) and every command an
/// evolved body adds, while a built-in with a wrong argument (`/stop now`, `/panic now`) went to the
/// model. Now a path goes out at once; a built-in with an argument its fixed grammar refuses, or in
/// another case, or joined with `-`, is refused with the right form; any other `/word` is held once
/// ("did you mean …?") and the same line again sends it. A first Enter never sends what the owner
/// may not have meant.
#[test]
fn slash_lines_paths_typos_and_the_bodys_own_commands() {
    let (mut a, mut r, _rx) = live();
    // a path, one segment or more, goes out at once
    for l in ["/tmp is full", "/Users/x/file.go:12 panics"] {
        enter(&mut a, l);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": l})], "{l}");
    }
    // a built-in with an argument its grammar refuses: never sent, the line stays, the right form named
    for (l, want) in [("/stop now", "/stop takes no argument"), ("/panic now", "/panic takes no argument"), ("/help me", "/help takes no argument"),
        ("/full of", "did you mean /full off?"), ("/root stauts", "did you mean /root status?"), ("/undo lst", "did you mean /undo list?"),
        ("/accept", "/accept takes"), ("/later", "/later takes"), ("/undo-list", "did you mean /undo list?"), ("/Help", "did you mean /help?")] {
        enter(&mut a, l);
        assert!(sent(&mut r).is_empty(), "{l}: nothing is sent");
        assert_eq!(a.ed.text(), l, "{l}: the line stays in the box");
        let b = blocks(&a).last().unwrap().clone();
        assert!(b.contains(want) && b.contains("start the line with // to send it as a message") && b.ends_with("(nothing was sent)"), "{l}: {b}");
        key(&mut a, KeyCode::Enter);
        assert!(sent(&mut r).is_empty(), "{l}: a second Enter does not send a refused line either");
        a.ed.clear();
    }
    // a `/word` the TUI does not know: held once, with the nearest built-in; the second Enter sends it
    enter(&mut a, "/hepl");
    assert!(sent(&mut r).is_empty(), "the first Enter sends nothing");
    assert_eq!(a.ed.text(), "/hepl", "the line stays in the box");
    let b = blocks(&a).last().unwrap().clone();
    assert!(b.contains("did you mean /help?") && b.contains("Enter again to send it to the model as text") && b.ends_with("(nothing was sent)"), "{b}");
    key(&mut a, KeyCode::Enter);
    assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "/hepl"})], "the second Enter sends it as it is");
    // a command an evolved body added, next to a built-in or not: the same
    for (l, near) in [("/stats", Some("/status")), ("/labels e1", Some("/label")), ("/myskill run", None)] {
        enter(&mut a, l);
        assert!(sent(&mut r).is_empty(), "{l}");
        let b = blocks(&a).last().unwrap().clone();
        assert_eq!(b.contains("did you mean"), near.is_some(), "{l}: {b}");
        assert!(near.is_none_or(|n| b.contains(&format!("did you mean {n}?"))), "{l}: {b}");
        key(&mut a, KeyCode::Enter);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": l})], "{l}");
    }
    // an edited line is asked again, and so is one after any other line
    enter(&mut a, "/stats");
    typed(&mut a, " now");
    key(&mut a, KeyCode::Enter);
    assert!(sent(&mut r).is_empty(), "an edited line is asked again");
    key(&mut a, KeyCode::Enter);
    assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "/stats now"})]);
    enter(&mut a, "/stats");
    a.ed.clear();
    enter(&mut a, "hello");
    assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "hello"})]);
    enter(&mut a, "/stats");
    assert!(sent(&mut r).is_empty(), "another line in between: asked again");
    // the body's own words and the Root's claim-all ones still go out at once, arguments and all
    a.ed.clear();
    for l in ["/new", "/compact the tests", "/branch e1 try again", "/goal stop", "/twin", "/undo list", "/root status", "/full on confirm", "//stop now"] {
        enter(&mut a, l);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": l})], "{l}");
    }
}

/// Finding 4: `init-agent` as the owner types it in a new terminal. The launcher's path (`~/…` under
/// HOME, quoted when it needs it), INIT_ROOT_HOME= in front for a Root elsewhere; an INIT_AGENT_LAUNCHER
/// that is relative or unprintable falls back to the TUI's own directory, then to ~/.local/bin.
#[test]
fn agent_cmd_names_the_launcher_by_its_path() {
    use crate::app::agent_cmd;
    use std::path::Path;
    let (h, rh) = (Some("/Users/o"), Path::new("/Users/o/.init-root"));
    assert_eq!(agent_cmd(Some("/Users/o/.local/bin/init-agent"), None, h, rh), "~/.local/bin/init-agent");
    assert_eq!(agent_cmd(Some("/Users/o/my bin/init-agent"), None, h, rh), "~/'my bin/init-agent'");
    assert_eq!(agent_cmd(Some("/Users/o/.local/bin/init-agent"), None, Some("/Users/o/"), rh), "~/.local/bin/init-agent");
    assert_eq!(agent_cmd(Some("/Users/other/bin/init-agent"), None, h, rh), "/Users/other/bin/init-agent", "not under HOME");
    assert_eq!(agent_cmd(Some("/Users/oo/bin/init-agent"), None, h, rh), "/Users/oo/bin/init-agent", "a longer name is not under HOME");
    assert_eq!(agent_cmd(Some("/opt/it's/init-agent"), None, h, rh), "'/opt/it'\\''s/init-agent'");
    assert_eq!(agent_cmd(Some("/Users/o/.local/bin/init-agent"), None, h, Path::new("/Users/o/r h")),
        "INIT_ROOT_HOME='/Users/o/r h' ~/.local/bin/init-agent", "a Root elsewhere");
    assert_eq!(agent_cmd(Some("/Users/o/.local/bin/init-agent"), None, None, rh),
        "INIT_ROOT_HOME=/Users/o/.init-root /Users/o/.local/bin/init-agent", "no HOME: the long form");
    assert_eq!(agent_cmd(Some("/b/init-agent"), None, Some("/"), Path::new("/.init-root")), "/b/init-agent", "HOME / is no prefix");
    // a bad INIT_AGENT_LAUNCHER: the TUI's own directory, then the default
    let exe = Path::new("/Users/o/.local/bin/init-tui");
    for bad in ["init-agent", "", "./init-agent", "/x/\u{1b}[2Jinit-agent", "/x/a\nb"] {
        assert_eq!(agent_cmd(Some(bad), Some(exe), h, rh), "~/.local/bin/init-agent", "{bad:?}");
    }
    assert_eq!(agent_cmd(None, Some(Path::new("/opt/i5/init-tui")), h, rh), "/opt/i5/init-agent");
    assert_eq!(agent_cmd(Some("rel"), Some(Path::new("rel/init-tui")), h, rh), "~/.local/bin/init-agent");
    assert_eq!(agent_cmd(None, None, h, rh), "~/.local/bin/init-agent");
}

/// The coordinator's shared rule (v0.5.3): a TUI started without the launcher takes its own directory,
/// except ROOT_HOME/bin (no launcher there), then INIT_BIN_DIR's launcher, then ~/.local/bin's.
#[test]
fn agent_sources_without_the_launcher() {
    use crate::app::{agent_cmd, agent_sources};
    use std::path::{Path, PathBuf};
    let (h, rh) = (Some("/Users/o"), Path::new("/Users/o/.init-root"));
    let cmd = |l: Option<&str>, b: Option<&str>, e: Option<&str>| {
        let (l, e) = agent_sources(l.map(Into::into), b.map(Into::into), || e.map(PathBuf::from), rh);
        agent_cmd(l.as_deref(), e.as_deref(), h, rh)
    };
    assert_eq!(cmd(Some("/Users/o/apps/init-agent"), Some("/opt/b"), Some("/Users/o/.local/bin/init-tui")), "~/apps/init-agent");
    assert_eq!(cmd(None, Some("/opt/b"), Some("/Users/o/x/init-tui")), "~/x/init-agent", "beside init-tui");
    assert_eq!(cmd(None, Some("/opt/b/"), Some("/Users/o/.init-root/bin/init-tui")), "/opt/b/init-agent", "not ROOT_HOME/bin");
    assert_eq!(cmd(Some("rel/init-agent"), Some("bin"), Some("/Users/o/.init-root/bin/init-tui")), "~/.local/bin/init-agent");
    assert_eq!(cmd(None, None, None), "~/.local/bin/init-agent");
}

/// Finding 4: the quit line and the no-workspace notice name the launcher by its path, never a bare
/// `init-agent`; the App passes its own (`Info::agent`) to the access flow.
#[test]
fn quit_and_no_workspace_lines_name_the_launcher() {
    let (mut a, _r, _rx) = live();
    assert!(a.info.agent.contains("/init-agent"), "the path, never a bare init-agent: {}", a.info.agent);
    a.handle(Ev::Access { op: crate::access::Op::Workspace, result: Ok(json!({"access": {"access": "full", "full": true, "workspace": null,
        "mode": "full", "read_home": true, "allow_full": true, "restart_pending": false, "snapshot": null}})) });
    let notice = blocks(&a).into_iter().find(|b| b.contains("To let it edit a project")).unwrap_or_else(|| panic!("{:?}", blocks(&a)));
    assert!(notice.contains(&format!("cd into one and run {} again.", a.info.agent)), "{notice}");
    a.info.agent = crate::app::agent_cmd(Some("/Users/o/.local/bin/init-agent"), None, Some("/Users/o"), std::path::Path::new("/Users/o/.init-root"));
    let rows = live::goodbye(&a.info, 400, true);
    let text: String = rows.iter().map(|l| l.text()).collect::<Vec<_>>().join(" ");
    assert!(text.contains("run ~/.local/bin/init-agent to come back;") && !text.contains("run init-agent"), "{text}");
}

/// v0.5.3 (REL-052 §7.5 finding 3): `/panic`'s warning names what `/resume` drops by the Root's own pick of the resume
/// target (`status.resume`), not by audited-good. On the free path that is `settled`, then LKG: with LKG the running
/// tuple, nothing is dropped and the warning says nothing of a loss (v0.5.2 compared with audited-good and did).
#[test]
fn panic_names_what_the_resume_really_drops() {
    let ask = |resume: Value| {
        let mut st = status("running", "k1", "t06f78dd675f4", "tb75a7fb288b4");
        st["resume"] = resume;
        panic_ask_text(Some(&st))
    };
    let none = ask(json!({"pointer": "lkg", "tuple": "t06f78dd675f4"}));
    assert_eq!(none, PANIC_ASK, "the running tuple: nothing is lost");
    assert!(PANIC_ASK.contains("in free mode: the settled one, else the last known-good"), "where /resume goes on the free path: {PANIC_ASK}");
    let settled = ask(json!({"pointer": "settled", "tuple": "t5e77ed000001"}));
    assert!(settled.contains("What you lose: this agent runs version t06f78dd675f4, newer than the settled t5e77ed000001, so after /resume the \
        self-changes kept since t5e77ed000001 no longer run"), "{settled}");
    assert!(!settled.contains("audited-good tb75a7fb288b4"), "{settled}");
    // a Root before v0.5.3 (no `resume` block): audited-good, as before
    let old = panic_ask_text(Some(&status("running", "k1", "t06f78dd675f4", "tb75a7fb288b4")));
    assert!(old.contains("newer than the audited-good tb75a7fb288b4"), "{old}");
}
