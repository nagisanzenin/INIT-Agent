//! TUI-BRAND: the Root's JSON answers (`/root status`, `/root ledger`, docs/tui.md §3.10) as aligned
//! key/value tables instead of minified JSON cut mid-token. Every field the Root sent is shown: the
//! known ones in a fixed, readable order, the rest after them in the Root's (sorted) order. Values
//! pass the sanitizer like any frame text (§3.13). Line mode keeps the raw JSON line for scripts.

use super::{Line, Style, sanitize, usd, wrap};
use serde_json::{Map, Value};

/// The key column's width.
const KEY_W: usize = 12;

/// A table for a Root answer `[root] {json}`: its title (`status`, `ledger`, `answer`) and rows.
/// None when the text after `[root] ` is not a JSON object.
pub fn root_answer(rest: &str) -> Option<(&'static str, Vec<(String, String)>)> {
    let rest = rest.trim();
    if !rest.starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(rest).ok()?;
    let o = v.as_object()?;
    if o.contains_key("slot") && o.contains_key("tuples") {
        Some(("status", status_rows(o)))
    } else if o.contains_key("path") && o.contains_key("entered_seq") {
        Some(("ledger", ledger_rows(o)))
    } else {
        Some(("answer", rest_rows(o, &mut Vec::new())))
    }
}

/// The rows of a table under a title line: `[root] status` (the tag in the Root's colour when the
/// Root stamped the frame), then `  key         value`, values wrapped under the value column.
pub fn rows(tag: Line, title: &str, rows: &[(String, String)], width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let mut head = tag;
    head.push(" ", Style::PLAIN);
    head.push(title, Style::dim());
    out.push(head.truncate(width));
    let indent = 2 + KEY_W + 1;
    for (k, v) in rows {
        let mut l = Line::from_span("  ", Style::PLAIN);
        let key = sanitize::inline(k, Style::mute());
        let kw = key.width();
        l.extend(key);
        l.push(" ".repeat((KEY_W + 1).saturating_sub(kw).max(1)), Style::PLAIN);
        l.extend(sanitize::inline(v, Style::PLAIN));
        out.extend(wrap::wrap(&l, width, indent.min(width / 2)));
    }
    out
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => "none".into(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) if a.is_empty() => "none".into(),
        Value::Object(o) if o.is_empty() => "none".into(),
        Value::Array(a) => a.iter().map(s).collect::<Vec<_>>().join(" · "),
        Value::Object(o) => o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| format!("{k} {}", s(v))).collect::<Vec<_>>().join(" · "),
    }
}

/// A short hash: `sha256:5e0efc71bf37…`, or an id cut to 12.
fn short(v: &Value) -> String {
    let t = s(v);
    match t.split_once(':') {
        Some((alg, h)) if h.len() > 12 => format!("{alg}:{}…", &h[..12]),
        _ if t.len() > 16 && t.is_ascii() => format!("{}…", &t[..12]),
        _ => t,
    }
}

fn hms(v: &Value) -> String {
    v.as_str().and_then(crate::time::parse_rfc3339).map_or_else(|| s(v), crate::time::utc_hms)
}

fn level(o: &Map<String, Value>, used: &mut Vec<&'static str>) -> String {
    used.extend(["level", "cap", "max_level", "supported_max_level"]);
    let l = |k: &str| o.get(k).and_then(Value::as_i64).map_or_else(|| "?".into(), |n| format!("L{n}"));
    format!("{} · cap {} · max {} · supported {}", l("level"), l("cap"), l("max_level"), l("supported_max_level"))
}

/// Any field not shown yet, in the Root's order (`v` is the format version, not news).
fn rest_rows(o: &Map<String, Value>, used: &mut Vec<&'static str>) -> Vec<(String, String)> {
    used.push("v");
    o.iter().filter(|(k, _)| !used.contains(&k.as_str())).map(|(k, v)| (k.replace('_', " "), s(v))).collect()
}

fn status_rows(o: &Map<String, Value>) -> Vec<(String, String)> {
    let mut used: Vec<&'static str> = Vec::new();
    let mut rows: Vec<(String, String)> = Vec::new();
    let g = |k: &str| o.get(k).unwrap_or(&Value::Null);
    let mut state = format!("{} · mode {}", s(g("slot")), s(g("mode")));
    if g("queued").as_u64().is_some_and(|q| q > 0) {
        state.push_str(&format!(" · queued {}", s(g("queued"))));
    }
    used.extend(["slot", "mode", "queued"]);
    rows.push(("state".into(), state));
    if !g("panic").is_null() {
        rows.push(("panic".into(), s(g("panic"))));
    }
    if !g("halted").is_null() {
        rows.push(("halted".into(), s(g("halted"))));
    }
    used.extend(["panic", "halted"]);
    rows.push(("level".into(), level(o, &mut used)));
    let t = g("tuples");
    let mut tuples = format!("current {} · lkg {} · audited {}", s(&t["current"]), s(&t["lkg"]), s(&t["audited"]));
    for k in ["settled", "twin"] {
        if !t[k].is_null() {
            tuples.push_str(&format!(" · {k} {}", s(&t[k])));
        }
    }
    used.push("tuples");
    rows.push(("tuples".into(), tuples));
    let k = g("kernel");
    let kernel = if k.is_null() {
        "none running".into()
    } else {
        let ready = if k["ready"] == true { "ready" } else { "not ready" };
        format!("{ready} · boot {} · pid {} · {}", s(&k["boot_seq"]), s(&k["pid"]), s(&k["reason"]))
    };
    used.push("kernel");
    rows.push(("kernel".into(), kernel));
    if let Some(a) = g("instances").as_array() {
        let list: Vec<String> = a
            .iter()
            .map(|i| {
                let lane = i["lane"].as_str().map(|l| format!("{l}, ")).unwrap_or_default();
                format!("{} {} ({lane}pid {})", s(&i["kind"]), short(&i["instance"]), s(&i["pid"]))
            })
            .collect();
        rows.push(("instances".into(), if list.is_empty() { "none".into() } else { list.join(" · ") }));
        used.push("instances");
    }
    let c = g("ceilings");
    if c.is_object() {
        // TUI-FIX (v0.5.1): the charter's dollar amounts are records, not caps, since the owner's
        // 2026-10-23 decision (docs/guide.md "Spending"); "ceilings" beside "caps off" read as a cap.
        // A Root that enforces them says so (`ceilings.enforced: true`, docs/tui.md §9 Q5).
        let m = |k: &str| c[k].as_i64().map_or_else(|| "none".into(), usd);
        let (key, tail) = if c["enforced"] == true { ("ceilings", "") } else { ("dollar refs", " (charter records, not caps)") };
        rows.push((key.into(), format!("{} a day · {} in all{tail}", m("day_musd"), m("total_musd"))));
        used.push("ceilings");
    }
    let lin = g("lineage");
    if lin.is_object() {
        rows.push(("lineage".into(), format!("seq {} · {}", s(&lin["seq"]), short(&lin["hash"]))));
        used.push("lineage");
    }
    let mi = g("mirror");
    if mi.is_object() {
        let mut m = s(&mi["state"]);
        if !mi["pushed_seq"].is_null() {
            m.push_str(&format!(" · pushed seq {}", s(&mi["pushed_seq"])));
        }
        if !mi["last_push_at"].is_null() {
            m.push_str(&format!(" · last push {}", hms(&mi["last_push_at"])));
        }
        if !mi["last_error"].is_null() {
            m.push_str(&format!(" · last error {}", s(&mi["last_error"])));
        }
        rows.push(("mirror".into(), m));
        used.push("mirror");
    }
    rows.push(("last audit".into(), s(g("last_audit"))));
    used.push("last_audit");
    if o.contains_key("charter_sha256") {
        let pending = if g("charter_pending") == &Value::Bool(true) { " · a change is pending (/root help)" } else { "" };
        rows.push(("charter".into(), format!("{}{pending}", short(g("charter_sha256")))));
        used.extend(["charter_sha256", "charter_pending"]);
    }
    let r = g("root");
    if r.is_object() {
        let mut root = format!("{} · pid {} · started {}", s(&r["version"]), s(&r["pid"]), hms(&r["started_at"]));
        if r["test_mode"] == true {
            root.push_str(" · test mode");
        }
        rows.push(("root".into(), root));
        used.push("root");
    }
    rows.extend(rest_rows(o, &mut used));
    rows
}

fn ledger_rows(o: &Map<String, Value>) -> Vec<(String, String)> {
    let mut used: Vec<&'static str> = Vec::new();
    let g = |k: &str| o.get(k).unwrap_or(&Value::Null);
    let mut rows = vec![("level".to_string(), level(o, &mut used))];
    rows.push(("path".into(), format!("{} · since {} (record {})", s(g("path")), hms(g("entered_at")), s(g("entered_seq")))));
    used.extend(["path", "entered_at", "entered_seq"]);
    for (k, label) in [("last_change", "last change"), ("next", "next"), ("incident", "incident")] {
        rows.push((label.into(), s(g(k))));
        used.push(k);
    }
    if let Some(p) = g("evolution_share_max_permille").as_i64() {
        rows.push(("evo share".into(), format!("at most {}.{} %", p / 10, p % 10)));
    }
    used.push("evolution_share_max_permille");
    rows.push(("record".into(), format!("seq {}", s(g("seq")))));
    used.push("seq");
    rows.extend(rest_rows(o, &mut used));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Role;

    const STATUS: &str = r#"{"cap":0,"ceilings":{"day_musd":5000000,"total_musd":25000000},"charter_pending":false,"charter_sha256":"02690f3c2064ac92a4823429d001cb9315279b161584bde890f70b82b1be2bd3","halted":null,"instances":[{"instance":"62eedcb71882","kind":"body","lane":"b0","pid":20070,"started_at":"2026-10-27T10:52:13.588Z"},{"instance":"888eb55e6751","kind":"kernel","lane":null,"pid":19998,"started_at":"2026-10-27T10:52:07.628Z"}],"kernel":{"boot_seq":1,"instance":"888eb55e6751","pid":19998,"ready":true,"reason":"cold"},"last_audit":null,"level":0,"lineage":{"hash":"sha256:5e0efc71bf37a0bbee06db6ddb7979de5af0a5af08bc5ea0f66a4051ac0fc367","seq":13},"max_level":4,"mirror":{"last_error":null,"last_push_at":null,"pushed_seq":null,"state":"off"},"mode":"normal","panic":null,"queued":0,"root":{"epoch":"2026-10-27T10:52:07.410Z/19993","now":"2026-10-27T10:54:59.345Z","pid":19993,"started_at":"2026-10-27T10:52:07.410Z","test_mode":true,"version":"0.4.1"},"slot":"running","supported_max_level":0,"sweep":"held_only","tuples":{"audited":"t72995ada3f8d","current":"t72995ada3f8d","lkg":"t72995ada3f8d","settled":null,"twin":null},"v":1,"new_field":{"x":1}}"#;
    const LEDGER: &str = r#"{"cap":0,"entered_at":"2026-10-27T10:49:45.957Z","entered_seq":0,"evolution_share_max_permille":null,"incident":null,"last_change":null,"level":0,"max_level":4,"next":null,"path":"gated","seq":13,"supported_max_level":0,"v":1}"#;

    fn text(title: &str, r: &[(String, String)], w: usize) -> String {
        rows(Line::from_span("[root]", Style::fg(Role::Root)), title, r, w).iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn status_as_a_table_with_every_field() {
        let (title, r) = root_answer(STATUS).unwrap();
        assert_eq!(title, "status");
        let t = text(title, &r, 100);
        for want in [
            "[root] status",
            "  state        running · mode normal",
            "  level        L0 · cap L0 · max L4 · supported L0",
            "  tuples       current t72995ada3f8d · lkg t72995ada3f8d · audited t72995ada3f8d",
            "  kernel       ready · boot 1 · pid 19998 · cold",
            "  instances    body 62eedcb71882 (b0, pid 20070) · kernel 888eb55e6751 (pid 19998)",
            "  dollar refs  $5.00 a day · $25.00 in all (charter records, not caps)",
            "  lineage      seq 13 · sha256:5e0efc71bf37…",
            "  mirror       off",
            "  charter      02690f3c2064…",
            "  root         0.4.1 · pid 19993 · started 10:52:07Z · test mode",
            "  sweep        held_only",
            "  new field    x 1",
        ] {
            assert!(t.contains(want), "missing {want:?} in\n{t}");
        }
        assert!(!t.contains('{') && !t.contains("\"v\""), "{t}");
        assert!(!t.contains("ceilings"), "no cap is implied: {t}");
        // a Root that enforces its dollar amounts keeps the word
        let enforced = STATUS.replace(r#""ceilings":{"day_musd""#, r#""ceilings":{"enforced":true,"day_musd""#);
        let (_, r2) = root_answer(&enforced).unwrap();
        assert!(text(title, &r2, 100).contains("  ceilings     $5.00 a day · $25.00 in all\n"), "{}", text(title, &r2, 100));
        // narrow: values wrap under the value column, nothing is wider than the screen
        for l in rows(Line::from_span("[root]", Style::PLAIN), title, &r, 40) {
            assert!(l.width() <= 40, "{}", l.text());
        }
    }

    #[test]
    fn ledger_and_unknown_answers() {
        let (title, r) = root_answer(LEDGER).unwrap();
        assert_eq!(title, "ledger");
        let t = text(title, &r, 100);
        assert!(t.contains("  path         gated · since 10:49:45Z (record 0)"), "{t}");
        assert!(t.contains("  record       seq 13"), "{t}");
        let (title, r) = root_answer(r#"{"b":[1,2],"a":"x"}"#).unwrap();
        assert_eq!((title, r), ("answer", vec![("a".to_string(), "x".to_string()), ("b".to_string(), "1 · 2".to_string())]));
        assert!(root_answer("not json").is_none());
        assert!(root_answer("[1,2]").is_none());
        // a hostile value stays inert
        let (_, r) = root_answer("{\"a\":\"\\u001b[2J\"}").unwrap();
        assert!(!text("answer", &r, 80).contains('\u{1b}'));
    }
}
