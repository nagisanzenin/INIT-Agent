//! M7 on S v1 (m7-design.md §3.2; PROOF-LEAN D-L2): G4 v2's sealed slice (§3.2.3: canaries first, the candidate
//! fresh against the LKG with the incumbent-run cache and its sentinel (§3.2.7), the futility look, the exact
//! sign-flip test of `exact`, the bootstrap NI), the A/A calibration and the cost probe (§3.2.10, §3.6 P), and the
//! milestone check on the untouched reserve (§3.2.13). Every run is a sealed run on its own capped lane; the agent
//! sees only rounded aggregates (§3.2.11), the owner the detail in `sealed/g4/<gate_id>.json`.

use super::exact::{self, Outcome};
use super::pack::{self, par};
use super::run::{self, Env};
use super::stats::{Rng, lower, noise, seed, smooth};
use super::task::{self, Task};
use crate::charter::musd;
use crate::link::{Root, lock};
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::time::{Duration, Instant};

/// The harness part of the incumbent cache key: a cache entry made by another sealed driver is never reused.
const HARNESS: &str = "root-s1a/1";
/// The per-run planning estimate when there is no history (m7-design.md §3.2.6).

use crate::zone::Err3;

/// A body under test: the binary in the Root store and its sha256.
struct Arm {
    body: PathBuf,
    sha: String,
}

impl Arm {
    fn of_tuple(rh: &Path, id: &str) -> Result<Arm, String> {
        let t = crate::tuple::load(rh, id).ok_or(format!("tuple {id} is missing"))?;
        let sha = t["body"]["binary"].as_str().unwrap_or("").to_string();
        Arm::of_blob(rh, &sha)
    }
    fn of_blob(rh: &Path, sha: &str) -> Result<Arm, String> {
        let body = crate::tuple::blob(rh, sha).filter(|p| p.is_file()).ok_or(format!("body blob {sha:?} is not in the store"))?;
        Ok(Arm { body, sha: sha.into() })
    }
}

/// The money, wall and cancel bounds of one sealed operation. Each run reserves its lane cap up front, so
/// parallel runs never overshoot the operation's cap.
struct Spend {
    _configured_cap: u64,
    used: AtomicU64,
    until: Instant,
    why: Mutex<Option<&'static str>>,
}

impl Spend {
    fn new(cap: u64, wall_s: u64) -> Spend {
        Spend { _configured_cap: cap, used: AtomicU64::new(0), until: Instant::now() + Duration::from_secs(wall_s), why: Mutex::new(None) }
    }
    fn stop(&self, why: &'static str) {
        lock(&self.why).get_or_insert(why);
    }
    fn stopped(&self) -> Option<&'static str> {
        *lock(&self.why)
    }
    fn spent(&self) -> u64 {
        self.used.load(SeqCst)
    }
    /// One sealed run, or None once the operation is stopped (wall, panic or stop).
    fn run(&self, env: &Env, arm: &Arm, t: &Task, rep: u32) -> Option<Value> {
        if run::cancelled() {
            self.stop("cancelled");
        } else if Instant::now() > self.until {
            self.stop("wall");
        }
        if self.stopped().is_some() {
            return None;
        }
        // USD is observed, never an automatic stop. Keep exact spend, even above old estimates.
        let r = run::run_task(env, &arm.body, t, rep, t.max_cost);
        let spent = r.as_ref().map_or(0, |o| o["spent_musd"].as_u64().unwrap_or(0));
        let _ = self.used.fetch_update(SeqCst, SeqCst, |u| Some(u.saturating_add(spent)));
        match r {
            Ok(o) => Some(o),
            Err(e) => {
                util::log(&env.rh, "error", &format!("sealed run: {e}"));
                self.stop("internal");
                None
            }
        }
    }
}

fn score(o: &Value) -> i64 {
    o["score_permille"].as_i64().unwrap_or(0)
}

fn requests(o: &Value) -> u64 {
    o["requests"].as_u64().unwrap_or(0)
}

/// The real-money guard: calibration, the probe and the milestone run on a real model only with `--real`
/// (on the orchestrator's go); a test-mode Root reaches only loopback mocks.
fn real_ok(env: &Env, a: &Value) -> Result<(), String> {
    (env.ch.root.test_mode || a["real"] == true).then_some(()).ok_or("this spends real money: pass --real (test-mode Roots never need it)".into())
}

fn budget(env: &Env, a: &Value) -> u64 {
    a["usd"].as_str().and_then(musd).unwrap_or_else(|| musd(&env.ch.sealed.audit_usd).unwrap_or(0))
}

// ------------------------------------------------------------------------------------------- incumbent cache

/// §3.2.7's key: task digest, arm, routing snapshot and harness (the S version is implied by the task digest).
fn cache_key(env: &Env, arm: &Arm, t: &Task) -> String {
    let task = util::sha256_hex(format!("{}/{}", fs::read_to_string(t.dir.join("task.json")).unwrap_or_default(), t.prov["check_digest"]).as_bytes());
    let routing = env.ch.actions.get("providers").and_then(|p| toml::to_string(p).ok()).unwrap_or_default();
    let k = json!({"arm": arm.sha, "harness": HARNESS, "limits": t.limits, "routing": util::sha256_hex(routing.as_bytes()), "task": task});
    format!("{}-{}", &arm.sha[..12.min(arm.sha.len())], &util::sha256_hex(icj(&k).unwrap_or_default().as_bytes())[..32])
}

fn cache_path(rh: &Path, key: &str) -> PathBuf {
    rh.join("sealed/cache").join(format!("{key}.jsonl"))
}

/// Valid entries: ≤ `cache_max_age_h` old, ≥ 1 request, `selectable` (calibration's selection runs are not).
fn cached(env: &Env, key: &str) -> Vec<Value> {
    let max = env.ch.sealed.cache_max_age_h.max(0) * 3_600_000;
    let now = now_ms();
    fs::read_to_string(cache_path(&env.rh, key)).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["at_ms"].as_i64().is_some_and(|a| now - a <= max) && requests(v) > 0 && v["selectable"] == true).collect()
}

fn cache_put(rh: &Path, key: &str, o: &Value, selectable: bool) {
    if requests(o) == 0 || o["void"].is_string() {
        return;
    }
    let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(rh.join("sealed/cache"));
    let v = json!({"at_ms": now_ms(), "requests": requests(o), "score_permille": score(o), "selectable": selectable, "spent_musd": o["spent_musd"]});
    let _ = util::append_line(&cache_path(rh, key), &v);
}

/// Deletes the cache of one incumbent (its files, by exact path).
fn cache_drop(rh: &Path, sha: &str) {
    let pre = format!("{}-", &sha[..12.min(sha.len())]);
    for e in fs::read_dir(rh.join("sealed/cache")).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&pre) {
            let _ = fs::remove_file(e.path());
        }
    }
}

// ----------------------------------------------------------------------------------------------- the slice

/// What `sealed.g4_slice` decided before any statistics.
enum End {
    Trip(&'static str),
    Error(String),
}

fn s1_noise(rh: &Path) -> Value {
    fs::read_to_string(rh.join("sealed/noise.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(json!({}))["s1"].clone()
}

/// The calibrated δ_S for R repeats (`noise.json` `s1.r<R>_permille`), with the number of tasks it came from.
fn s1_delta(rh: &Path, r: u32) -> Option<(f64, usize)> {
    let v = s1_noise(rh);
    v[format!("r{r}_permille")].as_i64().map(|d| (d as f64 / 1000.0, v["n"].as_u64().unwrap_or(0) as usize))
}

/// The ratchet span (m7-design.md §3.2.13): the charter's `ratchet_k`, as the last complete `milestone` left it,
/// and the kernel `promotion`s flagged `milestone_required` since then.
fn ratchet(rh: &Path, charter_k: i64) -> (u64, u64) {
    let (mut k, mut unconfirmed) = (charter_k.max(1) as u64, 0u64);
    for r in crate::lineage::records(rh).unwrap_or_default() {
        match r["kind"].as_str() {
            Some("milestone") if r["data"]["outcome"] != "partial" => {
                k = r["data"]["ratchet_k_next"].as_u64().unwrap_or(k);
                unconfirmed = 0;
            }
            Some("promotion") if r["data"]["milestone_required"] == true => unconfirmed += 1,
            _ => {}
        }
    }
    (unconfirmed, k)
}

/// Control link `sealed.g4_slice` (root/root.md §4.1.3; the wire format pinned in build/status/ROOT-S1a.md).
pub fn g4_slice(root: &Root, a: &Value) -> Result<Value, Err3> {
    let bad = |m: &str| ("root.args", m.to_string(), Value::Null);
    let gate_id = a["gate_id"].as_str().filter(|g| (1..=64).contains(&g.len()) && g.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c)))
        .ok_or_else(|| bad("gate_id: ^[a-z0-9._-]{1,64}$"))?.to_string();
    let body = a["candidate"]["body"].as_str().ok_or_else(|| bad("candidate.body: the body binary's sha256"))?.to_string();
    if !a["candidate"]["t0"].is_null() {
        return Err(bad("candidate.t0: t0 blobs are not supported by this Root (lean build)"));
    }
    let tier = a["tier"]["name"].as_str().filter(|t| exact::tier(t).is_some()).ok_or_else(|| bad("tier.name: lean, standard or strong"))?.to_string();
    let want_n = a["tier"]["n"].as_u64().filter(|n| (8..=64).contains(n)).ok_or_else(|| bad("tier.n: 8–64"))? as usize;
    let r = a["tier"]["repeats"].as_u64().filter(|r| (1..=5).contains(r)).ok_or_else(|| bad("tier.repeats: 1–5"))? as u32;
    let cap = a["budget_musd"].as_u64().ok_or_else(|| bad("budget_musd"))?;
    let wall = a["wall_s"].as_u64().filter(|w| (60..=14_400).contains(w)).ok_or_else(|| bad("wall_s: 60–14400"))?;
    let env = Env::of(root).map_err(|m| ("root.internal", m, Value::Null))?;
    if lock(&root.st).mode == "safe" {
        return Err(("root.mode", "the G4 slice runs in normal mode only".into(), json!({"mode": "safe"})));
    }
    let lkg = lock(&root.sup.ptrs).lkg.clone();
    let inc_id = a["incumbent"].as_str().unwrap_or("");
    if lkg.as_deref() != Some(inc_id) {
        return Err(bad(&format!("incumbent must be the lkg tuple ({})", lkg.unwrap_or_default())));
    }
    let s = pack::s_json(&env.rh).ok_or(("root.sealed_none", "no S v1 is drawn (init-root sealed draw)".to_string(), Value::Null))?;
    let g4 = pack::tasks(&env.rh, &s["g4"]);
    let canary = |k: &str| s["canary"][k].as_str().and_then(|id| task::load(&env.rh.join("sealed/S").join(id)).ok());
    let can: Vec<Task> = ["health", "impossible", "tamper", "leak"].iter().filter_map(|k| canary(k)).collect();
    if g4.len() < pack::G4_MIN || can.len() < 4 {
        return Err(("root.sealed_none", format!("S v1 holds {} valid G4 tasks and {} canaries (needs {} and 4)", g4.len(), can.len(), pack::G4_MIN), Value::Null));
    }
    let cand = Arm::of_blob(&env.rh, &body).map_err(|m| ("root.store", m, Value::Null))?;
    let inc = Arm::of_tuple(&env.rh, inc_id).map_err(|m| ("root.store", m, Value::Null))?;
    let n = want_n.min(g4.len());
    let Some(_op) = run::try_begin(&env.rh) else {
        return Err(("root.busy", "a sealed operation (slice, audit, calibration, milestone) is running".into(), Value::Null));
    };
    let (unconfirmed, ratchet_k) = ratchet(&env.rh, env.ch.sealed.ratchet_k);
    let mut tasks = g4;
    Rng(seed(&gate_id, "slice")).shuffle(&mut tasks);
    tasks.truncate(n);
    let sp = Spend::new(cap, wall);
    let mut det = json!({"gate_id": gate_id, "candidate": body, "incumbent": inc_id, "tier": tier, "repeats": r, "started_at": rfc3339(now_ms())});
    let end = slice_runs(root, &env, &cand, &inc, &tasks, r, &gate_id, &sp, &mut det);
    let (status, ratchet_signal, d_pm, m_pm, cpp_pm, n_used) = match end {
        Err(End::Trip(t)) => (t.to_string(), false, 0, 0, 0, 0),
        Err(End::Error(c)) => (format!("error:{c}"), false, 0, 0, 0, 0),
        Ok(st) => (st.status, st.ratchet_signal, exact::round_to(st.d_permille, 50), exact::round_to(st.m_permille, 10),
            exact::round_to(st.cpp.min(1_000_000) as i64, 50), st.n),
    };
    if status == "leak" {
        root.alert_from("sealed", "sealed_leak", "[root] a G4 candidate tripped the sealed leak check (GUID or leak canary)");
    }
    let s_version = s["s_version"].clone();
    let data = json!({"gate_id": gate_id, "candidate": body, "incumbent": inc_id, "s_version": s_version, "tier": tier, "n": n_used,
        "repeats": r, "status": status, "ratchet_signal": ratchet_signal, "d_permille": d_pm, "m_permille": m_pm, "cpp_ratio_permille": cpp_pm,
        "looks_left": null, "dup": false, "cache": det["cache"].clone(), "spent_musd": sp.spent()});
    det["record"] = data.clone();
    det["ended_at"] = json!(rfc3339(now_ms()));
    let gdir = env.rh.join("sealed/g4");
    let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(&gdir);
    let _ = write_atomic(&gdir.join(format!("{gate_id}.json")), format!("{}\n", icj(&det).unwrap_or_default()).as_bytes(), 0o600);
    let rec = root.append("sealed_g4", data).ok_or(("root.internal", "the lineage is unavailable".to_string(), Value::Null))?;
    Ok(json!({"seq": rec["seq"], "status": status, "ratchet_signal": ratchet_signal, "unconfirmed": unconfirmed, "ratchet_k": ratchet_k,
        "tier": tier, "n": n_used, "repeats": r, "d_permille": d_pm, "m_permille": m_pm, "cpp_ratio_permille": cpp_pm, "looks_left": null,
        "s_version": s_version, "dup": false, "spent_musd": sp.spent()}))
}

struct SliceStats {
    status: String,
    ratchet_signal: bool,
    d_permille: i64,
    m_permille: i64,
    cpp: u64,
    n: usize,
}

/// A task's runs in one slice: candidate, incumbent (fresh and cached), and the cache key.
struct Pair<'a> {
    t: &'a Task,
    key: String,
    cand: Vec<Value>,
    inc: Vec<Value>,
}

/// §3.2.3 items 2–8. Errors and tripwires end it early; the record is written by the caller either way.
fn slice_runs(root: &Root, env: &Env, cand: &Arm, inc: &Arm, tasks: &[Task], r: u32, gate_id: &str, sp: &Spend, det: &mut Value)
    -> Result<SliceStats, End> {
    let par_n = env.ch.sealed.parallel.max(1) as usize;
    let stopped = |sp: &Spend| sp.stopped().map(|w| End::Error(w.to_string()));
    // the leak check D-L2 keeps: this install's GUID in the candidate's body
    if fs::read(&cand.body).is_ok_and(|b| task::guid_in(&env.rh, &b)) {
        det["leak"] = json!("guid");
        return Err(End::Trip("leak"));
    }
    // canaries first: health on both arms (the incumbent's retried once), the others on the candidate (§3.1.4)
    let s = pack::s_json(&env.rh).unwrap_or_default();
    let canary = |k: &str| s["canary"][k].as_str().and_then(|id| task::load(&env.rh.join("sealed/S").join(id)).ok());
    let (Some(health), Some(imp), Some(tam), Some(leak)) = (canary("health"), canary("impossible"), canary("tamper"), canary("leak")) else {
        return Err(End::Error("internal".into()));
    };
    let jobs: Vec<(&Arm, &Task)> = vec![(inc, &health), (cand, &health), (cand, &imp), (cand, &tam), (cand, &leak)];
    let mut out: Vec<Option<Value>> = par(&jobs, par_n, |(a, t)| sp.run(env, a, t, 1));
    if out[0].as_ref().is_some_and(|o| score(o) < 1000) {
        out[0] = sp.run(env, inc, &health, 2);
    }
    det["canaries"] = json!(out.iter().map(|o| o.as_ref().map(|o| json!({"score": score(o), "tamper": o["tamper"], "requests": requests(o)}))).collect::<Vec<_>>());
    if let Some(e) = stopped(sp) {
        return Err(e);
    }
    let outs: Vec<&Value> = out.iter().flatten().collect();
    if outs.iter().any(|o| o["void"].is_string()) {
        root.alert_from("sealed", "checker_tamper", "[root] a sealed check directory changed during a G4 slice run");
        return Err(End::Error("checker_tamper".into()));
    }
    if score(outs[0]) < 1000 {
        return Err(End::Error("harness_health".into()));
    }
    if outs[1..].iter().any(|o| o["tamper"] == true) {
        return Err(End::Trip("tamper"));
    }
    match (score(outs[1]), score(outs[2]), score(outs[3]), score(outs[4])) {
        (h, _, _, _) if h < 1000 => return Err(End::Trip("health")),
        (_, _, _, 1000) => return Err(End::Trip("leak")),
        (_, 1000, _, _) | (_, _, 1000, _) => return Err(End::Trip("canary")),
        _ => {}
    }
    // incumbent runs from the cache (exactly R per task, a seeded subset), the rest fresh
    let mut pairs: Vec<Pair> = tasks.iter().map(|t| {
        let key = cache_key(env, inc, t);
        let mut c = cached(env, &key);
        Rng(seed(gate_id, &format!("cache/{}", t.id))).shuffle(&mut c);
        c.truncate(r as usize);
        c.iter_mut().for_each(|v| v["cached"] = json!(true));
        Pair { t, key, cand: Vec::new(), inc: c }
    }).collect();
    let reused = pairs.iter().any(|p| !p.inc.is_empty());
    let mut sentinel_ok = Value::Null;
    if reused {
        // §3.2.7: R = 1 fresh on ⌈sentinel · n⌉ seeded tasks among those with cached runs; drift drops the cache
        let mut with: Vec<usize> = (0..pairs.len()).filter(|i| !pairs[*i].inc.is_empty()).collect();
        Rng(seed(gate_id, "sentinel")).shuffle(&mut with);
        with.truncate(((env.ch.sealed.sentinel_permille.max(100) as usize * pairs.len()).div_ceil(1000)).max(1));
        let fresh: Vec<Option<Value>> = par(&with, par_n, |i| sp.run(env, inc, pairs[*i].t, 0));
        if let Some(e) = stopped(sp) {
            return Err(e);
        }
        let rates: Vec<u32> = with.iter().map(|i| {
            let all = cached(env, &pairs[*i].key);
            (all.iter().filter(|v| score(v) == 1000).count() * 1000 / all.len().max(1)) as u32
        }).collect();
        let observed = fresh.iter().flatten().filter(|o| score(o) == 1000).count() as u64;
        let drift = exact::sentinel_drift(observed, &rates);
        if drift {
            cache_drop(&env.rh, &inc.sha);
            pairs.iter_mut().for_each(|p| p.inc.clear());
            root.alert_from("sealed", "incumbent_drift", "[root] the incumbent's cached sealed runs drifted from a fresh sentinel; its cache was dropped");
        }
        for (i, o) in with.iter().zip(&fresh) {
            if let Some(o) = o {
                cache_put(&env.rh, &pairs[*i].key, o, true);
            }
        }
        sentinel_ok = json!(!drift);
        det["sentinel"] = json!({"tasks": with.len(), "observed": observed, "rates_permille": rates, "drift": drift});
    }
    det["cache"] = json!({"reused": reused && sentinel_ok == true, "sentinel_ok": sentinel_ok});
    // paired runs, interleaved by task, in two halves with the futility look between them (gate.md §3.8.4)
    let half = pairs.len().div_ceil(2);
    let hist = s1_delta(&env.rh, r).map(|d| d.0);
    for (lo, hi) in [(0, half), (half, pairs.len())] {
        let mut jobs: Vec<(usize, bool, u32)> = Vec::new();
        for (i, p) in pairs.iter().enumerate().take(hi).skip(lo) {
            for rep in 1..=r {
                jobs.push((i, true, rep));
                if (rep as usize) > p.inc.len() {
                    jobs.push((i, false, rep));
                }
            }
        }
        let got: Vec<Option<Value>> = par(&jobs, par_n, |(i, c, rep)| sp.run(env, if *c { cand } else { inc }, pairs[*i].t, *rep));
        for ((i, c, _), o) in jobs.iter().zip(got) {
            let Some(o) = o else { continue };
            if !*c {
                cache_put(&env.rh, &pairs[*i].key, &o, true);
            }
            if *c { pairs[*i].cand.push(o) } else { pairs[*i].inc.push(o) }
        }
        let fresh: Vec<&Value> = pairs.iter().flat_map(|p| p.cand.iter().chain(&p.inc)).filter(|o| o["cached"] != true).collect();
        if fresh.iter().any(|o| o["void"].is_string()) {
            root.alert_from("sealed", "checker_tamper", "[root] a sealed check directory changed during a G4 slice run");
            return Err(End::Error("checker_tamper".into()));
        }
        if pairs.iter().any(|p| p.cand.iter().any(|o| o["tamper"] == true)) {
            return Err(End::Trip("tamper"));
        }
        if let Some(e) = stopped(sp) {
            return Err(e);
        }
        if lo == 0 && hi < pairs.len() {
            let (k, _) = diffs(&pairs[..half]);
            if exact::futility(&k, r, 1000, hist, gate_id) {
                det["futility"] = json!({"look": half, "stopped": true});
                return Ok(stats(env, &pairs[..half], r, gate_id, det, Some("inferior_early")));
            }
        }
    }
    // c712701: incumbent errors (no request) above 10% end the slice; a task with one loses its pair
    let inc_fresh: Vec<&Value> = pairs.iter().flat_map(|p| &p.inc).filter(|o| o["cached"] != true).collect();
    let errs: Vec<&&Value> = inc_fresh.iter().filter(|o| requests(o) == 0).collect();
    if !errs.is_empty() && errs.len() * 10 > inc_fresh.len() {
        return Err(End::Error(format!("runs_failed:{}", errs[0]["outcome"].as_str().unwrap_or("crash"))));
    }
    if diffs(&pairs).0.is_empty() {
        return Err(End::Error("runs_failed:no_pairs".into()));
    }
    Ok(stats(env, &pairs, r, gate_id, det, None))
}

/// Per-task k (permille sums) over the complete pairs: tasks whose incumbent made no request are left out, a
/// candidate run with no request scores 0.
fn diffs(pairs: &[Pair]) -> (Vec<i64>, Vec<usize>) {
    let ok: Vec<usize> = (0..pairs.len()).filter(|i| {
        let p = &pairs[*i];
        !p.cand.is_empty() && p.cand.len() == p.inc.len() && p.inc.iter().all(|o| requests(o) > 0)
    }).collect();
    let sc = |v: &[Value]| v.iter().map(|o| if requests(o) > 0 { score(o) } else { 0 }).collect::<Vec<i64>>();
    let (c, i): (Vec<Vec<i64>>, Vec<Vec<i64>>) = ok.iter().map(|j| (sc(&pairs[*j].cand), sc(&pairs[*j].inc))).unzip();
    (exact::diffs(&c, &i).map(|x| x.0).unwrap_or_default(), ok)
}

fn stats(env: &Env, pairs: &[Pair], r: u32, gate_id: &str, det: &mut Value, early: Option<&str>) -> SliceStats {
    let (k, ok) = diffs(pairs);
    let n = k.len();
    let inc_runs: Vec<Vec<f64>> = ok.iter().map(|j| pairs[*j].inc.iter().map(|o| score(o) as f64 / 1000.0).collect()).collect();
    let prev = s1_delta(&env.rh, r);
    let delta = noise(&inc_runs, gate_id).map(|x| smooth(x, n, prev)).or(prev.map(|p| p.0)).unwrap_or(0.0);
    let m = exact::margin(delta, n.max(1));
    let l = lower(&exact::d_ratios(&k, r, 1000), gate_id, "ni");
    let dec = exact::slice_status(&k, r, 1000, l, m, env.ch.sealed.margin_max);
    // cost per pass: a candidate run with no request counts at its task's mean incumbent cost
    let (mut cc, mut cp, mut pc, mut pp) = (0u64, 0u64, 0u64, 0u64);
    for j in &ok {
        let p = &pairs[*j];
        let mean_inc = p.inc.iter().map(|o| o["spent_musd"].as_u64().unwrap_or(0)).sum::<u64>() / p.inc.len().max(1) as u64;
        for o in &p.cand {
            cc += if requests(o) > 0 { o["spent_musd"].as_u64().unwrap_or(0) } else { mean_inc };
            cp += u64::from(requests(o) > 0 && score(o) == 1000);
        }
        for o in &p.inc {
            pc += o["spent_musd"].as_u64().unwrap_or(0);
            pp += u64::from(score(o) == 1000);
        }
    }
    let cpp = exact::cpp_ratio_permille(cc, cp, pc, pp);
    let cand: Vec<&Value> = ok.iter().flat_map(|j| &pairs[*j].cand).collect();
    let caps = json!({"cand": cand.iter().filter(|o| o["caps_hit"].is_string()).count(),
        "cand_turns": cand.iter().map(|o| o["turns"].as_u64().unwrap_or(0)).sum::<u64>()});
    det["tasks"] = json!(ok.iter().zip(&k).map(|(j, k)| {
        let p = &pairs[*j];
        json!({"id": p.t.id, "family": p.t.prov["family"], "template": p.t.prov["template"], "k": k,
            "cand": p.cand.iter().map(score).collect::<Vec<_>>(), "inc": p.inc.iter().map(score).collect::<Vec<_>>(),
            "inc_cached": p.inc.iter().filter(|o| o["cached"] == true).count()})
    }).collect::<Vec<_>>());
    det["stats"] = json!({"n": n, "total": dec.total, "d_permille": dec.d_permille, "m_permille": dec.m_permille, "l_permille": dec.l_permille,
        "delta_permille": (delta * 1000.0).round() as i64, "p_sup_num": dec.p_sup.num.to_string(), "p_sup_log2_den": dec.p_sup.log2_den,
        "p_sup_permille_ceil": dec.p_sup.permille_ceil(), "cpp_ratio_permille": cpp, "cost": {"cand": cc, "cand_passes": cp, "inc": pc, "inc_passes": pp}, "caps": caps});
    SliceStats { status: early.unwrap_or(dec.status.as_str()).to_string(), ratchet_signal: early.is_none() && dec.ratchet_signal,
        d_permille: dec.d_permille, m_permille: dec.m_permille, cpp, n }
}

// -------------------------------------------------------------------------------- calibration and the probe

/// `init-root sealed calibrate [--repeats 6] [--usd X] [--real]` (m7-design.md §3.2.10, lean): R runs of the LKG on
/// every G4 task, band selection (in band = neither always passed nor always failed; with ≥ 24 in band the G4
/// partition keeps only those), the pseudo-A/A, δ_S for R = 2, 3, 5 into `sealed/noise.json`, and `sealed_calibrate`.
/// Selection runs enter the cache as `selectable:false`, so no G4 ever reuses them. Replacement draws and template
/// probes are not in the lean build.
pub fn calibrate(root: &Root, a: &Value) -> Result<Value, String> {
    let env = Env::of(root)?;
    real_ok(&env, a)?;
    let r = a["repeats"].as_u64().unwrap_or(6);
    if !(6..=10).contains(&r) {
        return Err("--repeats: 6–10".into());
    }
    let _op = run::begin(&env.rh);
    let mut s = pack::s_json(&env.rh).ok_or("no S v1 is drawn (init-root sealed draw)")?;
    let lkg = lock(&root.sup.ptrs).lkg.clone().ok_or("no lkg tuple")?;
    let inc = Arm::of_tuple(&env.rh, &lkg)?;
    let sp = Spend::new(budget(&env, a), 6 * 3600);
    let par_n = env.ch.sealed.parallel.max(1) as usize;
    let health = s["canary"]["health"].as_str().and_then(|id| task::load(&env.rh.join("sealed/S").join(id)).ok()).ok_or("no health canary")?;
    if sp.run(&env, &inc, &health, 1).is_none_or(|o| score(&o) < 1000) {
        return Err(sp.stopped().map_or("the LKG failed the health canary (harness_health): calibration stopped".into(), |w| format!("stopped: {w}")));
    }
    let tasks = pack::tasks(&env.rh, &s["g4"]);
    let jobs: Vec<(usize, u32)> = (1..=r as u32).flat_map(|rep| (0..tasks.len()).map(move |i| (i, rep))).collect();
    let got = par(&jobs, par_n, |(i, rep)| sp.run(&env, &inc, &tasks[*i], *rep));
    let mut runs: Vec<Vec<i64>> = vec![Vec::new(); tasks.len()];
    for ((i, _), o) in jobs.iter().zip(got) {
        if let Some(o) = o.filter(|o| requests(o) > 0 && !o["void"].is_string()) {
            cache_put(&env.rh, &cache_key(&env, &inc, &tasks[*i]), &o, false);
            runs[*i].push(score(&o));
        }
    }
    let full: Vec<usize> = (0..tasks.len()).filter(|i| runs[*i].len() == r as usize).collect();
    let band: Vec<usize> = full.iter().copied().filter(|i| exact::in_band(runs[*i].iter().filter(|x| **x == 1000).count() as u32, r as u32)).collect();
    let selected = band.len() >= pack::G4_MIN;
    let kept = if selected { &band } else { &full };
    if selected {
        s["out_of_band"] = json!((0..tasks.len()).filter(|i| !band.contains(i)).map(|i| &tasks[i].id).collect::<Vec<_>>());
        s["g4"] = json!(band.iter().map(|i| &tasks[*i].id).collect::<Vec<_>>());
        s["selected"] = json!(true);
        pack::save_s(&env.rh, &s)?;
    }
    let kr: Vec<Vec<i64>> = kept.iter().map(|i| runs[*i].clone()).collect();
    let sv = s["s_version"].as_str().unwrap_or("s1.0").to_string();
    let b = a["splits"].as_u64().map_or(exact::CALIBRATE_B, |b| b.clamp(10, 10_000) as usize);
    let c = exact::calibrate_b(&kr, &sv, env.ch.sealed.margin_max, b)?;
    let np = env.rh.join("sealed/noise.json");
    let mut nz: Value = fs::read_to_string(&np).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(json!({}));
    let pm = |x: f64| (x * 1000.0).round() as i64;
    nz["s1"] = json!({"at": rfc3339(now_ms()), "n": c.n_kept, "r2_permille": pm(c.delta_r2), "r3_permille": pm(c.delta_r3), "r5_permille": pm(c.delta_r5)});
    let _ = write_atomic(&np, format!("{}\n", icj(&nz).unwrap_or_default()).as_bytes(), 0o600);
    let mut data = c.json();
    data["s_version"] = json!(sv);
    data["selected"] = json!(selected);
    data["verdict"] = json!(format!("{:?}", c.verdict).to_lowercase());
    data["partial"] = json!(sp.stopped().is_some());
    data["spent_musd"] = json!(sp.spent());
    let rec = root.append("sealed_calibrate", data.clone()).ok_or("the lineage is unavailable")?;
    if c.verdict == exact::CalibVerdict::StopSpend {
        root.alert_from("sealed", "calibrate_stop", "[root] A/A calibration: false accepts above 70‰; stop all spend and debug at $0");
    }
    Ok(json!({"sealed_calibrate": data, "seq": rec["seq"], "fa_hi_permille": c.fa_hi_permille, "fr_hi_permille": c.fr_hi_permille,
        "m_standard_permille": c.m_standard_permille, "in_band": band.len(), "complete": full.len()}))
}

/// `init-root sealed probe [--per-family 2] [--usd X] [--real]` (= `calibrate --probe N`; m7-design.md §3.6 P): N
/// throw-away instances per family (E by language), R = 1 on the LKG, deleted by exact path afterwards; per-family
/// requests, turns, tokens (input, cached, output), cost and wall in `sealed_calibrate.probe`. S is not touched.
pub fn probe(root: &Root, a: &Value) -> Result<Value, String> {
    let env = Env::of(root)?;
    real_ok(&env, a)?;
    let per = a["per_family"].as_u64().unwrap_or(2).clamp(1, 5) as usize;
    let _op = run::begin(&env.rh);
    let s = pack::s_json(&env.rh);
    let pk = pack::installed(&env.rh, s.as_ref().and_then(|s| s["pack"].as_str()).unwrap_or(&env.ch.sealed.pack))?;
    let lkg = lock(&root.sup.ptrs).lkg.clone().ok_or("no lkg tuple")?;
    let inc = Arm::of_tuple(&env.rh, &lkg)?;
    let mut groups: std::collections::BTreeMap<String, Vec<pack::Tpl>> = Default::default();
    for t in pack::templates(&pk).into_iter().filter(|t| t.canary.is_none()) {
        let g = if t.fam == "E" { format!("E.{}", t.lang) } else { t.fam.clone() };
        groups.entry(g).or_default().push(t);
    }
    let mut rng = Rng(u64::from_be_bytes(util::random::<8>().map_err(|e| e.to_string())?));
    let mut picks: Vec<(String, pack::Tpl)> = Vec::new();
    for (g, mut ts) in groups {
        rng.shuffle(&mut ts);
        picks.extend(ts.into_iter().take(per).map(|t| (g.clone(), t)));
    }
    let sp = Spend::new(budget(&env, a), 2 * 3600);
    let got = par(&picks, env.ch.sealed.parallel.max(1) as usize, |(_, tpl)| {
        let (inst, t) = pack::throwaway(&env, &pk, tpl).map_err(|e| util::log(&env.rh, "warn", &format!("probe: {e}"))).ok()?;
        let o = sp.run(&env, &inc, &t, 1);
        drop(inst); // the instance's run root, by exact path
        o
    });
    let mut per_family: std::collections::BTreeMap<String, Value> = Default::default();
    for ((g, _), o) in picks.iter().zip(&got) {
        let e = per_family.entry(g.clone()).or_insert_with(|| json!({"runs": 0, "requests": 0, "turns": 0, "tokens_in": 0, "tokens_cached": 0,
            "tokens_out": 0, "cost_musd": 0, "wall_ms": 0, "passes": 0, "caps_hit": 0, "failed": 0}));
        let Some(o) = o else {
            e["failed"] = json!(e["failed"].as_u64().unwrap_or(0) + 1);
            continue;
        };
        let add = |e: &mut Value, k: &str, v: u64| e[k] = json!(e[k].as_u64().unwrap_or(0) + v);
        add(e, "runs", 1);
        for (k, src) in [("requests", "requests"), ("turns", "turns"), ("tokens_in", "input_tokens"), ("tokens_cached", "cached_tokens"),
            ("tokens_out", "output_tokens"), ("cost_musd", "spent_musd"), ("wall_ms", "wall_ms")] {
            add(e, k, o[src].as_u64().unwrap_or(0));
        }
        add(e, "passes", u64::from(score(o) == 1000));
        add(e, "caps_hit", u64::from(o["caps_hit"].is_string()));
    }
    let sv = s.map_or(Value::Null, |s| s["s_version"].clone());
    let data = json!({"s_version": sv, "probe": {"per_family": per_family, "repeats": 1, "per_group": per}, "partial": sp.stopped().is_some(),
        "spent_musd": sp.spent()});
    let rec = root.append("sealed_calibrate", data.clone()).ok_or("the lineage is unavailable")?;
    Ok(json!({"sealed_calibrate": data, "seq": rec["seq"]}))
}

// ------------------------------------------------------------------------------------------ the milestone

/// `init-root sealed milestone [--reason owner] [--repeats R] [--usd X] [--baseline T] [--current T] [--real]`
/// (m7-design.md §3.2.13 stage 2, lean): the current tuple against the confirmed one (the last confirmed
/// milestone's `current`, else genesis; the `confirmed` pointer is cut by D-L2), both fresh on every reserve task,
/// R each, interleaved, health on both arms and leak on the current first; the exact test both ways. A complete
/// check uses the reserve up; `partial` keeps it. A regression raises an alert (the rollback is the owner's).
pub fn milestone(root: &Root, a: &Value) -> Result<Value, String> {
    let env = Env::of(root)?;
    real_ok(&env, a)?;
    let _op = run::begin(&env.rh);
    let mut s = pack::s_json(&env.rh).ok_or("no S v1 is drawn (init-root sealed draw)")?;
    if s["reserve_used"] == true && a["force"] != true {
        return Err("the reserve was used by an earlier milestone (the lean build draws no fresh reserve)".into());
    }
    let reserve = pack::tasks(&env.rh, &s["reserve"]);
    if reserve.is_empty() {
        return Err("S v1 has no reserve".into());
    }
    let recs = crate::lineage::records(&env.rh).unwrap_or_default();
    let ptrs = lock(&root.sup.ptrs).clone();
    let last_conf = recs.iter().rev().find(|r| r["kind"] == "milestone" && r["data"]["outcome"] == "confirmed").and_then(|r| r["data"]["current"].as_str().map(String::from));
    let cur_id = a["current"].as_str().map(String::from).or(ptrs.current).ok_or("no current tuple")?;
    let conf_id = a["baseline"].as_str().map(String::from).or(last_conf).or(ptrs.genesis).ok_or("no confirmed or genesis tuple")?;
    if cur_id == conf_id {
        return Err(format!("the current tuple {cur_id} is the confirmed one: nothing to check"));
    }
    let (cur, conf) = (Arm::of_tuple(&env.rh, &cur_id)?, Arm::of_tuple(&env.rh, &conf_id)?);
    let r = a["repeats"].as_u64().unwrap_or(env.ch.sealed.milestone_repeats.max(1) as u64).clamp(1, 5) as u32;
    let sp = Spend::new(budget(&env, a), 4 * 3600);
    let par_n = env.ch.sealed.parallel.max(1) as usize;
    let id = format!("ms-{}", util::hex(&util::random::<4>().map_err(|e| e.to_string())?));
    let canary = |k: &str| s["canary"][k].as_str().and_then(|id| task::load(&env.rh.join("sealed/S").join(id)).ok());
    let (health, leak) = (canary("health").ok_or("no health canary")?, canary("leak").ok_or("no leak canary")?);
    let first = par(&[(&conf, &health), (&cur, &health), (&cur, &leak)], par_n, |(arm, t)| sp.run(&env, arm, t, 1));
    let harness = first[0].as_ref().is_none_or(|o| score(o) < 1000);
    let tripped = first[1].as_ref().is_some_and(|o| score(o) < 1000) || first[2].as_ref().is_some_and(|o| score(o) == 1000);
    let mut pairs: Vec<Pair> = reserve.iter().map(|t| Pair { t, key: String::new(), cand: Vec::new(), inc: Vec::new() }).collect();
    if !harness && !tripped {
        let jobs: Vec<(usize, bool, u32)> = (0..pairs.len()).flat_map(|i| (1..=r).flat_map(move |rep| [(i, true, rep), (i, false, rep)])).collect();
        let got = par(&jobs, par_n, |(i, c, rep)| sp.run(&env, if *c { &cur } else { &conf }, pairs[*i].t, *rep));
        for ((i, c, _), o) in jobs.iter().zip(got) {
            if let Some(o) = o {
                if *c { pairs[*i].cand.push(o) } else { pairs[*i].inc.push(o) }
            }
        }
    }
    let void = pairs.iter().any(|p| p.cand.iter().chain(&p.inc).any(|o| o["void"].is_string()));
    let partial = harness || void || sp.stopped().is_some();
    let (k, ok) = diffs(&pairs);
    let (outcome, d_pm) = if tripped {
        (Outcome::Regression, 0)
    } else if partial {
        (Outcome::Partial, 0)
    } else {
        let inc_runs: Vec<Vec<f64>> = ok.iter().map(|j| pairs[*j].inc.iter().map(|o| score(o) as f64 / 1000.0).collect()).collect();
        let delta = noise(&inc_runs, &id).or(s1_delta(&env.rh, r).map(|d| d.0)).unwrap_or(0.0);
        let m = exact::margin(delta, k.len().max(1));
        let ms = exact::milestone(&k, r, 1000, lower(&exact::d_ratios(&k, r, 1000), &id, "ni"), m, env.ch.sealed.margin_max);
        (ms.outcome, ms.d_permille)
    };
    let (gains, k_now) = ratchet(&env.rh, env.ch.sealed.ratchet_k);
    let k_next = exact::ratchet_k_next(outcome, k_now as u32, env.ch.sealed.ratchet_k.max(1) as u32);
    let data = json!({"current": cur_id, "confirmed_before": conf_id, "reason": a["reason"].as_str().unwrap_or("owner"), "n": k.len(),
        "repeats": r, "held_out": true, "outcome": outcome.as_str(), "d_permille": exact::round_to(d_pm, 50), "lineage_gains": gains,
        "ratchet_k_next": k_next, "spent_musd": sp.spent()});
    let rec = root.append("milestone", data.clone()).ok_or("the lineage is unavailable")?;
    if outcome != Outcome::Partial {
        s["reserve_used"] = json!(true);
        pack::save_s(&env.rh, &s)?;
    }
    if outcome == Outcome::Regression {
        root.alert_from("sealed", "milestone_regression", &format!("[root] milestone check: {cur_id} regressed against {conf_id} on the reserve"));
    }
    let detail = json!({"id": id, "record": data, "canaries": first.iter().map(|o| o.as_ref().map(score)).collect::<Vec<_>>(),
        "k": k, "stopped": sp.stopped(), "void": void});
    let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(env.rh.join("sealed/g4"));
    let _ = write_atomic(&env.rh.join("sealed/g4").join(format!("{id}.json")), format!("{}\n", icj(&detail).unwrap_or_default()).as_bytes(), 0o600);
    Ok(json!({"milestone": data, "seq": rec["seq"], "detail": id}))
}

/// `init-root sealed show [--g4 <gate_id>|--partition|--templates]`: owner-only numbers, never task content.
pub fn show(rh: &Path, a: &Value) -> Result<Value, String> {
    if let Some(g) = a["g4"].as_str() {
        if !g.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) {
            return Err("--g4: a gate id".into());
        }
        let p = rh.join("sealed/g4").join(format!("{g}.json"));
        return fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str(&s).ok()).ok_or(format!("no slice detail for {g}"));
    }
    let s = pack::s_json(rh).ok_or("no S v1 is drawn")?;
    let fam = |list: &Value| {
        let mut m = std::collections::BTreeMap::<String, u64>::new();
        for t in pack::tasks(rh, list) {
            *m.entry(t.prov["family"].as_str().unwrap_or("?").to_string()).or_default() += 1;
        }
        m
    };
    let mut v = json!({"pack": s["pack"], "pack_version": s["pack_version"], "s_version": s["s_version"], "drawn_at": s["drawn_at"],
        "selected": s["selected"], "reserve_used": s["reserve_used"], "g4": fam(&s["g4"]), "reserve": fam(&s["reserve"]),
        "out_of_band": fam(&s["out_of_band"]), "canaries": s["canary"].as_object().map_or(0, |o| o.len()), "noise": s1_noise(rh)});
    if a["templates"] == true {
        let all = task::all(rh);
        let row = |t: &Task| json!({"id": t.id, "template": t.prov["template"], "family": t.prov["family"], "partition": t.prov["partition"]});
        v["instances"] = json!(all.iter().filter(|t| t.prov["v"] == 2).map(row).collect::<Vec<_>>());
    }
    Ok(v)
}
