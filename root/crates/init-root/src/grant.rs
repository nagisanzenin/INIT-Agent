//! ADR-011 §7.4 (lane X7): owner grants. The agent never grants itself anything; it may ask. A request
//! (`grant.request{kind, value, duration_h, why}` on the control link) is stored in `ROOT_HOME/state/grants/<id>.json`
//! (0600; the `why` text stays off the lineage), recorded as `grant_request{…, why_sha256}` and alerted, at most 3 per
//! rolling 24 h. Only an owner line decides: `/grant <id> [for <dur>]` (with the typed confirmation) or `/deny <id>`.
//! An approval writes the overlay `ROOT_HOME/state/grants.toml` (values with an expiry: by default the request's
//! duration, else 30 days, at most 365), appends `grant{id, kind, value, until, decision}`, and the Root's charter
//! becomes the active charter with the overlay on top, re-validated by C1–C17 (`st.charter`; the digest stays the
//! file's). Expiry appends `grant{decision:"expired"}`. ROOT_HOME is out of the zone's reach (Z1), so only an owner
//! line can create an overlay entry, and no kind names a Root mechanism (sentinel thresholds, snapshots, the twin,
//! profiles, keys, the deny list): those are refused `root.args`.

use crate::charter::{self, Budget, Charter, Loaded};
use crate::install::Paths;
use crate::link::{Root, lock};
use crate::util::{self, now_ms, parse_rfc3339, rfc3339, sha256_hex};
use crate::zone::Err3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The grant kinds of §7.4 (plus `actions.<class_id>`). Anything else is refused.
const KINDS: [&str; 11] = ["money.day_usd", "money.total_usd", "quotas.parallel_bodies", "extend.open_groups", "extend.changes_day",
    "extend.bloat_factor", "extend.background_share", "extend.snap_max_gib", "fleet.workspace_write", "dep", "autonomy.max_level"];
const PER_DAY: usize = 3;
const HOUR: i64 = 3_600_000;
const DEFAULT_H: i64 = 30 * 24;
const MAX_H: i64 = 365 * 24;
const WHY_MAX: usize = 4096;

/// One overlay entry of `ROOT_HOME/state/grants.toml`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub value: Value,
    pub until: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Overlay {
    #[serde(default)]
    grant: Vec<Entry>,
}

fn no(code: &'static str, m: impl Into<String>) -> Err3 {
    (code, m.into(), Value::Null)
}

fn int(v: &Value, lo: i64, hi: i64) -> Result<Value, String> {
    v.as_i64().filter(|x| (lo..=hi).contains(x)).map(Value::from).ok_or_else(|| format!("value must be an integer in {lo}..{hi}"))
}

/// The value a kind takes, checked and normalised against the active charter.
pub fn validate(kind: &str, v: &Value, ch: &Charter) -> Result<Value, String> {
    match kind {
        "money.day_usd" | "money.total_usd" => v.as_str().filter(|s| charter::musd(s).is_some_and(|m| m > 0)).map(Value::from)
            .ok_or_else(|| "value must be a decimal string like \"8.00\"".into()),
        "quotas.parallel_bodies" | "fleet.workspace_write" => int(v, i64::from(kind == "quotas.parallel_bodies"), 32),
        "extend.open_groups" => int(v, 1, 64),
        "extend.changes_day" => int(v, 1, 10_000),
        "extend.bloat_factor" => int(v, 1000, 10_000).map_err(|e| format!("{e} (permille)")),
        "extend.background_share" => int(v, 0, 1000).map_err(|e| format!("{e} (permille)")),
        "extend.snap_max_gib" => int(v, 1, 256),
        "autonomy.max_level" => int(v, 0, 4),
        "dep" => {
            let ok = |c: &Value| {
                let s = |k: &str| c[k].as_str().unwrap_or("");
                let name = s("name");
                !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                    && (1..=40).contains(&s("version").len()) && s("version").bytes().all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
                    && s("checksum").len() == 64 && s("checksum").bytes().all(|b| b.is_ascii_hexdigit())
                    && c.as_object().is_some_and(|o| o.len() == 3)
            };
            v.as_array().filter(|a| (1..=20).contains(&a.len()) && a.iter().all(ok)).map(|_| v.clone())
                .ok_or_else(|| "value must be 1–20 crates [{name, version, checksum}] (exact versions, sha256 checksums)".into())
        }
        k if k.starts_with("actions.") => {
            let class = &k["actions.".len()..];
            let min = ch.actions.get(class).and_then(|t| t.get("min_level")).and_then(toml::Value::as_integer)
                .ok_or_else(|| format!("the charter lists no action class {class:?}"))?;
            int(v, 0, 4).and_then(|x| if x.as_i64() < Some(min) { Ok(x) } else { Err(format!("value must be below the class's min_level {min}")) })
        }
        _ => Err(format!("{kind:?} is not a grant kind (§7.4); a grant never names a Root mechanism")),
    }
}

/// One entry on top of a charter: scalars are replaced, per-level budgets are raised to at least the value at every
/// level, an action class gets its `min_level`. `dep` and `fleet.workspace_write` change no charter key.
fn apply(ch: &mut Charter, e: &Entry) {
    let n = e.value.as_i64().unwrap_or(0);
    let raise = |a: &mut Vec<i64>| a.iter_mut().for_each(|x| *x = (*x).max(n));
    let x = &mut ch.extend;
    match e.kind.as_str() {
        "money.day_usd" => ch.money.day_usd = e.value.as_str().unwrap_or("").into(),
        "money.total_usd" => ch.money.total_usd = e.value.as_str().unwrap_or("").into(),
        "quotas.parallel_bodies" => ch.quotas.parallel_bodies = n,
        "autonomy.max_level" => ch.autonomy.max_level = n,
        "extend.open_groups" => raise(&mut x.open_groups),
        "extend.changes_day" => raise(&mut x.changes_day),
        "extend.bloat_factor" => raise(&mut x.bloat_permille),
        "extend.background_share" => raise(&mut x.background_permille),
        "extend.snap_max_gib" => x.snap_max_gib = n,
        k if k.starts_with("actions.") => {
            if let Some(toml::Value::Table(t)) = ch.actions.get_mut(&k["actions.".len()..]) {
                t.insert("min_level".into(), toml::Value::Integer(n));
            }
        }
        _ => {}
    }
}

/// The live entries (`until` > now) on top of `base`, in grant order; an entry that would make the charter fail
/// C1–C17 is skipped and returned with the findings.
pub fn merge(base: &Charter, entries: &[Entry], cx: &charter::Ctx, now: i64) -> (Charter, Vec<(String, String)>) {
    let (mut ch, mut skipped) = (base.clone(), Vec::new());
    for e in entries.iter().filter(|e| live(e, now)) {
        let mut next = ch.clone();
        apply(&mut next, e);
        match charter::check(&next, cx) {
            f if f.is_empty() => ch = next,
            f => skipped.push((e.id.clone(), f.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))),
        }
    }
    (ch, skipped)
}

fn live(e: &Entry, now: i64) -> bool {
    parse_rfc3339(&e.until).is_some_and(|u| u > now)
}

fn overlay_path(p: &Paths) -> PathBuf {
    p.r("state/grants.toml")
}

fn read(p: &Paths) -> Vec<Entry> {
    std::fs::read_to_string(overlay_path(p)).ok().and_then(|s| toml::from_str::<Overlay>(&s).ok()).map(|o| o.grant).unwrap_or_default()
}

fn write(p: &Paths, entries: &[Entry]) -> std::io::Result<()> {
    let body = toml::to_string(&Overlay { grant: entries.to_vec() }).map_err(|e| util::bad(e.to_string()))?;
    let head = "# Written by init-root from your /grant lines (ADR-011 §7.4). Each entry expires at `until`. Do not edit.\n";
    util::write_atomic(&overlay_path(p), format!("{head}{body}").as_bytes(), 0o600)?;
    *lock(next_expiry()).entry(p.rh.clone()).or_default() = entries.iter().filter_map(|e| parse_rfc3339(&e.until)).min().unwrap_or(i64::MAX);
    Ok(())
}

/// Per ROOT_HOME: the earliest `until` in the overlay (`tick` reads the file only when it is due).
fn next_expiry() -> &'static Mutex<BTreeMap<PathBuf, i64>> {
    static M: std::sync::OnceLock<Mutex<BTreeMap<PathBuf, i64>>> = std::sync::OnceLock::new();
    M.get_or_init(Mutex::default)
}

/// The active charter with the overlay on top (Root start and `charter.apply`, link.rs); the digest stays the file's.
pub fn merged(p: &Paths, mut l: Loaded) -> Loaded {
    let entries = read(p);
    if entries.is_empty() {
        return l;
    }
    let (ch, skipped) = merge(&l.ch, &entries, &p.ctx(), now_ms());
    for (id, why) in skipped {
        util::log(&p.rh, "warn", &format!("grant {id} is not applied: the effective charter would fail validation ({why})"));
    }
    l.ch = ch;
    l
}

/// After an overlay change: the effective charter becomes `st.charter`, then egress, level and views follow.
fn refresh(root: &Root) {
    let l = match charter::load(&root.p.r("state/charter.active.toml"), &root.p.ctx()) {
        Ok(l) => merged(&root.p, l),
        Err(f) => return util::log(&root.p.rh, "error", &format!("grant refresh: the active charter fails to load: {f:?}")),
    };
    let ch = l.ch.clone();
    lock(&root.st).charter = Some(l);
    if let Some(e) = root.egress.get() {
        e.reload(&ch);
    }
    root.sync_egress();
    root.publish();
}

fn req_dir(root: &Root) -> PathBuf {
    root.p.r("state/grants")
}

fn load_req(root: &Root, id: &str) -> Option<Value> {
    let ok = id.len() == 9 && id.starts_with('g') && id[1..].bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| std::fs::read_to_string(req_dir(root).join(format!("{id}.json"))).ok()).flatten().and_then(|s| serde_json::from_str(&s).ok())
}

fn save_req(root: &Root, r: &Value) -> std::io::Result<()> {
    let id = r["id"].as_str().unwrap_or("");
    util::write_atomic(&req_dir(root).join(format!("{id}.json")), util::icj(r)?.as_bytes(), 0o600)
}

fn requests(root: &Root) -> Vec<Value> {
    let mut v: Vec<Value> = std::fs::read_dir(req_dir(root)).into_iter().flatten().flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect();
    v.sort_by(|a: &Value, b: &Value| a["at"].as_str().cmp(&b["at"].as_str()));
    v
}

/// `grant.request{kind, value, duration_h?, why}` (control link) → `{id, status:"pending"}`. It changes no value.
pub fn request(root: &Root, a: &Value) -> Result<Value, Err3> {
    let kind = a["kind"].as_str().unwrap_or("");
    if !KINDS.contains(&kind) && !kind.starts_with("actions.") {
        return Err(no("root.args", format!("{kind:?} is not a grant kind (ADR-011 §7.4): grants never touch a Root mechanism")));
    }
    let value = {
        let st = lock(&root.st);
        let ch = &st.charter.as_ref().ok_or_else(|| no("root.mode", "no active charter"))?.ch;
        validate(kind, &a["value"], ch).map_err(|m| no("root.args", m))?
    };
    let h = a["duration_h"].as_i64().unwrap_or(DEFAULT_H);
    if !(1..=MAX_H).contains(&h) {
        return Err(no("root.args", format!("duration_h must be 1..{MAX_H}")));
    }
    let why: String = a["why"].as_str().unwrap_or("").chars().filter(|c| !c.is_control() || *c == '\n').take(WHY_MAX).collect();
    let now = now_ms();
    let recent = requests(root).iter().filter(|r| r["at"].as_str().and_then(parse_rfc3339).is_some_and(|t| now - t < 24 * HOUR)).count();
    if recent >= PER_DAY {
        return Err(no("root.rate", format!("at most {PER_DAY} grant requests per rolling 24 h")));
    }
    let id = format!("g{}", util::hex(&util::random::<4>().map_err(|e| no("root.internal", e.to_string()))?));
    let r = json!({"at": rfc3339(now), "duration_h": h, "id": id, "kind": kind, "status": "pending", "value": value, "why": why, "v": 1});
    std::fs::create_dir_all(req_dir(root)).and_then(|()| save_req(root, &r)).map_err(|e| no("root.internal", e.to_string()))?;
    root.append("grant_request", json!({"duration_h": h, "id": id, "kind": kind, "value": value, "why_sha256": sha256_hex(why.as_bytes())}))
        .ok_or_else(|| no("root.internal", "grant_request: the lineage append failed"))?;
    let short: String = why.chars().map(|c| if c == '\n' { ' ' } else { c }).take(300).collect();
    root.alert("grant_request", &format!("[root] the agent asks for grant {id}: {kind} = {} for {h} h. Why: {short}. \
        Type /grant {id} to approve (typed confirmation) or /deny {id}.", util::icj(&value).unwrap_or_default()));
    Ok(json!({"id": id, "status": "pending"}))
}

/// The owner's decision on a pending request. Approval needs `confirm` = the typed phrase (`root.confirm` otherwise)
/// and an effective charter that still passes C1–C17.
pub fn decide(root: &Root, id: &str, approve: bool, hours: Option<i64>, confirm: Option<&str>) -> Result<Value, Err3> {
    decide_at(root, id, approve, hours, confirm, now_ms())
}

fn decide_at(root: &Root, id: &str, approve: bool, hours: Option<i64>, confirm: Option<&str>, now: i64) -> Result<Value, Err3> {
    let mut r = load_req(root, id).ok_or_else(|| no("root.not_found", format!("no grant request {id:?}")))?;
    if r["status"] != "pending" {
        return Err(no("root.args", format!("grant {id} was already decided ({})", r["status"].as_str().unwrap_or("?"))));
    }
    let (kind, value) = (r["kind"].as_str().unwrap_or("").to_string(), r["value"].clone());
    if !approve {
        r["status"] = json!("denied");
        save_req(root, &r).map_err(|e| no("root.internal", e.to_string()))?;
        root.append("grant", json!({"decision": "deny", "id": id, "kind": kind, "until": null, "value": value}));
        return Ok(json!({"decision": "deny", "id": id}));
    }
    let h = hours.or(r["duration_h"].as_i64()).unwrap_or(DEFAULT_H);
    if !(1..=MAX_H).contains(&h) {
        return Err(no("root.args", format!("a grant lasts 1 h to 365 days (got {h} h)")));
    }
    let until = rfc3339(now + h * HOUR);
    if confirm != Some(crate::access::PHRASE) {
        let m = format!("Grant {id} sets {kind} = {} until {until} (the agent cannot change it; it expires by itself). Type `{}` to approve.",
            util::icj(&value).unwrap_or_default(), crate::access::PHRASE);
        return Err(("root.confirm", m, json!({"phrase": crate::access::PHRASE})));
    }
    let base = charter::load(&root.p.r("state/charter.active.toml"), &root.p.ctx())
        .map_err(|f| no("root.mode", format!("the active charter fails to load: {f:?}")))?;
    let mut entries: Vec<Entry> = read(&root.p).into_iter().filter(|e| live(e, now)).collect();
    entries.push(Entry { id: id.into(), kind: kind.clone(), value: value.clone(), until: until.clone() });
    let (_, skipped) = merge(&base.ch, &entries, &root.p.ctx(), now);
    if let Some((_, why)) = skipped.iter().find(|s| s.0 == id) {
        return Err(no("root.args", format!("grant {id} would make the charter invalid: {why}")));
    }
    write(&root.p, &entries).map_err(|e| no("root.internal", e.to_string()))?;
    r["status"] = json!("approved");
    save_req(root, &r).map_err(|e| no("root.internal", e.to_string()))?;
    root.append("grant", json!({"decision": "approve", "id": id, "kind": kind, "until": until, "value": value}));
    refresh(root);
    Ok(json!({"decision": "approve", "id": id, "until": until}))
}

/// Expiry, from `ledger::tick`: entries whose `until` passed leave the overlay, each with `grant{decision:"expired"}`.
pub fn tick(root: &Root) {
    tick_at(root, now_ms());
}

fn tick_at(root: &Root, now: i64) {
    {
        let mut m = lock(next_expiry());
        let due = m.get(&root.p.rh).is_none_or(|&t| t <= now);
        if !due {
            return;
        }
        m.insert(root.p.rh.clone(), i64::MAX); // claimed: a re-entered tick returns here
    }
    let (keep, gone): (Vec<Entry>, Vec<Entry>) = read(&root.p).into_iter().partition(|e| live(e, now));
    if gone.is_empty() {
        lock(next_expiry()).insert(root.p.rh.clone(), keep.iter().filter_map(|e| parse_rfc3339(&e.until)).min().unwrap_or(i64::MAX));
        return;
    }
    if let Err(e) = write(&root.p, &keep) {
        lock(next_expiry()).remove(&root.p.rh); // retried at the next tick
        return util::log(&root.p.rh, "error", &format!("grant expiry: {e}"));
    }
    for e in &gone {
        root.append("grant", json!({"decision": "expired", "id": e.id, "kind": e.kind, "until": e.until, "value": e.value}));
        root.notice("info", &format!("[root] grant {} expired: {} is back to the charter's value", e.id, e.kind));
    }
    refresh(root);
}

/// `/grants`, `/grant <id> [for <n>d|<n>h] [confirm]`, `/deny <id>`, `/release <cap>` (root.md §4.2). False: not ours.
pub fn line(root: &Root, t: &str) -> bool {
    let w: Vec<&str> = t.split_whitespace().collect();
    let say = |r: Result<Value, Err3>, ok: String| match r {
        Ok(_) => root.notice("info", &ok),
        Err(("root.confirm", m, _)) => root.notice("info", &format!("[root] {}", m.replace(&format!("Type `{}`", crate::access::PHRASE),
            &format!("Type {} {}", w.iter().filter(|x| **x != crate::access::PHRASE).copied().collect::<Vec<_>>().join(" "), crate::access::PHRASE)))),
        Err((_, m, _)) => root.notice("warn", &format!("[root] {m}")),
    };
    match w.as_slice() {
        ["/grants"] => {
            let now = now_ms();
            let pend: Vec<String> = requests(root).into_iter().filter(|r| r["status"] == "pending")
                .map(|r| format!("{} {} = {}", r["id"].as_str().unwrap_or(""), r["kind"].as_str().unwrap_or(""), r["value"])).collect();
            let live: Vec<String> = read(&root.p).into_iter().filter(|e| live(e, now)).map(|e| format!("{} {} = {} until {}", e.id, e.kind, e.value, e.until)).collect();
            root.notice("info", &format!("[root] grants · pending: {} · live: {}", if pend.is_empty() { "none".into() } else { pend.join("; ") },
                if live.is_empty() { "none".into() } else { live.join("; ") }));
        }
        ["/grant", id, rest @ ..] => {
            let confirm = rest.last().filter(|x| **x == crate::access::PHRASE).copied();
            let dur = match rest.iter().filter(|x| **x != crate::access::PHRASE).copied().collect::<Vec<_>>().as_slice() {
                [] => Ok(None),
                ["for", d] => parse_dur(d).map(Some).ok_or(()),
                _ => Err(()),
            };
            match dur {
                Ok(h) => say(decide(root, id, true, h, confirm), format!("[root] grant {id} approved")),
                Err(()) => root.notice("warn", "[root] usage: /grant <id> [for <n>d|<n>h]"),
            }
        }
        ["/deny", id] => say(decide(root, id, false, None, None), format!("[root] grant {id} denied")),
        ["/release", cap] => say(crate::select::release(root, cap), format!("[root] {cap} released from quarantine: a new version may be added at once")),
        _ => return false,
    }
    true
}

/// `30d`, `12h` → hours.
fn parse_dur(s: &str) -> Option<i64> {
    let (n, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: i64 = n.parse().ok().filter(|n| *n > 0)?;
    match unit {
        "d" => Some(n * 24),
        "h" => Some(n),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ accessors

fn extend(root: &Root) -> charter::Extend {
    lock(&root.st).charter.as_ref().map(|l| l.ch.extend.clone()).unwrap_or_default()
}

/// The blast-radius budget of `level` (charter `[extend]` with the overlay), for X3's A5 group checks.
pub fn budget(root: &Root, level: i64) -> Budget {
    extend(root).budget(level)
}

/// `extend.snap_max_gib` with the overlay, for X4's snapshot cap.
pub fn snap_max_gib(root: &Root) -> u64 {
    extend(root).snap_max_gib.max(1) as u64
}

/// The group window (turns, max ms, min ms): the charter's for body groups, ADR-011 §3.2's constants for the kernel.
pub fn win(root: &Root, kernel: bool) -> (i64, i64, i64) {
    let x = extend(root);
    if kernel { (40, 120 * HOUR, 24 * HOUR) } else { (x.win_turns, x.win_max_h * HOUR, x.win_min_h * HOUR) }
}

/// H5a's floor in micro-USD.
pub fn burst_floor_musd(root: &Root) -> u64 {
    charter::musd(&extend(root).burst_floor_usd).unwrap_or(500_000)
}

/// A `dep` change passes A5 only when a live grant covers its exact crate set (names, versions, checksums); read by
/// select.rs `admit` (`extend.dep_grant`).
pub fn dep_covers(root: &Root, crates: &Value) -> bool {
    let key = |v: &Value| {
        let mut s: Vec<String> = v.as_array().into_iter().flatten().map(|c| format!("{}@{}#{}", c["name"], c["version"], c["checksum"])).collect();
        s.sort();
        s
    };
    let want = key(crates);
    let now = now_ms();
    !want.is_empty() && read(&root.p).iter().any(|e| e.kind == "dep" && live(e, now) && key(&e.value) == want)
}

/// Members that may write the workspace (`fleet.workspace_write`; 0 without a grant); read by access.rs `spawn_grant`.
pub fn workspace_writers(root: &Root) -> i64 {
    let now = now_ms();
    read(&root.p).iter().filter(|e| e.kind == "fleet.workspace_write" && live(e, now)).filter_map(|e| e.value.as_i64()).max().unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests;
