//! The gate record of one job (gate.md §3.13): built stage by stage, then written to
//! `vault/gate/jobs/<job>/result.json`, `vault/lineage.jsonl` (full), Root lineage (the agent-safe
//! subset, waited for: "lineage before LKG"), `state/kernel/gate/<job>/report.json` (redacted for
//! the agent), and `refs/notes/gate` on the candidate (redacted, after the Root's ack).

use super::Env;
use kernel_gate::record as rec;
use serde_json::{json, Value};
use std::path::PathBuf;

/// One gate's record under construction.
#[derive(Clone, Debug)]
pub struct Record {
    pub v: Value,
    pub failure_text: Option<String>,
}

impl Record {
    #[allow(clippy::too_many_arguments)]
    pub fn new(env: &Env, gate_id: &str, gate_kind: &str, scope: Option<&str>, sha: &str, branch: Option<&str>, validated: Option<&kernel_gate::evo::Validated>, running: &str, lkg: &str) -> Record {
        let v = json!({
            "kind": "gate", "v": rec::RECORD_V, "level": env.level_str(),
            "gate_id": gate_id, "gate_kind": gate_kind, "scope": scope,
            "candidate": {"sha": sha, "branch": branch,
                          "change_type": validated.map(|v| v.change_type.as_str()),
                          "capability_labels": validated.map(|v| v.capability_labels.clone()).unwrap_or_default()},
            "parent": {"running": running, "lkg": lkg},
            "model": {"id": env.model, "provider": env.cfg.model.provider},
            "kernel": {"build_sha256": env.kernel.build_sha256, "charter_sha256": env.kernel.charter_sha256,
                       "config_sha256": env.kernel.config_sha256, "version": env.kernel.version},
            "started": crate::clock::now_rfc3339(), "finished": Value::Null,
            "stages": {}, "verdict": Value::Null, "reason": Value::Null, "spent_musd": 0, "escalated": false,
        });
        Record { v, failure_text: None }
    }

    /// ADR-011: a free-path record is v5 with `accept:"free"`.
    pub fn free(&mut self) {
        self.v["v"] = json!(rec::RECORD_V_FREE);
        self.v["accept"] = json!("free");
    }

    /// The A-stages of a v5 record from its G-stages and the Root's answer: A1 = G0 and G1, A2 = G2
    /// (kernel-run); `root_passed` names the Root's checks that passed (`A0`, `A5`, and A4 for
    /// `t2`/`dep`); A3 is asynchronous after going live (`t2`, `dep`, and the body's tests).
    pub fn free_stages(&mut self, root_passed: &[&str]) {
        let st = self.v["stages"].clone();
        let scope = self.v["scope"].as_str().unwrap_or("").to_string();
        let run = |k: &str| st.get(k).filter(|x| !x.is_null());
        let pass = |k: &str| run(k).map(|x| x["pass"] == true);
        let a1 = match (pass("G0"), pass("G1")) {
            (None, None) => json!({"skip": true}),
            (g0, g1) => json!({"pass": g0 == Some(true) && g1 == Some(true), "stages": ["G0", "G1"]}),
        };
        let a2 = match pass("G2") {
            None => json!({"skip": true}),
            Some(p) => json!({"pass": p, "stages": ["G2"]}),
        };
        let heavy = matches!(scope.as_str(), "t2" | "dep");
        let root = |k: &str| if root_passed.contains(&k) { json!({"pass": true, "by": "root"}) } else { Value::Null };
        self.v["stages"]["A0"] = root("A0");
        self.v["stages"]["A1"] = a1;
        self.v["stages"]["A2"] = a2;
        self.v["stages"]["A3"] = if heavy && !root_passed.is_empty() { json!({"status": "async"}) } else { json!({"skip": true}) };
        // The Root runs A4 for t2 and dep before it answers, so an accepted change passed it.
        self.v["stages"]["A4"] = if heavy { root("A0") } else { json!({"skip": true}) };
        self.v["stages"]["A5"] = root("A5");
    }

    pub fn stage(&mut self, name: &str, r: Value) {
        self.v["stages"][name] = r;
    }

    /// Set the verdict (`accept|reject|inconclusive|error`) and its reason.
    pub fn verdict(&mut self, verdict: &str, reason: &str) {
        self.v["verdict"] = json!(verdict);
        self.v["reason"] = json!(reason);
        self.v["finished"] = json!(crate::clock::now_rfc3339());
        let spent: u64 = self.v["stages"].as_object().map(|m| m.values().filter_map(|s| s["spent_musd"].as_u64()).sum()).unwrap_or(0);
        self.v["spent_musd"] = json!(spent);
    }

    pub fn sha(&self) -> String {
        self.v["candidate"]["sha"].as_str().unwrap_or("").to_string()
    }

    fn gate_id(&self) -> String {
        self.v["gate_id"].as_str().unwrap_or("").to_string()
    }

    /// The agent report (`state/kernel/gate/<job>/report.json`).
    pub fn write_report(&self, env: &Env) -> PathBuf {
        let dir = env.home.join("state/kernel/gate").join(self.gate_id());
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("report.json");
        let _ = crate::fsx::write_json(&p, &rec::report(&self.v, self.failure_text.as_deref()));
        p
    }

    /// The full record locally (`result.json`, `vault/lineage.jsonl`), then the subset to Root
    /// lineage. `Ok(seq)` once the Root acknowledged; then the redacted note. On `Err` the record
    /// stays in `vault/gate/jobs/<job>/` for the resubmission (`gate.lineage.unavailable`).
    pub fn publish(&mut self, env: &Env, note_on: Option<&str>) -> Result<u64, String> {
        let dir = env.home.join("vault/gate/jobs").join(self.gate_id());
        let _ = std::fs::create_dir_all(&dir);
        self.write_report(env);
        env.lineage.local("gate", self.v.clone());
        let subset = rec::root_subset(&self.v);
        let r = env.lineage.append_wait("gate", subset);
        let mut full = self.v.clone();
        full["root_ack"] = match &r {
            Ok(seq) => json!({"seq": seq}),
            Err(e) => json!({"error": e}),
        };
        let _ = crate::fsx::write_json(&dir.join("result.json"), &full);
        if let Ok(seq) = &r {
            self.v["root_seq"] = json!(seq);
            if let Some(sha) = note_on {
                self.note(env, sha);
            }
        }
        r
    }

    /// Append the redacted record to `refs/notes/gate` on `sha` (gate.md §3.13 "Notes").
    pub fn note(&self, env: &Env, sha: &str) {
        let git = env.git();
        let mk = |args: &[&str]| git.command(args);
        let tmp = env.home.join("vault/gate/jobs").join(self.gate_id()).join("note.tmp");
        if let Err(e) = rec::append_note(&mk, sha, &rec::redact_for_note(&self.v), &tmp) {
            crate::klog::log("gate", &format!("refs/notes/gate on {sha}: {e}"));
        }
    }

    /// The stage notice to the body.
    pub fn notice(&self, env: &Env, stage: &str, pass: Option<bool>, reason: Option<&str>, score: Option<i64>) {
        let st = &self.v["stages"][stage];
        let data = rec::stage_notice(&self.gate_id(), &self.sha(), stage, pass, reason, score, st["spent_musd"].as_u64().unwrap_or(0), st["wall_ms"].as_u64().unwrap_or(0), st["cache_permille"].as_u64());
        // TUI-OBSERVE: the owner's copy also says which gate and scope (the body's stays as it was).
        env.sup.notice_owner("gate", data, self.owner_extra(stage));
    }

    /// The owner-only fields of a stage notice: the gate's kind and scope, and (v0.5.2, R-TUI) for G0
    /// the time before its tests that is G0's too: `build_ms` (export, dependencies, body build) and
    /// the dependency part, `deps_ms` with `deps_mode` (`hit`, `stub` built now, `cold`). G0's `wall`
    /// is its tests and clippy only, so without these a cold dependency build read as "waited".
    pub fn owner_extra(&self, stage: &str) -> Value {
        let st = &self.v["stages"][stage];
        let mut x = json!({"gate_kind": self.v["gate_kind"], "scope": self.v["scope"]});
        if let Some(b) = st["build_ms"].as_u64() {
            x["build_ms"] = json!(b);
        }
        if let Some(m) = st["deps"]["ms"].as_u64() {
            x["deps_ms"] = json!(m);
        }
        if let Some(m) = st["deps"]["mode"].as_str() {
            x["deps_mode"] = json!(m);
        }
        x
    }
}

#[cfg(test)]
mod owner_extra_tests {
    use super::*;

    /// v0.5.2 (R-TUI, user session v0.5.1-new: G0 "pass · 2 min 32 s · waited 17 min 46 s" while the job
    /// record said G0 took 20 min 19 s): the owner's G0 notice carries the build and dependency times.
    #[test]
    fn g0_owner_notice_carries_the_build_and_dependency_times() {
        let mut r = Record { v: json!({"gate_kind": "promote", "scope": "t1", "stages": {}}), failure_text: None };
        r.stage("G0", json!({"pass": true, "wall_ms": 152_648, "build_ms": 1_066_000, "cold_target": true,
            "deps": {"mode": "stub", "key": "k", "ms": 1_036_000, "build_ms": 30_000}}));
        r.stage("G1", json!({"pass": true, "wall_ms": 2_565}));
        assert_eq!(r.owner_extra("G0"), json!({"gate_kind": "promote", "scope": "t1", "build_ms": 1_066_000, "deps_ms": 1_036_000, "deps_mode": "stub"}));
        assert_eq!(r.owner_extra("G1"), json!({"gate_kind": "promote", "scope": "t1"}), "only G0 has a build");
        // through the owner's view: the stage's own fields win, the extra ones are added
        let d = kernel_gate::record::stage_notice("j-1", "9aa7d39624ce", "G0", Some(true), None, None, 0, 152_648, None);
        let (_, _, data) = crate::supervisor::owner_notice_view("gate", &d, &r.owner_extra("G0"));
        assert_eq!((data["wall_ms"].clone(), data["build_ms"].clone(), data["deps_ms"].clone(), data["deps_mode"].clone()),
            (json!(152_648), json!(1_066_000), json!(1_036_000), json!("stub")));
        assert!(init_proto::icj::to_line(&data).is_ok(), "ICJ: integers only");
    }
}
