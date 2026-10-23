//! Rehydration as boot orders it (body/boot.md §3.7, §3.8; log-context.md §3.6): open and lock the
//! session (repairing a torn tail and closing interrupted tool rounds), build the head, then append
//! the `boot` event with the boot facts and the awareness boot block, and sync (D3).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use super::BootError;
use super::awareness::{self, BlockIn, ProbeIn};
use super::env::{BootEnv, BootJson, ProcFacts, mode_str, valid_sha};
use super::gitsync::GitSync;
use crate::context::compact::Summarizer;
use crate::context::head::{Head, build_head};
use crate::context::{BoundaryIn, Context, ContextError, Thresholds};
use crate::log::event::{BootEv, ErrorEv, EventBody, Kind};
use crate::log::session::{OpenParams, OpenReport, SessionLog};
use crate::log::writer::LogError;
use crate::model::{Effort, ModelInfo};
use crate::provider::models;
use crate::tools::Mode;

/// `$INIT_HOME/body`, or the LKG tree in safe mode (§3.14).
pub fn head_root(env: &BootEnv, boot: &BootJson) -> PathBuf {
    match (env.mode, &boot.lkg_tree) {
        (Mode::Safe, Some(t)) => env.home.join(t),
        _ => env.home.join("body"),
    }
}

pub fn constitution_path(home: &Path) -> PathBuf {
    home.join("kernel").join("CONSTITUTION.md")
}

/// The head from `root` (GENESIS.md, SELF.md, skills) and the constitution, with the registry.
pub fn load_head(root: &Path, home: &Path) -> Result<Head, String> {
    build_head(root, &constitution_path(home), crate::tools::spec_values()).map_err(|e| e.to_string())
}

pub struct RehydrateIn<'a> {
    pub env: &'a BootEnv,
    pub proc_facts: &'a ProcFacts,
    pub boot: &'a BootJson,
    pub info: &'static ModelInfo,
    pub effort: Effort,
    pub git_sync: &'a GitSync,
    pub spill_bytes: u64,
    /// Warnings from the earlier steps (process facts, git sync, housekeeping, ext host).
    pub warnings: Vec<String>,
    /// Boot facts `ext` (`{"status": …}`).
    pub ext_fact: Value,
    pub summarizer: Box<dyn Summarizer>,
    pub lock_wait: Duration,
    /// Per-process timeout of the environment probes (awareness §3.5.1: 2 s).
    pub probe_timeout: Duration,
}

pub struct Rehydrated {
    pub ctx: Context,
    pub report: OpenReport,
    /// The `boot` event id (16 hex).
    pub boot_event_id: String,
    /// The session file was created by this boot.
    pub created: bool,
    pub prefix_hash: String,
}

fn log_err(e: LogError) -> BootError {
    BootError::log("log", e.to_string())
}

fn ctx_err(e: ContextError) -> BootError {
    match e {
        ContextError::Io(e) => BootError::log("log", format!("log: {e}")),
        other => BootError::internal("head", other.to_string()),
    }
}

/// The previous `boot` event on the path (newest first), as its id and its JSON fields.
fn prev_boot(log: &SessionLog) -> Result<Option<(String, Value)>, BootError> {
    let ents = log.index.ents();
    for &p in log.path().iter().rev() {
        if ents[p].kind != Kind::Boot {
            continue;
        }
        let ev = log.load(p).map_err(|e| BootError::log("log", format!("log: {e}")))?;
        if let EventBody::Boot(b) = &ev.body {
            let v = serde_json::to_value(b.as_ref())
                .map_err(|e| BootError::internal("boot_event", e.to_string()))?;
            return Ok(Some((ev.id_hex(), v)));
        }
    }
    Ok(None)
}

/// `update_failed` seqs already in the log: `update_failed` events and earlier boot events.
fn logged_update_failed_seqs(log: &SessionLog) -> Vec<u64> {
    let mut seqs: Vec<u64> = log.index.update_failed_seqs.iter().copied().collect();
    for (p, e) in log.index.ents().iter().enumerate() {
        if e.kind != Kind::Boot {
            continue;
        }
        if let Ok(ev) = log.load(p)
            && let EventBody::Boot(b) = &ev.body
            && let Some(q) = b
                .extra
                .get("update_failed")
                .and_then(|u| u.get("seq"))
                .and_then(Value::as_u64)
        {
            seqs.push(q);
        }
    }
    seqs
}

fn opt_sha(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str) {
        Some(s) if valid_sha(s) => Value::String(s.to_string()),
        _ => Value::Null,
    }
}

/// `boot.json.cause` (an object `{kind, …}` since v0.3, or a string) as the event's string.
fn cause_str(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(Value::Object(o)) => o.get("kind").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// `live.capability` from the kernel's live copy, when readable (awareness §3.7.1, D12).
fn read_capability(home: &Path) -> Option<Value> {
    let bytes = std::fs::read(home.join("state/kernel/self.json")).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("live")?.get("capability").cloned()
}

/// Steps 7.1–7.6: lock, scan, repair, head, `boot` event, sync.
pub fn rehydrate(r: RehydrateIn) -> Result<Rehydrated, BootError> {
    let env = r.env;
    let raw = &r.boot.raw;
    // 1–4: select, lock, scan, repair (log-context §3.6 steps 1–6, 9).
    let (log, report) = SessionLog::open(&OpenParams {
        home: &env.home,
        env_session: &env.session,
        sha: &env.sha,
        lock_wait: r.lock_wait,
    })
    .map_err(log_err)?;
    let created = report.created.is_some();
    let sid = log.sid().to_string();
    let prev = prev_boot(&log)?;
    let logged_seqs = logged_update_failed_seqs(&log);

    // 5: the head and prefix_hash.
    let root = head_root(env, r.boot);
    let head = load_head(&root, &env.home).map_err(|e| BootError::internal("head", e))?;
    let cons_bytes = std::fs::read(constitution_path(&env.home))
        .map_err(|e| BootError::internal("head", format!("kernel/CONSTITUTION.md: {e}")))?;
    let constitution_sha256 = crate::sha256::tagged(&cons_bytes);
    let target = models::target(r.info, Some(r.effort));
    let th = Thresholds::for_target(&target);
    let mut ctx =
        Context::new(log, head, target, r.summarizer, &report, &env.sha).map_err(ctx_err)?;
    let prefix_hash = ctx.head_hash().to_string();

    // The boot facts (§3.8), sorted keys.
    let mut warnings = r.warnings;
    warnings.extend(r.git_sync.warnings.iter().cloned());
    let facts = json!({
        "bash": r.proc_facts.bash_value(),
        "body_version": env!("CARGO_PKG_VERSION"),
        "boot_id": env.boot_id,
        "cwd": r.proc_facts.cwd.to_string_lossy(),
        "ext": r.ext_fact,
        "git_sync": r.git_sync.to_value(),
        "kernel": raw,
        "kernel_version": env.kernel_version,
        "kill": r.proc_facts.kill,
        "mode": mode_str(env.mode),
        "model": {"effort": r.effort.as_str(), "model": r.info.id, "provider": r.info.provider},
        "pid": std::process::id(),
        "reason": env.reason,
        "sha": env.sha,
        "spill_bytes": r.spill_bytes,
        "warnings": warnings,
    });

    // The environment snapshot (awareness §3.5).
    let probed = awareness::probe_env(&ProbeIn {
        home: &env.home,
        cwd: &r.proc_facts.cwd,
        bash: &r.proc_facts.bash_value(),
        path_var: env.path_var.as_deref(),
        effective_window: th.w_eff,
        effort: r.effort.as_str(),
        boot: raw,
        now: SystemTime::now(),
        timeout: r.probe_timeout,
    });
    let env_snap = awareness::snapshot(raw, probed);
    let prev_env = prev.as_ref().and_then(|(_, v)| v.get("env"));
    let env_changed = awareness::env_changed(&env_snap, prev_env);

    // update_failed: verbatim, unless its seq is already logged.
    let update_failed = raw
        .get("update_failed")
        .filter(|u| !u.is_null())
        .filter(|u| {
            u.get("seq")
                .and_then(Value::as_u64)
                .is_none_or(|q| !logged_seqs.contains(&q))
        })
        .cloned();
    let capability = read_capability(&env.home);
    let git_sync_v = r.git_sync.to_value();
    let view = super::rootview::render_boot(raw, &env.home);
    let text = awareness::boot_text(&BlockIn {
        boot: &view,
        sha: &env.sha,
        mode: mode_str(env.mode),
        reason: &env.reason,
        provider: r.info.provider,
        model: r.info.id,
        effort: r.effort.as_str(),
        session: &sid,
        session_created: created,
        rehydrated_events: report.rehydrated_events,
        interrupted_calls: report.interrupted_calls(),
        prefix_hash: &prefix_hash,
        prev_boot: prev.as_ref().map(|(id, v)| (id.as_str(), v)),
        env: &env_snap,
        git_sync: &git_sync_v,
        update_failed: update_failed.as_ref(),
        capability: capability.as_ref(),
        access: env.access.as_deref(),
        workspace: env.workspace.as_deref(),
    });

    // log-context §4.2 "boot field sources": copied from boot.json, never inferred.
    let evo = raw.get("evolution").cloned().unwrap_or(json!({}));
    let selfmap = raw.get("selfmap").cloned().unwrap_or(json!({}));
    let mut extra = Map::new();
    let mut put = |k: &str, v: Value| {
        extra.insert(k.to_string(), v);
    };
    if let Value::String(s) = opt_sha(raw.get("lkg_sha")) {
        put("lkg_sha", Value::String(s));
    }
    put(
        "provisional",
        Value::Bool(raw.get("provisional").is_some_and(|p| !p.is_null())),
    );
    put("branch", raw.get("branch").cloned().unwrap_or(Value::Null));
    put("cause", cause_str(raw.get("cause")));
    put("constitution_sha256", Value::String(constitution_sha256.clone()));
    if let Some(n) = raw.get("restarts_since_lkg").and_then(Value::as_u64) {
        put("restarts_since_lkg", n.into());
    }
    put("update_failed", update_failed.clone().unwrap_or(Value::Null));
    for k in ["retire_candidates", "thin_candidates"] {
        if let Some(a) = evo.get(k).filter(|a| a.is_array()) {
            put(k, a.clone());
        }
    }
    put("upstream", evo.get("upstream").cloned().unwrap_or(Value::Null));
    put(
        "predict_hit_rate",
        evo.get("predict_hit_rate")
            .filter(|p| p.as_u64().is_some_and(|n| n <= 1000))
            .cloned()
            .unwrap_or(Value::Null),
    );
    if let Some(s) = evo.get("state").and_then(Value::as_str) {
        put("evolution_state", s.into());
    }
    // evolution-v2 §4.6: the engine's counts (the boot block's `- evolution:` line renders them).
    for k in ["frictions_eligible", "proposals_open"] {
        if let Some(n) = evo.get(k).and_then(Value::as_u64) {
            put(k, n.into());
        }
    }
    put(
        "stash",
        r.git_sync.stash.clone().map(Value::String).unwrap_or(Value::Null),
    );
    put("crash", raw.get("crash").cloned().unwrap_or(Value::Null));
    if let Some((_, pv)) = &prev
        && let Some(h) = pv.get("prefix_hash").filter(|h| h.is_string())
    {
        put("prefix_hash_prev", h.clone());
    }
    if let Some(n) = selfmap.get("body_tokens_prev").and_then(Value::as_u64) {
        put("body_tokens_prev", n.into());
    }
    put(
        "env_changed",
        Value::Array(env_changed.into_iter().map(Value::String).collect()),
    );
    if let Some(d) = awareness::capability_digest(capability.as_ref()) {
        put("capability", d);
    }
    let boot_ev = BootEv {
        boot_id: env.boot_id.clone(),
        reason: env.reason.clone(),
        mode: mode_str(env.mode).to_string(),
        sha: env.sha.clone(),
        from_sha: match opt_sha(raw.get("from_sha")) {
            Value::String(s) => Some(s),
            _ => None,
        },
        model: r.info.id.to_string(),
        provider: r.info.provider.to_string(),
        effort: r.effort.as_str().to_string(),
        context_window: r.info.context_window,
        effective_window: th.w_eff,
        body_tokens: selfmap
            .get("body_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        prefix_hash: String::new(), // filled by append_boot
        env: env_snap,
        facts,
        log_repair: Default::default(), // filled by append_boot
        text,
        extra,
    };
    // 6: append the boot event, then sync (D3).
    let id = ctx
        .append_boot(boot_ev, &report)
        .map_err(|e| BootError::log("log", format!("log: {e}")))?;

    // awareness §3.9 / §5 checks, logged after the boot event (never fatal).
    let mut errors = Vec::new();
    let root_v = raw.get("env").and_then(|e| e.get("root"));
    match root_v.and_then(|r| r.get("constitution_sha256")).and_then(Value::as_str) {
        Some(d) if d != constitution_sha256 => errors.push(ErrorEv {
            where_: "boot".into(),
            code: "constitution_mismatch".into(),
            message: format!(
                "kernel/CONSTITUTION.md hashes to {constitution_sha256}, the Root's charter digest is {d}"
            ),
            data: Some(json!({"body": constitution_sha256, "root": d})),
        }),
        _ => {}
    }
    if root_v.is_none_or(Value::is_null) {
        errors.push(ErrorEv {
            where_: "boot".into(),
            code: "root_status_missing".into(),
            message: "boot.json has no env.root: the autonomy and ceilings lines show ?".into(),
            data: None,
        });
    }
    let missing: Vec<&str> = ["env", "lkg_sha", "restarts_since_lkg", "selfmap"]
        .into_iter()
        .filter(|k| raw.get(*k).is_none())
        .collect();
    if !missing.is_empty() {
        errors.push(ErrorEv {
            where_: "boot".into(),
            code: "boot_json_field".into(),
            message: format!("boot.json lacks {}", missing.join(", ")),
            data: Some(json!({"missing": missing})),
        });
    }
    if !errors.is_empty() {
        ctx.boundary_append(BoundaryIn {
            errors,
            ..Default::default()
        })
        .map_err(|e| BootError::log("log", format!("log: {e}")))?;
    }
    Ok(Rehydrated {
        ctx,
        report,
        boot_event_id: crate::log::clock::id_hex(id),
        created,
        prefix_hash,
    })
}
