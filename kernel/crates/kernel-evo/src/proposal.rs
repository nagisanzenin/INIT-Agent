//! Proposal records (evolution-v2 §3.4.7): `initctl evo propose <file>` validates the agent's
//! draft, the kernel assigns the id and owns `state`, and the record lives in
//! `state/kernel/evo/proposals/<id>.json` (kernel-written, agent-readable).
//!
//! Validation collects every problem (codes of §4.4): `evo.schema`, `evo.evidence_missing`,
//! `evo.evidence_unattested`, `evo.not_failure`, `evo.quota`. Evidence: ≥ 2 refs of attested
//! kinds (`trace:`, `run:`, `friction:`), at least one not a `friction:` id, each failure-bearing
//! (a `trace:` request carrying an error result, a `run:` with score < 1000, an eligible
//! `friction:`), and spanning ≥ 2 tasks or sessions unless a cited friction is eligible or a run
//! is on a divergence task. `event:` refs are supplementary only.
//!
//! D-L5 cuts: no lessons (`lessons` must be empty), no compose or bank. The orchestrator's D-L2
//! rule: only body-binary (T2) changes are measured on S, so the record carries `s_measurable`.

use crate::fsx;
use crate::mine::{self, Cluster};
use crate::runs;
use kernel_gate::evo::trailers::TrailerParse;
use kernel_gate::evo::ExtendMode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `evolution.engine.proposals_day` (§4.5).
pub const PROPOSALS_DAY: u64 = 6;
/// `evolution.engine.k_variants`.
pub const K_MAX: u32 = 3;
/// A draft file is at most this large.
pub const MAX_DRAFT_BYTES: usize = 64 * 1024;
/// Free-text fields.
pub const MAX_TEXT: usize = 1000;

/// Engine codes (§4.4).
pub mod codes {
    pub const SCHEMA: &str = "evo.schema";
    pub const EVIDENCE_MISSING: &str = "evo.evidence_missing";
    pub const EVIDENCE_UNATTESTED: &str = "evo.evidence_unattested";
    pub const NOT_FAILURE: &str = "evo.not_failure";
    pub const QUOTA: &str = "evo.quota";
    pub const STATE: &str = "evo.state";
    pub const PATCH_APPLY: &str = "evo.patch_apply";
    pub const DUPLICATE_VARIANT: &str = "evo.duplicate_variant";
    pub const SCOPE_PATHS: &str = "evo.scope_paths";
    pub const DEP: &str = "evo.dep";
    pub const IO: &str = "evo.io";
}

/// One problem.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub detail: String,
}

impl Problem {
    pub fn new(code: &str, detail: impl Into<String>) -> Problem {
        Problem { code: code.to_string(), detail: crate::classify::cap(&detail.into(), 500) }
    }
}

/// A kernel-built variant (§3.5.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    pub k: u32,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub paths: Vec<String>,
    pub diff_stat: String,
    /// The gate scope is `t2` or `dep` (the body binary changes): measurable on S (D-L2).
    pub s_measurable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<Value>,
}

/// The chosen variant after the go/no-go (§3.5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    pub k: u32,
    pub sha: String,
    /// `better` (est ≥ 150‰) or `lineage_gain` (80–150‰).
    pub aim: String,
    pub s_measurable: bool,
}

/// The stored record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub v: u32,
    pub id: String,
    pub scope: String,
    pub frictions: Vec<String>,
    pub hypothesis: String,
    pub change: String,
    pub repro: Vec<String>,
    pub predicts: Vec<String>,
    pub subject: String,
    pub compensates_for: String,
    pub prefix_change: bool,
    pub lessons: Vec<String>,
    pub k: u32,
    pub tier_hint: String,
    /// `tier_hint == "t2"` at filing; per variant from its paths after `evo variant`.
    pub s_measurable: bool,
    /// Pool tasks where the cited frictions were fatal (E3's targeted tasks).
    pub targets: Vec<String>,
    pub by: Value,
    /// `proposed → variants → screened → promoted`, then `verdict` (gated) or `opening → open →`
    /// `verdict | rolled_back | collateral` (free), or `abandoned:<why>` ([`states`], X0b).
    pub state: String,
    pub est: Option<u32>,
    pub go: Option<bool>,
    pub chosen: Option<Chosen>,
    pub variants: Vec<Variant>,
    pub created: String,
    pub history: Vec<Value>,
    /// The change group the chosen variant is live in (ADR-011 free path), from the Root's
    /// `change_open` (X0b). Absent before `open` and after the proposal leaves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<ChangeRef>,
    /// Re-admissions used after a rollback that was not a verdict (ADR-011 §3.5: one).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub readmits: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Proposal {
    /// Open (not terminal) states.
    pub fn is_open(&self) -> bool {
        !self.state.starts_with("abandoned") && !matches!(self.state.as_str(), states::VERDICT | states::ROLLED_BACK | states::COLLATERAL)
    }
    /// A state transition, with its kernel record (§4.7 `proposal`).
    pub fn transition(&mut self, state: &str, note: Value) -> Value {
        self.state = state.to_string();
        let h = json!({"state": state, "ts": fsx::now_rfc3339(), "note": note});
        self.history.push(h.clone());
        json!({"id": self.id, "state": state, "scope": self.scope, "frictions": self.frictions, "note": h["note"], "ts": h["ts"]})
    }
}

/// `state/kernel/evo/proposals`.
pub fn dir(home: &Path) -> PathBuf {
    home.join(mine::EVO_DIR).join("proposals")
}

/// `p-` + 6 digits.
pub fn is_id(s: &str) -> bool {
    s.len() == 8 && s.starts_with("p-") && s[2..].bytes().all(|b| b.is_ascii_digit())
}

/// Reads a proposal.
pub fn load(home: &Path, id: &str) -> io::Result<Option<Proposal>> {
    if !is_id(id) {
        return Ok(None);
    }
    match fsx::read_capped(&dir(home).join(format!("{id}.json")), 4 << 20)? {
        None => Ok(None),
        Some(b) => serde_json::from_slice(&b).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{id}: {e}"))),
    }
}

/// Writes a proposal (kernel only).
pub fn store(home: &Path, p: &Proposal) -> io::Result<()> {
    let mut b = serde_json::to_vec_pretty(p).map_err(io::Error::other)?;
    b.push(b'\n');
    fsx::write_atomic(&dir(home).join(format!("{}.json", p.id)), &b)
}

/// Every proposal, by id.
pub fn all(home: &Path) -> Vec<Proposal> {
    let mut v: Vec<Proposal> = std::fs::read_dir(dir(home))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_string))
                .filter_map(|id| load(home, &id).ok().flatten())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Open proposals counted by state (the boot block's engine line).
pub fn open_by_state(home: &Path) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for p in all(home).into_iter().filter(Proposal::is_open) {
        *m.entry(p.state).or_insert(0) += 1;
    }
    m
}

static NUMBERING: Mutex<()> = Mutex::new(());

/// What a ref is and whether it is good evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefCheck {
    pub attested: bool,
    pub problem: Option<Problem>,
    /// The task or session it comes from (eligibility units).
    pub unit: Option<String>,
    /// A `run:` on a task with passing and failing runs (a divergence pair).
    pub divergent: bool,
    /// A `friction:` that is eligible.
    pub eligible_friction: bool,
}

/// Checks one evidence ref against kernel-written data.
pub fn check_ref(home: &Path, r: &str, index: &[Cluster], pool: &[runs::PoolRun], mixed: &BTreeSet<String>) -> RefCheck {
    let mut c = RefCheck { attested: false, problem: None, unit: None, divergent: false, eligible_friction: false };
    if let Some(id) = r.strip_prefix("trace:") {
        c.attested = true;
        match crate::trace::view(&home.join("vault/traces/v1"), id) {
            Ok(Some(v)) => {
                c.unit = v.task.clone().or(v.session.clone()).or(Some(format!("trace:{id}")));
                if crate::trace::failure(&v).is_none() {
                    c.problem = Some(Problem::new(codes::NOT_FAILURE, format!("{r}: the request carries no failing tool result, abort or refusal")));
                }
            }
            Ok(None) => c.problem = Some(Problem::new(codes::EVIDENCE_MISSING, format!("{r}: no agent-visible request with this id"))),
            Err(e) => c.problem = Some(Problem::new(codes::IO, format!("{r}: trace store: {e}"))),
        }
    } else if let Some(rest) = r.strip_prefix("run:") {
        c.attested = true;
        let (g, run) = rest.split_once('/').unwrap_or((rest, ""));
        match pool.iter().find(|p| p.gate_id == g && p.run_id == run) {
            Some(p) => {
                c.unit = Some(p.task.clone());
                c.divergent = mixed.contains(&p.task);
                if !p.failing() {
                    c.problem = Some(Problem::new(codes::NOT_FAILURE, format!("{r}: the run scored {} (not < 1000)", p.score)));
                }
            }
            None => c.problem = Some(Problem::new(codes::EVIDENCE_MISSING, format!("{r}: no pool RunRecord with this id"))),
        }
    } else if let Some(id) = r.strip_prefix("friction:") {
        c.attested = true;
        match index.iter().find(|x| x.id == id) {
            Some(f) if f.eligible => c.eligible_friction = true,
            Some(_) => c.problem = Some(Problem::new(codes::NOT_FAILURE, format!("{r}: the cluster is not eligible (latent risk)"))),
            None => c.problem = Some(Problem::new(codes::EVIDENCE_MISSING, format!("{r}: no such friction"))),
        }
    } else if r.starts_with("event:") || r.starts_with("task:") {
        // Supplementary only.
    } else {
        c.problem = Some(Problem::new(codes::SCHEMA, format!("repro ref '{r}': expected trace:, run:, friction:, event: or task:")));
    }
    let _ = home;
    c
}

fn str_field(v: &Value, k: &str, max: usize, probs: &mut Vec<Problem>) -> String {
    match v.get(k) {
        Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= max && !s.contains('\n') => s.clone(),
        Some(Value::String(s)) if s.len() > max => {
            probs.push(Problem::new(codes::SCHEMA, format!("{k}: at most {max} bytes")));
            String::new()
        }
        Some(Value::String(_)) => {
            probs.push(Problem::new(codes::SCHEMA, format!("{k}: one non-empty line")));
            String::new()
        }
        _ => {
            probs.push(Problem::new(codes::SCHEMA, format!("{k}: required string")));
            String::new()
        }
    }
}

fn str_list(v: &Value, k: &str, probs: &mut Vec<Problem>) -> Vec<String> {
    match v.get(k) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => {
            probs.push(Problem::new(codes::SCHEMA, format!("{k}: an array of strings")));
            vec![]
        }
    }
}

/// The commit message the kernel writes for every variant of `p` (§3.5.4's form).
pub fn commit_message(p: &Proposal, k: u32, index: &[Cluster]) -> String {
    let mut body = format!("Engine proposal {} (scope {}), variant {k} of {}.", p.id, p.scope, p.k);
    for f in &p.frictions {
        if let Some(c) = index.iter().find(|c| &c.id == f) {
            body.push_str(&format!(" Cluster {} {}: {} instances in {} sessions, {} fatal pool runs.", c.id, c.signature, c.counts.instances, c.counts.sessions, c.counts.fatal_runs));
        }
    }
    body.push_str(&format!("\n\nThe change: {}", p.change));
    let repro: Vec<&String> = p.repro.iter().filter(|r| !r.starts_with("task:") || true).take(5).collect();
    let mut m = format!("{}\n\n{}\n\n", p.subject, wrap(&body, 72));
    m.push_str(&format!("Why: {}\n", p.hypothesis));
    if !p.predicts.is_empty() {
        m.push_str(&format!("Predicts: {}\n", p.predicts.join("; "))); // optional on the free path (ADR-011)
    }
    m.push_str(&format!("Compensates-for: {}\n", p.compensates_for));
    m.push_str(&format!("Prefix-change: {}\n", if p.prefix_change { "yes" } else { "no" }));
    if !repro.is_empty() {
        m.push_str(&format!("Repro: {}\n", repro.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    m.push_str(&format!("Proposal: {}\n", p.id));
    m
}

fn wrap(s: &str, w: usize) -> String {
    let mut out = String::new();
    for para in s.split("\n\n") {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > w {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push_str(&line);
    }
    out
}

/// Who filed it (`by`, §3.4.7).
#[derive(Clone, Debug, Default)]
pub struct Filer {
    pub lane: String,
    pub session: Option<String>,
    pub model: Option<String>,
}

/// `evo propose`: validates `draft` (the agent's JSON) and stores the record. Returns the record
/// and its transition record, or every problem at once. The gated path's rules ([`propose_in`]).
pub fn propose(home: &Path, draft: &[u8], filer: &Filer, parser: &dyn TrailerParse) -> Result<(Proposal, Value), Vec<Problem>> {
    propose_in(home, draft, filer, parser, ExtendMode::Gated)
}

/// [`propose`] under `mode`. ADR-011 decision 1: on the free path `predicts` is optional (0 to 5
/// clauses; a clause given must still parse, and is scored only when an instrument measures it).
pub fn propose_in(home: &Path, draft: &[u8], filer: &Filer, parser: &dyn TrailerParse, mode: ExtendMode) -> Result<(Proposal, Value), Vec<Problem>> {
    let mut probs = Vec::new();
    if draft.len() > MAX_DRAFT_BYTES {
        return Err(vec![Problem::new(codes::SCHEMA, format!("the draft is {} bytes; at most {MAX_DRAFT_BYTES}", draft.len()))]);
    }
    let v: Value = match serde_json::from_slice(draft) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => return Err(vec![Problem::new(codes::SCHEMA, "the draft must be a JSON object")]),
        Err(e) => return Err(vec![Problem::new(codes::SCHEMA, format!("not JSON: {e}"))]),
    };
    const KNOWN: &[&str] = &["v", "scope", "frictions", "hypothesis", "change", "repro", "predicts", "subject", "compensates_for", "prefix_change", "k", "tier_hint", "lessons", "targets"];
    for k in v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
        if !KNOWN.contains(&k.as_str()) {
            probs.push(Problem::new(codes::SCHEMA, format!("unknown field '{k}' (the kernel sets id, state, est, by, created)")));
        }
    }
    if v.get("v").and_then(Value::as_u64) != Some(1) {
        probs.push(Problem::new(codes::SCHEMA, "v: must be 1"));
    }
    let scope = str_field(&v, "scope", 8, &mut probs);
    if !scope.is_empty() && scope != "inv" && scope != "dom" {
        probs.push(Problem::new(codes::SCHEMA, "scope: inv or dom"));
    }
    let hypothesis = str_field(&v, "hypothesis", MAX_TEXT, &mut probs);
    let change = str_field(&v, "change", MAX_TEXT, &mut probs);
    let subject = str_field(&v, "subject", 72, &mut probs);
    let compensates_for = str_field(&v, "compensates_for", 200, &mut probs);
    let frictions = str_list(&v, "frictions", &mut probs);
    let repro = str_list(&v, "repro", &mut probs);
    let predicts = str_list(&v, "predicts", &mut probs);
    let lessons = str_list(&v, "lessons", &mut probs);
    let targets_in = str_list(&v, "targets", &mut probs);
    let prefix_change = match v.get("prefix_change") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => {
            probs.push(Problem::new(codes::SCHEMA, "prefix_change: a boolean"));
            false
        }
    };
    let k = match v.get("k") {
        None => K_MAX,
        Some(x) => match x.as_u64() {
            Some(n) if (1..=K_MAX as u64).contains(&n) => n as u32,
            _ => {
                probs.push(Problem::new(codes::SCHEMA, format!("k: 1 to {K_MAX}")));
                K_MAX
            }
        },
    };
    let tier_hint = match v.get("tier_hint").and_then(Value::as_str) {
        None => "t2".to_string(),
        Some(t @ ("t0" | "t1" | "t2")) => t.to_string(),
        Some(_) => {
            probs.push(Problem::new(codes::SCHEMA, "tier_hint: t0, t1 or t2"));
            "t2".into()
        }
    };
    if !lessons.is_empty() {
        probs.push(Problem::new(codes::SCHEMA, "lessons: not available in this build (D-L5); leave it empty"));
    }
    if frictions.is_empty() || frictions.len() > 4 {
        probs.push(Problem::new(codes::SCHEMA, "frictions: 1 to 4 friction ids (f-NNNNNN)"));
    }
    if mode == ExtendMode::Free {
        if predicts.len() > 5 {
            probs.push(Problem::new(codes::SCHEMA, "predicts: at most 5 clauses (optional on the free path)"));
        }
    } else if predicts.is_empty() || predicts.len() > 5 {
        probs.push(Problem::new(codes::SCHEMA, "predicts: 1 to 5 clauses"));
    }
    if repro.len() > 5 {
        probs.push(Problem::new(codes::SCHEMA, "repro: at most 5 refs"));
    }

    // Evidence against kernel-written data.
    let index = mine::read_index(home);
    let pool = runs::pool_runs(home);
    let mixed = runs::mixed_tasks(&pool);
    for f in &frictions {
        match index.iter().find(|c| &c.id == f) {
            None => probs.push(Problem::new(codes::SCHEMA, format!("frictions: {f} is not in the friction index (run `initctl evo frictions`)"))),
            Some(c) if !c.eligible => probs.push(Problem::new(codes::NOT_FAILURE, format!("frictions: {f} ({}) is not eligible", c.signature))),
            _ => {}
        }
    }
    let checks: Vec<(String, RefCheck)> = repro.iter().map(|r| (r.clone(), check_ref(home, r, &index, &pool, &mixed))).collect();
    let attested: Vec<&(String, RefCheck)> = checks.iter().filter(|(_, c)| c.attested).collect();
    let non_friction = attested.iter().filter(|(r, _)| !r.starts_with("friction:")).count();
    let supplementary = checks.iter().filter(|(r, _)| r.starts_with("event:") || r.starts_with("task:")).count();
    if attested.len() < 2 || non_friction < 1 {
        let code = if supplementary > 0 { codes::EVIDENCE_UNATTESTED } else { codes::EVIDENCE_MISSING };
        probs.push(Problem::new(code, format!("repro: needs ≥ 2 kernel-attested refs (trace:, run:, friction:), at least one not a friction:, each failure-bearing; got {} attested ({non_friction} trace/run), {supplementary} event:/task: (supplementary only)", attested.len())));
    }
    for (_, c) in &checks {
        if let Some(p) = &c.problem {
            probs.push(p.clone());
        }
    }
    let good: Vec<&RefCheck> = attested.iter().filter(|(_, c)| c.problem.is_none()).map(|(_, c)| c).collect();
    let units: BTreeSet<&String> = good.iter().filter_map(|c| c.unit.as_ref()).collect();
    let any_eligible = good.iter().any(|c| c.eligible_friction);
    let any_div = good.iter().any(|c| c.divergent);
    if attested.len() >= 2 && non_friction >= 1 && !any_eligible && !any_div && units.len() < 2 {
        probs.push(Problem::new(codes::EVIDENCE_MISSING, "repro: the attested refs come from one task or session and no divergence pair; cite a second task/session or an eligible friction:"));
    }
    for (i, c) in predicts.iter().enumerate() {
        if let Err(e) = kernel_gate::evo::predicts::parse_clause(c) {
            probs.push(Problem::new(codes::SCHEMA, format!("predicts[{i}] '{c}': {e}")));
        }
    }

    // Targets: the pool tasks where the cited clusters were fatal (plus the agent's own).
    let mut targets: BTreeSet<String> = targets_in.into_iter().collect();
    for f in &frictions {
        if let Some(c) = index.iter().find(|c| &c.id == f) {
            targets.extend(c.fatal_tasks.iter().cloned());
        }
    }

    let mut p = Proposal {
        v: 1,
        id: "p-000000".into(),
        scope,
        frictions,
        hypothesis,
        change,
        repro,
        predicts,
        subject,
        compensates_for,
        prefix_change,
        lessons,
        k,
        s_measurable: tier_hint == "t2",
        tier_hint,
        targets: targets.into_iter().collect(),
        by: json!({"lane": filer.lane, "session": filer.session, "model": filer.model, "purpose": "turn"}),
        state: "proposed".into(),
        est: None,
        go: None,
        chosen: None,
        variants: vec![],
        created: fsx::now_rfc3339(),
        history: vec![],
        live: None,
        readmits: 0,
    };
    // The commit message dry run (the validator's message checks, §3.5.2 E0's trailer part).
    if probs.iter().all(|x| x.code != codes::SCHEMA)
        && let Err(errs) = kernel_gate::evo::validate_message_in(parser, &commit_message(&p, 1, &index), mode)
    {
        {
            for e in errs {
                if e.code == "evo.io" {
                    probs.push(Problem::new(codes::IO, e.detail));
                } else {
                    probs.push(Problem::new(codes::SCHEMA, format!("commit message: {}: {}", e.code, e.detail)));
                }
            }
        }
    }
    let _g = NUMBERING.lock().unwrap_or_else(|e| e.into_inner());
    let existing = all(home);
    let today = fsx::utc_day(fsx::now_s());
    if existing.iter().filter(|x| x.created.starts_with(&today)).count() as u64 >= PROPOSALS_DAY {
        probs.push(Problem::new(codes::QUOTA, format!("{PROPOSALS_DAY} proposals were filed today (UTC); try tomorrow")));
    }
    if !probs.is_empty() {
        return Err(probs);
    }
    let n = existing.iter().filter_map(|x| x.id[2..].parse::<u32>().ok()).max().unwrap_or(0) + 1;
    p.id = format!("p-{n:06}");
    let rec = p.transition("proposed", json!({"by": p.by}));
    std::fs::create_dir_all(dir(home)).map_err(|e| vec![Problem::new(codes::IO, e.to_string())])?;
    store(home, &p).map_err(|e| vec![Problem::new(codes::IO, e.to_string())])?;
    Ok((p, rec))
}

/// `evo abandon <p> <reason>`.
pub fn abandon(home: &Path, id: &str, reason: &str) -> Result<(Proposal, Value), Problem> {
    let mut p = load(home, id).map_err(|e| Problem::new(codes::IO, e.to_string()))?.ok_or_else(|| Problem::new(codes::STATE, format!("{id}: no such proposal")))?;
    if !p.is_open() || matches!(p.state.as_str(), states::PROMOTED | states::OPENING | states::OPEN) {
        return Err(Problem::new(codes::STATE, format!("{id} is {}", p.state)));
    }
    let r: String = reason.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(40).collect();
    let r = if r.is_empty() { "keep_base".to_string() } else { r };
    let rec = p.transition(&format!("abandoned:{r}"), json!({"reason": reason}));
    store(home, &p).map_err(|e| Problem::new(codes::IO, e.to_string()))?;
    Ok((p, rec))
}

/// `evo status [<p>]`: one record, or a summary of all.
pub fn status(home: &Path, id: Option<&str>) -> Result<Value, Problem> {
    match id {
        Some(id) => {
            let p = load(home, id).map_err(|e| Problem::new(codes::IO, e.to_string()))?.ok_or_else(|| Problem::new(codes::STATE, format!("{id}: no such proposal")))?;
            serde_json::to_value(p).map_err(|e| Problem::new(codes::IO, e.to_string()))
        }
        None => Ok(json!({
            "proposals": all(home).iter().map(|p| json!({"id": p.id, "state": p.state, "scope": p.scope, "frictions": p.frictions, "est": p.est, "go": p.go, "variants": p.variants.len(), "s_measurable": p.s_measurable, "subject": p.subject})).collect::<Vec<_>>(),
        })),
    }
}

// ---- change states (ADR-011 X0b: STRANDED-PROPOSAL and change-state recovery) ----------------

/// Proposal states (evolution-v2 §3.4.7; ADR-011 X0b adds `opening`, `open`, `rolled_back` and
/// `collateral`).
///
/// ```text
/// proposed → variants → screened → promoted ─┬→ verdict                      (gated: G4's final verdict)
///                          ↑                  └→ opening → open ─┬→ verdict      (free: clean, quiet, flagged)
///                          │                                     ├→ rolled_back  (free: direct; terminal)
///                          │                                     └→ rolled_back | collateral ─→ screened (once)
///                          └──── no verdict: a rollback, a failed promote job, a kernel restart
/// ```
/// The validator gates an evo commit only while its proposal is `screened`, so every path that
/// ends without a verdict returns there; that is the STRANDED-PROPOSAL fix.
pub mod states {
    pub const PROPOSED: &str = "proposed";
    pub const VARIANTS: &str = "variants";
    pub const SCREENED: &str = "screened";
    /// The chosen variant entered the promote pipeline.
    pub const PROMOTED: &str = "promoted";
    /// Free path: `change.open` is being sent for the chosen variant; the Root has not answered.
    pub const OPENING: &str = "opening";
    /// Free path: the Root appended `change_open`; the group's window runs.
    pub const OPEN: &str = "open";
    /// The group was rolled back as `direct` harm (or rolled back again after its re-admission).
    pub const ROLLED_BACK: &str = "rolled_back";
    /// Rolled back as collateral again after its re-admission.
    pub const COLLATERAL: &str = "collateral";
    /// A verdict: G4's final accept or reject (gated), or a `clean`, `quiet` or `flagged` close (free).
    pub const VERDICT: &str = "verdict";
}

/// Re-admissions after a rollback that was not a `direct` verdict (ADR-011 §3.5: once each; the
/// Root enforces the same rule on `change.open`).
pub const READMITS: u32 = 1;

/// Where the chosen variant is live (free path).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRef {
    pub group: u64,
    /// The `change_open` record's lineage seq.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// What happened to a proposal's chosen variant after `promote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeEv {
    /// Free path: `change.open` is about to be sent by the promote job `job`.
    Opening { job: Option<String> },
    /// Free path: the Root appended `change_open` (lineage `seq`) in group `group`.
    Open { group: u64, seq: Option<u64> },
    /// Free path: the Root closed the group (`change_close`, or `extend.window{state:"closed"}`).
    Closed { group: u64, outcome: String, attribution: Option<String> },
    /// Gated path: G4's final `accept` or `reject`.
    Verdict { verdict: String, reason: String },
    /// No verdict was reached: a rollback, a promote job that failed, a G4 lost to a kernel restart.
    NoVerdict { reason: String },
}

impl Proposal {
    fn chosen_sha(&self) -> Option<&str> {
        self.chosen.as_ref().map(|c| c.sha.as_str())
    }

    /// The groups this proposal was already open in (an older `change_open` of the same sha is not
    /// a re-admission's).
    fn past_groups(&self) -> BTreeSet<u64> {
        self.history.iter().filter(|h| h["state"] == states::OPEN).filter_map(|h| h["note"]["group"].as_u64()).collect()
    }

    /// Applies `ev`; returns the transition records (none when `ev` does not apply in this state).
    pub fn apply(&mut self, ev: &ChangeEv) -> Vec<Value> {
        use states::*;
        let s = self.state.clone();
        let sha = self.chosen_sha().map(str::to_string);
        match ev {
            ChangeEv::Opening { job } if matches!(s.as_str(), SCREENED | PROMOTED) => {
                self.live = None;
                vec![self.transition(OPENING, json!({"job": job, "sha": sha}))]
            }
            ChangeEv::Open { group, seq } if matches!(s.as_str(), SCREENED | PROMOTED | OPENING) => {
                self.live = Some(ChangeRef { group: *group, seq: *seq });
                vec![self.transition(OPEN, json!({"group": group, "seq": seq, "sha": sha}))]
            }
            ChangeEv::Closed { group, outcome, attribution } if s == OPEN && self.live.as_ref().is_some_and(|c| c.group == *group) => {
                let note = json!({"group": group, "outcome": outcome, "attribution": attribution});
                match outcome.as_str() {
                    "clean" | "quiet" | "flagged" => {
                        self.live = None;
                        vec![self.transition(VERDICT, note)]
                    }
                    "rolled_back" | "collateral" => {
                        self.live = None;
                        let to = if outcome == "collateral" { COLLATERAL } else { ROLLED_BACK };
                        let mut recs = vec![self.transition(to, note)];
                        if attribution.as_deref() != Some("direct") && self.readmits < READMITS {
                            self.readmits += 1;
                            recs.push(self.transition(SCREENED, json!({"reason": "readmit", "group": group, "readmits": self.readmits})));
                        }
                        recs
                    }
                    _ => vec![],
                }
            }
            ChangeEv::Verdict { verdict, reason } if s == PROMOTED => vec![self.transition(VERDICT, json!({"verdict": verdict, "reason": reason, "sha": sha}))],
            ChangeEv::NoVerdict { reason } if matches!(s.as_str(), PROMOTED | OPENING | OPEN) => {
                let group = self.live.take().map(|c| c.group);
                vec![self.transition(SCREENED, json!({"reason": reason, "from": s, "group": group}))]
            }
            _ => vec![],
        }
    }
}

/// Proposal read-modify-writes of [`on_sha`], [`on_group`] and [`resume`] (promote, G4 and event
/// threads).
static CHANGES: Mutex<()> = Mutex::new(());

fn apply_where(home: &Path, pick: impl Fn(&Proposal) -> bool, ev: &ChangeEv) -> Vec<Value> {
    let _g = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for mut p in all(home).into_iter().filter(|p| p.is_open() && pick(p)) {
        let recs = p.apply(ev);
        if !recs.is_empty() && store(home, &p).is_ok() {
            out.extend(recs);
        }
    }
    out
}

/// Applies `ev` to the proposals whose chosen variant is `sha`. Returns the transition records
/// (the caller logs each as a kernel-local `proposal` lineage line).
pub fn on_sha(home: &Path, sha: &str, ev: &ChangeEv) -> Vec<Value> {
    if sha.is_empty() {
        return vec![];
    }
    apply_where(home, |p| p.chosen_sha() == Some(sha), ev)
}

/// Applies `ev` to the proposals whose latest job note (`promoted` or `opening`) names `job`
/// (the promote job's terminal failure, which carries no sha).
pub fn on_job(home: &Path, job: &str, ev: &ChangeEv) -> Vec<Value> {
    apply_where(home, |p| p.history.iter().rev().find_map(|h| h["note"]["job"].as_str()) == Some(job), ev)
}

/// Applies `ev` to the proposals open in change group `group`.
pub fn on_group(home: &Path, group: u64, ev: &ChangeEv) -> Vec<Value> {
    apply_where(home, |p| p.live.as_ref().is_some_and(|c| c.group == group), ev)
}

/// The free path's callback (K2's `promote/admit.rs`): `state` is `opening`, `open`, or a close
/// outcome (`clean`, `quiet`, `flagged`, `rolled_back`, `collateral`); `note` carries `job`,
/// `group`, `seq` and `attribution` as far as they are known. A close without an attribution
/// (`extend.window` carries none) reads it from the Root's `change_close` in its published lineage
/// view ([`root_change_records`]) when there.
pub fn on_change_state(home: &Path, sha: &str, state: &str, note: Value) -> Vec<Value> {
    on_change_state_in(home, sha, state, note, None)
}

/// [`on_change_state`] with the Root's records given (`None`: read the view when needed).
pub fn on_change_state_in(home: &Path, sha: &str, state: &str, note: Value, root: Option<&[Value]>) -> Vec<Value> {
    let group = note["group"].as_u64();
    let ev = match (state, group) {
        ("opening", _) => ChangeEv::Opening { job: note["job"].as_str().map(str::to_string) },
        ("open", Some(g)) => ChangeEv::Open { group: g, seq: note["seq"].as_u64() },
        ("clean" | "quiet" | "flagged" | "rolled_back" | "collateral", Some(g)) => {
            let attribution = note["attribution"].as_str().map(str::to_string).or_else(|| {
                let read;
                let root = match root {
                    Some(r) => r,
                    None => {
                        read = root_change_records(home);
                        &read[..]
                    }
                };
                root.iter().rev().find(|r| r["kind"] == "change_close" && r["data"]["group"].as_u64() == Some(g)).and_then(|r| r["data"]["attribution"].as_str().map(str::to_string))
            });
            return on_group(home, g, &ChangeEv::Closed { group: g, outcome: state.to_string(), attribution });
        }
        _ => return vec![],
    };
    on_sha(home, sha, &ev)
}

/// X0b: after a kernel restart, every proposal past `screened` resumes from the Root's records
/// (`root`: its `change_open` and `change_close` records, `by:"root"`, in seq order):
/// - a `change_open` of the chosen sha in a group it was not open in before → `open` there; a
///   `change_close` of its group → that close (RF-01);
/// - otherwise nothing of it is live any more: `opening` (the change never opened) and `promoted`
///   (the restart ended the promote job, or the G4 job as `kernel_restart`) return to `screened`;
/// - `open` with its group not yet closed stays `open` (the Root's `extend.window` closes it).
pub fn resume(home: &Path, root: &[Value]) -> Vec<Value> {
    let _g = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    fn rec<'a>(root: &'a [Value], kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        root.iter().filter(move |r| r["kind"] == kind && r["by"] == "root")
    }
    let mut out = Vec::new();
    for mut p in all(home).into_iter().filter(|p| matches!(p.state.as_str(), states::PROMOTED | states::OPENING | states::OPEN)) {
        let mut recs = Vec::new();
        if p.state != states::OPEN {
            let past = p.past_groups();
            let sha = p.chosen_sha().unwrap_or("").to_string();
            let opened = rec(root, "change_open")
                .filter(|r| !sha.is_empty() && r["data"]["sha"].as_str() == Some(sha.as_str()))
                .filter_map(|r| Some((r["data"]["group"].as_u64()?, r["seq"].as_u64())))
                .filter(|(g, _)| !past.contains(g))
                .last();
            recs.extend(p.apply(&match opened {
                Some((group, seq)) => ChangeEv::Open { group, seq },
                None => ChangeEv::NoVerdict { reason: "kernel_restart".into() },
            }));
        }
        let open = p.live.as_ref().map(|c| c.group).filter(|_| p.state == states::OPEN);
        if let Some((g, d)) = open.and_then(|g| Some((g, rec(root, "change_close").map(|r| &r["data"]).filter(|d| d["group"].as_u64() == Some(g)).last()?))) {
            let ev = ChangeEv::Closed { group: g, outcome: d["outcome"].as_str().unwrap_or("").to_string(), attribution: d["attribution"].as_str().map(str::to_string) };
            recs.extend(p.apply(&ev));
        }
        if !recs.is_empty() && store(home, &p).is_ok() {
            out.extend(recs);
        }
    }
    out
}

/// The Root's `change_open` and `change_close` records from its published lineage view
/// (`INIT_HOME/root/lineage.jsonl`, root/root.md §3.10.6), `by:"root"` only, in file (seq) order.
/// Read line by line; only lines naming a `change_` kind are parsed.
pub fn root_change_records(home: &Path) -> Vec<Value> {
    use std::io::BufRead;
    let Ok(Some(f)) = fsx::open_nofollow(&home.join("root/lineage.jsonl")) else { return vec![] };
    std::io::BufReader::new(f)
        .split(b'\n')
        .map_while(Result::ok)
        .filter(|l| l.windows(15).any(|w| w == b"\"kind\":\"change_"))
        .filter_map(|l| serde_json::from_slice::<Value>(&l).ok())
        .filter(|r| r["by"] == "root" && matches!(r["kind"].as_str(), Some("change_open" | "change_close")))
        .collect()
}

#[cfg(test)]
mod change_tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/K1").join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(dir(&d)).unwrap();
        d
    }

    fn put(h: &Path, id: &str, state: &str, sha: &str) -> Proposal {
        let p = Proposal {
            v: 1,
            id: id.into(),
            scope: "inv".into(),
            frictions: vec!["f-000001".into()],
            hypothesis: "h".into(),
            change: "c".into(),
            repro: vec![],
            predicts: vec![],
            subject: "s".into(),
            compensates_for: "none".into(),
            prefix_change: false,
            lessons: vec![],
            k: 1,
            tier_hint: "t2".into(),
            s_measurable: true,
            targets: vec![],
            by: Value::Null,
            state: state.into(),
            est: Some(300),
            go: Some(true),
            chosen: Some(Chosen { k: 1, sha: sha.into(), aim: "better".into(), s_measurable: true }),
            variants: vec![],
            created: "2026-10-22T00:00:00.000Z".into(),
            history: vec![],
            live: None,
            readmits: 0,
        };
        store(h, &p).unwrap();
        p
    }

    fn state(h: &Path, id: &str) -> Proposal {
        load(h, id).unwrap().unwrap()
    }

    fn rec(seq: u64, by: &str, kind: &str, data: Value) -> Value {
        json!({"at": "2026-10-22T00:00:00.000Z", "by": by, "data": data, "kind": kind, "prev": "x", "seq": seq, "v": 1})
    }

    /// RF-01: a restart during an open group resumes from the Root's `change_open`: a proposal
    /// the kernel left `opening` becomes `open` in the Root's group (its seq kept), and a later
    /// close event finds it there. One whose group closed while the kernel was down takes that
    /// close; one whose `change.open` never reached the Root, and a gated `promoted` one whose G4
    /// job the restart ended (STRANDED-PROPOSAL), return to `screened`. A kernel-written record
    /// and an older group of the same sha are ignored.
    #[test]
    fn rf01_restart_resumes_from_change_open() {
        let h = home("rf01");
        put(&h, "p-000001", "opening", "aaa1");
        put(&h, "p-000002", "opening", "bbb2");
        put(&h, "p-000003", "opening", "ccc3");
        put(&h, "p-000004", "promoted", "ddd4");
        put(&h, "p-000005", "screened", "eee5");
        let root = vec![
            rec(10, "kernel:abcdef012345", "change_open", json!({"sha": "ccc3", "group": 10})),
            rec(11, "root", "change_open", json!({"sha": "aaa1", "group": 11, "scope": "t2"})),
            rec(12, "root", "change_open", json!({"sha": "bbb2", "group": 12, "scope": "t1"})),
            rec(13, "root", "change_close", json!({"group": 12, "outcome": "clean", "attribution": null, "trigger": null})),
        ];
        let recs = resume(&h, &root);
        let p1 = state(&h, "p-000001");
        assert_eq!((p1.state.as_str(), p1.live.clone()), ("open", Some(ChangeRef { group: 11, seq: Some(11) })));
        let p2 = state(&h, "p-000002");
        assert_eq!((p2.state.as_str(), p2.live.clone()), ("verdict", None));
        assert_eq!(p2.history.last().unwrap()["note"]["outcome"], "clean");
        let p3 = state(&h, "p-000003");
        assert_eq!((p3.state.as_str(), p3.history.last().unwrap()["note"]["reason"].as_str()), ("screened", Some("kernel_restart")));
        assert_eq!(state(&h, "p-000004").state, "screened", "the stranded gated proposal is gateable again");
        assert_eq!(state(&h, "p-000005").state, "screened");
        assert_eq!(recs.len(), 5, "{recs:?}");
        // A second resume changes nothing; the close event then finds p-000001 by its group.
        assert!(resume(&h, &root).is_empty());
        let recs = on_change_state_in(&h, "aaa1", "flagged", json!({"group": 11}), Some(&root[..]));
        assert_eq!(recs.len(), 1);
        assert_eq!(state(&h, "p-000001").state, "verdict");
        // A re-admitted proposal does not resume into its old group.
        let mut p = put(&h, "p-000006", "screened", "fff6");
        p.history.push(json!({"state": "open", "note": {"group": 20}}));
        p.state = "opening".into();
        store(&h, &p).unwrap();
        let root2 = vec![rec(20, "root", "change_open", json!({"sha": "fff6", "group": 20}))];
        resume(&h, &root2);
        assert_eq!(state(&h, "p-000006").state, "screened", "group 20 was the first attempt's");
        let _ = std::fs::remove_dir_all(&h);
    }

    /// RF-02: a rollback without a verdict returns the proposal to `screened`. Gated: a stale
    /// provisional or `initctl rollback` (no G4 verdict); a G4 verdict is terminal. Free: a
    /// `collateral` or `suspect` (or unattributed veto) rollback is re-admitted once, as the Root
    /// allows; a `direct` rollback and a second non-verdict rollback are terminal.
    #[test]
    fn rf02_rollback_without_verdict_returns_to_screened() {
        let h = home("rf02");
        put(&h, "p-000001", "promoted", "aaa1");
        let r = on_sha(&h, "aaa1", &ChangeEv::NoVerdict { reason: "provisional_stale".into() });
        assert_eq!(r.len(), 1);
        let p = state(&h, "p-000001");
        assert_eq!((p.state.as_str(), p.history[0]["note"]["reason"].as_str()), ("screened", Some("provisional_stale")));
        assert!(on_sha(&h, "aaa1", &ChangeEv::NoVerdict { reason: "again".into() }).is_empty(), "screened stays");
        put(&h, "p-000002", "promoted", "bbb2");
        on_sha(&h, "bbb2", &ChangeEv::Verdict { verdict: "reject".into(), reason: "worse".into() });
        assert_eq!(state(&h, "p-000002").state, "verdict");
        assert!(on_sha(&h, "bbb2", &ChangeEv::NoVerdict { reason: "rollback".into() }).is_empty(), "a verdict is terminal");
        assert!(!state(&h, "p-000002").is_open());

        // A promote job that failed (no verdict): found by the job its `promoted` note names.
        let mut p = put(&h, "p-000008", "screened", "hhh8");
        p.transition("promoted", json!({"job": "j-000031", "sha": "hhh8"}));
        store(&h, &p).unwrap();
        assert!(on_job(&h, "j-000099", &ChangeEv::NoVerdict { reason: "promote_failed:x".into() }).is_empty());
        assert_eq!(on_job(&h, "j-000031", &ChangeEv::NoVerdict { reason: "promote_failed:inner_g2".into() }).len(), 1);
        assert_eq!(state(&h, "p-000008").state, "screened");

        // Free path through the callback.
        put(&h, "p-000003", "promoted", "ccc3");
        on_change_state_in(&h, "ccc3", "opening", json!({"job": "j-000009"}), Some(&[][..]));
        on_change_state_in(&h, "ccc3", "open", json!({"group": 31, "seq": 31}), Some(&[][..]));
        assert_eq!(state(&h, "p-000003").state, "open");
        assert!(abandon(&h, "p-000003", "x").is_err(), "a live proposal cannot be abandoned");
        on_change_state_in(&h, "ccc3", "collateral", json!({"group": 31}), Some(&[][..]));
        let p = state(&h, "p-000003");
        assert_eq!((p.state.as_str(), p.readmits), ("screened", 1));
        assert_eq!(p.history.iter().map(|x| x["state"].as_str().unwrap()).collect::<Vec<_>>(), ["opening", "open", "collateral", "screened"]);
        // Re-admitted, then rolled back again (suspect): no second re-admission.
        on_change_state_in(&h, "ccc3", "open", json!({"group": 40}), Some(&[][..]));
        on_change_state_in(&h, "ccc3", "rolled_back", json!({"group": 40, "attribution": "suspect"}), Some(&[][..]));
        let p = state(&h, "p-000003");
        assert_eq!((p.state.as_str(), p.is_open()), ("rolled_back", false));

        // `direct` is a verdict on the change: terminal. The attribution comes from the Root's record.
        put(&h, "p-000004", "promoted", "ddd4");
        on_change_state_in(&h, "ddd4", "open", json!({"group": 50}), Some(&[][..]));
        let root = [rec(51, "root", "change_close", json!({"group": 50, "outcome": "rolled_back", "attribution": "direct", "trigger": "H5b"}))];
        on_change_state_in(&h, "ddd4", "rolled_back", json!({"group": 50}), Some(&root[..]));
        assert_eq!(state(&h, "p-000004").state, "rolled_back");
        // An unattributed rollback (an H12 veto) is re-admitted once.
        put(&h, "p-000005", "promoted", "eee5");
        on_change_state_in(&h, "eee5", "open", json!({"group": 60}), Some(&[][..]));
        on_change_state_in(&h, "eee5", "rolled_back", json!({"group": 60}), Some(&[][..]));
        assert_eq!(state(&h, "p-000005").state, "screened");
        // Old records (no `change`, no `readmits`) still load.
        let old = r#"{"v":1,"id":"p-000007","scope":"inv","frictions":[],"hypothesis":"h","change":"c","repro":[],"predicts":[],"subject":"s","compensates_for":"n","prefix_change":false,"lessons":[],"k":1,"tier_hint":"t2","s_measurable":true,"targets":[],"by":null,"state":"promoted","est":null,"go":null,"chosen":null,"variants":[],"created":"x","history":[]}"#;
        let p: Proposal = serde_json::from_str(old).unwrap();
        assert_eq!((p.live, p.readmits), (None, 0));
        let _ = std::fs::remove_dir_all(&h);
    }
}
