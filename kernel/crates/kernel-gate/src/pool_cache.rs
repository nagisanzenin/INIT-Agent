//! The pool cache: incumbent runs on the visible pool, reused across gates (m7-design.md §3.2.7;
//! kernel/gate.md §4.4 "RunRecord v3").
//!
//! Layout: `vault/gate/cache/<tuple12>/<key64>.jsonl`, one [`CacheEntry`] per line, where `tuple12` is
//! the incumbent arm's id ([`ArmKey::tuple_id`]) and `key64` is
//! `sha256(ICJ{task, arm, model, harness})` ([`entry_key`]). The served model string is part of the
//! key, so a reusing gate looks entries up with the served string its own fresh runs observed.
//!
//! An entry is valid when it is at most `max_age_s` old, made ≥ 1 request, is `selectable`, and its
//! run is an incumbent (`p`) run: the cache stores only incumbent-arm runs, so a winning candidate's
//! runs never become incumbent data (winner's curse). The incumbent arm reuses exactly R runs per task
//! (a seeded subset when more are cached; fewer than R → fresh). A sentinel ([`sentinel_drift`])
//! re-runs ⌈20% · n⌉ seeded tasks fresh and invalidates the tuple on drift. Only the `lkg` and
//! `audited` tuples are kept ([`PoolCache::prune`], exact paths).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::caps::RunCaps;
use crate::g4::RunRecord;
use crate::stats::rng::SplitMix64;

/// The cache entry format version.
pub const CACHE_V: u32 = 1;
/// `gate.g4.cache_max_age_s` default (72 h).
pub const DEFAULT_MAX_AGE_S: u64 = 259_200;
/// `gate.g4.sentinel_permille` default.
pub const DEFAULT_SENTINEL_PERMILLE: u64 = 200;
/// The harness version of this gate's screen runner: part of every key, bumped when the way a pool
/// run is executed or scored changes.
pub const HARNESS_V: &str = "gate-v2b.1";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// The arm part of the key: the body binary, the ext set and the T0 text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmKey {
    /// sha256 of the arm's `init-body` binary.
    pub body_sha256: String,
    /// The ext set id (`""` when the arm has none).
    pub ext_set: String,
    /// sha256 of the arm's T0 (GENESIS.md, SELF.md, skills/**). The kernel may pass the lineage commit
    /// sha instead: a strictly finer key, which only costs reuse.
    pub t0_sha256: String,
}

impl ArmKey {
    /// The tuple id: the first 12 hex of `sha256(ICJ(arm))`.
    pub fn tuple_id(&self) -> String {
        let v = json!({"body": self.body_sha256, "ext": self.ext_set, "t0": self.t0_sha256});
        sha256_hex(&serde_json::to_vec(&v).unwrap_or_default())[..12].to_string()
    }
}

/// The model part of the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelKey {
    /// The routed id, `provider/model`.
    pub routed: String,
    /// The served model string the responses named (whatever the provider returned).
    pub served: String,
    /// The routing snapshot (level and routable set), as one string.
    pub routing: String,
}

/// The harness part of the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessKey {
    /// The gate's runner version ([`HARNESS_V`] unless the caller pins another).
    pub runner: String,
    pub caps: RunCaps,
    /// The model effort setting (DeepSeek `high`).
    pub effort: String,
    pub reserve_output_tokens: u64,
}

/// The lookup context of one gate: everything in the key except the task and the served model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheQuery {
    pub arm: ArmKey,
    pub routed: String,
    pub routing: String,
    pub runner: String,
    pub effort: String,
    pub reserve_output_tokens: u64,
}

impl CacheQuery {
    pub fn model(&self, served: &str) -> ModelKey {
        ModelKey { routed: self.routed.clone(), served: served.into(), routing: self.routing.clone() }
    }
    pub fn harness(&self, caps: RunCaps) -> HarnessKey {
        HarnessKey { runner: self.runner.clone(), caps, effort: self.effort.clone(), reserve_output_tokens: self.reserve_output_tokens }
    }
}

/// `sha256(ICJ{task, arm, model, harness})` (§3.2.7), 64 hex.
pub fn entry_key(task_digest: &str, arm: &ArmKey, model: &ModelKey, harness: &HarnessKey) -> String {
    let v = json!({
        "task": task_digest,
        "arm": {"body": arm.body_sha256, "ext": arm.ext_set, "t0": arm.t0_sha256},
        "model": {"routed": model.routed, "served": model.served, "routing": model.routing},
        "harness": {"runner": harness.runner, "caps": {"max_turns": harness.caps.max_turns,
                    "max_output_tokens": harness.caps.max_output_tokens, "max_input_tokens": harness.caps.max_input_tokens},
                    "effort": harness.effort, "reserve_output_tokens": harness.reserve_output_tokens},
    });
    sha256_hex(&serde_json::to_vec(&v).unwrap_or_default())
}

fn tree_lines(root: &Path, rel: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut names: Vec<_> = std::fs::read_dir(root.join(rel))?.flatten().map(|e| e.file_name()).collect();
    names.sort();
    for n in names {
        let r = rel.join(&n);
        let p = root.join(&r);
        let m = std::fs::symlink_metadata(&p)?;
        if m.is_dir() {
            tree_lines(root, &r, out)?;
        } else if m.file_type().is_symlink() {
            out.push(format!("l {} {}", r.display(), std::fs::read_link(&p)?.display()));
        } else {
            out.push(format!("f {} {}", r.display(), sha256_hex(&std::fs::read(&p)?)));
        }
    }
    Ok(())
}

/// The task digest: `sha256` over task.json's bytes, the workspace digest (from task.json) and the
/// check directory's tree digest. A changed prompt, workspace or check gives a new key.
pub fn task_digest(task_dir: &Path) -> std::io::Result<String> {
    let mut lines = vec![format!("task.json {}", sha256_hex(&std::fs::read(task_dir.join("task.json"))?))];
    if task_dir.join("check").is_dir() {
        tree_lines(&task_dir.join("check"), Path::new(""), &mut lines)?;
    }
    Ok(sha256_hex(lines.join("\n").as_bytes()))
}

/// One cached incumbent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub v: u32,
    pub key: String,
    pub task: String,
    /// Unix seconds when the run finished.
    pub at_unix: i64,
    /// `false` for calibration selection runs (m7-design.md §3.1.6): never reused.
    pub selectable: bool,
    /// The gate that made the run.
    pub gate_id: String,
    /// The RunRecord, as written to the job's `runs.jsonl`.
    pub run: RunRecord,
}

impl CacheEntry {
    /// §3.2.7's validity rules (the key already matched).
    pub fn valid(&self, now_unix: i64, max_age_s: u64) -> bool {
        let age = now_unix.saturating_sub(self.at_unix);
        age >= 0 && (age as u64) <= max_age_s && self.run.requests >= 1 && self.selectable && self.run.arm == "p" && self.run.outcome != "infra"
    }
    /// Pass = a full score.
    pub fn passed(&self) -> bool {
        self.run.score_permille >= 1000
    }
}

/// `vault/gate/cache/`.
#[derive(Debug, Clone)]
pub struct PoolCache {
    root: PathBuf,
}

fn valid_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl PoolCache {
    /// The cache under `$INIT_HOME/vault/gate/cache/` (created on the first put).
    pub fn open(home: &Path) -> PoolCache {
        PoolCache { root: home.join("vault/gate/cache") }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file(&self, tuple: &str, key: &str) -> PathBuf {
        self.root.join(tuple).join(format!("{key}.jsonl"))
    }

    /// Append one incumbent run. Refused (`Ok(false)`) for a candidate (`c`) run, a run with no
    /// request, or an `infra` run: none of them may ever serve as incumbent data.
    pub fn put(&self, arm: &ArmKey, key: &str, task: &str, at_unix: i64, selectable: bool, run: &RunRecord) -> std::io::Result<bool> {
        if run.arm != "p" || run.requests == 0 || run.outcome == "infra" || !valid_hex(key, 64) {
            return Ok(false);
        }
        let e = CacheEntry { v: CACHE_V, key: key.into(), task: task.into(), at_unix, selectable, gate_id: run.gate_id.clone(), run: run.clone() };
        let tuple = arm.tuple_id();
        std::fs::create_dir_all(self.root.join(&tuple))?;
        let mut line = serde_json::to_vec(&e).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.file(&tuple, key))?;
        f.write_all(&line)?;
        Ok(true)
    }

    /// Every parseable entry under one key (valid or not), oldest first.
    pub fn entries(&self, arm: &ArmKey, key: &str) -> Vec<CacheEntry> {
        if !valid_hex(key, 64) {
            return Vec::new();
        }
        let Ok(text) = std::fs::read_to_string(self.file(&arm.tuple_id(), key)) else { return Vec::new() };
        text.lines().filter_map(|l| serde_json::from_str::<CacheEntry>(l).ok()).filter(|e| e.key == key).collect()
    }

    /// The valid entries under one key.
    pub fn valid(&self, arm: &ArmKey, key: &str, now_unix: i64, max_age_s: u64) -> Vec<CacheEntry> {
        self.entries(arm, key).into_iter().filter(|e| e.valid(now_unix, max_age_s)).collect()
    }

    /// Exactly `r` valid entries for one task, a seeded subset when more are cached (equal R, §3.2.7);
    /// `None` when fewer than `r` are valid (the incumbent then runs fresh).
    pub fn lookup(&self, arm: &ArmKey, key: &str, now_unix: i64, max_age_s: u64, r: usize, seed: (&str, &str)) -> Option<Vec<CacheEntry>> {
        let mut v = self.valid(arm, key, now_unix, max_age_s);
        if r == 0 || v.len() < r {
            return None;
        }
        let mut rng = SplitMix64::for_gate(seed.0, &format!("cache/{}/{key}", seed.1));
        rng.shuffle(&mut v);
        v.truncate(r);
        Some(v)
    }

    /// Invalidate one tuple's cache (sentinel drift): its directory is removed by exact path.
    pub fn invalidate(&self, arm: &ArmKey) -> std::io::Result<()> {
        let d = self.root.join(arm.tuple_id());
        match std::fs::remove_dir_all(&d) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Keep only the given tuples (the `lkg` and `audited` arms); every other tuple directory is
    /// removed by its exact path. Returns the removed tuple ids.
    pub fn prune(&self, keep: &[&ArmKey]) -> std::io::Result<Vec<String>> {
        let keep: Vec<String> = keep.iter().map(|a| a.tuple_id()).collect();
        let mut removed = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else { return Ok(removed) };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if valid_hex(&name, 12) && !keep.contains(&name) && e.file_type().is_ok_and(|t| t.is_dir()) {
                std::fs::remove_dir_all(self.root.join(&name))?;
                removed.push(name);
            }
        }
        removed.sort();
        Ok(removed)
    }

    /// The tasks one tuple passed in a valid cached run (any key): E3's control candidates.
    pub fn passed_tasks(&self, arm: &ArmKey, now_unix: i64, max_age_s: u64) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let Ok(rd) = std::fs::read_dir(self.root.join(arm.tuple_id())) else { return out };
        for f in rd.flatten() {
            if let Ok(text) = std::fs::read_to_string(f.path()) {
                for e in text.lines().filter_map(|l| serde_json::from_str::<CacheEntry>(l).ok()) {
                    if e.valid(now_unix, max_age_s) && e.passed() {
                        out.insert(e.task);
                    }
                }
            }
        }
        out
    }

    /// Every entry in the cache (read-only; the miner computes its failure statistics from these).
    pub fn all_runs(&self) -> Vec<CacheEntry> {
        let mut out = Vec::new();
        let Ok(tuples) = std::fs::read_dir(&self.root) else { return out };
        let mut dirs: Vec<PathBuf> = tuples.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            files.sort();
            for f in files {
                if let Ok(text) = std::fs::read_to_string(&f) {
                    out.extend(text.lines().filter_map(|l| serde_json::from_str::<CacheEntry>(l).ok()));
                }
            }
        }
        out
    }
}

/// ⌈`permille` · n / 1000⌉ sentinel tasks (at least 1 when n > 0).
pub fn sentinel_count(n: usize, permille: u64) -> usize {
    if n == 0 {
        return 0;
    }
    ((n as u64 * permille).div_ceil(1000) as usize).clamp(1, n)
}

/// The sentinel's drift test (§3.2.7): the fresh pass count against Σ p̂ of the cached runs on the
/// same tasks, with `p̂` each task's cached pass rate. Drift when
/// `|passes − Σ p̂| > 2.5 · √(max(Σ p̂(1 − p̂), 1))`. `p_hat` is in permille per task.
pub fn sentinel_drift(fresh_passes: u64, p_hat_permille: &[u64]) -> bool {
    let sum: f64 = p_hat_permille.iter().map(|p| *p as f64 / 1000.0).sum();
    let var: f64 = p_hat_permille.iter().map(|p| {
        let q = *p as f64 / 1000.0;
        q * (1.0 - q)
    }).sum();
    (fresh_passes as f64 - sum).abs() > 2.5 * var.max(1.0).sqrt()
}

/// A task's cached pass rate in permille (all valid entries).
pub fn p_hat_permille(entries: &[CacheEntry]) -> u64 {
    if entries.is_empty() {
        return 0;
    }
    entries.iter().filter(|e| e.passed()).count() as u64 * 1000 / entries.len() as u64
}

/// The most frequent non-empty served model string among runs (ties: the smallest string).
pub fn served_model<'a>(runs: impl IntoIterator<Item = &'a RunRecord>) -> Option<String> {
    let mut n: BTreeMap<&str, usize> = BTreeMap::new();
    for r in runs {
        if let Some(m) = r.model_served.as_deref().filter(|m| !m.is_empty()) {
            *n.entry(m).or_default() += 1;
        }
    }
    n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).map(|(m, _)| m.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{remove_home, test_home};

    fn arm(b: &str) -> ArmKey {
        ArmKey { body_sha256: b.repeat(64)[..64].into(), ext_set: String::new(), t0_sha256: "a".repeat(40) }
    }
    fn q(a: &ArmKey) -> CacheQuery {
        CacheQuery { arm: a.clone(), routed: "deepseek/deepseek-flash".into(), routing: "L0".into(), runner: HARNESS_V.into(), effort: "high".into(), reserve_output_tokens: 65_536 }
    }
    fn run(task: &str, k: u32, score: u32, served: &str) -> RunRecord {
        RunRecord { v: 3, gate_id: "j-000010".into(), run_id: format!("p-{task}-r{k}"), arm: "p".into(), task: task.into(), set: "pool".into(),
                    repeat: k, outcome: "done".into(), score_permille: score, requests: 4, model_served: Some(served.into()), ..Default::default() }
    }

    /// M7-G08 (= M7-R08 on the pool cache): a served-model change, a harness change, age > 72 h,
    /// `selectable:false`, a winning (candidate) arm and a run with no request each force a fresh
    /// incumbent run; with more cached runs than R the incumbent gets a seeded subset of exactly R.
    #[test]
    fn m7_g08_pool_cache_rules() {
        let home = test_home("m7-g08");
        let c = PoolCache::open(&home);
        let a = arm("1");
        let qa = q(&a);
        let caps = RunCaps::default();
        let td = "d".repeat(64);
        let key = entry_key(&td, &a, &qa.model("deepseek-flash-1017"), &qa.harness(caps));
        let now = 1_800_000_000i64;
        for k in 1..=3 {
            assert!(c.put(&a, &key, "t1", now - 3600, true, &run("t1", k, if k == 2 { 0 } else { 1000 }, "deepseek-flash-1017")).unwrap());
        }
        // Reuse within 72 h: exactly R, seeded, deterministic.
        let one = c.lookup(&a, &key, now, DEFAULT_MAX_AGE_S, 2, ("j-000020", "t1")).unwrap();
        assert_eq!(one.len(), 2);
        assert_eq!(one, c.lookup(&a, &key, now, DEFAULT_MAX_AGE_S, 2, ("j-000020", "t1")).unwrap());
        assert!(c.lookup(&a, &key, now, DEFAULT_MAX_AGE_S, 4, ("j-000020", "t1")).is_none(), "fewer than R → fresh");
        assert_eq!(p_hat_permille(&c.valid(&a, &key, now, DEFAULT_MAX_AGE_S)), 666);
        assert_eq!(c.passed_tasks(&a, now, DEFAULT_MAX_AGE_S).into_iter().collect::<Vec<_>>(), ["t1"]);
        assert!(c.passed_tasks(&arm("9"), now, DEFAULT_MAX_AGE_S).is_empty());
        // The served model is in the key: a new served string finds nothing.
        let key2 = entry_key(&td, &a, &qa.model("deepseek-flash-1101"), &qa.harness(caps));
        assert_ne!(key, key2);
        assert!(c.lookup(&a, &key2, now, DEFAULT_MAX_AGE_S, 1, ("g", "t1")).is_none());
        // A harness change (caps, effort, runner, reservation) changes the key.
        let mut h = qa.harness(caps);
        h.caps.max_turns = 8;
        assert_ne!(entry_key(&td, &a, &qa.model("deepseek-flash-1017"), &h), key);
        h = qa.harness(caps);
        h.effort = "max".into();
        assert_ne!(entry_key(&td, &a, &qa.model("deepseek-flash-1017"), &h), key);
        // A different task digest or arm changes the key and the tuple.
        assert_ne!(entry_key(&"e".repeat(64), &a, &qa.model("deepseek-flash-1017"), &qa.harness(caps)), key);
        assert_ne!(arm("2").tuple_id(), a.tuple_id());
        // Age > 72 h.
        assert!(c.lookup(&a, &key, now + DEFAULT_MAX_AGE_S as i64 + 3600, DEFAULT_MAX_AGE_S, 1, ("g", "t1")).is_none());
        // selectable:false, a candidate's run (the winning arm), a run with no request, an infra run.
        let k3 = entry_key(&"f".repeat(64), &a, &qa.model("m"), &qa.harness(caps));
        assert!(c.put(&a, &k3, "t3", now, false, &run("t3", 1, 1000, "m")).unwrap());
        let mut cand = run("t3", 2, 1000, "m");
        cand.arm = "c".into();
        assert!(!c.put(&a, &k3, "t3", now, true, &cand).unwrap(), "candidate runs never enter the cache");
        let mut none = run("t3", 3, 1000, "m");
        none.requests = 0;
        assert!(!c.put(&a, &k3, "t3", now, true, &none).unwrap());
        let mut infra = run("t3", 4, 1000, "m");
        infra.outcome = "infra".into();
        assert!(!c.put(&a, &k3, "t3", now, true, &infra).unwrap());
        assert!(c.lookup(&a, &k3, now, DEFAULT_MAX_AGE_S, 1, ("g", "t3")).is_none());
        assert_eq!(c.entries(&a, &k3).len(), 1);
        // all_runs sees everything; invalidate and prune remove by exact tuple path.
        assert_eq!(c.all_runs().len(), 4);
        let b = arm("2");
        let kb = entry_key(&td, &b, &q(&b).model("m"), &q(&b).harness(caps));
        assert!(c.put(&b, &kb, "t1", now, true, &run("t1", 1, 1000, "m")).unwrap());
        assert_eq!(c.prune(&[&a]).unwrap(), vec![b.tuple_id()]);
        assert!(!c.root().join(b.tuple_id()).exists() && c.root().join(a.tuple_id()).exists());
        c.invalidate(&a).unwrap();
        assert!(c.all_runs().is_empty());
        c.invalidate(&a).unwrap();
        remove_home(&home);
    }

    /// M7-R09's rule on the pool cache: a 40-point drift on 7 sentinel tasks trips; a small one does not.
    #[test]
    fn sentinel_rule() {
        assert_eq!(sentinel_count(32, 200), 7);
        assert_eq!(sentinel_count(12, 200), 3);
        assert_eq!(sentinel_count(1, 200), 1);
        assert_eq!(sentinel_count(0, 200), 0);
        // Σ p̂ = 7 × 0.8 = 5.6, var = 7 × 0.16 = 1.12 → bound 2.5 × 1.058 = 2.65.
        let p = [800u64; 7];
        assert!(!sentinel_drift(5, &p));
        assert!(!sentinel_drift(3, &p));
        assert!(sentinel_drift(2, &p), "a 40-point drift (0.8 → 0.4) trips");
        // Floor at 1: all-pass cache, 1 fresh fail of 3 is within 2.5.
        assert!(!sentinel_drift(2, &[1000, 1000, 1000]));
        assert!(sentinel_drift(0, &[1000, 1000, 1000]));
    }

    #[test]
    fn served_model_mode() {
        let rs = [run("a", 1, 0, "x"), run("b", 1, 0, "y"), run("c", 1, 0, "y"), run("d", 1, 0, "")];
        assert_eq!(served_model(&rs).as_deref(), Some("y"));
        assert_eq!(served_model(&rs[..2]).as_deref(), Some("x"));
        assert_eq!(served_model(&rs[3..]), None);
    }

    #[test]
    fn task_digest_covers_task_and_check() {
        let home = test_home("task-digest");
        let t = home.join("t1");
        std::fs::create_dir_all(t.join("check")).unwrap();
        std::fs::write(t.join("task.json"), b"{\"v\":3}").unwrap();
        std::fs::write(t.join("check/run"), b"#!/bin/sh\nexit 0\n").unwrap();
        let d1 = task_digest(&t).unwrap();
        std::fs::write(t.join("check/run"), b"#!/bin/sh\nexit 1\n").unwrap();
        let d2 = task_digest(&t).unwrap();
        std::fs::write(t.join("task.json"), b"{\"v\":3,\"x\":1}").unwrap();
        let d3 = task_digest(&t).unwrap();
        assert!(d1 != d2 && d2 != d3 && valid_hex(&d1, 64));
        remove_home(&home);
    }
}
