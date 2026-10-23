//! TUI-1 render snapshots: the renderer is a pure function of (state, width, height) → a text grid
//! with a style mask, compared with golden files under tests/golden/ (`UPDATE_GOLDEN=1` rewrites).
//! TUI-4's renderer half: no ESC from a frame survives into a row.

use super::live::{self, Ui};
use super::{Line, grid};
use crate::input::Editor;
use crate::model::{Conn, Info, Kind, OwnerSt, Transcript};
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
    ed: Editor,
    info: Info,
    base: Instant,
    scroll: Vec<Line>,
}

fn d(kind: DisplayKind, text: &str) -> Display {
    Display { kind, text: text.into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None }
}

fn tool(kind: DisplayKind, id: &str, name: &str, text: &str, ok: Option<bool>) -> Display {
    Display { kind, text: text.into(), call_id: Some(id.into()), name: Some(name.into()), ok, boot_seq: Some(1), detail: None }
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
        World { t: Transcript::default(), ed: Editor::default(), info, base, scroll: Vec::new() }
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
            let ui = Ui { t: &self.t, ed: &self.ed, info: &self.info, verbose, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
            live::max_live(&ui, w, h)
        };
        let commits = live::commit(&mut self.t, w, max, verbose, now, spin);
        self.scroll.extend(commits);
        let ui = Ui { t: &self.t, ed: &self.ed, info: &self.info, verbose, overlay: false, hint: None, now, now_ms: NOW_MS, no_color: false, spin };
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
        w.show(Display { kind: DisplayKind::ToolStart, text: evil.into(), call_id: Some(evil.into()), name: Some(evil.into()), ok: None, boot_seq: Some(1), detail: None }, 0.0);
        w.show(Display { kind: DisplayKind::ToolEnd, text: evil.into(), call_id: Some(evil.into()), name: Some(evil.into()), ok: Some(false), boot_seq: Some(1), detail: Some(evil.into()) }, 0.1);
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
        w.show(Display { kind: k, text: s.clone(), call_id: Some(id), name: Some(s.clone()), ok: Some(next() % 2 == 0), boot_seq: Some(1 + next() % 2), detail: None }, i as f64 * 1e-4);
        if i % 997 == 0 {
            w.t.notice("warn", &s, None, None, NOW_MS);
            let screen = w.screen(80, 24, i as f64 * 1e-4, i % 2 == 0);
            assert!(!screen.contains('\u{1b}') && !screen.contains('\u{9b}') && !screen.contains('\u{202e}'), "frame {i}");
        }
    }
}
