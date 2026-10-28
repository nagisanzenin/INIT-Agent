//! TUI-OBSERVE goldens that need the whole live region: the failed-call tail (live, frozen, single
//! call, and the transcript view's `detail`), and a self-change in flight above the input box with
//! the plain notices it replaced gone.

use super::live::{self, RowOpts, Ui};
use super::{Line, grid};
use crate::input::Editor;
use crate::model::{Conn, Info, Kind, Transcript, Tray};
use crate::proto::{Display, DisplayKind};
use serde_json::json;
use std::time::{Duration, Instant};

const NOW_MS: i64 = 1_792_530_245_000; // 2026-10-20T21:04:05Z

fn golden(name: &str, got: &str) {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing golden {} (run with UPDATE_GOLDEN=1)", p.display()));
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        assert_eq!(a, b, "golden {name} differs at line {}", i + 1);
    }
    assert_eq!(want.lines().count(), got.lines().count(), "golden {name} differs in length");
}

fn call(kind: DisplayKind, id: &str, name: &str, text: &str, ok: Option<bool>, detail: Option<&str>) -> Display {
    Display { kind, text: text.into(), call_id: Some(id.into()), name: Some(name.into()), ok, boot_seq: Some(1), detail: detail.map(str::to_string), parent: None }
}

const TAIL: &str = "error: the candidate is not on a branch under evo/\n  hint: commit on evo/<name> and promote that ref\nusage: initctl promote <ref> [--wait]\n\
                    see `initctl help promote`\nline 5\nline 6\nexit status 1";

fn info() -> Info {
    let mut info = Info::default();
    info.conn = Conn::Live;
    info.on_state(json!({"slot": "running", "mode": "normal", "busy": true, "sha12": "3f9c0a1b2c4d", "queued": 0, "level": 0, "members": 0}), Instant::now());
    info
}

/// The live region at `w`×`h` after committing what is finished (commits are returned too).
fn screen(t: &mut Transcript, w: usize, h: usize, now: Instant, now_ms: i64) -> String {
    let (tray, ed, info) = (Tray::default(), Editor::default(), info());
    let max = {
        let ui = Ui { t, tray: &tray, ed: &ed, info: &info, verbose: false, overlay: false, hint: None, now, now_ms, no_color: false, spin: 3 };
        live::max_live(&ui, w, h)
    };
    let mut rows: Vec<Line> = live::commit(t, w, max, false, now, 3);
    let ui = Ui { t, tray: &tray, ed: &ed, info: &info, verbose: false, overlay: false, hint: None, now, now_ms, no_color: false, spin: 3 };
    rows.extend(live::region(&ui, w, h).rows);
    for r in &rows {
        assert!(r.width() <= w, "row wider than {w}: {:?}", r.text());
    }
    format!("== {w}x{h} ==\n{}", grid(&rows, w))
}

/// "The output above says why" is now true: a failed call's last lines show under it (3 while the
/// activity runs, all 10 once it is frozen, all in the transcript view); a call that worked shows none.
#[test]
fn failed_call_tail_live_frozen_and_expanded() {
    let base = Instant::now();
    let at = |s: f64| base + Duration::from_secs_f64(s);
    let mut out = String::new();
    let mut t = Transcript::default();
    t.display(&call(DisplayKind::ToolStart, "c1", "bash", "initctl promote evo/timeout-flag --wait", None, None), at(0.0), NOW_MS);
    t.display(&call(DisplayKind::ToolEnd, "c1", "bash", "exit 1, 7 lines", Some(false), Some(TAIL)), at(0.6), NOW_MS);
    t.display(&call(DisplayKind::ToolStart, "c2", "bash", "cargo test -p init-body", None, None), at(0.7), NOW_MS);
    t.display(&call(DisplayKind::ToolEnd, "c2", "bash", "exit 0, 40 lines", Some(true), Some("never shown")), at(2.0), NOW_MS);
    t.display(&call(DisplayKind::ToolStart, "c3", "read", "read SELF.md", None, None), at(2.1), NOW_MS);
    out.push_str("-- live: 3 lines under the failed call\n");
    out.push_str(&screen(&mut t, 80, 20, at(3.0), NOW_MS));
    t.display(&call(DisplayKind::ToolEnd, "c3", "read", "96 lines", Some(true), None), at(3.1), NOW_MS);
    t.display(&Display { kind: DisplayKind::Text, text: "The promote did not start; the output above says why.\n".into(), call_id: None, name: None, ok: None, boot_seq: Some(1), detail: None, parent: None }, at(3.5), NOW_MS);
    t.push(Kind::Divider("end".into()), NOW_MS);
    out.push_str("-- frozen: every line under the failed call\n");
    out.push_str(&screen(&mut t, 80, 24, at(4.0), NOW_MS));
    // a single failed call
    let mut t = Transcript::default();
    t.display(&call(DisplayKind::ToolStart, "c1", "bash", "initctl promote evo/x --wait", None, None), at(0.0), NOW_MS);
    t.display(&call(DisplayKind::ToolEnd, "c1", "bash", "exit 1, 7 lines", Some(false), Some(TAIL)), at(0.6), NOW_MS);
    t.end_activity(at(0.7));
    let i = t.last_activity().unwrap();
    let rows = live::block_rows(&t.get(i).unwrap().kind, 80, at(1.0), &RowOpts { verbose: false, expanded: false, spin: 0, first: false });
    out.push_str(&format!("-- frozen single call\n{}", grid(&rows, 80)));
    let rows = live::block_rows(&t.get(i).unwrap().kind, 80, at(1.0), &RowOpts { verbose: false, expanded: true, spin: 0, first: false });
    out.push_str(&format!("-- transcript view (expanded)\n{}", grid(&rows, 80)));
    golden("observe_failed_tail", &out);
}

/// A self-change in flight sits above the input box (never pushing it off), its notices are not
/// printed as plain lines, and once it ends one committed line remains.
#[test]
fn gate_block_in_the_live_region() {
    let base = Instant::now();
    let mut out = String::new();
    let mut t = Transcript::default();
    t.display(&call(DisplayKind::ToolStart, "c1", "bash", "initctl promote evo/compaction --wait", None, None), base, NOW_MS);
    let n = |t: &mut Transcript, d: serde_json::Value, ms: i64| {
        let done = t.gates.on_notice(&d, NOW_MS + ms).expect("a gate-track notice");
        for r in done {
            t.push(Kind::Gate(Box::new(r)), NOW_MS + ms);
        }
    };
    n(&mut t, json!({"kind": "gate_start", "gate_id": "j-9", "sha": "06be87a1234567890", "parent": "3f9c0a1b2c4d5e6f", "scope": "t2",
        "steps": ["G0", "G1", "G2", "G3", "handoff", "G4"]}), 0);
    let g = |stage: &str| json!({"kind": "gate", "gate_kind": "promote", "sha": "06be87a1234567890", "stage": stage, "pass": true, "reason": null, "wall_ms": 2_000});
    n(&mut t, g("G0"), 9_000);
    n(&mut t, g("G1"), 9_500);
    out.push_str("-- G2 running, the promote call running\n");
    out.push_str(&screen(&mut t, 80, 18, base + Duration::from_secs(12), NOW_MS + 12_000));
    out.push_str(&screen(&mut t, 80, 12, base + Duration::from_secs(12), NOW_MS + 12_000));
    n(&mut t, g("G2"), 12_500);
    n(&mut t, g("G3"), 30_000);
    n(&mut t, json!({"kind": "promoted", "sha": "06be87a1234567890", "handoff": true, "handoff_ms": 4_600}), 34_600);
    n(&mut t, json!({"kind": "gate", "gate_kind": "g4", "sha": "06be87a1234567890", "stage": "G4", "pass": true, "reason": null, "wall_ms": 900}), 35_600);
    n(&mut t, json!({"kind": "lkg", "sha": "06be87a1234567890"}), 35_700);
    t.display(&call(DisplayKind::ToolEnd, "c1", "bash", "exit 0, 3 lines", Some(true), None), base + Duration::from_secs(36), NOW_MS);
    out.push_str("-- kept: one line\n");
    out.push_str(&screen(&mut t, 80, 12, base + Duration::from_secs(37), NOW_MS + 37_000));
    golden("observe_gate_region", &out);
}
