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
    let policy = if egress.is_some_and(observe_only) { "Spending is metered, not capped." } else { "Monetary policy is reported by Root." };
    let mut rows: Vec<(&str, String)> = vec![
        ("Root ledger", format!("{} total settled estimate · {} today", amount("total_spent_musd"), amount("day_spent_musd"))),
        ("pending", format!("upper bound {} (separate from settled cost; interrupted requests may still be billed)", amount("total_reserved_musd"))),
        ("tokens", tokens),
        ("self-changes", self_change_cost(info)),
        ("policy", format!("{policy} /stop stops the current run; /panic stops all agent activity; Ctrl-C interrupts.")),
    ];
    if info.session.requests == 0 {
        rows.push(("this TUI", "no token lines seen yet".into()));
    } else {
        let s = &info.session;
        rows.push(("this TUI", format!("{}: input {} ({} cached), output {}", plural(s.requests, "request"), s.input, s.cached, s.output)));
        rows.push(("cache hit", format!("{} (token-weighted: cached / input; ? when the provider reports no cache field)", s.cache().show())));
        let mut last = format!("{}: {}", plural(info.turn.requests, "request"), info.turn_line());
        if info.turn_replayed {
            // v0.5.2: why a replayed turn has no figure
            last.push_str(" (it ran, at least in part, before this TUI attached; a turn's cost is measured only while the TUI watches it, and the Root's total includes it)");
        }
        rows.push(("last turn", last));
    }
    ("usage (USD estimated from usage and tariffs, not an invoice)".to_string(), rows.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// `/usage`'s self-change row, at one price: the Root's (v0.5.3 L6). The Root's ledger prices every request; from
/// v0.5.3 it also splits today's spend by the kernel's scope label, so the row gives the self-changes' checks today
/// from the ledger, the same figure before and after a relaunch. From an older Root without the split it gives what
/// this TUI measured, labelled as what it is: the ledger's change while self-changes were in flight (TUI-FIX,
/// v0.5.1). The kernel's own gate-budget counter is not shown: it prices the same requests from the kernel's config
/// (DeepSeek's off-peak window included), a second price for one request (v0.5.2's known limit).
fn self_change_cost(info: &Info) -> String {
    if let Some(g) = info.gate_day_musd() {
        return format!("{} today, from the Root's ledger at its prices (the requests the kernel sent for self-change checks)", usd(g));
    }
    let mut out = match (info.gates.seen, info.gate_musd()) {
        // v0.5.2: one that ran before this TUI attached was not measured here
        (false, _) if info.gates.seen_replayed => "a self-change ran before this TUI attached: its cost is in the Root's total, not measured here".to_string(),
        (false, _) => "none seen by this TUI".to_string(),
        (true, Some(m)) => format!("{} metered by the Root while self-changes ran (seen by this TUI; any other spend in that time is included)", usd(m)),
        (true, None) => "a self-change ran, but its cost is unknown (no ledger total from the Root)".to_string(),
    };
    out.push_str(". The Root does not split spend by purpose yet.");
    out
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
        // TUI-FIX (v0.5.1 user session: "queued: the body is backoff", with no reason and "root ok"
        // beside it): the agent's slot in plain words; the Root's alert above says why
        "stopped" => (with_ok(Line::from_span("agent stopped · /resume", Style::dim())), false),
        "backoff" => (with_ok(Line::from_span("agent failed to start, retrying", Style::fg(Role::Warn))), false),
        "starting" => (with_ok(Line::from_span("agent starting", Style::dim())), false),
        "stopping" => (with_ok(Line::from_span("agent stopping", Style::dim())), false),
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
        l.push(info.turn_cost(), Style::PLAIN);
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
        // TUI-FIX (v0.5.1): the self-changes' spend, once one was seen (never a `$?` or a kernel
        // figure; `/usage` says what it is): the Root's figure for today when it splits spend (v0.5.3 L6)
        if let Some(m) = info.gate_day_musd().filter(|m| *m > 0).or_else(|| info.gate_musd()) {
            left.push(Field { drop: 8, line: Line::from_span(format!("checks {} est", usd(m)), Style::dim()) });
        }
        left.push(Field { drop: 9, line: Line::from_span("/usage · /stop", Style::mute()) });
    } else {
        // spend since attach (drop 5), from the Root's ledger. v0.5.3 (the release review): not the kernel's
        // `state.task_musd` ("$0.040 run"), which prices the run at the kernel's own prices (root/egress.md §3.4:
        // one price per request, the Root's); the ledger has no per-run figure
        if let (Some(now_spent), Some(at)) = (egress.and_then(|e| money(e, "day_spent_musd")), info.day_spent_at_attach) {
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
    // v0.5.2: not a body id from before a panic, a resume or a kernel restart (`Info::body_stale`)
    if let Some(sha) = state.and_then(|s| s["sha12"].as_str()).filter(|s| !s.is_empty() && !info.body_stale) {
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
        } else if a["workspace"].is_null() {
            // TUI-FIX (v0.5.1): "no workspace · WORKSPACE" contradicted itself; confined without a
            // workspace, the agent writes only its own folder
            Line::from_span("agent's own folder only", Style::PLAIN)
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
        assert!(!bar.contains("checks") && !bar.contains("$?"), "no self-change seen: no checks field, no unexplained ?: {bar}");
        assert!(bar.contains("/usage · /stop"), "{bar}");
        assert!(!bar.contains("ceiling") && !bar.contains("/$"), "{bar}");
    }

    /// v0.5.3 (the release review): under an enforced policy the bar showed the kernel's `state.task_musd` as
    /// "$0.040 run", the run at the kernel's own prices, beside the Root's figures. Every figure it shows is the
    /// ledger's now (root/egress.md §3.4): the spend since this TUI attached, and today against the ceiling.
    #[test]
    fn enforced_bar_shows_no_kernel_price() {
        let mut info = Info::default();
        let money = |spent: i64| json!({"money": {"enforced": true, "policy": "enforce", "day_spent_musd": spent,
            "total_spent_musd": spent, "day_limit_musd": 5_000_000, "total_limit_musd": 25_000_000}});
        info.on_egress(money(1_230_000));
        info.on_state(json!({"busy": true, "mode": "normal", "sha12": "3f9c0a1b2c4d", "task_musd": 40_000}), Instant::now());
        info.on_egress(money(1_242_000));
        let b = bar(&info, 240, Instant::now(), 0, 0, false).text();
        assert!(b.contains("+$0.012 since attach") && b.contains("$1.24/$5.00 today"), "{b}");
        assert!(!b.contains("$0.040") && !b.contains(" run"), "the kernel's price: {b}");
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
        assert!(known.contains("self-changes none seen by this TUI. The Root does not split spend by purpose yet."), "{known}");
        assert!(!known.contains("gate-budget counter"), "the kernel's figure is never shown: {known}");
        assert!(known.contains("Spending is metered, not capped"));
        info.egress_failed = true;
        assert!(usage_summary(&info).contains("unknown total"));
    }

    /// TUI-FIX (v0.5.1 user session): `/usage` said "evolution checks today $0.00" while the gate's
    /// probe and G3 requests were two thirds of the ledger (the kernel's figure came from a `state`
    /// frame sent before the gate ran). The row now gives the ledger's change while the self-change
    /// was in flight, and never the stale kernel figure as the checks' cost.
    #[test]
    fn self_change_spend_is_what_the_ledger_metered() {
        let t0 = Instant::now();
        let money = |m: i64| json!({"money": {"enforced": false, "policy": "observe", "total_spent_musd": m, "day_spent_musd": m}});
        let mut info = Info::default();
        info.on_egress(money(10_000));
        info.on_state(json!({"busy": false, "evolution_checks_today_musd": 0}), t0);
        assert!(info.gate_musd().is_none());
        info.on_gate(true, false, t0 + Duration::from_secs(1)); // gate_start: the window opens at $0.010
        info.on_egress(money(25_000));
        assert_eq!(info.gate_musd(), Some(15_000), "the open window so far");
        info.on_egress(money(40_000));
        info.on_gate(false, false, t0 + Duration::from_secs(600)); // kept: closes at the next ledger answer
        info.on_egress(money(40_850));
        info.on_egress(money(45_850)); // a turn after the gate is not the gate's
        assert_eq!(info.gate_musd(), Some(30_850));
        let s = usage_summary(&info);
        assert!(s.contains("$0.031 metered by the Root while self-changes ran"), "{s}");
        assert!(!s.contains("$0.000"), "the kernel's stale $0 is not shown as the checks' cost: {s}");
        assert!(s.contains("$0.046 total settled estimate"), "{s}");
        let bar = bar(&info, 240, t0, 0, 0, false).text();
        assert!(bar.contains("$0.046 live total est") && bar.contains("checks $0.031 est"), "{bar}");
        // v0.5.3 L6: a fresh kernel figure is not shown either (it priced the same requests at its own prices)
        info.on_state(json!({"busy": false, "evolution_checks_today_musd": 31_000}), t0 + Duration::from_secs(700));
        assert!(!usage_summary(&info).contains("gate-budget counter"), "{}", usage_summary(&info));
    }

    /// v0.5.3 L6 (v0.5.2's known limit, user session finding 2): the same 15 checks showed as $0.039 metered by
    /// the Root before a relaunch and as the kernel's off-peak $0.018 after it. With the Root's split the row
    /// shows the Root's figure on both sides of the relaunch, and the kernel's never.
    #[test]
    fn self_changes_show_at_the_roots_price_across_a_relaunch() {
        let t0 = Instant::now();
        let money = |m: i64, gate: i64| json!({"money": {"enforced": false, "policy": "observe", "total_spent_musd": m, "day_spent_musd": m,
            "day_spent_by_scope_musd": {"gate": gate, "live": m - gate}}});
        // before the relaunch: this TUI watched the checks run
        let mut before = Info::default();
        before.on_egress(money(10_000, 0));
        before.on_gate(true, false, t0);
        before.on_egress(money(40_000, 36_102));
        before.on_gate(false, false, t0 + Duration::from_secs(600));
        before.on_egress(money(49_000, 36_102));
        before.on_state(json!({"busy": false, "evolution_checks_today_musd": 18_049}), t0 + Duration::from_secs(700));
        // after it: a new TUI that saw the checks only in the replay
        let mut after = Info::default();
        after.replaying = true;
        after.on_gate(true, true, t0);
        after.on_gate(false, false, t0);
        after.replaying = false;
        after.on_egress(money(50_275, 36_102));
        after.on_state(json!({"busy": false, "evolution_checks_today_musd": 18_049}), t0 + Duration::from_secs(5));
        for (when, info) in [("before", &before), ("after", &after)] {
            let s = usage_summary(info);
            assert!(s.contains("self-changes $0.036 today, from the Root's ledger at its prices"), "{when}: {s}");
            assert!(!s.contains("$0.018") && !s.contains("gate-budget counter"), "{when}: the kernel's off-peak figure: {s}");
            let b = bar(info, 240, t0, 0, 0, false).text();
            assert!(b.contains("checks $0.036 est"), "{when}: {b}");
        }
        // the Root splits spend and nothing was a check today: $0.00, and no checks field in the bar
        let mut none = Info::default();
        none.on_egress(json!({"money": {"enforced": false, "total_spent_musd": 5, "day_spent_musd": 5, "day_spent_by_scope_musd": {"live": 5}}}));
        assert!(usage_summary(&none).contains("self-changes $0.000 today"), "{}", usage_summary(&none));
        assert!(!bar(&none, 240, t0, 0, 0, false).text().contains("checks"));
    }

    /// TUI-FIX (v0.5.1): the agent's slot in words a stranger reads, beside `root ok`.
    #[test]
    fn slot_words_are_plain() {
        let mut info = Info::default();
        info.conn = Conn::Live;
        for (slot, want) in [("backoff", "agent failed to start, retrying · root ok"), ("stopped", "agent stopped · /resume · root ok"), ("starting", "agent starting · root ok")] {
            info.on_status(json!({"slot": slot, "mode": "normal", "level": 0, "kernel": null}), Instant::now());
            let b = bar(&info, 120, Instant::now(), 0, 0, false).text();
            assert!(b.trim_end().ends_with(want), "{slot}: {b}");
        }
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
