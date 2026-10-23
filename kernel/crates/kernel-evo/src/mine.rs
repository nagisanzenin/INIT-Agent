//! `evo::mine` (evolution-v2 §3.4.3–§3.4.5): sources → instances → clusters → the friction index
//! `state/kernel/evo/frictions.jsonl`, incrementally, with cursors in `cursor.json`.
//!
//! Sources (§3.4.1): the live body's session logs, the proxy's agent-visible traces, the visible
//! pool runs (RunRecords) and the retained logs of the failing ones. One failure event seen in
//! several sources (a live tool result is in the session log *and* in the next request's trace)
//! is one instance: instances are merged by `<session>/<call_id>`, keeping every ref.
//!
//! Incremental mining: a session log is re-parsed only when its `(len, inode)` changed (a log's
//! signals need look-ahead within the file: red-then-green, fatal attribution, `finish.*`), and a
//! trace day file is read from its byte cursor (each request's carried tool results stand alone).
//! The index is therefore identical to a full rescan's (EV-05), except for friction ids, which
//! are stable across mines.

use crate::classify::{self, PathCtx};
use crate::fsx;
use crate::runs::{self, PoolRun};
use crate::session::{self, Instance, RunCtx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The engine's kernel-written state directory.
pub const EVO_DIR: &str = "state/kernel/evo";
/// Read cap per session log (K-F2).
pub const SESSION_CAP: u64 = 256 << 20;
/// Example refs kept per cluster.
pub const MAX_REFS: usize = 5;
/// Digest cap (§3.4.4).
pub const DIGEST_MAX: usize = 2048;

/// What to mine.
#[derive(Clone, Debug)]
pub struct MineCfg {
    pub path: PathCtx,
    /// Extra session logs imported read-only (e.g. the S-REAL fixtures of §3.10 D-1).
    pub imports: Vec<PathBuf>,
    pub live_sessions: bool,
    pub traces: bool,
    pub pool: bool,
}

impl MineCfg {
    /// Everything, for the kernel at `home`.
    pub fn for_home(home: &Path) -> MineCfg {
        MineCfg { path: PathCtx { cwd_rel: "work".into(), init_home: Some(home.to_path_buf()), check_fs: true }, imports: vec![], live_sessions: true, traces: true, pool: true }
    }
}

/// One cluster: one line of `frictions.jsonl` (§3.4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cluster {
    pub v: u32,
    pub id: String,
    pub signature: String,
    pub counts: Counts,
    pub s_c_permille: u32,
    pub labels: Vec<String>,
    pub families: Vec<String>,
    pub prio: u64,
    pub eligible: bool,
    pub tried: Vec<String>,
    pub refs: Vec<String>,
    pub first: String,
    pub last: String,
    /// The pool runs where this cluster was fatal, as `<job>/<run>` (for the screen's union s_c).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fatal: Vec<String>,
    /// Pool tasks where it was fatal (the screen's targeted tasks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fatal_tasks: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub instances: u64,
    pub sessions: u64,
    pub tasks: u64,
    pub fatal_runs: u64,
    pub divergence_pairs: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cursor {
    v: u32,
    #[serde(default)]
    ids: BTreeMap<String, String>,
    #[serde(default)]
    next: u64,
    /// Session-log sources: key → `[len, inode]`.
    #[serde(default)]
    files: BTreeMap<String, (u64, u64)>,
    /// Trace day files: name → byte offset.
    #[serde(default)]
    traces: BTreeMap<String, u64>,
    /// `cache.break`: session → (last cache_permille, last prefix_hash).
    #[serde(default)]
    chains: BTreeMap<String, (u64, String)>,
    /// Trace starts still waiting for their `end` (`req_id` → session, prefix_hash).
    #[serde(default)]
    pending: BTreeMap<String, (String, String)>,
    /// Failing pool runs (the s_c denominator) at the last mine.
    #[serde(default)]
    failing_runs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    src: String,
    i: Instance,
}

/// What a mine did.
#[derive(Clone, Debug, Default)]
pub struct MineOut {
    pub clusters: Vec<Cluster>,
    pub instances: usize,
    /// Failing test commands excluded as red-then-green.
    pub expected: Vec<String>,
    pub parsed_sources: usize,
    pub failing_runs: u64,
    pub ms: u64,
}

fn evo(home: &Path) -> PathBuf {
    home.join(EVO_DIR)
}

fn load_cursor(home: &Path) -> Cursor {
    fsx::read_capped(&evo(home).join("cursor.json"), 64 << 20)
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Cursor { v: 1, next: 1, ..Default::default() })
}

fn load_instances(home: &Path) -> BTreeMap<String, Vec<Instance>> {
    let mut m: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    if let Ok(Some(b)) = fsx::read_capped(&evo(home).join("instances.jsonl"), 512 << 20) {
        for l in String::from_utf8_lossy(&b).split('\n') {
            if let Ok(s) = serde_json::from_str::<Stored>(l) {
                m.entry(s.src).or_default().push(s.i);
            }
        }
    }
    m
}

fn list_jsonl(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Mines `home` into `state/kernel/evo/`.
pub fn mine(home: &Path, cfg: &MineCfg) -> io::Result<MineOut> {
    let t0 = Instant::now();
    let mut cur = load_cursor(home);
    if cur.next == 0 {
        cur.next = 1;
    }
    let mut by_src = load_instances(home);
    let mut out = MineOut::default();
    let mut live_keys: BTreeSet<String> = BTreeSet::new();

    // 1. Session logs: live and imported.
    let mut logs: Vec<(String, PathBuf, Option<RunCtx>)> = Vec::new();
    if cfg.live_sessions {
        for p in list_jsonl(&home.join("state/sessions")) {
            logs.push((format!("s:{}", p.file_name().and_then(|n| n.to_str()).unwrap_or("")), p, None));
        }
    }
    for p in &cfg.imports {
        logs.push((format!("i:{}", p.display()), p.clone(), None));
    }
    // 2. Pool runs and the retained logs of the failing ones.
    let pool: Vec<PoolRun> = if cfg.pool { runs::pool_runs(home) } else { vec![] };
    for r in pool.iter().filter(|r| r.failing()) {
        let p = runs::retained_log(home, r);
        if p.is_file() {
            let rc = RunCtx { run: format!("{}/{}", r.gate_id, r.run_id), task: r.task.clone(), label: Some(r.label()), family: r.family.clone(), failing: true };
            logs.push((format!("r:{}/{}", r.gate_id, r.run_id), p, Some(rc)));
        }
    }
    for (key, path, rc) in &logs {
        live_keys.insert(key.clone());
        let Some(id) = fsx::identity(path) else { continue };
        if cur.files.get(key) == Some(&id) && by_src.contains_key(key) {
            continue;
        }
        let Some(bytes) = fsx::read_capped(path, SESSION_CAP)? else { continue };
        let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("session").to_string();
        let parsed = session::parse(&bytes, &fallback, &cfg.path, rc.as_ref());
        out.parsed_sources += 1;
        by_src.insert(key.clone(), parsed.instances);
        by_src.insert(format!("x:{key}"), parsed.expected.iter().map(|u| expected_marker(u)).collect());
        cur.files.insert(key.clone(), id);
    }
    // Sources that are gone (retention, deleted logs) take their instances with them.
    let gone: Vec<String> = by_src.keys().filter(|k| (k.starts_with("s:") || k.starts_with("i:") || k.starts_with("r:")) && !live_keys.contains(*k)).cloned().collect();
    for k in gone {
        by_src.remove(&k);
        by_src.remove(&format!("x:{k}"));
        cur.files.remove(&k);
    }

    // 3. Traces, from the byte cursors.
    if cfg.traces {
        let root = home.join("vault/traces/v1");
        let mut new = Vec::new();
        for f in list_jsonl(&root.join("req")) {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let from = cur.traces.get(&name).copied().unwrap_or(0);
            let Some(bytes) = fsx::read_from(&f, from, 256 << 20)? else { continue };
            let Some(last_nl) = bytes.iter().rposition(|b| *b == b'\n') else { continue };
            let chunk = &bytes[..=last_nl];
            for l in String::from_utf8_lossy(chunk).split('\n') {
                let Ok(rec) = serde_json::from_str::<Value>(l) else { continue };
                trace_record(&root, &rec, &mut cur, &mut new);
            }
            cur.traces.insert(name, from + chunk.len() as u64);
            out.parsed_sources += 1;
        }
        by_src.entry("t".into()).or_default().extend(new);
        if cur.pending.len() > 4096 {
            let drop: Vec<String> = cur.pending.keys().take(cur.pending.len() - 4096).cloned().collect();
            for k in drop {
                cur.pending.remove(&k);
            }
        }
    }

    // 4. Run-level signals (recomputed: the RunRecords are small).
    by_src.insert("runs".into(), runs::run_instances(&pool));

    // 5. Merge and cluster.
    let expected: BTreeSet<String> = by_src.iter().filter(|(k, _)| k.starts_with("x:")).flat_map(|(_, v)| v.iter().map(|i| i.uid.clone())).collect();
    out.expected = expected.iter().cloned().collect();
    let mut merged: BTreeMap<String, Instance> = BTreeMap::new();
    for (k, v) in &by_src {
        if k.starts_with("x:") {
            continue;
        }
        for i in v {
            if expected.contains(&i.uid) {
                continue;
            }
            match merged.get_mut(&i.uid) {
                None => {
                    let mut i = i.clone();
                    if !i.refs.contains(&i.reference) {
                        i.refs.insert(0, i.reference.clone());
                    }
                    merged.insert(i.uid.clone(), i);
                }
                Some(m) => merge(m, i),
            }
        }
    }
    let failing_cached = pool.iter().filter(|r| r.cached && r.failing()).count() as u64;
    let failing_all = pool.iter().filter(|r| r.failing()).count() as u64;
    let failing = if failing_cached > 0 { failing_cached } else { failing_all };
    cur.failing_runs = failing;
    out.failing_runs = failing;
    let mixed = runs::mixed_tasks(&pool);
    let mut groups: BTreeMap<String, Vec<&Instance>> = BTreeMap::new();
    for i in merged.values() {
        groups.entry(i.signal.clone()).or_default().push(i);
    }
    // Ids in order of first appearance (ts, signature), stable once assigned.
    let mut order: Vec<(&String, String)> = groups.iter().map(|(s, v)| (s, v.iter().map(|i| i.ts.clone()).filter(|t| !t.is_empty()).min().unwrap_or_default())).collect();
    order.sort_by(|a, b| (a.1.is_empty(), &a.1, a.0).cmp(&(b.1.is_empty(), &b.1, b.0)));
    for (sig, _) in &order {
        if !cur.ids.contains_key(*sig) {
            cur.ids.insert((*sig).clone(), format!("f-{:06}", cur.next));
            cur.next += 1;
        }
    }
    let mut clusters: Vec<Cluster> = groups.iter().map(|(sig, v)| cluster(sig, v, &cur.ids[sig], failing, &mixed)).collect();
    clusters.sort_by(|a, b| b.prio.cmp(&a.prio).then(a.id.cmp(&b.id)));
    out.instances = merged.len();

    // 6. Write.
    let dir = evo(home);
    std::fs::create_dir_all(&dir)?;
    let mut lines = String::new();
    for c in &clusters {
        lines.push_str(&serde_json::to_string(c).map_err(io::Error::other)?);
        lines.push('\n');
    }
    fsx::write_atomic(&dir.join("frictions.jsonl"), lines.as_bytes())?;
    let mut inst = String::new();
    for (k, v) in &by_src {
        for i in v {
            inst.push_str(&serde_json::to_string(&Stored { src: k.clone(), i: i.clone() }).map_err(io::Error::other)?);
            inst.push('\n');
        }
    }
    fsx::write_atomic(&dir.join("instances.jsonl"), inst.as_bytes())?;
    fsx::write_atomic(&dir.join("cursor.json"), serde_json::to_vec(&cur).map_err(io::Error::other)?.as_slice())?;
    out.clusters = clusters;
    out.ms = t0.elapsed().as_millis() as u64;
    Ok(out)
}

fn expected_marker(uid: &str) -> Instance {
    Instance {
        signal: "expected".into(),
        weight: 0,
        reference: String::new(),
        refs: vec![],
        session: String::new(),
        task: None,
        label: None,
        family: None,
        run: None,
        ts: String::new(),
        detail: String::new(),
        failure: false,
        fatal: false,
        uid: uid.to_string(),
    }
}

fn merge(m: &mut Instance, i: &Instance) {
    for r in std::iter::once(&i.reference).chain(i.refs.iter()) {
        if !r.is_empty() && !m.refs.contains(r) {
            m.refs.push(r.clone());
        }
    }
    m.fatal |= i.fatal;
    m.failure |= i.failure;
    if m.ts.is_empty() {
        m.ts = i.ts.clone();
    }
    if m.task.is_none() {
        m.task = i.task.clone();
    }
    if i.run.is_some() {
        m.run = i.run.clone();
        m.label = i.label.clone().or(m.label.take());
        m.family = i.family.clone().or(m.family.take());
    }
    if m.label.is_none() {
        m.label = i.label.clone();
    }
}

/// Attested kinds first: `trace:`, `run:`, then the rest.
fn rank(r: &str) -> u8 {
    if r.starts_with("trace:") {
        0
    } else if r.starts_with("run:") {
        1
    } else {
        2
    }
}

/// True for the attested ref kinds of §3.4.1 (`friction:` is attested too, but is a cluster id).
pub fn is_attested(r: &str) -> bool {
    r.starts_with("trace:") || r.starts_with("run:") || r.starts_with("friction:")
}

fn cluster(sig: &str, v: &[&Instance], id: &str, failing: u64, mixed: &BTreeSet<String>) -> Cluster {
    let sessions: BTreeSet<&str> = v.iter().map(|i| i.session.as_str()).collect();
    let tasks: BTreeSet<&str> = v.iter().filter_map(|i| i.task.as_deref()).collect();
    let fatal_runs: BTreeSet<&str> = v.iter().filter(|i| i.fatal).filter_map(|i| i.run.as_deref()).collect();
    let fatal_tasks: BTreeSet<&str> = v.iter().filter(|i| i.fatal).filter_map(|i| i.task.as_deref()).collect();
    let fail_tasks: BTreeSet<&str> = v.iter().filter(|i| i.run.is_some() && i.failure).filter_map(|i| i.task.as_deref()).collect();
    let div = fail_tasks.iter().filter(|t| mixed.contains(**t)).count() as u64;
    let labels: BTreeSet<String> = v.iter().filter_map(|i| i.label.clone()).collect();
    let families: BTreeSet<String> = v.iter().filter_map(|i| i.family.clone()).collect();
    let mut refs: Vec<String> = v.iter().flat_map(|i| i.refs.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    refs.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
    // Eligibility (§3.4.3): ≥ 2 attested refs from ≥ 2 distinct tasks or sessions, or 1 + a
    // divergence pair. A unit is the instance's task when it has one, else its session.
    let mut units: BTreeSet<String> = BTreeSet::new();
    let mut attested: BTreeSet<&str> = BTreeSet::new();
    for i in v {
        let a: Vec<&String> = i.refs.iter().filter(|r| r.starts_with("trace:") || r.starts_with("run:")).collect();
        if !a.is_empty() && i.failure {
            units.insert(i.task.clone().unwrap_or_else(|| i.session.clone()));
            attested.extend(a.iter().map(|s| s.as_str()));
        }
    }
    let eligible = (attested.len() >= 2 && units.len() >= 2) || (!attested.is_empty() && div >= 1);
    let weight: u64 = v.iter().map(|i| i.weight as u64).sum();
    let ts: Vec<&str> = v.iter().map(|i| i.ts.as_str()).filter(|t| !t.is_empty()).collect();
    Cluster {
        v: 1,
        id: id.to_string(),
        signature: sig.to_string(),
        counts: Counts { instances: v.len() as u64, sessions: sessions.len() as u64, tasks: tasks.len() as u64, fatal_runs: fatal_runs.len() as u64, divergence_pairs: div },
        s_c_permille: (fatal_runs.len() as u64 * 1000).checked_div(failing).unwrap_or(0).min(1000) as u32,
        labels: labels.into_iter().collect(),
        families: families.into_iter().collect(),
        prio: 3 * div + 2 * fatal_tasks.len() as u64 + weight,
        eligible,
        tried: vec![],
        refs: refs.into_iter().take(MAX_REFS).collect(),
        first: ts.iter().min().map(|s| s.to_string()).unwrap_or_default(),
        last: ts.iter().max().map(|s| s.to_string()).unwrap_or_default(),
        fatal: fatal_runs.iter().map(|s| s.to_string()).collect(),
        fatal_tasks: fatal_tasks.iter().map(|s| s.to_string()).collect(),
    }
}

/// One trace record (a `start` or an `end` line) into instances.
fn trace_record(root: &Path, rec: &Value, cur: &mut Cursor, out: &mut Vec<Instance>) {
    let t = rec.get("t").and_then(Value::as_str).unwrap_or("");
    let Some(id) = rec.get("req_id").and_then(Value::as_str) else { return };
    match t {
        "start" => {
            if rec.get("vis").and_then(Value::as_str) != Some("agent") {
                return;
            }
            let elems = match kernel_proxy::trace::reconstruct(root, id) {
                Ok((_, e)) => e.iter().filter_map(|b| serde_json::from_slice::<Value>(b).ok()).collect::<Vec<_>>(),
                Err(_) => vec![],
            };
            let v = crate::trace::from_records(id, rec, None, &elems);
            let session = v.session.clone().unwrap_or_else(|| "_none".into());
            cur.pending.insert(id.to_string(), (session.clone(), v.prefix_hash.clone().unwrap_or_default()));
            let cx = PathCtx::default();
            for c in &v.carried {
                let exit = classify::exit_from_text(&c.text);
                let ev = classify::ToolEvent { tool: &c.name, input: &c.args, text: &c.text, exit_code: exit, is_error: classify::text_is_failure(&c.text) };
                let Some(h) = classify::classify(&ev, &cx) else { continue };
                out.push(Instance {
                    weight: h.weight,
                    reference: format!("trace:{id}"),
                    refs: vec![format!("trace:{id}")],
                    session: session.clone(),
                    task: v.task.clone(),
                    label: (v.scope == "live").then(|| "live".to_string()),
                    family: None,
                    run: None,
                    ts: v.ts.clone(),
                    detail: h.detail.clone(),
                    failure: h.failure,
                    fatal: false,
                    uid: format!("{session}/{}", c.call_id),
                    signal: h.signal,
                });
            }
        }
        "end" => {
            let Some((session, prefix)) = cur.pending.remove(id) else { return };
            let Some(cp) = rec.get("cache_permille").and_then(Value::as_u64) else { return };
            if let Some((prev, pprefix)) = cur.chains.get(&session)
                && *prev >= 900
                && cp < 500
                && *pprefix == prefix
            {
                let ts = rec.get("ts").and_then(Value::as_str).unwrap_or("").to_string();
                out.push(Instance {
                    signal: "cache.break".into(),
                    weight: 1,
                    reference: format!("trace:{id}"),
                    refs: vec![format!("trace:{id}")],
                    session: session.clone(),
                    task: None,
                    label: None,
                    family: None,
                    run: None,
                    ts,
                    detail: format!("cache {cp}‰ after {prev}‰ with the same prefix"),
                    failure: false,
                    fatal: false,
                    uid: format!("{session}/{id}/cache"),
                });
            }
            cur.chains.insert(session, (cp, prefix));
        }
        _ => {}
    }
}

/// Reads the index (`frictions.jsonl`).
pub fn read_index(home: &Path) -> Vec<Cluster> {
    let Ok(Some(b)) = fsx::read_capped(&evo(home).join("frictions.jsonl"), 16 << 20) else { return vec![] };
    String::from_utf8_lossy(&b).split('\n').filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Failing pool runs at the last mine (the s_c denominator).
pub fn failing_runs(home: &Path) -> u64 {
    load_cursor(home).failing_runs
}

/// The `initctl evo frictions` digest: the top 10 by priority, ≤ 2 KiB (§3.4.4).
pub fn digest(clusters: &[Cluster]) -> String {
    let eligible = clusters.iter().filter(|c| c.eligible).count();
    let mut s = format!("frictions: {} clusters, {eligible} eligible (top 10 by priority; --json --all for the index)\n", clusters.len());
    for c in clusters.iter().take(10) {
        let line = format!(
            "{} {} ×{} · {} sess · {} tasks · fatal {} · s_c {}‰ · div {} · prio {}{} · {}\n",
            c.id,
            c.signature,
            c.counts.instances,
            c.counts.sessions,
            c.counts.tasks,
            c.counts.fatal_runs,
            c.s_c_permille,
            c.counts.divergence_pairs,
            c.prio,
            if c.eligible { " · eligible" } else { "" },
            c.refs.iter().take(2).cloned().collect::<Vec<_>>().join(", ")
        );
        if s.len() + line.len() > DIGEST_MAX {
            break;
        }
        s.push_str(&line);
    }
    s
}

/// The `evolution` object for `boot.json` (evolution-v2 §3.4.4, §4.6): eligible frictions, the
/// top one, open proposals by state.
pub fn boot_summary(home: &Path) -> Value {
    let idx = read_index(home);
    let eligible: Vec<&Cluster> = idx.iter().filter(|c| c.eligible).collect();
    let mut o = serde_json::Map::new();
    o.insert("frictions_eligible".into(), json!(eligible.len()));
    if let Some(top) = eligible.first() {
        o.insert("frictions_top".into(), json!({"signature": top.signature, "instances": top.counts.instances}));
    }
    let by = crate::proposal::open_by_state(home);
    o.insert("proposals_open".into(), json!(by.values().sum::<u64>()));
    o.insert("proposals_by_state".into(), json!(by));
    Value::Object(o)
}
