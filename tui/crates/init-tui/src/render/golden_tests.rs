//! TUI-1 render snapshots: the renderer is a pure function of (state, width, height) → a text grid
//! with a style mask, compared with golden files under tests/golden/ (`UPDATE_GOLDEN=1` rewrites).
//! TUI-4's renderer half: no ESC from a frame survives into a row.

use super::live::{self, Ui};
use super::{Line, grid};
use crate::input::Editor;
use crate::model::{Conn, Info, Kind, OwnerSt, Transcript, Tray, TrayOut};
use crate::proto::{Display, DisplayKind};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NOW_MS: i64 = 1_792_530_245_000; // 2026-10-20T21:04:05Z

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"))
}

/// Compares with the golden file, or rewrites it with UPDATE_GOLDEN=1.
fn check(name: &str, got: &str) {
    let p = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing golden {} (run with UPDATE_GOLDEN=1)", p.display()));
    if want != got {
        let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
        for (i, (a, b)) in w.iter().zip(g.iter()).enumerate() {
            if a != b {
                panic!("golden {name} differs at line {}:\nwant {a}\ngot  {b}", i + 1);
            }
        }
        panic!("golden {name} differs in length: want {} lines, got {}", w.len(), g.len());
    }
}

struct World {
    t: Transcript,
    tray: Tray,
    ed: Editor,
    info: Info,
    base: Instant,
    scroll: Vec<Line>,
}

fn d(kind: DisplayKind, text: &str) -> Display {
    Display { kind, text: text.into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None, parent: None }
}

fn tool(kind: DisplayKind, id: &str, name: &str, text: &str, ok: Option<bool>) -> Display {
    Display { kind, text: text.into(), call_id: Some(id.into()), name: Some(name.into()), ok, boot_seq: Some(1), detail: None, parent: None }
}

fn status(slot: &str, mode: &str) -> Value {
    json!({"slot": slot, "mode": mode, "halted": null, "panic": null, "level": 0, "cap": 1, "max_level": 4,
        "tuples": {"current": "t1", "lkg": "t1", "audited": "t1"}, "kernel": {"ready": true, "boot_seq": 1}, "queued": 0,
        "root": {"started_at": "2026-10-20T20:00:00Z", "pid": 42, "version": "0.1.0", "test_mode": true}})
}

fn egress(spent: i64, limit: i64) -> Value {
    json!({"money": {"day_spent_musd": spent, "day_reserved_musd": 0, "day_limit_musd": limit, "day_resets_at": "2026-10-21T02:16:05Z",
        "total_spent_musd": spent, "total_limit_musd": 100_000_000}, "models": [{"model": "deepseek-flash", "routable": true}]})
}

impl World {
    fn new() -> World {
        let base = Instant::now();
        let mut info = Info::default();
        info.conn = Conn::Live;
        info.on_status(status("running", "normal"), base);
        info.on_egress(egress(1_230_000, 5_000_000));
        info.on_state(json!({"slot": "running", "mode": "normal", "busy": true, "sha12": "3f9c0a1b2c4d", "queued": 0, "level": 0, "members": 0}), base);
        World { t: Transcript::default(), tray: Tray { tracked: true, ..Tray::default() }, ed: Editor::default(), info, base, scroll: Vec::new() }
    }
    fn at(&self, secs: f64) -> Instant {
        self.base + Duration::from_secs_f64(secs)
    }
    fn show(&mut self, k: Display, secs: f64) {
        let now = self.at(secs);
        self.t.display(&k, now, NOW_MS);
    }
    /// One render at `secs`: commits go to the simulated scrollback; returns the screen.
    fn screen(&mut self, w: usize, h: usize, secs: f64, verbose: bool) -> String {
        let now = self.at(secs);
        let spin = 3;
        let max = {
            let ui = Ui { t: &self.t, tray: &self.tray, ed: &self.ed, info: &self.info, verbose, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
            live::max_live(&ui, w, h)
        };
        let commits = live::commit(&mut self.t, w, max, verbose, now, spin);
        self.scroll.extend(commits);
        let ui = Ui { t: &self.t, tray: &self.tray, ed: &self.ed, info: &self.info, verbose, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
        let region = live::region(&ui, w, h);
        let mut all: Vec<Line> = self.scroll.clone();
        all.extend(region.rows.iter().cloned());
        let start = all.len().saturating_sub(h);
        let rows: Vec<Line> = all[start..].to_vec();
        for r in &rows {
            assert!(r.width() <= w, "row wider than {w}: {:?}", r.text());
            assert!(!r.text().contains('\u{1b}'), "an ESC reached a row");
        }
        let mut s = format!("== {w}x{h} ==\n");
        s.push_str(&grid(&rows, w));
        if let Some((cx, cy)) = region.cursor {
            s.push_str(&format!("cursor {cx},{}\n", cy + rows.len() - region.rows.len().min(rows.len())));
        }
        s
    }
}

/// STEER: a line typed while the agent works, echoed by the kernel as `seq` (or not yet, or Root-held).
fn typed(w: &mut World, id: u64, text: &str, follow_up: bool, echo: Option<OwnerSt>, seq: u64) {
    w.tray.add(id, text, follow_up, w.base);
    if let Some(st) = echo {
        assert_eq!(w.tray.on_echo(id, st, (st == OwnerSt::Delivered).then_some(seq)), None);
    }
}

/// STEER (§3.7.1): the pending tray above the input box in each state, and a steer moving into the
/// transcript, marked seen, once the body acks it, with the activity frozen and no false `? no result`.
#[test]
fn steer_tray_states() {
    let mut out = String::new();
    // 1. a long bash call runs: a steer waits behind it, a follow-up waits for the next task, one is
    //    still on its way
    let mut w = World::new();
    w.t.push(Kind::Owner { id: 1, text: "fix the flaky test".into(), st: OwnerSt::Delivered }, NOW_MS);
    w.show(tool(DisplayKind::ToolStart, "c1", "bash", "cargo test -p init-root --offline", None), 0.0);
    typed(&mut w, 2, "fix the test name too", false, Some(OwnerSt::Delivered), 7);
    typed(&mut w, 3, "then update the changelog", true, Some(OwnerSt::Delivered), 8);
    typed(&mut w, 4, "also run clippy\nand fmt", false, None, 0);
    out.push_str(&w.screen(80, 16, 41.0, false));
    // 2. the reply is being written: after this reply; a line being taken back
    let mut w = World::new();
    w.show(d(DisplayKind::Text, "Running the tests now."), 0.0);
    typed(&mut w, 2, "use --nocapture", false, Some(OwnerSt::Delivered), 9);
    typed(&mut w, 3, "never mind", false, Some(OwnerSt::Delivered), 10);
    assert_eq!(w.tray.retract_newest(), Ok(10));
    out.push_str(&w.screen(80, 12, 2.0, false));
    // 3. INIT is updating (the body is paused) and the Root holds a line while the kernel restarts
    let mut w = World::new();
    w.info.on_state(json!({"slot": "pausing", "mode": "normal", "busy": true, "sha12": "3f9c0a1b2c4d", "queued": 1, "level": 0, "members": 0}), w.base);
    typed(&mut w, 2, "keep going after the update", false, Some(OwnerSt::Delivered), 11);
    typed(&mut w, 3, "and say when done", false, Some(OwnerSt::Queued), 0);
    out.push_str(&w.screen(80, 12, 1.0, false));
    // 4. many lines: the oldest fold
    let mut w = World::new();
    for i in 0..6u64 {
        typed(&mut w, 10 + i, &format!("note {i}"), false, Some(OwnerSt::Delivered), 20 + i);
    }
    out.push_str(&w.screen(80, 14, 1.0, false));
    // 5. the batch ends, the body acks seq 7: the steer is in the transcript, marked seen
    let mut w = World::new();
    w.t.push(Kind::Owner { id: 1, text: "fix the flaky test".into(), st: OwnerSt::Delivered }, NOW_MS);
    w.show(tool(DisplayKind::ToolStart, "c1", "bash", "cargo test -p init-root --offline", None), 0.0);
    typed(&mut w, 2, "fix the test name too", false, Some(OwnerSt::Delivered), 7);
    w.show(tool(DisplayKind::ToolEnd, "c1", "bash", "exit 0, 214 passed", Some(true)), 41.0);
    w.show(d(DisplayKind::TurnEnd, "tokens: in 900 (cached 800), out 20"), 41.0);
    for o in w.tray.on_state(&json!({"busy": true, "t": "state", "steer_pending": [], "steer_seen": [7], "steer_retracted": []})) {
        let TrayOut::Seen(it) = o else { panic!("seen") };
        w.t.push(Kind::Owner { id: it.id, text: it.text, st: OwnerSt::Seen }, NOW_MS);
    }
    w.show(d(DisplayKind::Text, "Renaming it as well."), 42.0);
    let s5 = w.screen(80, 14, 42.5, false);
    assert!(!s5.contains("no result"), "{s5}");
    assert!(s5.contains("· seen"), "{s5}");
    out.push_str(&s5);
    check("steer_tray", &out);
}

/// A conversation: an owner line, streaming Markdown text, thinking, a parallel batch, a turn end.
fn conversation(w: &mut World) {
    let id = w.t.len() as u64 + 1;
    w.t.push(Kind::Owner { id, text: "add a pty tool".into(), st: OwnerSt::Delivered }, NOW_MS);
    w.show(d(DisplayKind::Thinking, "The user wants a pty tool. First read SELF.md."), 0.0);
    w.show(d(DisplayKind::Text, "I'll check `SELF.md` first, then **write** the tool.\n\n"), 2.0);
    w.show(d(DisplayKind::Text, "Plan:\n- read the file\n- edit `src/tools/pty.rs`\n"), 2.1);
    w.show(tool(DisplayKind::ToolStart, "c1", "bash", "cat SELF.md", None), 2.2);
    w.show(tool(DisplayKind::ToolStart, "c2", "edit", "src/tools/pty.rs (2 edits)", None), 2.2);
    w.show(tool(DisplayKind::ToolEnd, "c2", "edit", "error: no_match", Some(false)), 2.2);
    w.show(tool(DisplayKind::ToolEnd, "c1", "bash", "exit 0, 96 lines", Some(true)), 2.5);
    w.t.notice("info", "[kernel] promoted", Some("kernel"), None, NOW_MS);
    w.show(d(DisplayKind::TurnEnd, "tokens: in 12345 (cached 11002), out 321"), 3.0);
    w.show(d(DisplayKind::Text, "The anchor moved, so I'll read the file again and retry"), 4.0);
    w.show(tool(DisplayKind::ToolStart, "c3", "bash", "cargo test -p init-body pty", None), 5.0);
}

#[test]
fn tui1_conversation_at_three_sizes() {
    let mut out = String::new();
    for (wd, ht) in [(80, 24), (120, 40), (40, 12)] {
        let mut w = World::new();
        conversation(&mut w);
        w.ed.insert("also add a timeout flag");
        out.push_str(&w.screen(wd, ht, 17.0, false));
    }
    check("tui1_conversation", &out);
}

#[test]
fn tui1_thinking_verbose_and_running_batch() {
    let mut w = World::new();
    w.show(d(DisplayKind::Thinking, "Step one: read.\nStep two: write."), 0.0);
    let mut out = w.screen(80, 24, 1.5, false);
    let mut v = World::new();
    v.show(d(DisplayKind::Thinking, "Step one: read.\nStep two: write."), 0.0);
    out.push_str(&v.screen(80, 24, 1.5, true));
    let mut b = World::new();
    b.show(tool(DisplayKind::ToolStart, "a", "bash", "sleep 5", None), 0.0);
    b.show(tool(DisplayKind::ToolStart, "b", "read", "read docs/tui.md", None), 0.0);
    b.show(tool(DisplayKind::ToolEnd, "b", "read", "512 lines", Some(true)), 0.4);
    out.push_str(&b.screen(80, 24, 3.0, false));
    check("tui1_thinking_batch", &out);
}

#[test]
fn tui1_notice_classes() {
    let mut w = World::new();
    let n = |w: &mut World, level: &str, text: &str, src: Option<&str>, code: Option<&str>| w.t.notice(level, text, src, code, NOW_MS);
    n(&mut w, "info", "[kernel] promoted", Some("kernel"), None);
    n(&mut w, "alert", "[root] level L0→L1: 20 promotions, audit clean", Some("root"), Some("level_up"));
    n(&mut w, "alert", "[root] kernel crash loop: rolling back to the LKG tuple", Some("root"), Some("kernel_crash_loop"));
    n(&mut w, "alert", "[root] demoted L1→L0 (ceiling_hit)", Some("root"), Some("demoted"));
    n(&mut w, "alert", "[root] ceiling reached: day (root.ceiling)", Some("root"), Some("ceiling_hit"));
    n(&mut w, "alert", "PANIC at 2026-10-20T21:00:00Z via owner_socket", Some("root"), Some("panic"));
    n(&mut w, "warn", "[root] system safe mode: 3 demotions in 24 h", Some("root"), None);
    n(&mut w, "alert", "[root] Halted{lineage}: the zone will not run", Some("root"), Some("halted"));
    n(&mut w, "warn", "something else", Some("kernel"), None);
    n(&mut w, "alert", "[root] demoted L1→L0 (forged by the kernel)", Some("kernel"), None);
    w.t.push(Kind::Divider("Root restarted (21:04:05Z); output from while it was down is in the session log, not replayed".into()), NOW_MS);
    check("tui1_notices", &w.screen(80, 24, 1.0, false));
}

#[test]
fn tui1_status_bar_states() {
    let mut out = String::new();
    type Setup = Box<dyn Fn(&mut World)>;
    let cases: Vec<(&str, Setup)> = vec![
        ("running", Box::new(|_| {})),
        ("safe mode", Box::new(|w| w.info.on_status(status("running", "safe"), w.base))),
        (
            "panicked",
            Box::new(|w| {
                let mut s = status("panicked", "normal");
                s["panic"] = json!({"at": "2026-10-20T21:01:05Z", "channel": "owner_socket"});
                w.info.on_status(s, w.base);
            }),
        ),
        (
            "halted",
            Box::new(|w| {
                let mut s = status("halted", "normal");
                s["halted"] = json!("lineage");
                w.info.on_status(s, w.base);
            }),
        ),
        ("unreachable", Box::new(|w| w.info.conn = Conn::Down { retry_at: w.at(42.6) })),
        ("ceiling 80%", Box::new(|w| w.info.on_egress(egress(4_000_000, 5_000_000)))),
        ("ceiling 100%", Box::new(|w| w.info.on_egress(egress(5_000_000, 5_000_000)))),
        ("provisional", Box::new(|w| w.info.on_state(json!({"busy": false, "mode": "provisional", "sha12": "aaaabbbbcccc", "queued": 2}), w.base))),
        ("no egress", Box::new(|w| w.info.egress_failed = true)),
        ("state model+cost", Box::new(|w| w.info.on_state(json!({"busy": true, "mode": "normal", "sha12": "3f9c0a1b2c4d", "model": "deepseek-flash", "task_musd": 40_000}), w.base))),
    ];
    for (name, f) in cases {
        for width in [120, 80, 40] {
            let mut w = World::new();
            f(&mut w);
            let now = w.at(41.0);
            let l = super::status::bar(&w.info, width, now, NOW_MS, 3, false);
            out.push_str(&format!("-- {name} @{width}\n"));
            out.push_str(&grid(&[l], width));
        }
    }
    // NO_COLOR: reverse video for the error states
    let mut w = World::new();
    w.info.conn = Conn::Down { retry_at: w.at(1.0) };
    out.push_str("-- unreachable NO_COLOR @80\n");
    out.push_str(&grid(&[super::status::bar(&w.info, 80, w.at(0.0), NOW_MS, 3, true)], 80));
    check("tui1_status", &out);
}

/// WIRE-009 (ADR-009): the workspace as a short path, `no workspace`, and `FULL`, which is never
/// dropped; with NO_COLOR, `FULL` is in reverse video.
#[test]
fn wire9_access_in_the_status_bar() {
    let mut out = String::new();
    let view = |ws: Option<&str>, full: bool| {
        json!({"access": if full { "full" } else if ws.is_some() { "workspace" } else { "read_home" }, "allow_full": true, "full": full,
               "read_home": true, "restart_pending": false, "snapshot": null, "workspace": ws})
    };
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        ("workspace", Some("/Users/o/code/proj"), false),
        ("long workspace", Some("/Users/o/code/clients/acme/monorepo/services/billing-api"), false),
        ("no workspace", None, false),
        ("full", Some("/Users/o/code/proj"), true),
        ("full, no workspace", None, true),
    ];
    for (name, ws, full) in cases {
        for width in [120, 80, 40] {
            let mut w = World::new();
            w.info.home = Some("/Users/o".into());
            let mut s = status("running", "normal");
            s["access"] = view(ws, full);
            w.info.on_status(s, w.base);
            let l = super::status::bar(&w.info, width, w.at(41.0), NOW_MS, 3, false);
            out.push_str(&format!("-- {name} @{width}\n"));
            out.push_str(&grid(&[l], width));
        }
    }
    let mut w = World::new();
    w.info.on_access(view(Some("/Users/o/p"), true));
    out.push_str("-- full NO_COLOR, no HOME @80\n");
    out.push_str(&grid(&[super::status::bar(&w.info, 80, w.at(41.0), NOW_MS, 3, true)], 80));
    check("wire9_access_status", &out);
    assert_eq!(super::status::short_path("/Users/o/a/b", Some("/Users/o")), "~/a/b");
    assert!(super::status::short_path(&format!("/x/{}", "y".repeat(80)), None).chars().count() <= 32);
}

#[test]
fn tui1_input_box_popup_and_small_terminal() {
    let mut w = World::new();
    w.ed.insert("/ro");
    let mut out = w.screen(80, 24, 1.0, false);
    let mut m = World::new();
    m.ed.insert("line one\nline two is longer than the box is wide so that it wraps around once or twice, really\n日本語 👍 ok");
    out.push_str(&m.screen(60, 24, 1.0, false));
    let mut s = World::new();
    s.ed.insert("hello");
    out.push_str(&s.screen(18, 4, 1.0, false));
    check("tui1_input", &out);
}

#[test]
fn tui1_long_stream_commits_lines_and_fits_the_screen() {
    let mut w = World::new();
    for i in 0..60 {
        w.show(d(DisplayKind::Text, &format!("line {i} of a long streaming reply without blank lines\n")), i as f64 * 0.01);
        let s = w.screen(80, 24, i as f64 * 0.01, false);
        assert_eq!(s.lines().filter(|l| l.contains('|')).count(), 24.min(s.lines().count() - 2).max(1), "the screen never exceeds 24 rows");
    }
    // the first lines were committed to the scrollback exactly once, in order
    let texts: Vec<String> = w.scroll.iter().map(Line::text).collect();
    for (i, t) in texts.iter().enumerate() {
        assert_eq!(t, &format!("line {i} of a long streaming reply without blank lines"));
    }
    assert!(texts.len() >= 40, "{}", texts.len());
}

#[test]
fn tui4_sanitizer_in_every_rendered_string() {
    let evil = "x\u{1b}]52;c;SGVsbG8=\u{7}\u{1b}]8;;http://e\u{1b}\\link\u{1b}[2J\u{1b}[?1049h\u{9b}31m\u{202e}y";
    let build = || {
        let mut w = World::new();
        w.t.push(Kind::Owner { id: 1, text: evil.into(), st: OwnerSt::Delivered }, NOW_MS);
        w.show(d(DisplayKind::Text, evil), 0.0);
        w.show(Display { kind: DisplayKind::ToolStart, text: evil.into(), call_id: Some(evil.into()), name: Some(evil.into()), ok: None, boot_seq: Some(1), detail: None, parent: None }, 0.0);
        w.show(Display { kind: DisplayKind::ToolEnd, text: evil.into(), call_id: Some(evil.into()), name: Some(evil.into()), ok: Some(false), boot_seq: Some(1), detail: Some(evil.into()), parent: None }, 0.1);
        w.t.notice("alert", evil, Some("root"), None, NOW_MS);
        w.info.on_state(json!({"busy": true, "sha12": evil, "model": evil, "mode": evil}), w.base);
        w.ed.paste(evil);
        w
    };
    for (wd, ht) in [(80, 24), (40, 12)] {
        let mut w = build();
        let s = w.screen(wd, ht, 1.0, true);
        assert!(!s.contains('\u{1b}') && !s.contains('\u{9b}') && !s.contains('\u{7}') && !s.contains('\u{202e}'), "{s}");
    }
    // the expanded transcript view too
    let mut w = build();
    for b in w.t.blocks.iter_mut() {
        b.expanded = true;
    }
    let mut vs = super::transcript::ViewState::default();
    for r in super::transcript::render(&w.t, &mut vs, 80, 40, true, w.at(1.0)) {
        assert!(!r.text().contains('\u{1b}'));
        assert!(r.width() <= 80);
    }
}

#[test]
fn tui4_fuzz_random_frames() {
    // a small xorshift: deterministic, no crate
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut w = World::new();
    let kinds = [DisplayKind::Text, DisplayKind::Thinking, DisplayKind::ToolStart, DisplayKind::ToolEnd, DisplayKind::TurnEnd, DisplayKind::Info, DisplayKind::Error];
    for i in 0..100_000u64 {
        let len = (next() % 12) as usize;
        let s: String = (0..len)
            .map(|_| {
                let r = next();
                match r % 5 {
                    0 => char::from_u32((r >> 8) as u32 % 0xA0).unwrap_or('?'),
                    1 => ['\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}', '\u{200e}', '\u{061c}', '\u{1b}', '\u{9b}'][(r >> 8) as usize % 8],
                    2 => char::from_u32(0x4E00 + (r >> 8) as u32 % 200).unwrap_or('?'),
                    _ => (b'a' + (r >> 8) as u8 % 26) as char,
                }
            })
            .collect();
        let k = kinds[(next() % kinds.len() as u64) as usize];
        let id = format!("c{}", next() % 4);
        w.show(Display { kind: k, text: s.clone(), call_id: Some(id), name: Some(s.clone()), ok: Some(next() % 2 == 0), boot_seq: Some(1 + next() % 2), detail: None, parent: None }, i as f64 * 1e-4);
        if i % 997 == 0 {
            w.t.notice("warn", &s, None, None, NOW_MS);
            let screen = w.screen(80, 24, i as f64 * 1e-4, i % 2 == 0);
            assert!(!screen.contains('\u{1b}') && !screen.contains('\u{9b}') && !screen.contains('\u{202e}'), "frame {i}");
        }
    }
}

// -------------------------------------------------------------------------------------------
// TUI-UX: the per-turn activity block (§3.6), the heartbeat, and the token segment (§3.9)
// -------------------------------------------------------------------------------------------

/// A turn as the owner saw it flood by: a short reply, then 8 calls (one fails) with a token line
/// per request, then a long `cargo build` still running.
fn busy_turn(w: &mut World) {
    let id = w.t.len() as u64 + 1;
    w.t.push(Kind::Owner { id, text: "wire the delegate worktree".into(), st: OwnerSt::Delivered }, NOW_MS);
    w.info.on_owner_delivered();
    w.show(d(DisplayKind::Text, "Let me look at the worktree first.\n"), 0.5);
    let tokens = |w: &mut World, i: u64, secs: f64| {
        let t = format!("tokens: in {} (cached {}), out {}", 118_497 + i * 1089, 117_760 + i * 1100, 961 - i * 100);
        w.show(d(DisplayKind::TurnEnd, &t), secs);
        w.info.on_tokens(&t);
    };
    tokens(w, 0, 1.0);
    let calls = [
        ("bash", "cd \"$INIT_HOME/work/wt-delegate\" && python3 - <<'EOF'", "exit 0, 3 lines", true),
        ("read", "read root/crates/init-root/src/link.rs", "412 lines", true),
        ("bash", "git -C wt-delegate status --short", "exit 0, 7 lines", true),
        ("edit", "src/delegate.rs (2 edits)", "error: no_match", false),
        ("read", "read src/delegate.rs", "96 lines", true),
        ("edit", "src/delegate.rs (1 edit)", "ok", true),
        ("bash", "rg -n 'fn spawn' src", "exit 0, 4 lines", true),
    ];
    let mut at = 1.2;
    for (i, (name, text, end, ok)) in calls.iter().enumerate() {
        let cid = format!("c{i}");
        w.show(tool(DisplayKind::ToolStart, &cid, name, text, None), at);
        w.show(tool(DisplayKind::ToolEnd, &cid, name, end, Some(*ok)), at + 0.4);
        tokens(w, i as u64 + 1, at + 1.0);
        at += 1.5;
    }
    w.show(tool(DisplayKind::ToolStart, "c9", "bash", "cd wt-delegate && cargo build --offline 2>&1 | tail -40", None), 12.0);
}

#[test]
fn tuiux_activity_running_heartbeat_frozen_expanded() {
    let mut out = String::new();
    // collapsed while running: header, +N more, the failure, the last 3, the running call; the
    // heartbeat after 10 s with no output
    let mut w = World::new();
    busy_turn(&mut w);
    out.push_str("-- running, no output for 52 s (heartbeat)\n");
    out.push_str(&w.screen(100, 24, 64.0, false));
    // the newest output line, dim; then stale output with the heartbeat
    w.show(tool(DisplayKind::ToolProgress, "c9", "bash", "   Compiling init-root v0.2.0 (/x/root/crates/init-root)\n", None), 63.0);
    out.push_str("-- running, output 1 s ago\n");
    out.push_str(&w.screen(100, 24, 64.0, false));
    out.push_str("-- running, output 15 s ago\n");
    out.push_str(&w.screen(100, 24, 78.0, false));
    out.push_str("-- running, narrow\n");
    out.push_str(&w.screen(40, 14, 78.0, false));
    // frozen when the reply's text arrives: one summary line plus the failure, printed once
    w.show(tool(DisplayKind::ToolEnd, "c9", "bash", "exit 0, 40 lines", Some(true)), 80.0);
    w.show(d(DisplayKind::Text, "The delegate builds. One edit missed its anchor; I re-read and fixed it.\n"), 84.0);
    w.info.on_state(json!({"busy": false}), w.at(85.0));
    out.push_str("-- frozen\n");
    out.push_str(&w.screen(100, 24, 86.0, false));
    // expanded: the transcript view (Ctrl-O) on the activity, and the /details copy
    let i = w.t.last_activity().unwrap();
    w.t.get_mut(i).unwrap().expanded = true;
    let mut vs = super::transcript::ViewState { sel: i, top: i };
    out.push_str("-- expanded (Ctrl-O)\n== 100x40 ==\n");
    out.push_str(&grid(&super::transcript::render(&w.t, &mut vs, 100, 40, false, w.at(86.0)), 100));
    // /details: the same activity printed in full into the main screen (with the first request's
    // token line, which came before the first call)
    let d_ = d;
    let mut d = World::new();
    assert_eq!(d.t.details(NOW_MS), Err("no tool activity yet"));
    busy_turn(&mut d);
    assert!(d.t.details(NOW_MS).is_err(), "still running");
    d.show(Display { detail: Some("warning: unused import `Read`\n --> src/delegate.rs:3:5".into()), ..tool(DisplayKind::ToolEnd, "c9", "bash", "exit 0, 40 lines", Some(true)) }, 80.0);
    d.show(d_(DisplayKind::Thinking, "The build passed."), 81.0);
    d.show(d_(DisplayKind::Text, "Built.\n"), 84.0);
    d.t.details(NOW_MS).unwrap();
    let copy = d.t.blocks.back().map(|b| b.kind.clone());
    assert!(matches!(copy, Some(Kind::Activity(a)) if a.full && matches!(&a.steps[0], crate::model::Step::Tokens(x) if x.starts_with("tokens: in 118497"))));
    out.push_str("-- /details (printed in full; the tail of the screen)\n");
    out.push_str(&d.screen(100, 30, 86.0, false));
    check("tuiux_activity", &out);
}

#[test]
fn tuiux_failed_single_and_thinking_states() {
    let mut out = String::new();
    // a single call freezes to its own row; two failures both stay visible
    let mut w = World::new();
    w.show(tool(DisplayKind::ToolStart, "a", "bash", "cat SELF.md", None), 0.0);
    w.show(tool(DisplayKind::ToolEnd, "a", "bash", "exit 0, 96 lines", Some(true)), 0.3);
    w.show(d(DisplayKind::Text, "Read it.\n"), 1.0);
    for (i, ok) in [true, false, true, true, true, false].iter().enumerate() {
        let cid = format!("f{i}");
        w.show(tool(DisplayKind::ToolStart, &cid, "bash", &format!("cargo test -p crate{i}"), None), 2.0 + i as f64);
        w.show(tool(DisplayKind::ToolEnd, &cid, "bash", if *ok { "exit 0" } else { "exit 101, 12 lines" }, Some(*ok)), 2.5 + i as f64);
    }
    out.push_str("-- failures never fold (live)\n");
    out.push_str(&w.screen(80, 24, 8.0, false));
    // thinking between calls: the header says so, and verbose shows its tail
    w.show(d(DisplayKind::Thinking, "Two crates fail.\nBoth on the same assert.\nCheck the fixture first."), 8.2);
    out.push_str("-- thinking between calls\n");
    out.push_str(&w.screen(80, 24, 9.0, false));
    out.push_str(&w.screen(80, 24, 9.0, true));
    // an interrupt: the run ends with a call still running -> no result, visible
    w.show(tool(DisplayKind::ToolStart, "z", "bash", "sleep 600", None), 9.5);
    w.info.on_state(json!({"busy": false}), w.at(12.0));
    w.t.end_activity(w.at(12.0));
    out.push_str("-- frozen with failures and an interrupted call\n");
    out.push_str(&w.screen(80, 24, 12.0, false));
    check("tuiux_failed", &out);
}

#[test]
fn tuiux_token_segment_in_the_status_bar() {
    let mut out = String::new();
    let mut observed = egress(1_230_000, 5_000_000);
    observed["money"]["enforced"] = json!(false);
    observed["money"]["policy"] = json!("observe");
    type Setup = Box<dyn Fn(&mut World)>;
    let cases: Vec<(&str, Setup)> = vec![
        (
            "turn with cache",
            Box::new(|w| {
                w.info.begin_turn();
                w.info.on_tokens("tokens: in 118497 (cached 117760), out 961");
                w.info.on_tokens("tokens: in 119586 (cached 119424), out 79");
                let mut e = egress(1_234_200, 5_000_000);
                e["money"]["total_spent_musd"] = json!(1_234_200);
                w.info.on_egress(e);
            }),
        ),
        (
            "no cache field",
            Box::new(|w| {
                w.info.begin_turn();
                w.info.on_tokens("tokens: in 5, out 6");
            }),
        ),
        ("no token lines yet", Box::new(|_| {})),
    ];
    for (name, f) in cases {
        for width in [140, 100, 80, 40] {
            let mut w = World::new();
            w.info.on_egress(observed.clone());
            f(&mut w);
            let l = super::status::bar(&w.info, width, w.at(41.0), NOW_MS, 3, false);
            out.push_str(&format!("-- {name} @{width}\n"));
            out.push_str(&grid(&[l], width));
        }
    }
    check("tuiux_status", &out);
}

fn sub(kind: DisplayKind, parent: &str, id: &str, name: &str, text: &str, ok: Option<bool>) -> Display {
    Display { parent: Some(parent.into()), ..tool(kind, &format!("{parent}/{id}"), name, text, ok) }
}

/// DELEGATE: a sub-task's calls (`parent_call_id`) are nested and collapsed under their delegate
/// row: one `↳` line per delegate while running (with the running inner call), and after; the
/// expanded view lists them one level deeper. Two parallel delegates each get their own line.
#[test]
fn delegate_nested_activity_collapsed_and_expanded() {
    let mut out = String::new();
    let mut w = World::new();
    w.show(tool(DisplayKind::ToolStart, "d1", "delegate", "find every caller of render_subtask", None), 0.0);
    w.show(tool(DisplayKind::ToolStart, "d2", "delegate", "review src/loop/delegate.rs", None), 0.0);
    w.show(sub(DisplayKind::ToolStart, "d1", "c1", "bash", "rg -n render_subtask src", None), 0.5);
    w.show(sub(DisplayKind::ToolEnd, "d1", "c1", "bash", "exit 0, 4 lines", Some(true)), 0.9);
    w.show(sub(DisplayKind::ToolStart, "d2", "c1", "read", "read src/loop/delegate.rs", None), 0.6);
    w.show(sub(DisplayKind::ToolEnd, "d2", "c1", "read", "612 lines", Some(true)), 0.7);
    w.show(sub(DisplayKind::ToolStart, "d2", "c2", "write", "write notes.md", None), 1.0);
    w.show(sub(DisplayKind::ToolEnd, "d2", "c2", "write", "error: not_available", Some(false)), 1.0);
    w.show(sub(DisplayKind::ToolStart, "d1", "c2", "read", "read src/context/engine.rs", None), 1.2);
    out.push_str("-- two delegates running, nested calls collapsed\n");
    out.push_str(&w.screen(100, 16, 2.0, false));
    w.show(sub(DisplayKind::ToolEnd, "d1", "c2", "read", "1424 lines", Some(true)), 2.5);
    w.show(tool(DisplayKind::ToolEnd, "d2", "delegate", "done · 2 calls", Some(true)), 2.6);
    w.show(tool(DisplayKind::ToolEnd, "d1", "delegate", "done · 2 calls", Some(true)), 3.0);
    w.show(d(DisplayKind::Text, "Both scouts reported.\n"), 3.5);
    w.info.on_state(json!({"busy": false}), w.at(3.6));
    out.push_str("-- frozen: the summary counts the two delegate calls only\n");
    out.push_str(&w.screen(100, 16, 4.0, false));
    let i = w.t.last_activity().unwrap();
    w.t.get_mut(i).unwrap().expanded = true;
    let mut vs = super::transcript::ViewState { sel: i, top: i };
    out.push_str("-- expanded (Ctrl-O): sub-task calls one level deeper\n== 100x60 ==\n");
    out.push_str(&grid(&super::transcript::render(&w.t, &mut vs, 100, 60, false, w.at(4.0)), 100));
    // one delegate alone: its row and its nested line stay together when frozen
    let mut s = World::new();
    s.show(tool(DisplayKind::ToolStart, "d9", "delegate", "count the tests", None), 0.0);
    s.show(sub(DisplayKind::ToolStart, "d9", "c1", "bash", "cargo test -- --list", None), 0.2);
    s.show(sub(DisplayKind::ToolEnd, "d9", "c1", "bash", "exit 0, 312 lines", Some(true)), 1.2);
    s.show(tool(DisplayKind::ToolEnd, "d9", "delegate", "done · 1 call", Some(true)), 1.5);
    s.show(d(DisplayKind::Text, "312 tests.\n"), 2.0);
    s.info.on_state(json!({"busy": false}), s.at(2.1));
    out.push_str("-- one delegate, frozen\n");
    out.push_str(&s.screen(80, 10, 2.5, false));
    check("delegate_nested", &out);
}
