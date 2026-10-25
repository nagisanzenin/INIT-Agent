//! The lineage kind registry (root/root.md §3.10.5, D7) and the checks on kernel-reported records:
//! ICJ, ≤ 16 KiB, depth ≤ 8, required fields, the `owner` qid rule (§3.15) and privacy (§3.10.6).

use super::{DATA_MAX, KernelCtx};
use crate::util::icj;
use serde_json::Value;

/// Kinds the Root writes (`by:"root"`). `key_rotate` is reserved for a later Root release (§3.10.2).
/// `escapee` (ADR-008): a held-only Root found zone processes outside its hold and signalled none of them.
/// `access` (SBX-009, ADR-009): the access mode changed (`event`: workspace, full_on, full_off, charter, undo);
/// it holds no path, because the lineage is mirrored.
/// M7 (lane ROOT-S1a; root.md §3.10.5): `sealed_draw`, `sealed_g4`, `sealed_calibrate`, `milestone` (root/sealed-eval.md §4.1).
/// ADR-011 (Records and lineage; added together by lane X3 so the other lanes need not edit this array):
/// `change_open`, `change_close`, `harm`, `snapshot`, `restore`, `settled`, `cap_state`, `cap_use`, `grant_request`,
/// `grant`, `twin`, `twin_report`. X6/X7: `turns` (the owner day's owner and background turn counts, which the ladder
/// counts, ADR-011 §7.1).
/// ADR-013 (GBAL): `goal`, `goal_proposal`, `goal_predict`, `card_reading`, `goal_outcome` (none enters the ledger fold).
pub const ROOT: [&str; 50] = ["genesis", "root_start", "charter", "tuple", "lkg", "audited", "zone_start", "zone_exit",
    "crash_loop", "safe", "halt", "pause", "unpause", "rollback", "kernel_upgrade", "ladder", "share", "ceiling_hit",
    "egress_action", "family_eval", "panic", "resume", "stop", "proposal", "audit", "mutation", "escapee", "access",
    "sealed_draw", "sealed_g4", "sealed_calibrate", "milestone",
    "change_open", "change_close", "harm", "snapshot", "restore", "settled", "cap_state", "cap_use", "grant_request",
    "grant", "twin", "twin_report", "turns", "goal", "goal_proposal", "goal_predict", "card_reading", "goal_outcome"];

/// Kinds the kernel reports, with the fields the Root requires (present; null allowed). The kinds with
/// no required fields are recorded as their owning docs define them: root.md §3.10.5, kernel/supervise.md
/// §4.7, kernel/gate.md, evolution.md §4.4, kernel/initctl.md (D7: every kind they use).
const KERNEL: &[(&str, &[&str])] = &[
    ("promotion", &["sha", "from_sha", "gate_job", "scope", "lkg", "body_binary", "ext_set", "components"]),
    ("gate", &["gate_id", "gate_kind", "sha", "verdict", "reason"]),
    ("task_registered", &["task_id"]),
    ("task_admit", &["family", "label", "n", "n_sealed", "form", "author_sha", "sealed_by", "sealed_commit", "model"]),
    ("task_suspended", &["task_id", "reason"]),
    ("owner", &["signal", "qid", "target"]),
    ("body_rollback", &["from_sha", "to_sha", "reason"]),
    ("body_safe", &["on", "sha", "crashes"]),
    ("value_window", &["window_start", "window_end", "evolution_spend_musd", "accepted_tasks"]),
    ("fleet_spawn", &[]), ("fleet_stop", &[]), ("qualified", &[]), ("adopt", &[]), ("fleet_campaign", &[]),
    ("fleet_member", &[]), ("fleet", &[]), ("kernel_candidate", &[]), ("kernel_main", &[]), ("kernel_start", &[]),
    ("handoff", &[]), ("handoff_failed", &[]), ("fetch", &[]), ("ext_publish", &[]), ("owner_signal", &[]),
    ("halted", &[]), ("level_applied", &[]), ("admission", &[]), ("import", &[]), ("validated", &[]),
    ("prediction", &[]), ("usage", &[]), ("retire_list", &[]), ("thinning", &[]), ("upstream", &[]),
    ("vendor", &[]), ("kernel_proposal", &[]),
];

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// An e-mail-like `local@domain.tld` other than the allowlisted `agent@init.local`.
fn address(s: &str) -> bool {
    let b = s.as_bytes();
    let local = |c: &&u8| c.is_ascii_alphanumeric() || b"._%+-".contains(c);
    let dom = |c: &&u8| c.is_ascii_alphanumeric() || **c == b'.' || **c == b'-';
    (0..b.len()).filter(|&i| b[i] == b'@').any(|i| {
        let l = b[..i].iter().rev().take_while(local).count();
        let d = &s[i + 1..i + 1 + b[i + 1..].iter().take_while(dom).count()];
        l > 0 && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && &s[i - l..i + 1 + d.len()] != "agent@init.local"
    })
}

/// The first string (key or value) that holds a denied path or an address.
pub(crate) fn private(v: &Value, deny: &[String]) -> Option<String> {
    let bad = |s: &str| deny.iter().any(|d| d.len() > 1 && s.contains(d.as_str())) || address(s);
    match v {
        Value::String(s) if bad(s) => Some("a local path or an e-mail address".into()),
        Value::Array(a) => a.iter().find_map(|x| private(x, deny)),
        Value::Object(o) => o.iter().find_map(|(k, x)| if bad(k) { Some("a local path or an e-mail address".into()) } else { private(x, deny) }),
        _ => None,
    }
}

fn x_kind(kind: &str) -> bool {
    kind.strip_prefix("x_").is_some_and(|r| (1..=40).contains(&r.len()) && r.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'))
}

/// The generic checks, the kind's required fields, and the privacy rules. Err: why (`root.lineage`).
pub fn check_kernel(kind: &str, data: &Value, k: &KernelCtx) -> Result<(), String> {
    let req: &[&str] = match KERNEL.iter().find(|(n, _)| *n == kind) {
        Some((_, r)) => r,
        None if x_kind(kind) => &[],
        None => return Err(format!("unknown lineage kind {kind:?}")),
    };
    let o = data.as_object().ok_or("data must be an object")?;
    let s = icj(data).map_err(|_| "data must be ICJ (integers only, no floats)")?;
    if s.len() > DATA_MAX || depth(data) > 8 {
        return Err("data must be at most 16 KiB and nest at most 8 deep".into());
    }
    if let Some(f) = req.iter().find(|f| !o.contains_key(**f)) {
        return Err(format!("{kind}: missing field {f}"));
    }
    let not = |f: &str, ok: fn(&Value) -> bool| o.get(f).is_some_and(|v| !ok(v));
    let typed = match kind {
        "promotion" => !(not("lkg", Value::is_boolean) || not("components", Value::is_array) || not("audit_required", Value::is_boolean)),
        "body_safe" => !not("on", Value::is_boolean),
        _ => true,
    };
    if !typed {
        return Err(format!("{kind}: lkg, on and audit_required are booleans, components an array"));
    }
    if kind == "owner" {
        let sig = data["signal"].as_str().filter(|s| ["accept", "bless", "reject"].contains(s)).ok_or("owner: signal must be accept, bless or reject")?;
        let line = data["qid"].as_str().and_then(|q| (k.relayed)(q)).ok_or("owner: the qid names no line the Root relayed")?;
        if line.split_whitespace().next().map(|w| w.trim_start_matches('/')) != Some(sig) {
            return Err("owner: the relayed line is not that signal".into());
        }
    }
    match private(data, k.deny) {
        Some(why) => Err(format!("{kind}: data holds {why} (root/root.md §3.10.6)")),
        None => Ok(()),
    }
}
