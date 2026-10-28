//! The lineage panel at the foot of the transcript view (Ctrl-O; docs/tui.md §3.6, TUI-OBSERVE):
//! the newest records of the Root's signed, hash-chained lineage, one row each (seq, kind, what it
//! says, short hash), and the Root's own check of the chain (`verify ✓` / `verify ✕ at seq n`).
//! The data comes from the Root's `lineage.tail` request on owner.sock, asked when the view opens;
//! the TUI reads no file (T-2). An older Root without the request says so.

use super::{Line, Role, Style, sanitize};
use crate::conn::ReqErr;
use serde_json::Value;

/// Records asked for, and the most rows the panel takes (the verify line and the rule included).
pub const ASK: u64 = 8;
pub const MAX_ROWS: usize = 10;

#[derive(Debug, Default)]
pub struct Panel {
    /// The last answer (`{records, rows, verify}`), or why there is none.
    pub answer: Option<Result<Value, String>>,
    /// A request is on its way.
    pub pending: bool,
}

impl Panel {
    pub fn on_answer(&mut self, r: Result<Value, ReqErr>) {
        self.pending = false;
        self.answer = Some(r.map_err(|e| match e {
            ReqErr::Refused(code, _, _) if code == "root.unknown_cmd" => "this Root has no lineage view (it predates v0.5.0)".to_string(),
            ReqErr::Refused(code, m, _) => format!("{code}: {m}"),
            ReqErr::Unreachable(e) => format!("the Root is not reachable ({e})"),
            ReqErr::Proto(m) => m,
        }));
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn sha12(v: &Value, k: &str) -> String {
    s(v, k).chars().take(12).collect()
}

/// What one record says, and the role that carries it: kept, refused, rolled back (by the kernel
/// or, in the Root's colour, by the Root), or just its kind.
fn says(r: &Value) -> (String, Style) {
    let d = &r["data"];
    let by_root = s(r, "by") == "root";
    match s(r, "kind") {
        "gate" => {
            let what = format!("{} {} {}", s(d, "verdict"), s(d, "reason"), sha12(d, "sha"));
            let st = match s(d, "verdict") {
                "accept" => Style::fg(Role::Pass),
                "reject" => Style::fg(Role::Harm),
                _ => Style::dim(),
            };
            (what, st)
        }
        "promotion" if d["lkg"] == true => (format!("kept (LKG) {}", sha12(d, "sha")), Style::fg(Role::Pass)),
        "promotion" => (format!("live, provisional {}", sha12(d, "sha")), Style::dim()),
        "body_rollback" => (format!("rolled back {} → {} {}", sha12(d, "from_sha"), sha12(d, "to_sha"), s(d, "reason")), Style::fg(Role::Harm)),
        "rollback" if by_root => (format!("Root rollback → {} {}", s(d, "to"), s(d, "reason")), Style::fg(Role::Root)),
        "lkg" if by_root => (format!("LKG tuple {}", s(d, "tuple")), Style::fg(Role::Root)),
        _ => (String::new(), Style::dim()),
    }
}

/// The panel's rows: a rule, the verify line, then the records (oldest first).
pub fn rows(p: &Panel, width: usize) -> Vec<Line> {
    let mut out = vec![Line::from_span("── lineage (signed by the Root) ", Style::fg(Role::Line)).pad(width, Style::fg(Role::Line)).truncate(width)];
    let ans = match &p.answer {
        None if p.pending => return [out, vec![Line::from_span("  asking the Root…", Style::dim())]].concat(),
        None => return [out, vec![Line::from_span("  not asked yet", Style::dim())]].concat(),
        Some(Err(e)) => {
            out.push(sanitize::inline(&format!("  lineage: {e}"), Style::fg(Role::Warn)).truncate(width));
            return out;
        }
        Some(Ok(a)) => a,
    };
    let v = &ans["verify"];
    let mut l = Line::from_span("  ", Style::PLAIN);
    if v["ok"] == true {
        l.push("verify ✓", Style::fg(Role::Pass));
        let n = v["records"].as_u64().unwrap_or(0);
        let sigs = v["sigs_checked"].as_u64().unwrap_or(0);
        let scope = if sigs >= n { "every signature".to_string() } else { format!("signatures of the last {sigs}") };
        let head = format!(" {n} records · {scope} · head {} #{}", v["head_seq"].as_u64().unwrap_or(0),
            s(v, "head_hash").trim_start_matches("sha256:").chars().take(12).collect::<String>());
        l.extend(sanitize::inline(&head, Style::dim()));
    } else {
        l.push("verify ✕", Style::fg(Role::Harm));
        let e = &v["error"];
        l.extend(sanitize::inline(&format!(" at seq {}: {}", e["seq"].as_u64().unwrap_or(0), s(e, "rule")), Style::fg(Role::Harm)));
    }
    if !s(v, "key").is_empty() {
        l.extend(sanitize::inline(&format!(" · key {}", s(v, "key")), Style::fg(Role::Mute)));
    }
    out.push(l.truncate(width));
    let recs: Vec<&Value> = ans["rows"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let room = MAX_ROWS.saturating_sub(out.len());
    for r in recs.iter().skip(recs.len().saturating_sub(room)) {
        let mut l = Line::from_span(format!("  {:>6} ", r["seq"].as_u64().map_or("?".into(), |x| x.to_string())), Style::fg(Role::Mute));
        l.extend(sanitize::inline(&format!("{:<14}", s(r, "kind")), Style::PLAIN));
        let (what, st) = says(r);
        // the signer's role (`kernel:<build>` → `kernel`): the build is in `init-root audit verify`
        let by = s(r, "by").split(':').next().unwrap_or("").to_string();
        let mut right = Line::new();
        right.extend(sanitize::inline(&format!(" {by} #{}", s(r, "hash")), Style::fg(Role::Mute)));
        l.extend(sanitize::inline(&what, st));
        let rw = right.width();
        let mut row = l.truncate(width.saturating_sub(rw + 1));
        let pad = width.saturating_sub(row.width() + rw);
        row.push(" ".repeat(pad), Style::PLAIN);
        row.extend(right);
        out.push(row.truncate(width));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::grid;
    use serde_json::json;

    fn answer(ok: bool) -> Value {
        let rec = |seq: u64, by: &str, kind: &str, data: Value, hash: &str| json!({"seq": seq, "at": "2026-10-27T10:00:00.000Z", "by": by, "kind": kind, "data": data, "hash": hash});
        json!({"records": 412, "rows": [
            rec(405, "kernel:3f9c0a1b2c4d", "gate", json!({"verdict": "accept", "reason": "g0_g3", "sha": "06be87a1234567890"}), "a1b2c3d4e5f6"),
            rec(406, "kernel:3f9c0a1b2c4d", "promotion", json!({"sha": "06be87a1234567890", "lkg": false}), "b2c3d4e5f6a1"),
            rec(407, "kernel:3f9c0a1b2c4d", "gate", json!({"verdict": "reject", "reason": "probe_exit", "sha": "77aa00bb11cc22dd"}), "c3d4e5f6a1b2"),
            rec(408, "kernel:3f9c0a1b2c4d", "promotion", json!({"sha": "06be87a1234567890", "lkg": true}), "d4e5f6a1b2c3"),
            rec(409, "kernel:06be87a12345", "body_rollback", json!({"from_sha": "88bb", "to_sha": "06be87a1234567890", "reason": "gate.g4.score_inferior"}), "e5f6a1b2c3d4"),
            rec(410, "root", "rollback", json!({"from": "tB", "to": "tA", "reason": "harm"}), "f6a1b2c3d4e5"),
            rec(411, "root", "turns\u{1b}[2J", json!({}), "0a1b2c3d4e5f"),
        ], "verify": if ok {
            json!({"ok": true, "records": 412, "sigs_checked": 412, "head_seq": 411, "head_hash": "sha256:0a1b2c3d4e5f6789", "key": "3f9c0a1b2c4d5e6f", "error": null})
        } else {
            json!({"ok": false, "records": 412, "error": {"seq": 407, "rule": "4: hash does not recompute"}, "key": "3f9c0a1b2c4d5e6f"})
        }})
    }

    /// The panel at 100 columns: verify ✓ with the records; verify ✕ at a seq; an older Root; asking.
    #[test]
    fn lineage_panel() {
        let mut out = String::new();
        let mut p = Panel { answer: None, pending: true };
        out.push_str(&format!("-- asking\n{}", grid(&rows(&p, 100), 100)));
        p.on_answer(Ok(answer(true)));
        let r = rows(&p, 100);
        assert!(r.len() <= MAX_ROWS);
        assert!(r.iter().all(|l| !l.text().contains('\u{1b}')), "zone text is inert");
        out.push_str(&format!("-- verify ok\n{}", grid(&r, 100)));
        p.on_answer(Ok(answer(false)));
        out.push_str(&format!("-- verify failed\n{}", grid(&rows(&p, 100), 100)));
        p.on_answer(Err(ReqErr::Refused("root.unknown_cmd".into(), "unknown command".into(), Value::Null)));
        out.push_str(&format!("-- an older Root\n{}", grid(&rows(&p, 100), 100)));
        p.on_answer(Ok(answer(true)));
        out.push_str(&format!("-- narrow\n{}", grid(&rows(&p, 60), 60)));
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/lineage_panel.txt");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &out).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path).expect("missing golden lineage_panel (run with UPDATE_GOLDEN=1)");
        assert_eq!(want, out);
    }
}
