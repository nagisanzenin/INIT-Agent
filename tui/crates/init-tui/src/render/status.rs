//! The status bar (docs/tui.md §3.9). Level, money and the Root state come from Root requests, which
//! no zone process can forge; body, mode, busy and model come from the kernel and are informative.

use super::live::SPINNER;
use super::{Color, Line, Style, sanitize, usd};
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

/// Monetary figures are ledger/tariff estimates, not reconciled provider invoices.
/// Missing or stale metering is never rendered as a free run.
pub fn usage_summary(info: &Info) -> String {
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
    let seen = if info.session.requests == 0 {
        "This TUI has seen no token lines yet".to_string()
    } else {
        let s = &info.session;
        format!(
            "This TUI saw {} requests: input {} ({} cached), output {}; cache hit {} (token-weighted: cached / input; ? when the provider reports no cache field); last turn {} requests: {}",
            s.requests, s.input, s.cached, s.output, s.cache().show(), info.turn.requests, info.turn_line()
        )
    };
    format!("Root live ledger: {} total settled estimate, {} today (USD estimated from usage/tariffs, not an invoice); pending upper bound {} (separate from settled cost; interrupted requests may still be billed); {tokens}; {evo}. Free-evolution reasoning cost is not attributed. {policy} /stop stops the current run; /panic stops all agent activity; Ctrl-C interrupts. {seen}.", amount("total_spent_musd"), amount("day_spent_musd"), amount("total_reserved_musd"))
}

fn observe_only(egress: &Value) -> bool {
    egress["money"]["enforced"] == false || egress["money"]["policy"] == "observe"
}

/// The Root-state field and whether it is an error (for `NO_COLOR` reverse video).
fn root_state(info: &Info, now: Instant, now_ms: i64) -> (String, Style) {
    match &info.conn {
        Conn::Down { retry_at } => {
            let left = retry_at.saturating_duration_since(now).as_secs_f64();
            return (format!("Root unreachable · retry in {left:.1} s"), Style::fg(Color::Red));
        }
        Conn::Connecting if info.status.is_none() => return ("connecting".into(), Style::dim()),
        _ => {}
    }
    let Some(s) = &info.status else { return ("attaching".into(), Style::dim()) };
    let slot = s["slot"].as_str().unwrap_or("?");
    let (text, st) = match slot {
        "panicked" => {
            let p = &s["panic"];
            let at = p["at"].as_str().and_then(crate::time::parse_rfc3339).map(|t| crate::time::ago(t, now_ms)).unwrap_or_else(|| "?".into());
            let ch = p["channel"].as_str().unwrap_or("?");
            (format!("PANICKED {at} via {} · /resume", sanitize::inline(ch, Style::PLAIN).text()), Style::fg(Color::Red).with_reverse().with_bold())
        }
        "halted" => {
            let r = s["halted"].as_str().unwrap_or("?");
            (format!("HALTED ({})", sanitize::inline(r, Style::PLAIN).text()), Style::fg(Color::Red))
        }
        _ if s["mode"] == "safe" => ("safe mode".into(), Style::fg(Color::Yellow)),
        "stopped" => ("stopped".into(), Style::dim()),
        other => (sanitize::inline(other, Style::PLAIN).text(), Style::PLAIN),
    };
    (text, st)
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

pub fn bar(info: &Info, width: usize, now: Instant, now_ms: i64, spin: usize, no_color: bool) -> Line {
    let mut fields: Vec<Field> = Vec::new();
    let status = info.status.as_ref();
    let state = info.state.as_ref();
    let egress = info.egress.as_ref().filter(|_| !info.egress_failed);
    let flash = info.level_flash_until.is_some_and(|t| t > now);
    // level (drop 1)
    let level = status.and_then(|s| s["level"].as_i64());
    let cap = status.and_then(|s| s["cap"].as_i64());
    let lvl_style = if flash { Style::fg(Color::Green).with_reverse() } else { Style::bold() };
    let level_short = Line::from_span(level.map_or_else(|| "L?".into(), |l| format!("L{l}")), lvl_style);
    let level_long = match (level, cap) {
        (Some(l), Some(c)) if c != l => Line::from_span(format!("L{l} (cap L{c})"), lvl_style),
        _ => level_short.clone(),
    };
    fields.push(Field { drop: 1, line: level_long });
    // model (drop 6)
    let model = match state.and_then(|s| s["model"].as_str()) {
        Some(m) => Line::from_span(sanitize::inline(m, Style::PLAIN).text(), Style::PLAIN),
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
    fields.push(Field { drop: 6, line: model });
    // tokens this turn, the cache-hit rate (turn and session, token-weighted) and the turn's
    // estimated cost: one live segment instead of a token line per request (§3.9)
    if info.session.requests > 0 {
        let t = &info.turn;
        let mut l = Line::from_span("turn ", Style::dim());
        l.push(format!("in {} out {}", tok(t.input), tok(t.output)), Style::PLAIN);
        l.push(" · cache ", Style::dim());
        l.push(t.cache().show(), Style::PLAIN);
        l.push(" · ", Style::dim());
        l.push(info.turn_musd().map_or_else(|| "$?".into(), super::usd_fine), Style::PLAIN);
        fields.push(Field { drop: 3, line: l });
        fields.push(Field { drop: 4, line: Line::from_span(format!("session cache {}", info.session.cache().show()), Style::dim()) });
    }
    if egress.is_some_and(observe_only) {
        let total = egress.and_then(|e| money(e, "total_spent_musd")).filter(|m| *m >= 0)
            .map(usd).unwrap_or_else(|| "$?".into());
        fields.push(Field { drop: 2, line: Line::from_span(format!("{total} live total est"), Style::PLAIN) });
        let tokens = egress.filter(|e| e["money"]["token_totals_complete"] == true)
            .and_then(|e| e["money"]["total_input_tokens"].as_u64()?.checked_add(e["money"]["total_output_tokens"].as_u64()?))
            .map(|n| format!("{n} live tokens")).unwrap_or_else(|| "live tokens ?".into());
        fields.push(Field { drop: 3, line: Line::from_span(tokens, Style::dim()) });
        let evo = state.and_then(|s| s["evolution_checks_today_musd"].as_i64()).filter(|m| *m >= 0)
            .map(usd).unwrap_or_else(|| "$?".into());
        fields.push(Field { drop: 3, line: Line::from_span(format!("evo checks today {evo} est"), Style::dim()) });
        fields.push(Field { drop: 4, line: Line::from_span("/usage · /stop", Style::dim()) });
    } else {
        // run cost (drop 5)
        if let Some(m) = state.and_then(|s| s["task_musd"].as_i64()) {
            fields.push(Field { drop: 5, line: Line::from_span(format!("{} run", usd(m)), Style::PLAIN) });
        } else if let (Some(now_spent), Some(at)) = (egress.and_then(|e| money(e, "day_spent_musd")), info.day_spent_at_attach) {
            fields.push(Field { drop: 5, line: Line::from_span(format!("+{} since attach", usd((now_spent - at).max(0))), Style::dim()) });
        }
        // today against the ceiling (drop 2)
        let today = match egress {
            Some(e) if money(e, "day_spent_musd").is_some_and(|m| m >= 0) => {
                let spent = money(e, "day_spent_musd").unwrap_or(0);
                let reserved = money(e, "day_reserved_musd").unwrap_or(0);
                match money(e, "day_limit_musd") {
                    Some(limit) if limit > 0 => {
                        let (st, tail) = if spent + reserved >= limit {
                            (Style::fg(Color::Red).with_bold(), " ceiling")
                        } else if spent * 10 >= limit * 8 {
                            (Style::fg(Color::Yellow), "")
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
        fields.push(Field { drop: 2, line: today });
    }
    // the Root state (never dropped)
    let (rs, mut rst) = root_state(info, now, now_ms);
    if no_color && matches!(rst.fg, Some(Color::Red | Color::Yellow)) {
        rst = rst.with_reverse();
    }
    let mut root_line = Line::from_span(rs, rst);
    if info.reconnected_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(5)) && matches!(info.conn, Conn::Live) {
        root_line.push(" · reconnected", Style::dim());
    }
    fields.push(Field { drop: 0, line: root_line });
    // ADR-009, ADR-012: the access mode, FULL (red bold) or WORKSPACE (yellow), both never
    // dropped, and the workspace (drop 3), from the Root's AccessView
    if let Some(a) = info.access.as_ref().filter(|a| a.is_object()) {
        let (label, mut st) = if a["full"] == true {
            ("FULL", Style::fg(Color::Red).with_bold())
        } else {
            ("WORKSPACE", Style::fg(Color::Yellow))
        };
        if no_color {
            st = st.with_reverse();
        }
        fields.push(Field { drop: 0, line: Line::from_span(label, st) });
        let ws = match a["workspace"].as_str() {
            Some(w) => Line::from_span(sanitize::inline(&short_path(w, info.home.as_deref()), Style::PLAIN).text(), Style::PLAIN),
            None => Line::from_span("no workspace", Style::dim()),
        };
        fields.push(Field { drop: 3, line: ws });
    }
    // body (drop 4)
    if let Some(sha) = state.and_then(|s| s["sha12"].as_str()).filter(|s| !s.is_empty()) {
        let mut l = Line::from_span(format!("body {}", sanitize::inline(sha, Style::PLAIN).text()), Style::PLAIN);
        match state.and_then(|s| s["mode"].as_str()) {
            Some("provisional") => l.push(" provisional", Style::fg(Color::Yellow)),
            Some("safe") => l.push(" safe", Style::fg(Color::Yellow)),
            _ => {
                let t = status.map(|s| &s["tuples"]);
                if t.is_some_and(|t| t["current"].is_string() && t["current"] == t["lkg"]) {
                    l.push(" lkg", Style::dim());
                }
            }
        }
        fields.push(Field { drop: 4, line: l });
    }
    // busy (drop 7)
    if let Some(since) = info.busy_since {
        let s = format!("{} busy {}", SPINNER[spin % SPINNER.len()], crate::time::span(now.duration_since(since).as_secs()));
        fields.push(Field { drop: 7, line: Line::from_span(s, Style::fg(Color::Cyan)) });
    }
    // queued (drop 8)
    let q = status.and_then(|s| s["queued"].as_u64()).unwrap_or(0).max(state.and_then(|s| s["queued"].as_u64()).unwrap_or(0));
    if q > 0 {
        fields.push(Field { drop: 8, line: Line::from_span(format!("queued {q}"), Style::fg(Color::Yellow)) });
    }
    let total = |fs: &[Field]| fs.iter().map(|f| f.line.width()).sum::<usize>() + 3 * fs.len().saturating_sub(1) + 1;
    if total(&fields) > width {
        fields[0].line = level_short; // the short level form first
    }
    while total(&fields) > width {
        let Some(i) = fields.iter().enumerate().filter(|(_, f)| f.drop > 0).max_by_key(|(_, f)| f.drop).map(|(i, _)| i) else { break };
        fields.remove(i);
    }
    let mut out = Line::from_span(" ", Style::PLAIN);
    for (i, f) in fields.into_iter().enumerate() {
        if i > 0 {
            out.push(" · ", Style::dim());
        }
        out.extend(f.line);
    }
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
        assert!(usage_summary(&info).contains("pending upper bound unknown"));
        info.egress.as_mut().unwrap()["money"]["total_reserved_musd"] = json!(0);
        assert!(usage_summary(&info).contains("pending upper bound $0.00"));
        info.egress.as_mut().unwrap()["money"]["total_reserved_musd"] = json!(2_000_000);
        let summary = usage_summary(&info);
        assert!(summary.contains("$12.50 total settled estimate"), "{summary}");
        assert!(summary.contains("pending upper bound $2.00"), "{summary}");
        assert!(summary.contains("interrupted requests may still be billed"), "{summary}");
        assert!(!summary.contains("$14.50"), "pending amounts are not settled costs: {summary}");
        info.egress_failed = true;
        assert!(usage_summary(&info).contains("pending upper bound unknown"));
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
