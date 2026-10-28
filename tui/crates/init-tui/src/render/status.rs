//! The status bar (docs/tui.md §3.9). Level, money and the Root state come from Root requests, which
//! no zone process can forge; body, mode, busy and model come from the kernel and are informative.

use super::{Line, Role, Style, sanitize, usd};
use crate::model::{Conn, Info, tok};
use serde_json::Value;
use std::time::{Duration, Instant};

struct Field {
    /// Higher drops first; the Root state (0) is never dropped.
    drop: u8,
    line: Line,
}

fn money(v: &Value, k: &str) -> Option<i64> {
    v.get("money")?.get(k)?.as_i64()
}

/// `1 request`, `7 requests`.
pub fn plural(n: u64, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// `/usage` (§3.8), as aligned rows (TUI-BRAND). Monetary figures are ledger/tariff estimates, not
/// reconciled provider invoices; missing or stale metering is never rendered as a free run.
pub fn usage_rows(info: &Info) -> (String, Vec<(String, String)>) {
    let egress = info.egress.as_ref().filter(|_| !info.egress_failed);
    let amount = |key| egress.and_then(|e| money(e, key)).filter(|n| *n >= 0).map(usd).unwrap_or_else(|| "unknown".into());
    let complete = egress.is_some_and(|e| e["money"]["token_totals_complete"] == true);
    let token = |key: &str| egress.and_then(|e| e["money"][key].as_u64()).map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
    let tokens = if complete {
        format!("live tokens: input {}, output {}", token("total_input_tokens"), token("total_output_tokens"))
    } else {
        format!("live token totals unknown; reported subtotal: input {}, output {}", token("reported_input_tokens"), token("reported_output_tokens"))
    };
    let evo = info.state.as_ref().and_then(|s| s["evolution_checks_today_musd"].as_i64()).filter(|n| *n >= 0)
        .map(|m| format!("evolution checks today {} estimated (gate/probe only)", usd(m)))
        .unwrap_or_else(|| "evolution checks cost unknown".into());
    let policy = if egress.is_some_and(observe_only) { "Monetary caps off." } else { "Monetary policy is reported by Root." };
    let mut rows: Vec<(&str, String)> = vec![
        ("Root ledger", format!("{} total settled estimate · {} today", amount("total_spent_musd"), amount("day_spent_musd"))),
        ("pending", format!("upper bound {} (separate from settled cost; interrupted requests may still be billed)", amount("total_reserved_musd"))),
        ("tokens", tokens),
        ("evolution", format!("{evo}. Free-evolution reasoning cost is not attributed.")),
        ("policy", format!("{policy} /stop stops the current run; /panic stops all agent activity; Ctrl-C interrupts.")),
    ];
    if info.session.requests == 0 {
        rows.push(("this TUI", "no token lines seen yet".into()));
    } else {
        let s = &info.session;
        rows.push(("this TUI", format!("{}: input {} ({} cached), output {}", plural(s.requests, "request"), s.input, s.cached, s.output)));
        rows.push(("cache hit", format!("{} (token-weighted: cached / input; ? when the provider reports no cache field)", s.cache().show())));
        rows.push(("last turn", format!("{}: {}", plural(info.turn.requests, "request"), info.turn_line())));
    }
    ("usage (USD estimated from usage and tariffs, not an invoice)".to_string(), rows.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// `/usage` as text (line mode): the title, then one `key  value` row per line.
pub fn usage_summary(info: &Info) -> String {
    let (title, rows) = usage_rows(info);
    let mut out = title;
    for (k, v) in rows {
        out.push_str(&format!("\n  {k:<12} {v}"));
    }
    out
}

fn observe_only(egress: &Value) -> bool {
    egress["money"]["enforced"] == false || egress["money"]["policy"] == "observe"
}

/// The Root's own state, the last field on the right (§3.9, TUI-BRAND): `root ok` in the Root's
/// colour while the Root answers and nothing is wrong; the harm states (PANICKED, HALTED, unreachable)
/// replace it in the harm colour. The agent's slot state, when it is not plainly running, is a word
/// beside it (`stopped`, `starting`, `safe mode`).
fn root_state(info: &Info, now: Instant, now_ms: i64) -> (Line, bool) {
    let harm = Style::fg(Role::Harm);
    match &info.conn {
        Conn::Down { retry_at } => {
            let left = retry_at.saturating_duration_since(now).as_secs_f64();
            return (Line::from_span(format!("Root unreachable · retry in {left:.1} s"), harm), true);
        }
        Conn::Connecting if info.status.is_none() => return (Line::from_span("connecting", Style::dim()), false),
        _ => {}
    }
    let Some(s) = &info.status else { return (Line::from_span("attaching", Style::dim()), false) };
    let slot = s["slot"].as_str().unwrap_or("?");
    let ok = Line::from_span("root ok", Style::fg(Role::Root));
    let with_ok = |word: Line| {
        let mut l = word;
        l.push(" · ", Style::mute());
        l.extend(ok.clone());
        l
    };
    match slot {
        "panicked" => {
            let p = &s["panic"];
            let at = p["at"].as_str().and_then(crate::time::parse_rfc3339).map(|t| crate::time::ago(t, now_ms)).unwrap_or_else(|| "?".into());
            let ch = p["channel"].as_str().unwrap_or("?");
            let st = harm.with_reverse().with_bold();
            (Line::from_span(format!("PANICKED {at} via {} · /resume", sanitize::inline(ch, Style::PLAIN).text()), st), true)
        }
        "halted" => {
            let r = s["halted"].as_str().unwrap_or("?");
            (Line::from_span(format!("HALTED ({})", sanitize::inline(r, Style::PLAIN).text()), harm), true)
        }
        _ if s["mode"] == "safe" => (with_ok(Line::from_span("safe mode", Style::fg(Role::Warn))), false),
        "running" => (ok, false),
        "stopped" => (with_ok(Line::from_span("stopped", Style::dim())), false),
        other => (with_ok(Line::from_span(sanitize::inline(other, Style::PLAIN).text(), Style::dim())), false),
    }
}

/// The workspace for the status bar: `~` for the home, and at most 32 columns (`…/` + the last
/// components that fit).
pub fn short_path(p: &str, home: Option<&str>) -> String {
    const MAX: usize = 32;
    let t = crate::access::tilde(p, home);
    if super::wrap::str_width(&t) <= MAX {
        return t;
    }
    let mut out = String::new();
    for c in t.rsplit('/').filter(|c| !c.is_empty()) {
        let next = if out.is_empty() { c.to_string() } else { format!("{c}/{out}") };
        if super::wrap::str_width(&next) + 2 > MAX {
            break;
        }
        out = next;
    }
    if out.is_empty() {
        let last = t.rsplit('/').next().unwrap_or(&t);
        return format!("…{}", last.chars().rev().take(MAX - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>());
    }
    format!("…/{out}")
}

/// The status bar (§3.9, TUI-BRAND): the agent on the left (busy time, body, model, the turn's
/// tokens, cache and cost, money), the Root on the right (workspace, access mode, level, and last
/// `root ok` or the Root's alarm). When narrow, fields drop by priority; the Root's state and the
/// access mode never drop, and the busy time drops late (the liveness row above the box says what
/// the agent is doing in any case).
pub fn bar(info: &Info, width: usize, now: Instant, now_ms: i64, _spin: usize, no_color: bool) -> Line {
    let mut left: Vec<Field> = Vec::new();
    let mut right: Vec<Field> = Vec::new();
    let status = info.status.as_ref();
    let state = info.state.as_ref();
    let egress = info.egress.as_ref().filter(|_| !info.egress_failed);
    let flash = info.level_flash_until.is_some_and(|t| t > now);
    // busy (drop 3): how long this run has been going
    if let Some(since) = info.busy_since {
        left.push(Field { drop: 3, line: Line::from_span(format!("busy {}", crate::time::span(now.duration_since(since).as_secs())), Style::PLAIN) });
    }
    // model (drop 6)
    let model = match state.and_then(|s| s["model"].as_str()) {
        Some(m) => Line::from_span(sanitize::inline(m, Style::PLAIN).text(), Style::dim()),
        None => {
            let routable: Vec<&str> = egress
                .and_then(|e| e["models"].as_array())
                .map(|a| a.iter().filter(|m| m["routable"] == true).filter_map(|m| m["model"].as_str()).collect())
                .unwrap_or_default();
            match routable.as_slice() {
                [one] => Line::from_span(sanitize::inline(one, Style::PLAIN).text(), Style::dim()),
                _ => Line::from_span("model ?", Style::dim()),
            }
        }
    };
    left.push(Field { drop: 6, line: model });
    // tokens this turn, the cache-hit rate (turn and session, token-weighted) and the turn's
    // estimated cost: one live segment instead of a token line per request (§3.9)
    if info.session.requests > 0 {
        let t = &info.turn;
        let mut l = Line::from_span("turn ", Style::mute());
        l.push(format!("in {} out {}", tok(t.input), tok(t.output)), Style::PLAIN);
        l.push(" · cache ", Style::mute());
        l.push(t.cache().show(), Style::PLAIN);
        l.push(" · ", Style::mute());
        l.push(info.turn_musd().map_or_else(|| "$?".into(), super::usd_fine), Style::PLAIN);
        left.push(Field { drop: 4, line: l });
        left.push(Field { drop: 7, line: Line::from_span(format!("session cache {}", info.session.cache().show()), Style::dim()) });
    }
    if egress.is_some_and(observe_only) {
        let total = egress.and_then(|e| money(e, "total_spent_musd")).filter(|m| *m >= 0)
            .map(usd).unwrap_or_else(|| "$?".into());
        left.push(Field { drop: 2, line: Line::from_span(format!("{total} live total est"), Style::PLAIN) });
        let tokens = egress.filter(|e| e["money"]["token_totals_complete"] == true)
            .and_then(|e| e["money"]["total_input_tokens"].as_u64()?.checked_add(e["money"]["total_output_tokens"].as_u64()?))
            .map(|n| format!("{n} live tokens")).unwrap_or_else(|| "live tokens ?".into());
        left.push(Field { drop: 8, line: Line::from_span(tokens, Style::dim()) });
        let evo = state.and_then(|s| s["evolution_checks_today_musd"].as_i64()).filter(|m| *m >= 0)
            .map(usd).unwrap_or_else(|| "$?".into());
        left.push(Field { drop: 8, line: Line::from_span(format!("evo checks today {evo} est"), Style::dim()) });
        left.push(Field { drop: 9, line: Line::from_span("/usage · /stop", Style::mute()) });
    } else {
        // run cost (drop 5)
        if let Some(m) = state.and_then(|s| s["task_musd"].as_i64()) {
            left.push(Field { drop: 5, line: Line::from_span(format!("{} run", usd(m)), Style::PLAIN) });
        } else if let (Some(now_spent), Some(at)) = (egress.and_then(|e| money(e, "day_spent_musd")), info.day_spent_at_attach) {
            left.push(Field { drop: 5, line: Line::from_span(format!("+{} since attach", usd((now_spent - at).max(0))), Style::dim()) });
        }
        // today against the ceiling (drop 2): the figure the Root's hard ceiling governs
        let today = match egress {
            Some(e) if money(e, "day_spent_musd").is_some_and(|m| m >= 0) => {
                let spent = money(e, "day_spent_musd").unwrap_or(0);
                let reserved = money(e, "day_reserved_musd").unwrap_or(0);
                match money(e, "day_limit_musd") {
                    Some(limit) if limit > 0 => {
                        let (st, tail) = if spent + reserved >= limit {
                            (Style::fg(Role::Harm).with_bold(), " ceiling")
                        } else if spent * 10 >= limit * 8 {
                            (Style::fg(Role::Warn), "")
                        } else {
                            (Style::PLAIN, "")
                        };
                        Line::from_span(format!("{}/{} today{tail}", usd(spent), usd(limit)), st)
                    }
                    _ => Line::from_span(format!("{} today", usd(spent)), Style::PLAIN),
                }
            }
            _ => Line::from_span("$?/$? today", Style::dim()),
        };
        left.push(Field { drop: 2, line: today });
    }
    // body (drop 4): which body runs, and whether it is the last known good one; a provisional body is
    // a change in flight
    if let Some(sha) = state.and_then(|s| s["sha12"].as_str()).filter(|s| !s.is_empty()) {
        let mut l = Line::from_span("body ", Style::mute());
        l.push(sanitize::inline(sha, Style::PLAIN).text(), Style::dim());
        match state.and_then(|s| s["mode"].as_str()) {
            Some("provisional") => l.push(" provisional", Style::fg(Role::Rewrite)),
            Some("safe") => l.push(" safe", Style::fg(Role::Warn)),
            _ => {
                let t = status.map(|s| &s["tuples"]);
                if t.is_some_and(|t| t["current"].is_string() && t["current"] == t["lkg"]) {
                    l.push(" lkg", Style::mute());
                }
            }
        }
        left.push(Field { drop: 4, line: l });
    }
    // queued (drop 6)
    let q = status.and_then(|s| s["queued"].as_u64()).unwrap_or(0).max(state.and_then(|s| s["queued"].as_u64()).unwrap_or(0));
    if q > 0 {
        left.push(Field { drop: 6, line: Line::from_span(format!("queued {q}"), Style::fg(Role::Warn)) });
    }
    // the Root's side: the workspace (drop 4) and the access mode (never dropped), from the Root's
    // AccessView (ADR-009, ADR-012); FULL is ink with a harm dot, not a red word on every screen
    if let Some(a) = info.access.as_ref().filter(|a| a.is_object()) {
        let ws = match a["workspace"].as_str() {
            Some(w) => Line::from_span(sanitize::inline(&short_path(w, info.home.as_deref()), Style::PLAIN).text(), Style::dim()),
            None => Line::from_span("no workspace", Style::mute()),
        };
        right.push(Field { drop: 4, line: ws });
        let mode = if a["full"] == true {
            let mut l = Line::from_span("● ", Style::fg(Role::Harm));
            l.push("FULL", if no_color { Style::PLAIN.with_reverse() } else { Style::PLAIN });
            l
        } else {
            Line::from_span("WORKSPACE", Style::PLAIN)
        };
        right.push(Field { drop: 0, line: mode });
    }
    // the level (drop 1; short form first): granted by the Root
    let level = status.and_then(|s| s["level"].as_i64());
    let cap = status.and_then(|s| s["cap"].as_i64());
    let lvl_style = if flash { Style::fg(Role::Pass).with_reverse() } else { Style::PLAIN };
    let level_short = Line::from_span(level.map_or_else(|| "L?".into(), |l| format!("L{l}")), lvl_style);
    let level_long = match (level, cap) {
        (Some(l), Some(c)) if c != l => Line::from_span(format!("L{l} (cap L{c})"), lvl_style),
        _ => level_short.clone(),
    };
    right.push(Field { drop: 1, line: level_long });
    // the Root's state (never dropped), last
    let (mut root_line, alarm) = root_state(info, now, now_ms);
    if no_color && alarm {
        root_line = Line(root_line.0.into_iter().map(|mut s| {
            s.style = s.style.with_reverse();
            s
        }).collect());
    }
    if info.reconnected_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(5)) && matches!(info.conn, Conn::Live) {
        let mut l = Line::from_span("reconnected · ", Style::dim());
        l.extend(root_line);
        root_line = l;
    }
    right.push(Field { drop: 0, line: root_line });
    let sum = |fs: &[Field]| fs.iter().map(|f| f.line.width()).sum::<usize>() + 3 * fs.len().saturating_sub(1);
    // a space at each end, and at least 3 columns between the agent's side and the Root's
    let total = |l: &[Field], r: &[Field]| 1 + sum(l) + if l.is_empty() { 0 } else { 3 } + sum(r) + 1;
    if total(&left, &right) > width
        && let Some(f) = right.iter_mut().find(|f| f.drop == 1)
    {
        f.line = level_short; // the short level form first
    }
    while total(&left, &right) > width {
        let worst = |fs: &[Field]| fs.iter().enumerate().filter(|(_, f)| f.drop > 0).max_by_key(|(_, f)| f.drop).map(|(i, f)| (i, f.drop));
        match (worst(&left), worst(&right)) {
            (Some((i, a)), Some((_, b))) if a >= b => {
                left.remove(i);
            }
            (_, Some((j, _))) => {
                right.remove(j);
            }
            (Some((i, _)), None) => {
                left.remove(i);
            }
            (None, None) => break,
        }
    }
    let join = |fs: Vec<Field>| {
        let mut out = Line::new();
        for (i, f) in fs.into_iter().enumerate() {
            if i > 0 {
                out.push(" · ", Style::mute());
            }
            out.extend(f.line);
        }
        out
    };
    let (l, r) = (join(left), join(right));
    let mut out = Line::from_span(" ", Style::PLAIN);
    out.extend(l);
    let pad = width.saturating_sub(out.width() + r.width() + 1).max(if out.width() > 1 { 3 } else { 0 });
    out.push(" ".repeat(pad), Style::PLAIN);
    out.extend(r);
    out.truncate(width)
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    use serde_json::json;

    fn observed() -> Info {
        let mut info = Info::default();
        info.on_egress(json!({"money": {
            "enforced": false, "policy": "observe", "day_spent_musd": 3_000_000,
            "total_spent_musd": 12_500_000, "day_limit_musd": 1, "total_limit_musd": 1,
            "total_input_tokens": 1000, "total_output_tokens": 250, "token_totals_complete": true
        }}));
        info
    }

    #[test]
    fn observed_usage_bar_shows_totals_without_cap_denominators() {
        let info = observed();
        let bar = bar(&info, 240, Instant::now(), 0, 0, false).text();
        assert!(bar.contains("$12.50 live total est"), "{bar}");
        assert!(bar.contains("1250 live tokens"), "{bar}");
        assert!(bar.contains("evo checks today $? est"), "{bar}");
        assert!(bar.contains("/usage · /stop"), "{bar}");
        assert!(!bar.contains("ceiling") && !bar.contains("/$"), "{bar}");
    }

    #[test]
    fn usage_summary_distinguishes_unknown_zero_and_partial_attribution() {
        let mut info = Info::default();
        let missing = usage_summary(&info);
        assert!(missing.contains("unknown total") && missing.contains("live token totals unknown"));
        assert!(!missing.contains("$0.00"));
        info = observed();
        info.on_state(json!({"evolution_checks_today_musd": 0}), Instant::now());
        let known = usage_summary(&info);
        assert!(known.contains("input 1000, output 250"), "{known}");
        assert!(known.contains("evolution checks today $0.00 estimated (gate/probe only)"), "{known}");
        assert!(known.contains("Free-evolution reasoning cost is not attributed"));
        assert!(known.contains("Monetary caps off"));
        info.egress_failed = true;
        assert!(usage_summary(&info).contains("unknown total"));
    }

    #[test]
    fn pending_upper_bound_distinguishes_unknown_zero_and_unsettled_cost() {
        let mut info = observed();
        assert!(usage_summary(&info).contains("upper bound unknown"));
        info.egress.as_mut().unwrap()["money"]["total_reserved_musd"] = json!(0);
        assert!(usage_summary(&info).contains("upper bound $0.00"));
        info.egress.as_mut().unwrap()["money"]["total_reserved_musd"] = json!(2_000_000);
        let summary = usage_summary(&info);
        assert!(summary.contains("$12.50 total settled estimate"), "{summary}");
        assert!(summary.contains("upper bound $2.00"), "{summary}");
        assert!(summary.contains("interrupted requests may still be billed"), "{summary}");
        assert!(!summary.contains("$14.50"), "pending amounts are not settled costs: {summary}");
        info.egress_failed = true;
        assert!(usage_summary(&info).contains("upper bound unknown"));
    }

    #[test]
    fn incomplete_tokens_are_reported_as_subtotals_not_total_zero() {
        let mut info = observed();
        info.on_egress(json!({"money": {
            "enforced": false, "token_totals_complete": false,
            "total_input_tokens": null, "total_output_tokens": null,
            "reported_input_tokens": 17, "reported_output_tokens": 8
        }}));
        let s = usage_summary(&info);
        assert!(s.contains("live token totals unknown; reported subtotal: input 17, output 8"), "{s}");
        let bar = bar(&info, 240, Instant::now(), 0, 0, false).text();
        assert!(bar.contains("$? live total est") && bar.contains("live tokens ?"), "{bar}");
        assert!(!bar.contains("$0.00"), "{bar}");
    }
}
