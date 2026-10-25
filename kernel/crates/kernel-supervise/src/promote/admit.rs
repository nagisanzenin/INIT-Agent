//! ADR-011 §2 on the kernel side (lane K2-ADMIT, X2): the free path's admission.
//!
//! ```text
//! promote evo/<id>   (charter extend.mode = "free")
//!   sync:  fetch · archive ref · kernel checks · validator (free matrix) · scope (no G4)
//!   job:   A1 = G0 + G1 (t1, t2, dep) · A2 = G2 (t2, dep)
//!          · store.put {body binary, ext components, t0 blob} · change.open → {seq, group, snapshot}
//!          · go live: overlapped handoff (t2, dep) or data-only reload (t0, t1)
//!          · gate record v5 {accept:"free", stages A0–A5} · A3 asynchronously (veto only)
//!   later: Root extend.window{closed, clean|quiet|flagged} → the local LKG commit (supervisor)
//!          Root extend.rollback{to, groups} → rollback to LKG at the next step boundary
//! ```
//!
//! A0, A4 and A5 are the Root's; a Root refusal leaves nothing live and maps to an `E_*` code
//! ([`refusal`]). The kernel never reports `promotion{lkg:true}` on this path: the Root moves `lkg`
//! itself on `change_close`, and the kernel's own LKG (bin, refs, `lkg.json`) follows that event.

use super::t0blob;
use super::Env;
use crate::link::{LinkError, RootLink};
use init_proto::envelope::ErrorBody;
use init_proto::errors::ErrorCode;
use init_proto::root_link::{ChangeVetoArgs, KernelRequest, StoreKind, StorePutArgs};
use kernel_gate::evo::{ChangeType, ExtendMode, Validated};
use kernel_gate::scope::Scope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `change.open` can wait for A4 (two `check` boots, ≤ 90 s each) and the system snapshot.
pub const CHANGE_OPEN_TIMEOUT: Duration = Duration::from_secs(600);

/// The Root half of the free path. Production: [`RootExtend`] over the control link; tests: the
/// fake Roots behind the same link.
pub trait ExtendLink: Send + Sync {
    /// The effective charter `extend.mode`, as the Root last reported it (`hello`, `ceilings`).
    fn mode(&self) -> ExtendMode;
    /// `store.put{kind, name?, sha256, bytes}` with the file's fd; returns the sha256.
    fn store_put(&self, kind: StoreKind, name: Option<&str>, path: &Path) -> Result<String, ErrorBody>;
    /// `change.open{…}` → `{seq, group, snapshot, window}`. A Root refusal is `Err` carrying the
    /// Root's dotted code (`extend.*`, `root.*`).
    fn change_open(&self, args: Value) -> Result<Value, ErrorBody>;
    /// `change.veto{group, reason}` (H12).
    fn change_veto(&self, group: u64, reason: &str) -> Result<(), String>;
}

/// [`ExtendLink`] over the control link. `free` is the supervisor's copy of the Root's
/// `extend.mode` ([`crate::supervisor::Services::extend_free`]).
pub struct RootExtend {
    pub link: Arc<RootLink>,
    pub free: Arc<AtomicBool>,
}

fn link_err(e: LinkError) -> ErrorBody {
    match e {
        LinkError::Root(b) => b,
        LinkError::Timeout => ErrorBody::new(ErrorCode::Root, "the Root did not answer in time").with_data(&json!({"reason": "stale"})),
        LinkError::Lost => ErrorBody::new(ErrorCode::Root, "the control link is down").with_data(&json!({"reason": "link_down"})),
        LinkError::Other(m) => ErrorBody::new(ErrorCode::Internal, m),
    }
}

impl ExtendLink for RootExtend {
    fn mode(&self) -> ExtendMode {
        if self.free.load(Ordering::SeqCst) { ExtendMode::Free } else { ExtendMode::Gated }
    }
    fn store_put(&self, kind: StoreKind, name: Option<&str>, path: &Path) -> Result<String, ErrorBody> {
        store_put_via(&self.link, kind, name, path)
    }
    fn change_open(&self, args: Value) -> Result<Value, ErrorBody> {
        self.link.call(&KernelRequest::ChangeOpen(args), CHANGE_OPEN_TIMEOUT).map_err(link_err)
    }
    fn change_veto(&self, group: u64, reason: &str) -> Result<(), String> {
        let reason: String = reason.chars().filter(|c| !c.is_control()).take(200).collect();
        self.link.call(&KernelRequest::ChangeVeto(ChangeVetoArgs { group, reason }), self.link.request_timeout).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// `store.put` of one file over a control link.
pub fn store_put_via(link: &RootLink, kind: StoreKind, name: Option<&str>, path: &Path) -> Result<String, ErrorBody> {
    use std::os::fd::AsFd;
    let sha256 = crate::fsx::sha256_file(path).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("{}: {e}", path.display())))?;
    let f = std::fs::File::open(path).map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("{}: {e}", path.display())))?;
    let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
    let put = KernelRequest::StorePut(StorePutArgs { kind, name: name.map(str::to_string), sha256: sha256.clone(), bytes, job: None });
    link.call_with_fd(&put, f.as_fd(), crate::link::STORE_TIMEOUT).map_err(link_err)?;
    Ok(sha256)
}

impl Env {
    /// The free path's link when the Root runs this kernel under `extend.mode = "free"`.
    pub fn free_link(&self) -> Option<Arc<dyn ExtendLink>> {
        self.extend.clone().filter(|l| l.mode() == ExtendMode::Free)
    }
}

// ---- the validator and scope on the free path ------------------------------------------------

/// `evo::validate` under `mode` (ADR-011: the free path's relaxed matrix), on lineage.
pub fn validate(env: &Env, parent: &str, candidate: &str, mode: ExtendMode) -> Result<Validated, Vec<kernel_gate::evo::ValidationError>> {
    let repo = kernel_gate::evo::repo::GitCliRepo::new(&env.cfg.supervise.git, &env.home)
        .with_trace_lookup(Box::new(kernel_evo::trace::lookup(env.home.join("vault/traces/v1"))));
    kernel_gate::evo::validate::validate_in(&repo, &super::stages::trailer_parser(env), parent, candidate, mode)
}

// ---- refusal codes → E_* ---------------------------------------------------------------------

/// A Root refusal of `change.open` as the kernel's `E_*` (ADR-011 X2; table in build/status/K2.md).
/// `data.root_code` always names the Root's code; `details` keep its message.
pub fn refusal(e: &ErrorBody) -> ErrorBody {
    let root_code = e.code.as_str().to_string();
    let msg = format!("change.open refused: {root_code}: {}", e.message);
    let mut data = json!({"root_code": root_code});
    if let Some(d) = e.data.as_ref().filter(|d| d.is_object()) {
        data["root_data"] = d.clone();
    }
    let (code, extra) = match root_code.as_str() {
        "extend.budget" => (ErrorCode::Quota, json!({"quota": "extend_budget"})),
        "extend.bloat" => (ErrorCode::Quota, json!({"quota": "extend_bloat"})),
        "extend.archive_debt" => (ErrorCode::Quota, json!({"quota": "extend_archive_debt"})),
        "extend.quarantine_debt" => (ErrorCode::Quota, json!({"quota": "extend_quarantine_debt"})),
        "extend.readmit" => (ErrorCode::Quota, json!({"quota": "extend_readmit"})),
        // ADR-011 §7.4: ask the owner with `initctl grant dep '[{name, version, checksum}…]'`.
        "extend.dep_grant" => (ErrorCode::Quota, json!({"quota": "extend_dep_grant"})),
        "extend.snapshot_too_large" => (ErrorCode::TooLarge, json!({})),
        "extend.rollback_incompatible" => (ErrorCode::GateFailed, json!({"stage": "A4", "reason": "rollback_incompatible", "report": null})),
        "root.boundary" => (ErrorCode::GateFailed, json!({"stage": "A0", "reason": "boundary", "report": null})),
        "root.quota" => (ErrorCode::Disk, json!({})),
        "root.level" => (ErrorCode::Level, json!({})),
        "root.busy" => (ErrorCode::Busy, json!({})),
        // The kernel's own link failures are already E_ROOT / E_INTERNAL.
        c if !c.contains('.') => return e.clone(),
        _ => (ErrorCode::Root, json!({"reason": "refused"})),
    };
    if let (Some(d), Some(x)) = (data.as_object_mut(), extra.as_object()) {
        d.extend(x.clone());
    }
    let mut out = ErrorBody::new(code, msg);
    out.data = Some(data);
    out.details = vec![init_proto::envelope::ErrorDetail { code: root_code, detail: kernel_gate::record::cap(&e.message, 1000) }];
    out
}

// ---- grant requests (ADR-011 §7.4) -----------------------------------------------------------

/// `initctl grant`: `grant.request{kind, value, duration_h?, why}` to the Root → `{id, status}`.
/// The Root stores and alerts it; only the owner decides.
pub fn grant_request(link: &RootLink, args: Value) -> Result<Value, ErrorBody> {
    link.call(&KernelRequest::GrantRequest(args), link.request_timeout).map_err(|e| match e {
        LinkError::Root(b) => grant_refusal(&b),
        other => link_err(other),
    })
}

/// A Root refusal of `grant.request` as the kernel's `E_*`: `root.args` (not a grant kind, a value
/// out of shape or bounds) → `E_ARGS`; `root.rate` (3 per rolling 24 h) → `E_RATE`; any other
/// `root.*` → `E_ROOT{reason:"refused"}`. `data.root_code` always names the Root's code.
pub fn grant_refusal(e: &ErrorBody) -> ErrorBody {
    let root_code = e.code.as_str().to_string();
    let code = match root_code.as_str() {
        "root.args" => ErrorCode::Args,
        "root.rate" => ErrorCode::Rate,
        c if !c.contains('.') => return e.clone(),
        _ => ErrorCode::Root,
    };
    let mut data = json!({"root_code": root_code});
    if code == ErrorCode::Root {
        data["reason"] = json!("refused");
    }
    let mut out = ErrorBody::new(code, format!("grant.request refused: {root_code}: {}", e.message));
    out.data = Some(data);
    out
}

// ---- the change group table (state/kernel/extend/groups.json) --------------------------------

/// One admitted change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRec {
    pub sha: String,
    pub from_sha: String,
    pub scope: String,
    pub job: String,
    /// The Root's `change_open` seq.
    pub seq: u64,
}

/// One change group as the kernel knows it (the Root's lineage stays authoritative).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRec {
    pub group: u64,
    pub changes: Vec<ChangeRec>,
    /// `open` | `closed`.
    pub state: String,
    pub outcome: Option<String>,
    pub snapshot: Option<String>,
}

impl GroupRec {
    /// The version before the group's first change: the rollback target of this group.
    pub fn parent(&self) -> Option<&str> {
        self.changes.first().map(|c| c.from_sha.as_str())
    }
    /// The group's newest version.
    pub fn head(&self) -> Option<&str> {
        self.changes.last().map(|c| c.sha.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Groups {
    pub v: u32,
    pub groups: Vec<GroupRec>,
}

/// Closed groups kept in the table.
const KEEP_CLOSED: usize = 64;

static GROUPS_LOCK: Mutex<()> = Mutex::new(());

pub fn groups_path(home: &Path) -> PathBuf {
    home.join("state/kernel/extend/groups.json")
}

impl Groups {
    pub fn load(home: &Path) -> Groups {
        std::fs::read(groups_path(home)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Groups { v: 1, groups: vec![] })
    }

    fn save(&mut self, home: &Path) {
        let mut closed = self.groups.iter().filter(|g| g.state != "open").count();
        self.groups.retain(|g| {
            if g.state != "open" && closed > KEEP_CLOSED {
                closed -= 1;
                return false;
            }
            true
        });
        let p = groups_path(home);
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(home));
        if let Err(e) = crate::fsx::write_json(&p, self) {
            crate::klog::log("gate", &format!("state/kernel/extend/groups.json: {e}"));
        }
    }

    /// Read-modify-write under the table lock.
    pub fn update<T>(home: &Path, f: impl FnOnce(&mut Groups) -> T) -> T {
        let _g = GROUPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = Groups::load(home);
        let t = f(&mut g);
        g.save(home);
        t
    }

    pub fn get(&self, group: u64) -> Option<&GroupRec> {
        self.groups.iter().find(|g| g.group == group)
    }

    pub fn open_groups(&self) -> impl Iterator<Item = &GroupRec> {
        self.groups.iter().filter(|g| g.state == "open")
    }

    /// ADR-013 G2: `from` reaches `to` going back through admitted changes that are all `t0` (no
    /// binary, no kept set changed on the way), so a rollback to `to` is data-only.
    pub fn data_only_back(&self, from: &str, to: &str) -> bool {
        let mut cur = from.to_string();
        for _ in 0..256 {
            if cur == to {
                return true;
            }
            let Some(c) = self.groups.iter().rev().flat_map(|g| g.changes.iter().rev()).find(|c| c.sha == cur) else { return false };
            if c.scope != "t0" || c.from_sha.is_empty() {
                return false;
            }
            cur = c.from_sha.clone();
        }
        false
    }

    /// The group whose change is the running version `sha` (the newest that holds it).
    pub fn holding(&self, sha: &str) -> Option<&GroupRec> {
        self.groups.iter().rev().find(|g| g.changes.iter().any(|c| c.sha == sha))
    }

    /// The rollback target of `groups` (the parent of the oldest one the kernel knows).
    pub fn parent_of(&self, groups: &[u64]) -> Option<String> {
        groups.iter().filter_map(|g| self.get(*g)).min_by_key(|g| g.group).and_then(|g| g.parent().map(str::to_string))
    }
}

/// Record an admitted change in its group.
pub fn record_open(home: &Path, group: u64, snapshot: Option<String>, c: ChangeRec) {
    Groups::update(home, |t| {
        if let Some(g) = t.groups.iter_mut().find(|g| g.group == group) {
            g.changes.push(c);
            g.state = "open".into();
            if g.snapshot.is_none() {
                g.snapshot = snapshot;
            }
        } else {
            t.groups.push(GroupRec { group, changes: vec![c], state: "open".into(), outcome: None, snapshot });
        }
    });
}

/// Record a close (`extend.window{closed, outcome}`); returns the group as it was.
pub fn record_close(home: &Path, group: u64, outcome: &str) -> Option<GroupRec> {
    Groups::update(home, |t| {
        let g = t.groups.iter_mut().find(|g| g.group == group)?;
        g.state = "closed".into();
        g.outcome = Some(outcome.to_string());
        Some(g.clone())
    })
}

// ---- change states for the proposal records (X0b, lane K1-FIXES) -----------------------------

/// A change's state for the engine's proposal records (X0b): `opening`, `open`, `clean`, `quiet`,
/// `flagged`, `rolled_back`, `collateral`. Each transition record is logged locally as `proposal`.
pub fn change_state(home: &Path, lineage: &dyn super::GateLineage, sha: &str, state: &str, note: Value) {
    // kernel_evo::proposal::on_change_state (X0b, c0a4348).
    for rec in kernel_evo::proposal::on_change_state(home, sha, state, note) {
        lineage.local("proposal", rec);
    }
}

// ---- the admission itself --------------------------------------------------------------------

/// What goes live, for `change.open`.
pub struct Candidate<'a> {
    pub job: &'a str,
    pub sha: &'a str,
    pub from_sha: &'a str,
    pub scope: Scope,
    /// The body binary that will run (t2/dep: the built candidate; t0/t1: `bin/init-body`).
    pub binary: &'a Path,
    /// The kept-component set that will run (`bin/ext.d/<set>`), if any.
    pub ext: Option<(&'a str, &'a [(String, String)])>,
    pub validated: &'a Validated,
    /// `cache/gate/<job>/`: the `t0` blob is written here before its `store.put`.
    pub cache: &'a Path,
}

/// The Root's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub group: u64,
    pub seq: u64,
    pub snapshot: Option<String>,
    pub window: Value,
    pub t0: String,
    pub body_binary: String,
}

/// The `Cap:` trailers as `change.open.caps` entries. `op`: `remove` on a `retire` commit, `modify`
/// when the parent's `self.json` names the id, else `add`. Versions are left to the Root (it
/// computes `skill/*`, `agent/*`, `prompt/*` and `ext/*` itself; the zone kinds have none here).
pub fn caps_of(env: &Env, v: &Validated) -> Vec<Value> {
    let parent = parent_components(env, &v.parent);
    v.trailers
        .caps
        .iter()
        .map(|id| {
            let op = if v.change_type == ChangeType::Retire {
                "remove"
            } else if parent.iter().any(|p| p == id) {
                "modify"
            } else {
                "add"
            };
            json!({"id": id, "op": op, "prev_ver": null, "ver": null})
        })
        .collect()
}

/// The registry crates a `dep` change brings in, from both lockfiles of `from` and `to`
/// ([`kernel_gate::scope::dep_crates`]), as `[{name, version, checksum}]`.
fn dep_crates(env: &Env, from: &str, to: &str) -> Result<Vec<Value>, String> {
    let git = env.git();
    let blob = |sha: &str, path: &str| git.path_id(sha, path).ok().flatten().and_then(|o| git.read_blob(&o).ok()).map(|b| String::from_utf8_lossy(&b).into_owned());
    let (bp, bc, ep, ec) = (blob(from, "Cargo.lock"), blob(to, "Cargo.lock"), blob(from, "ext/Cargo.lock"), blob(to, "ext/Cargo.lock"));
    let locks = kernel_gate::scope::Lockfiles { body_parent: bp.as_deref(), body_candidate: bc.as_deref(), ext_parent: ep.as_deref(), ext_candidate: ec.as_deref() };
    let crates = kernel_gate::scope::dep_crates(&locks).map_err(|e| e.0)?;
    Ok(crates.into_iter().map(|(name, version, checksum)| json!({"name": name, "version": version, "checksum": checksum})).collect())
}

/// Component ids of `sha`'s committed `self.json` (an object keyed by id, or an array of `{id}`).
fn parent_components(env: &Env, sha: &str) -> Vec<String> {
    let git = env.git();
    let Some(v) = git.path_id(sha, "self.json").ok().flatten().and_then(|o| git.read_blob(&o).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
        return vec![];
    };
    match &v["components"] {
        Value::Object(m) => m.keys().cloned().collect(),
        Value::Array(a) => a.iter().filter_map(|c| c["id"].as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// §2.3 steps 2–3: store the candidate's blobs, then `change.open`. On `Ok` the change may go live;
/// on `Err` nothing is live and the error is the kernel's `E_*` ([`refusal`]).
pub fn open(env: &Env, link: &dyn ExtendLink, c: &Candidate<'_>) -> Result<Admitted, ErrorBody> {
    let internal = |m: String| ErrorBody::new(ErrorCode::Internal, m);
    // The t0 blob of the candidate commit.
    let tar = t0blob::build(&env.git(), c.sha).map_err(|m| ErrorBody::new(ErrorCode::TooLarge, format!("t0 blob: {m}")))?;
    std::fs::create_dir_all(c.cache).map_err(|e| internal(format!("{}: {e}", c.cache.display())))?;
    let t0_path = c.cache.join("t0.tar");
    crate::fsx::write_atomic(&t0_path, &tar).map_err(|e| internal(format!("{}: {e}", t0_path.display())))?;
    // A0 needs every blob in the Root's store with the claimed digest.
    let body_binary = link.store_put(StoreKind::InitBody, None, c.binary).map_err(|e| refusal(&e))?;
    let mut components = Vec::new();
    let mut set = Value::Null;
    if let Some((id, comps)) = c.ext {
        let dir = env.home.join("bin/ext.d").join(id);
        for (name, sha256) in comps {
            let got = link.store_put(StoreKind::Ext, Some(name), &dir.join(format!("{name}.wasm"))).map_err(|e| refusal(&e))?;
            if &got != sha256 {
                return Err(internal(format!("bin/ext.d/{id}/{name}.wasm changed while it was stored")));
            }
            components.push(json!({"name": name, "sha256": sha256}));
        }
        set = json!(id);
    }
    let t0 = link.store_put(StoreKind::T0, None, &t0_path).map_err(|e| refusal(&e))?;
    let mut args = json!({
        "sha": c.sha,
        "from_sha": c.from_sha,
        "scope": c.scope.change_scope(),
        "candidate": {"body": {"binary": body_binary, "sha": c.sha, "t0": t0}, "ext": {"components": components, "set": set}},
        "caps": caps_of(env, c.validated),
        "prefix_change": c.validated.trailers.prefix_change,
        // ADR-013: the commit's `Measure: card:<id>[ gate]` requests; the Root queues those readings at admission
        "measure": c.validated.trailers.measure.iter().filter(|m| matches!(m, kernel_gate::evo::Measure::Card(..))).map(ToString::to_string).collect::<Vec<_>>(),
    });
    if c.scope == Scope::Dep {
        // ADR-011 §7.4: A5 admits a `dep` change only when a live owner grant covers this exact set.
        args["crates"] = json!(dep_crates(env, c.from_sha, c.sha).map_err(|m| internal(format!("the dep change's crate set: {m}")))?);
    }
    let r = link.change_open(args).map_err(|e| refusal(&e))?;
    let group = r["group"].as_u64().ok_or_else(|| internal(format!("change.open answered without a group: {r}")))?;
    let seq = r["seq"].as_u64().unwrap_or(0);
    let snapshot = r["snapshot"].as_str().map(str::to_string);
    record_open(&env.home, group, snapshot.clone(), ChangeRec { sha: c.sha.into(), from_sha: c.from_sha.into(), scope: c.scope.change_scope().into(), job: c.job.into(), seq });
    Ok(Admitted { group, seq, snapshot, window: r["window"].clone(), t0, body_binary })
}

// ---- A3 (asynchronous, veto only) ------------------------------------------------------------

/// What A3 needs once the change is live (X0c, lane K1-FIXES, owns the body of [`a3_after_live`]).
#[derive(Clone, Debug)]
pub struct A3Job {
    pub job: String,
    pub group: u64,
    pub sha: String,
    pub scope: Scope,
    /// The candidate's built binary (t2/dep), and the job cache holding it.
    pub built: Option<super::stages::Built>,
    pub validated: Validated,
    /// G0's stage record (advisory clippy and body tests).
    pub g0: Value,
}

/// A3 (ADR-011 §2.1): after the change is live, the body's own tests (from G0's record) and, for
/// `t2` and `dep`, G3 (scripted self-edit); the first failure is a zone veto ([`veto`]). Runs on its
/// own thread through [`super::stages::a3`] (X0c, lane K1-FIXES), under its own job id `<job>-a3`;
/// the promote job's cache (`cache/gate/<job>/`, which holds the built binary) is removed after it.
pub fn a3_after_live(env: &Arc<Env>, j: A3Job) {
    let env = env.clone();
    let _ = std::thread::Builder::new().name(format!("a3-{}", j.job)).spawn(move || {
        let binary = j.built.as_ref().map(|b| b.binary.clone()).unwrap_or_else(|| PathBuf::from("/nonexistent/a3-no-binary"));
        let (group, scope) = (j.group, j.scope.change_scope());
        let out = super::stages::a3(&env, &format!("{}-a3", j.job), scope, &j.sha, &binary, &j.g0, |r| veto(&env, group, r));
        let line = json!({"job": j.job, "group": group, "sha": j.sha, "scope": scope, "pass": out.pass, "reason": out.reason,
                          "veto": out.veto.as_ref().map(|v| v.as_ref().map(|_| "sent").unwrap_or_else(|e| e.as_str())), "record": out.record});
        env.lineage.local("a3", line);
        super::pipeline::cleanup_job(&env, &j.job);
    });
}

/// `change.veto{group, reason}` (H12): the zone asks the Root to roll the group back.
pub fn veto(env: &Env, group: u64, reason: &str) -> Result<(), String> {
    let link = env.extend.as_ref().ok_or("no extend link (gated kernel)")?;
    link.change_veto(group, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_err(code: &str) -> ErrorBody {
        ErrorBody::new(ErrorCode::parse(code), format!("{code} (test)"))
    }

    /// X2: every Root refusal of `change.open` maps to one `E_*`, keeping the Root's code.
    #[test]
    fn refusal_codes_map_to_e_codes() {
        let table = [
            ("extend.budget", "E_QUOTA", Some("extend_budget")),
            ("extend.bloat", "E_QUOTA", Some("extend_bloat")),
            ("extend.archive_debt", "E_QUOTA", Some("extend_archive_debt")),
            ("extend.quarantine_debt", "E_QUOTA", Some("extend_quarantine_debt")),
            ("extend.readmit", "E_QUOTA", Some("extend_readmit")),
            ("extend.dep_grant", "E_QUOTA", Some("extend_dep_grant")),
            ("extend.snapshot_too_large", "E_TOO_LARGE", None),
            ("extend.rollback_incompatible", "E_GATE_FAILED", None),
            ("root.boundary", "E_GATE_FAILED", None),
            ("root.quota", "E_DISK", None),
            ("root.level", "E_LEVEL", None),
            ("root.busy", "E_BUSY", None),
            ("root.store", "E_ROOT", None),
            ("root.args", "E_ROOT", None),
            ("root.internal", "E_ROOT", None),
        ];
        for (root, want, quota) in table {
            let e = refusal(&root_err(root));
            assert_eq!(e.code.as_str(), want, "{root}");
            let d = e.data.clone().unwrap();
            assert_eq!(d["root_code"], root);
            assert_eq!(d["quota"].as_str(), quota, "{root}");
            assert_eq!(e.details[0].code, root);
            // A quota's data deserializes as the registered quota names.
            if let Some(q) = quota {
                let _: init_proto::errors::QuotaName = serde_json::from_value(json!(q)).unwrap();
            }
        }
        let a4 = refusal(&root_err("extend.rollback_incompatible")).data.unwrap();
        assert_eq!((a4["stage"].as_str(), a4["reason"].as_str()), (Some("A4"), Some("rollback_incompatible")));
        assert_eq!(refusal(&root_err("root.boundary")).data.unwrap()["stage"], "A0");
        assert_eq!(refusal(&root_err("root.unknown")).data.unwrap()["reason"], "refused");
        // A kernel-side failure (no dotted code) passes through unchanged.
        let own = ErrorBody::new(ErrorCode::Root, "the control link is down");
        assert_eq!(refusal(&own), own);
    }

    /// ADR-011 §7.4: `initctl grant`'s refusals.
    #[test]
    fn grant_refusals_map_to_e_codes() {
        for (root, want) in [("root.args", "E_ARGS"), ("root.rate", "E_RATE"), ("root.mode", "E_ROOT"), ("root.internal", "E_ROOT")] {
            let e = grant_refusal(&root_err(root));
            assert_eq!((e.code.as_str(), e.data.as_ref().unwrap()["root_code"].as_str()), (want, Some(root)));
            assert!(e.message.contains(root));
        }
        assert_eq!(grant_refusal(&root_err("root.mode")).data.unwrap()["reason"], "refused");
        let own = ErrorBody::new(ErrorCode::Root, "the control link is down");
        assert_eq!(grant_refusal(&own), own);
    }

    #[test]
    fn group_table_parents_and_closes() {
        let dir = crate::testutil::test_dir("k2-groups");
        let home = dir.join("home");
        record_open(&home, 3, Some("3".into()), ChangeRec { sha: "b".into(), from_sha: "a".into(), scope: "t0".into(), job: "j-000001".into(), seq: 10 });
        record_open(&home, 3, None, ChangeRec { sha: "c".into(), from_sha: "b".into(), scope: "t1".into(), job: "j-000002".into(), seq: 11 });
        record_open(&home, 4, None, ChangeRec { sha: "d".into(), from_sha: "c".into(), scope: "t2".into(), job: "j-000003".into(), seq: 12 });
        let t = Groups::load(&home);
        assert_eq!((t.get(3).unwrap().parent(), t.get(3).unwrap().head(), t.get(3).unwrap().snapshot.as_deref()), (Some("a"), Some("c"), Some("3")));
        assert_eq!(t.parent_of(&[4]).as_deref(), Some("c"));
        assert_eq!(t.parent_of(&[4, 3]).as_deref(), Some("a"));
        assert_eq!(t.parent_of(&[9]), None);
        // ADR-013 G2: back from b to a is t0 only; from c (t1) or d (t2) it is not data-only
        assert!(t.data_only_back("b", "a") && t.data_only_back("a", "a"));
        assert!(!t.data_only_back("c", "a") && !t.data_only_back("d", "c") && !t.data_only_back("b", "z"));
        assert_eq!((t.holding("c").map(|g| g.group), t.holding("d").map(|g| g.group), t.holding("a").map(|g| g.group)), (Some(3), Some(4), None));
        assert_eq!(t.open_groups().count(), 2);
        let g = record_close(&home, 3, "clean").unwrap();
        assert_eq!((g.state.as_str(), g.outcome.as_deref()), ("closed", Some("clean")));
        assert_eq!(Groups::load(&home).open_groups().map(|g| g.group).collect::<Vec<_>>(), vec![4]);
        assert!(record_close(&home, 99, "clean").is_none());
        for i in 0..80 {
            record_open(&home, 100 + i, None, ChangeRec::default());
            record_close(&home, 100 + i, "quiet");
        }
        let t = Groups::load(&home);
        assert_eq!(t.groups.iter().filter(|g| g.state != "open").count(), KEEP_CLOSED);
        assert!(t.get(4).is_some(), "open groups are never pruned");
        crate::testutil::remove_test_dir(&dir);
    }
}
