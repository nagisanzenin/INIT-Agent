//! `init-kernel gate judge` (gate.md §4.1a): a full gate on an isolated copy of a system, for the
//! Root's gate-of-gates (root/sealed-eval.md §3.3) and K2.
//!
//! ```text
//! init-kernel [-c <key>=<value>]… gate judge --home <INIT_HOME copy> --candidate <tree> --running <tree>
//!             --lkg <tree> [--stages G0,G1,G2,G3,G4] [--level L0..L4] --out <file>
//! ```
//!
//! - Writes `{"verdict","reason","scope","stages":[…],"record":{…}}` (sorted keys, compact) to `--out`;
//!   exit 0 accept, 1 reject, 2 inconclusive, 3 error, 64 usage.
//! - **No traces (D5).** Model traffic goes through a per-run in-memory relay ([`relay`]) to the upstream
//!   in `INIT_PROXY_SOCK` (the Root's sealed lane); the live kernel proxy is never used. Nothing is
//!   written but `--out` and per-run scratch under `<home>/probe/` and `<home>/judge/`, each deleted by
//!   its exact path before exit. No lineage record, no note, no `noise.json`.
//! - **Deterministic.** The gate id (the seed of every shuffle and resample) is a digest of the inputs:
//!   the three trees, the task sets, the level, the stages and the configuration. δ comes from this
//!   gate's own LKG repeats only (no `noise.json` history, whose age filter depends on the clock), cost
//!   is the reference cost from tokens, wall time is not a judged metric (`ct_include_wall` defaults to
//!   false here) and no timing appears in the output.
//! - **Configuration:** `<home>/kernel/config.toml`'s `[gate.g4]` and `[gate.judge]` keys, then each
//!   `-c gate.g4.<key>=<toml value>` (an unknown key is a usage error).
//! - **Candidates** are trees in Phase 0 ([`tree`]: the script body of WP24's stand-in M). G2 and G3 need
//!   probes on a running kernel and are recorded as skipped for trees.

pub mod exec;
pub mod relay;
pub mod tree;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accept::{CostBudget, G4Config};
use crate::evo::ChangeType;
use crate::g4::{G4Plan, RunRecord, RunSet, TaskDef, load_task, run_g4, stage_record};
use exec::{ArmTree, Egress, TreeRuns};

/// The stages of a full gate, in order.
pub const STAGES: [&str; 5] = ["G0", "G1", "G2", "G3", "G4"];

/// Parsed `gate judge` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub home: PathBuf,
    pub candidate: PathBuf,
    pub running: PathBuf,
    pub lkg: PathBuf,
    pub stages: Vec<&'static str>,
    pub level: u8,
    pub out: PathBuf,
}

pub const USAGE: &str = "usage: init-kernel [-c <key>=<value>]… gate judge --home <INIT_HOME copy> --candidate <tree> \
--running <tree> --lkg <tree> [--stages G0,G1,G2,G3,G4] [--level L0..L4] --out <file>";

/// Parses the arguments after `gate judge`.
pub fn parse_args(a: &[String]) -> Result<Args, String> {
    let mut m: BTreeMap<&str, String> = BTreeMap::new();
    let mut i = 0;
    while i < a.len() {
        let k = a[i].as_str();
        if !matches!(
            k,
            "--home" | "--candidate" | "--running" | "--lkg" | "--stages" | "--level" | "--out"
        ) {
            return Err(format!("unknown argument {k:?}"));
        }
        let v = a.get(i + 1).ok_or_else(|| format!("{k} needs a value"))?;
        if m.insert(k, v.clone()).is_some() {
            return Err(format!("{k} given twice"));
        }
        i += 2;
    }
    let need = |k: &str| {
        m.get(k)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{k} is required"))
    };
    let stages = match m.get("--stages") {
        None => STAGES.to_vec(),
        Some(s) => {
            let mut v = Vec::new();
            for x in s.split(',') {
                let st = STAGES
                    .iter()
                    .find(|n| **n == x)
                    .ok_or_else(|| format!("unknown stage {x:?}"))?;
                if v.contains(st) {
                    return Err(format!("stage {x} given twice"));
                }
                v.push(*st);
            }
            v.sort_by_key(|s| STAGES.iter().position(|n| n == s));
            v
        }
    };
    let level = match m.get("--level").map(String::as_str) {
        None => 0,
        Some(l) => l
            .strip_prefix('L')
            .unwrap_or(l)
            .parse::<u8>()
            .ok()
            .filter(|n| *n <= 4)
            .ok_or_else(|| format!("bad --level {l:?}"))?,
    };
    Ok(Args {
        home: need("--home")?,
        candidate: need("--candidate")?,
        running: need("--running")?,
        lkg: need("--lkg")?,
        stages,
        level,
        out: need("--out")?,
    })
}

/// The judge's configuration (kernel/config.md §4.6 `[gate.g4]` keys, plus `[gate.judge]`).
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeCfg {
    pub g4: G4Config,
    pub repeats: u32,
    pub repeats_max: u32,
    pub parallel: usize,
    pub wall_s: u64,
    pub task_wall_s: u64,
    /// `gate.judge.quiet_ms`: a run ends after a `stop` finish and this long with nothing in flight.
    pub quiet_ms: u64,
    /// `gate.judge.step_timeout_s`: each G0 step of a tree.
    pub step_timeout_s: u64,
    /// `gate.judge.egress`: `auto` (a relay if a socket can be bound, else direct), `relay`, `direct`.
    pub egress: String,
}

impl Default for JudgeCfg {
    fn default() -> Self {
        JudgeCfg {
            g4: G4Config {
                ct_include_wall: false,
                ..G4Config::default()
            },
            repeats: 2,
            repeats_max: 3,
            parallel: 4,
            wall_s: 5400,
            task_wall_s: 900,
            quiet_ms: 5000,
            step_timeout_s: 300,
            egress: "auto".into(),
        }
    }
}

impl JudgeCfg {
    /// Applies one key; `Ok(false)` for a key this judge does not read.
    fn apply(&mut self, key: &str, v: &toml::Value) -> Result<bool, String> {
        let int = |v: &toml::Value| {
            v.as_integer()
                .filter(|n| *n >= 0)
                .map(|n| n as u64)
                .ok_or_else(|| format!("{key}: expected a non-negative integer"))
        };
        let flt = |v: &toml::Value| {
            v.as_float()
                .or_else(|| v.as_integer().map(|n| n as f64))
                .ok_or_else(|| format!("{key}: expected a number"))
        };
        let g = &mut self.g4;
        match key {
            "gate.g4.repeats" => self.repeats = int(v)? as u32,
            "gate.g4.repeats_max" => self.repeats_max = int(v)? as u32,
            "gate.g4.parallel" => self.parallel = int(v)? as usize,
            "gate.g4.wall_s" => self.wall_s = int(v)?,
            "gate.g4.task_wall_s" => self.task_wall_s = int(v)?,
            "gate.g4.min_pool" => g.min_pool = int(v)? as usize,
            "gate.g4.min_holdout" => g.min_holdout = int(v)? as usize,
            "gate.g4.min_canaries" => g.min_canaries = int(v)? as usize,
            "gate.g4.bootstrap_b" => g.bootstrap_b = int(v)? as usize,
            "gate.g4.noise_b" => g.noise_b = int(v)? as usize,
            "gate.g4.capability_min_tasks" => g.capability_min_tasks = int(v)? as usize,
            "gate.g4.alpha" => g.alpha = flt(v)?,
            "gate.g4.margin_factor" => g.margin_factor = flt(v)?,
            "gate.g4.margin_max" => g.margin_max = flt(v)?,
            "gate.g4.rho_max" => g.rho_max = flt(v)?,
            "gate.g4.ct_include_wall" => {
                g.ct_include_wall = v
                    .as_bool()
                    .ok_or_else(|| format!("{key}: expected a boolean"))?
            }
            "gate.judge.quiet_ms" => self.quiet_ms = int(v)?,
            "gate.judge.step_timeout_s" => self.step_timeout_s = int(v)?,
            "gate.judge.egress" => {
                self.egress = v
                    .as_str()
                    .filter(|e| matches!(*e, "auto" | "relay" | "direct"))
                    .ok_or_else(|| format!("{key}: expected \"auto\", \"relay\" or \"direct\""))?
                    .into()
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Defaults, then `<home>/kernel/config.toml` (keys this judge does not read are ignored), then
    /// each `-c key=value` override (an unknown key is an error).
    pub fn load(home: &Path, overrides: &[String]) -> Result<JudgeCfg, String> {
        let mut c = JudgeCfg::default();
        if let Ok(text) = std::fs::read_to_string(home.join("kernel/config.toml")) {
            let doc: toml::Table = text
                .parse()
                .map_err(|e| format!("kernel/config.toml: {e}"))?;
            for (sect, inner) in [("g4", "gate.g4"), ("judge", "gate.judge")] {
                if let Some(t) = doc
                    .get("gate")
                    .and_then(|g| g.get(sect))
                    .and_then(toml::Value::as_table)
                {
                    for (k, v) in t {
                        c.apply(&format!("{inner}.{k}"), v)?;
                    }
                }
            }
        }
        for o in overrides {
            let (k, v) = o
                .split_once('=')
                .ok_or_else(|| format!("-c {o:?}: expected <key>=<value>"))?;
            let doc: toml::Table = format!("x = {v}")
                .parse()
                .map_err(|e| format!("-c {k}: {e}"))?;
            if !c.apply(k.trim(), &doc["x"])? {
                return Err(format!(
                    "-c {k}: not a gate.g4 or gate.judge key this judge reads"
                ));
            }
        }
        if c.repeats < 2 || c.repeats > c.repeats_max || c.repeats_max > 5 || c.parallel == 0 {
            return Err("gate.g4: need 2 ≤ repeats ≤ repeats_max ≤ 5 and parallel ≥ 1".into());
        }
        Ok(c)
    }

    fn as_json(&self) -> Value {
        let g = &self.g4;
        json!({"repeats": self.repeats, "repeats_max": self.repeats_max, "parallel": self.parallel,
               "wall_s": self.wall_s, "task_wall_s": self.task_wall_s, "quiet_ms": self.quiet_ms,
               "min_pool": g.min_pool, "min_holdout": g.min_holdout, "min_canaries": g.min_canaries,
               "bootstrap_b": g.bootstrap_b, "noise_b": g.noise_b, "capability_min_tasks": g.capability_min_tasks,
               "alpha_ppm": (g.alpha * 1e6).round() as i64, "margin_factor_ppm": (g.margin_factor * 1e6).round() as i64,
               "margin_max_ppm": (g.margin_max * 1e6).round() as i64, "rho_max_ppm": (g.rho_max * 1e6).round() as i64,
               "ct_include_wall": g.ct_include_wall, "egress": self.egress})
    }
}

/// Compact JSON with every object's keys sorted at every level, whatever serde_json's features.
pub fn sorted_json(v: &Value) -> String {
    fn w(out: &mut String, v: &Value) {
        match v {
            Value::Object(m) => {
                let mut ks: Vec<&String> = m.keys().collect();
                ks.sort();
                out.push('{');
                for (i, k) in ks.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String((*k).clone()).to_string());
                    out.push(':');
                    w(out, &m[*k]);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    w(out, x);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut s = String::new();
    w(&mut s, v);
    s
}

/// A judge result.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub verdict: String,
    pub reason: String,
    pub doc: Value,
}

impl Outcome {
    pub fn exit_code(&self) -> i32 {
        match self.verdict.as_str() {
            "accept" => 0,
            "reject" => 1,
            "inconclusive" => 2,
            _ => 3,
        }
    }
}

fn load_set(home: &Path, rel: &str, set: RunSet, skipped: &mut Vec<String>) -> Vec<TaskDef> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(home.join(rel)) else {
        return v;
    };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for p in dirs {
        match load_task(&p) {
            Ok(t) => v.push(TaskDef {
                id: t.id.clone(),
                set,
                source: t.source.kind,
                labels: t.labels.clone(),
                family: None,
                dir: p,
                task: t,
            }),
            Err(_) => skipped.push(
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        }
    }
    v
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

struct Ctx<'a> {
    args: &'a Args,
    cfg: &'a JudgeCfg,
    gate_id: String,
    level: String,
    c_sha: String,
    lkg_sha: String,
    record: Value,
    stages_run: Vec<&'static str>,
}

impl Ctx<'_> {
    fn finish(&mut self, verdict: &str, reason: &str) -> Outcome {
        self.record["verdict"] = json!(verdict);
        self.record["reason"] = json!(reason);
        let spent: u64 = self.record["stages"]
            .as_object()
            .map(|m| m.values().filter_map(|s| s["spent_musd"].as_u64()).sum())
            .unwrap_or(0);
        self.record["spent_musd"] = json!(spent);
        let doc = json!({"verdict": verdict, "reason": reason, "scope": "t2", "stages": self.stages_run,
                         "record": std::mem::take(&mut self.record)});
        Outcome {
            verdict: verdict.into(),
            reason: reason.into(),
            doc,
        }
    }
}

fn error(args: &Args, reason: &str, detail: &str) -> Outcome {
    let record = json!({"kind": "gate", "v": crate::record::RECORD_V, "gate_kind": "judge", "verdict": "error",
                        "reason": reason, "detail": crate::record::cap(detail, 4096), "level": format!("L{}", args.level)});
    Outcome {
        verdict: "error".into(),
        reason: reason.into(),
        doc: json!({"verdict": "error", "reason": reason, "scope": "t2", "stages": [], "record": record}),
    }
}

/// Runs the judge. `upstream` is the model egress (`INIT_PROXY_SOCK`). Relay sockets are bound relative
/// to the process's working directory when that is shorter than their absolute path (`sun_path`).
pub fn judge(args: &Args, cfg: &JudgeCfg, upstream: Option<PathBuf>) -> Outcome {
    let canon = |p: &Path| std::fs::canonicalize(p).ok().filter(|c| c.is_dir());
    let (Some(home), Some(cand), Some(running), Some(lkg)) = (
        canon(&args.home),
        canon(&args.candidate),
        canon(&args.running),
        canon(&args.lkg),
    ) else {
        return error(
            args,
            "candidate_form",
            "--home, --candidate, --running and --lkg must be directories (sha candidates need the home copy's body.git, which judge does not read yet)",
        );
    };
    let cwd = std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("/"));
    for t in [&cand, &running, &lkg] {
        if let Err(e) = tree::check_form(t) {
            return error(args, "unsupported_tree", &format!("{}: {e}", t.display()));
        }
    }
    let dig = |p: &Path| tree::digest(p).map_err(|e| format!("{}: {e}", p.display()));
    let (c_sha, running_sha, lkg_sha) = match (dig(&cand), dig(&running), dig(&lkg)) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return error(args, "infra", &e),
    };
    let mut skipped = Vec::new();
    let pool = load_set(&home, "vault/gate/pool", RunSet::Pool, &mut skipped);
    let holdout = load_set(&home, "vault/holdout", RunSet::Holdout, &mut skipped);
    let canaries = load_set(&home, "vault/canaries", RunSet::Canary, &mut skipped);
    // the gate id: a digest of every input, so the same inputs give the same seeds
    let mut h = Sha256::new();
    for s in [&c_sha, &running_sha, &lkg_sha] {
        h.update(s.as_bytes());
        h.update(b"\n");
    }
    for t in pool.iter().chain(&holdout).chain(&canaries) {
        h.update(
            format!(
                "{:?}\t{}\t{}\n",
                t.set,
                t.id,
                tree::digest(&t.dir).unwrap_or_default()
            )
            .as_bytes(),
        );
    }
    h.update(
        format!(
            "L{}\n{}\n{}\n",
            args.level,
            args.stages.join(","),
            sorted_json(&cfg.as_json())
        )
        .as_bytes(),
    );
    let gate_id = format!("j-{}", &hex(&h.finalize())[..12]);
    let level = format!("L{}", args.level);
    let record = json!({
        "kind": "gate", "v": crate::record::RECORD_V, "gate_id": gate_id, "gate_kind": "judge", "scope": "t2", "level": level,
        "candidate": {"sha": c_sha, "form": "tree", "change_type": ChangeType::Feat.as_str(), "capability_labels": []},
        "parent": {"running": running_sha, "lkg": lkg_sha},
        "stages": {}, "escalated": false, "runs": [], "config": cfg.as_json(),
        "tasks": {"pool": pool.len(), "holdout": holdout.len(), "canaries": canaries.len(), "skipped": skipped},
    });
    let mut cx = Ctx {
        args,
        cfg,
        gate_id,
        level,
        c_sha,
        lkg_sha,
        record,
        stages_run: Vec::new(),
    };
    let scratch = home.join("judge").join(&cx.gate_id);
    let _ = std::fs::remove_dir_all(&scratch); // exact path: a stale scratch of this same gate
    let ours: Vec<PathBuf> = [home.join("judge"), home.join("probe")]
        .into_iter()
        .filter(|d| !d.exists())
        .collect();
    let out = run_stages(
        &mut cx, &home, &cand, &lkg, pool, holdout, canaries, upstream, &cwd, &scratch,
    );
    let _ = std::fs::remove_dir_all(&scratch);
    for d in ours {
        let _ = std::fs::remove_dir(&d); // created by this judge; only if empty, never recursive
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn run_stages(
    cx: &mut Ctx<'_>,
    home: &Path,
    cand: &Path,
    lkg: &Path,
    pool: Vec<TaskDef>,
    holdout: Vec<TaskDef>,
    canaries: Vec<TaskDef>,
    upstream: Option<PathBuf>,
    cwd: &Path,
    scratch: &Path,
) -> Outcome {
    let want = |s: &str| cx.args.stages.contains(&s);
    if want("G0") {
        let _ = std::fs::create_dir_all(scratch.join("g0"));
        let (v, f) = tree::g0(
            cand,
            &scratch.join("g0"),
            Duration::from_secs(cx.cfg.step_timeout_s),
        );
        cx.record["stages"]["G0"] = v;
        cx.stages_run.push("G0");
        if let Some(f) = f {
            return cx.finish("reject", f.reason());
        }
    }
    if want("G1") {
        let (mut v, f) = tree::g1(cand);
        v["body_tokens"] = json!({"c": tree::body_tokens(cand), "p": tree::body_tokens(lkg)});
        cx.record["stages"]["G1"] = v;
        cx.stages_run.push("G1");
        if let Some(f) = f {
            return cx.finish("reject", f.reason());
        }
    }
    for s in ["G2", "G3"] {
        if want(s) {
            cx.record["stages"][s] =
                json!({"skipped": "tree candidate: G2 and G3 need probes on a running kernel"});
        }
    }
    if !want("G4") {
        return cx.finish("accept", "stages_pass");
    }
    if upstream.is_none() {
        return cx.finish("error", "no_egress");
    }
    let mut ex = TreeRuns {
        home: home.to_path_buf(),
        cwd: cwd.to_path_buf(),
        upstream,
        egress: Egress::Relay,
        serial: std::sync::Mutex::new(()),
        c: ArmTree {
            dir: cand.to_path_buf(),
            sha: cx.c_sha.clone(),
        },
        p: ArmTree {
            dir: lkg.to_path_buf(),
            sha: cx.lkg_sha.clone(),
        },
        gate_id: cx.gate_id.clone(),
        level: cx.level.clone(),
        task_wall_s: cx.cfg.task_wall_s,
        quiet_ms: cx.cfg.quiet_ms,
    };
    let plan = G4Plan {
        gate_id: cx.gate_id.clone(),
        tasks: pool.into_iter().chain(holdout).collect(),
        canaries,
        repeats: cx.cfg.repeats,
        repeats_max: cx.cfg.repeats_max,
        parallel: cx.cfg.parallel,
        config: cx.cfg.g4.clone(),
        futility_alpha: 0.01,
        noise_history_days: 30,
        budget: CostBudget {
            soft_day_musd: 4_000_000,
            evolution_share_permille: 600,
            tasks_per_day: 10.0,
        },
        change_type: ChangeType::Feat,
        capability_labels: Vec::new(),
        body_tokens_c: tree::body_tokens(cand),
        body_tokens_p: tree::body_tokens(lkg),
        history: None,
        model: "deepseek-flash".into(),
        now_unix: 0,
        now_rfc3339: "1970-01-01T00:00:00Z".into(),
        wall_s: cx.cfg.wall_s,
        // The judge's relay caps each run at the task's cap itself; no proxy table here.
        request_reservation_musd: None,
    };
    ex.egress = match cx.cfg.egress.as_str() {
        "relay" => Egress::Relay,
        "direct" => Egress::Direct,
        _ if can_bind(&ex) => Egress::Relay,
        _ => Egress::Direct,
    };
    if ex.egress == Egress::Relay && ex.sock_of(&ex.root_of("c-x-r1")).is_none() {
        cx.record["stages"]["G4"] = json!({"status": "error", "reason": "socket_path",
            "detail": "the relay socket under <home>/probe/ does not fit sun_path (103 bytes) as an absolute path or relative to the cwd; run judge with its cwd at or near --home, or with gate.judge.egress = \"direct\""});
        return cx.finish("error", "socket_path");
    }
    cx.record["egress"] = json!(ex.egress.as_str());
    let o = run_g4(&plan, &ex, &|| {});
    let mut st = stage_record(&o, true);
    if let Some(m) = st.as_object_mut() {
        m.remove("wall_ms"); // timings are not reproducible (§4.1a determinism)
    }
    cx.record["stages"]["G4"] = st;
    cx.stages_run.push("G4");
    // run data for the Root (§4.4): sorted, without timings
    let set_rank = |s: &str| {
        ["canary", "pool", "holdout"]
            .iter()
            .position(|x| *x == s)
            .unwrap_or(3)
    };
    let mut runs: Vec<RunRecord> = o.runs.clone();
    runs.sort_by(|a, b| {
        (set_rank(&a.set), &a.task, a.repeat, &a.arm).cmp(&(
            set_rank(&b.set),
            &b.task,
            b.repeat,
            &b.arm,
        ))
    });
    let runs: Vec<Value> = runs
        .into_iter()
        .map(|mut r| {
            (
                r.wall_ms,
                r.ready_ms,
                r.check_ms,
                r.gap_ms_p50,
                r.ttft_ms_p50,
            ) = (0, 0, 0, 0, 0);
            serde_json::to_value(r).unwrap_or(Value::Null)
        })
        .collect();
    cx.record["runs"] = Value::Array(runs);
    cx.finish(o.verdict_kind(), o.verdict_reason())
}

/// `auto` egress: whether a relay socket can be bound here (the Root's sealed profile denies
/// `network-bind`, so there the judge goes direct).
fn can_bind(ex: &TreeRuns) -> bool {
    let root = ex.root_of("bind-probe");
    let ok = std::fs::create_dir_all(&root).is_ok()
        && ex.sock_of(&root).is_some_and(|p| {
            let ok = std::os::unix::net::UnixListener::bind(&p).is_ok();
            let _ = std::fs::remove_file(&p);
            ok
        });
    let _ = std::fs::remove_dir_all(&root);
    ok
}

/// `gate judge …` with the `-c` overrides that came before `gate`: parses, runs, writes `--out`, and
/// returns the exit code.
pub fn cli(a: &[String], overrides: &[String]) -> i32 {
    let args = match parse_args(a) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("init-kernel gate judge: {e}\n{USAGE}");
            return 64;
        }
    };
    let cfg = match JudgeCfg::load(&args.home, overrides) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("init-kernel gate judge: {e}\n{USAGE}");
            return 64;
        }
    };
    let upstream = std::env::var_os("INIT_PROXY_SOCK")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let o = judge(&args, &cfg, upstream);
    if let Err(e) = std::fs::write(&args.out, sorted_json(&o.doc)) {
        eprintln!("init-kernel gate judge: --out {}: {e}", args.out.display());
        return 3;
    }
    o.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args_parse_and_refuse() {
        let a = parse_args(&s(&[
            "--home",
            "h",
            "--candidate",
            "c",
            "--running",
            "r",
            "--lkg",
            "l",
            "--out",
            "o",
            "--level",
            "2",
            "--stages",
            "G4,G0",
        ]))
        .unwrap();
        assert_eq!((a.level, a.stages.clone()), (2, vec!["G0", "G4"]));
        assert_eq!(
            parse_args(&s(&[
                "--home",
                "h",
                "--candidate",
                "c",
                "--running",
                "r",
                "--lkg",
                "l",
                "--out",
                "o",
                "--level",
                "L3"
            ]))
            .unwrap()
            .level,
            3
        );
        for bad in [
            &["--home", "h"][..],
            &[
                "--home",
                "h",
                "--candidate",
                "c",
                "--running",
                "r",
                "--lkg",
                "l",
                "--out",
                "o",
                "--level",
                "L5",
            ],
            &[
                "--home",
                "h",
                "--candidate",
                "c",
                "--running",
                "r",
                "--lkg",
                "l",
                "--out",
                "o",
                "--stages",
                "G9",
            ],
            &[
                "--home",
                "h",
                "--candidate",
                "c",
                "--running",
                "r",
                "--lkg",
                "l",
                "--out",
                "o",
                "--bogus",
                "x",
            ],
            &[
                "--home",
                "h",
                "--home",
                "h",
                "--candidate",
                "c",
                "--running",
                "r",
                "--lkg",
                "l",
                "--out",
                "o",
            ],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn config_overrides_and_defaults() {
        let home = crate::testutil::test_home("judge-cfg");
        let c = JudgeCfg::load(&home, &[]).unwrap();
        assert!(
            !c.g4.ct_include_wall,
            "wall time is not judged in judge mode"
        );
        assert_eq!((c.repeats, c.g4.min_pool, c.g4.min_holdout), (2, 8, 4));
        std::fs::create_dir_all(home.join("kernel")).unwrap();
        std::fs::write(
            home.join("kernel/config.toml"),
            "[gate.g4]\nmin_pool = 6\npool_k = 12\n[gate.judge]\nquiet_ms = 100\n",
        )
        .unwrap();
        let c = JudgeCfg::load(
            &home,
            &s(&["gate.g4.min_holdout=1", "gate.g4.margin_max=0.2"]),
        )
        .unwrap();
        assert_eq!((c.g4.min_pool, c.g4.min_holdout, c.quiet_ms), (6, 1, 100));
        assert!((c.g4.margin_max - 0.2).abs() < 1e-12);
        assert!(JudgeCfg::load(&home, &s(&["gate.g4.nope=1"])).is_err());
        assert!(JudgeCfg::load(&home, &s(&["gate.g4.repeats=1"])).is_err());
        assert!(JudgeCfg::load(&home, &s(&["gate.g4.min_pool"])).is_err());
        crate::testutil::remove_home(&home);
    }

    #[test]
    fn sorted_json_sorts_every_level() {
        let v = json!({"b": [{"z": 1, "a": 2}], "a": "x"});
        assert_eq!(sorted_json(&v), r#"{"a":"x","b":[{"a":2,"z":1}]}"#);
    }
}
