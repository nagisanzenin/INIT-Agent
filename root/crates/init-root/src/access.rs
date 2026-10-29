//! SBX-009, ADR-009 (build/specs/ADR-009-spec.md, root/root.md §3.17): the owner's workspace, full mode, the
//! grant of each body spawn, workspace snapshots and `/undo`. ADR-012 (v0.3.1): full access is the charter's default
//! (`sandbox.mode`); the owner's session can confine it (`init-agent --workspace`, `/full off`) or turn it back on. The wire protocol is build/status/SBX-009.md
//! ("Protocol for WIRE-009"). What a grant allows is the sandbox crate's (`init_sandbox_profiles::access`).
//! Lock order: `access` is a leaf; nothing is appended, emitted or sent while it is held.

use crate::link::{Root, lock};
use crate::snapshot::{End, Unrestored};
use crate::util::{self, now_ms, rfc3339, write_atomic};
use crate::zone::Err3;
use init_sandbox_profiles::access::{self as pa, AccessState, Fixed, SpawnGrant};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The typed confirmation (`root.confirm`, `data.phrase`).
pub const PHRASE: &str = "confirm";
/// Spec item 4: no snapshot of a workspace over 2 GiB; the last 5 per workspace are kept.
const SNAP_MAX_BYTES: u64 = 2 << 30;
const SNAP_KEEP: usize = 5;

/// The Root's access state. ROOT_HOME/state/access.json holds the workspace and a session's confinement (`"full":
/// false`), so a Root restart never widens access; a session's full override (`Some(true)`) never survives a restart.
#[derive(Default)]
pub struct St {
    workspace: Option<PathBuf>,
    /// ADR-012: the owner session's choice over the charter's `sandbox.mode` (None: the charter's default).
    full: Option<bool>,
    /// Full mode was confirmed in this owner session (each `workspace.set` starts one).
    confirmed: bool,
    /// The live body last spawned: instance id, `INIT_ACCESS`, `INIT_WORKSPACE`.
    live: Option<(String, &'static str, Option<PathBuf>)>,
    /// Fleet members granted the workspace write (`fleet.workspace_write`): instance id → when it was granted.
    writers: std::collections::BTreeMap<String, i64>,
    /// ROOT-MSG (v0.5.1): why this session has no workspace, in the owner's words ([`refusal_text`]).
    why: Option<String>,
    /// R-UNDO (v0.5.2): the snapshot plain `/undo` last named in its `root.confirm`, so the confirmation restores
    /// that one or asks again ([`undo_to`]).
    offered: Option<String>,
}

/// A member's write slot counts from its grant until the broker records the instance (or the spawn failed).
const WRITER_PENDING_MS: i64 = 60_000;

/// At Root start: the recorded workspace, if it still validates.
pub fn load(p: &crate::install::Paths) -> St {
    let v: Value = fs::read_to_string(p.r("state/access.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let fixed = fixed_of(p);
    let deny = pa::read_deny(&fixed.0, &[]);
    let workspace = v["workspace"].as_str().and_then(|w| pa::validate_workspace(w, &fixed.as_fixed(), &deny).ok()).filter(|w| w.as_os_str() == v["workspace"].as_str().unwrap_or(""));
    St { workspace, full: (v["full"] == false).then_some(false), ..St::default() }
}

/// state/access.json: the workspace and the session's confinement (only `Some(false)` is kept, see [`St`]).
fn persist(root: &Root) {
    let (ws, full) = { let a = lock(&root.access); (a.workspace.clone(), a.full) };
    let rec = json!({"v": 1, "workspace": ws, "full": if full == Some(false) { json!(false) } else { Value::Null }, "set_at": rfc3339(now_ms())});
    if let Err(e) = write_atomic(&root.p.r("state/access.json"), format!("{rec}\n").as_bytes(), 0o600) {
        util::log(&root.p.rh, "warn", &format!("state/access.json: {e}"));
    }
}

/// Home (canonical), ROOT_HOME, INIT_HOME and the service directory.
struct Own(PathBuf, PathBuf, PathBuf, PathBuf);
impl Own {
    fn as_fixed(&self) -> Fixed<'_> {
        Fixed { home: &self.0, root_home: &self.1, init_home: &self.2, root_svc_dir: &self.3 }
    }
}

fn fixed_of(p: &crate::install::Paths) -> Own {
    let svc = crate::install::service_path(p);
    let dir = svc.parent().map(Path::to_path_buf).unwrap_or(svc);
    Own(home(p), p.rh.clone(), p.ih.clone(), pa::resolve(&dir))
}

/// The owner's home as the Root knows it (`HOME_REAL`), canonical.
pub fn home(p: &crate::install::Paths) -> PathBuf {
    fs::canonicalize(&p.home).unwrap_or_else(|_| p.home.clone())
}

/// The charter's `[sandbox]` access keys.
struct Ch {
    read_home: bool,
    allow_full: bool,
    deny: Vec<PathBuf>,
    /// ADR-012 `sandbox.mode == "full"` (the default).
    default_full: bool,
}

/// The charter's `[sandbox]` access keys.
/// LINUX: the bwrap zone has no deny overlays for workspace mode's home bind (init-sandbox-profiles `linux.rs` refuses
/// broad home reads and workspace writes), so the platform caps the home read off: in workspace mode the body works in
/// INIT_HOME only, and every view the owner sees says so. Full access (ADR-012) has its own overlays and is allowed.
/// Fail closed, never an unconfined fallback.
fn charter(root: &Root) -> Ch {
    let st = lock(&root.st);
    let mut c = st.charter.as_ref().map_or(Ch { read_home: true, allow_full: true, deny: vec![], default_full: true }, |l| {
        let s = &l.ch.sandbox;
        Ch { read_home: s.read_home, allow_full: s.allow_full, deny: s.read_deny.iter().filter_map(|x| pa::charter_entry(x)).collect(),
            default_full: s.mode == crate::charter::SANDBOX_MODES[0] }
    });
    c.read_home &= LINUX_HOME_GRANTS;
    c
}

/// Whether this platform's zone can grant workspace mode's home reads and workspace writes (ADR-009).
const LINUX_HOME_GRANTS: bool = !cfg!(target_os = "linux");

/// ADR-012's one-line description of full access (the confirmation, the notices).
pub const FULL_TEXT: &str = "read and write anywhere you can and use the network, except INIT's Root files (~/.init-root, its service, \
    INIT's launchers); writes outside the start folder are not undoable";
/// The owner's notice when a workspace is asked for where the zone cannot grant one.
const NO_WS_LINUX: &str = "[root] workspace mode is not available on Linux yet: the agent works in its own home (INIT_HOME) and cannot read or edit your files (docs/platforms.md)";

/// The read deny list (`Z_DENY_<i>`): the built-in list, then the other INIT installs, then the charter's.
pub fn deny(root: &Root) -> Vec<PathBuf> {
    pa::read_deny(&home(&root.p), &[others(root), charter(root).deny].concat())
}

/// v0.5.3 (lane H): the other INIT installs on this account (`init_sandbox_profiles::access::other_installs`), denied in
/// every mode: no install's agent reads another's keys or writes its Root.
fn others(root: &Root) -> Vec<PathBuf> {
    pa::other_installs(&home(&root.p), &root.p.rh, &root.p.ih)
}

/// The write deny list (`Z_WDENY_<i>`, tail Z3a): the built-in list, then INIT's bin directory (`install.json`).
pub fn write_deny(root: &Root) -> Vec<PathBuf> {
    let bin: Vec<PathBuf> = root.info["bin_dir"].as_str().map(PathBuf::from).into_iter().collect();
    pa::write_deny(&home(&root.p), &bin)
}

/// ADR-012: a full grant's read deny list: INIT's key directory, then the other INIT installs, then the charter's
/// `sandbox.read_deny`.
pub fn deny_full(root: &Root) -> Vec<PathBuf> {
    pa::full_read_deny(&home(&root.p), &[others(root), charter(root).deny].concat())
}

/// ADR-012: a full grant's write deny list: the Root's service directories, then INIT's launchers in `bin_dir`.
pub fn write_deny_full(root: &Root) -> Vec<PathBuf> {
    pa::full_write_deny(&home(&root.p), root.info["bin_dir"].as_str().map(Path::new))
}

/// What a workspace may not lie in: both deny lists.
fn ws_deny(root: &Root) -> Vec<PathBuf> {
    let mut d = deny(root);
    d.extend(write_deny(root));
    d
}

/// The effective access: full = the session's choice, else the charter's `sandbox.mode`, and never without
/// `sandbox.allow_full`. LINUX: the workspace is kept only while full access is in effect, so a non-full Linux grant
/// never carries a write (workspace mode stays capped off there).
fn state(root: &Root) -> AccessState {
    let c = charter(root);
    let a = lock(&root.access);
    let full = a.full.unwrap_or(c.default_full) && c.allow_full;
    let workspace = a.workspace.clone().filter(|_| full || LINUX_HOME_GRANTS);
    AccessState { read_home: c.read_home, workspace, full }
}

/// The grant of a `body`-profile spawn `inst` (kind `body` or `probe`; `live`: its INIT_HOME is the real one).
/// ADR-011 §4.5, §7.4 (INTEG-A): a fleet member (kind `body`, not live) reads the home only, unless an owner
/// `fleet.workspace_write` grant of N is live and a workspace is set: then at most N running members at once (first
/// come) also write the workspace (`INIT_ACCESS=workspace`, `workspace_only` without the home read; never the home in full
/// mode). No grant: unchanged.
pub fn spawn_grant(root: &Root, inst: &str, kind: &str, live: bool) -> SpawnGrant {
    let st = state(root);
    let g = pa::grant_for(kind, live, &st, &home(&root.p));
    // LINUX: a member's workspace write would be a workspace-mode grant, which the Linux zone refuses
    let writers = if kind == "body" && !live && LINUX_HOME_GRANTS { crate::grant::workspace_writers(root) } else { 0 };
    let (n, Some(ws)) = (writers, st.workspace) else { return g };
    if n <= 0 {
        return g;
    }
    // the running holders, read without the access lock (the broker's table has its own)
    let held: Vec<String> = lock(&root.access).writers.keys().cloned().collect();
    let running: std::collections::BTreeSet<String> = held.into_iter().filter(|id| root.zone.pid_of(id).is_some()).collect();
    let now = now_ms();
    let mut a = lock(&root.access);
    a.writers.retain(|id, at| running.contains(id) || now - *at < WRITER_PENDING_MS);
    if a.writers.len() as i64 >= n {
        return g;
    }
    a.writers.insert(inst.to_string(), now);
    let access = AccessState { read_home: g.grant.read_home, workspace: Some(ws.clone()), full: false }.level(); // v0.5.3: `workspace_only` without the home read
    SpawnGrant { grant: pa::Grant { read_home: g.grant.read_home, write: Some(ws.clone()), full: false }, access, workspace: Some(ws) }
}

/// A spawn that never started gives its member write slot back.
pub fn unspawned(root: &Root, inst: &str) {
    lock(&root.access).writers.remove(inst);
}

/// After a live body spawned with `g`: what `restart_pending` compares against.
pub fn spawned(root: &Root, inst: &str, g: &SpawnGrant) {
    lock(&root.access).live = Some((inst.to_string(), g.access, g.workspace.clone()));
}

/// A running live body holds a grant other than the current one.
fn restart_pending(root: &Root) -> bool {
    let st = state(root);
    let live = lock(&root.access).live.clone();
    live.is_some_and(|(id, a, w)| root.zone.pid_of(&id).is_some() && (a, w) != (st.level(), st.workspace.clone()))
}

fn snap_dir(root: &Root) -> PathBuf {
    root.p.r("snapshots")
}

/// `AccessView` (build/status/SBX-009.md; ADR-012 adds `mode`, the charter's default: "full" or "workspace").
pub fn view(root: &Root) -> Value {
    let st = state(root);
    let c = charter(root);
    let snap = st.workspace.as_ref().and_then(|w| snaps(root, w).pop());
    let mode = crate::charter::SANDBOX_MODES[usize::from(!c.default_full)];
    json!({"access": st.level(), "allow_full": c.allow_full, "full": st.full, "mode": mode, "read_home": c.read_home,
        "restart_pending": restart_pending(root), "snapshot": snap, "workspace": st.workspace})
}

/// The `access` block of `hello` and the `access` event (§4.1.4).
pub fn kernel_view(root: &Root) -> Value {
    let st = state(root);
    json!({"access": st.level(), "full": st.full, "workspace": st.workspace})
}

/// The `access` event to a ready kernel: after `ready` (`restart: null`) and after every change.
pub fn to_kernel(root: &Root, after_change: bool) {
    let Some(k) = root.sup.kernel().filter(|k| k.ready.load(std::sync::atomic::Ordering::SeqCst)) else { return };
    let mut d = kernel_view(root);
    d["restart"] = if after_change && restart_pending(root) { json!("idle") } else { Value::Null };
    k.event("access", d);
}

/// After a change: the `access` record (no paths: the lineage is mirrored), the kernel event, the notice.
fn changed(root: &Root, event: &str, text: &str) {
    let st = state(root);
    root.append("access", json!({"access": st.level(), "event": event, "full": st.full, "workspace": st.workspace.is_some()}));
    to_kernel(root, true);
    util::log(&root.p.rh, "info", &format!("access {event}: {} {:?}", st.level(), st.workspace));
    root.notice_code("info", "access", text);
}

/// ROOT-MSG (v0.5.1): why a launch folder gives no workspace, in the owner's words. The rule itself is unchanged
/// (`init_sandbox_profiles::access::validate_workspace`); this only says it.
pub fn refusal_text(r: pa::Refusal, cwd: &str) -> String {
    match r {
        pa::Refusal::Relative => format!("the start folder ({cwd}) was not given as an absolute path"),
        pa::Refusal::TooLong => "the start folder's path is longer than 1024 bytes".into(),
        pa::Refusal::Missing => format!("{cwd} does not exist"),
        pa::Refusal::NotDir => format!("{cwd} is not a folder"),
        pa::Refusal::OutsideHome => format!("{cwd} is outside your home folder"),
        pa::Refusal::Home => format!("{cwd} is your home folder itself"),
        pa::Refusal::RootPath => format!("{cwd} is, holds or lies in INIT's own files (~/.init-root, ~/.init or the Root's service folder)"),
        pa::Refusal::DenyList => format!("{cwd} is a protected folder (secrets such as ~/.ssh, INIT's launchers, shell startup \
            folders, or your charter's sandbox.read_deny)"),
    }
}

/// `p` with `~` for the home (display only).
fn tilde(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(r) if !r.as_os_str().is_empty() => format!("~/{}", r.display()),
        _ => p.display().to_string(),
    }
}

/// ROOT-MSG (v0.5.1): the workspace rule, where the agent works without one, and how to get one. `agent`: the launcher
/// as the owner runs it (v0.5.3, [`crate::install::Paths::agent_cmd`]).
fn ws_rule(home: &Path, ih: &Path, agent: &str) -> String {
    format!("A workspace is the folder you start init-agent in, and it must be a folder inside your home ({}), not your home \
        itself and not one of INIT's own or protected folders. Without one the agent works in {}, and /undo has nothing to \
        restore. To get one: quit, cd into a project folder inside your home, and run {agent} there.",
        home.display(), tilde(&ih.join("work"), home))
}

fn no_ws_text(why: Option<&str>, home: &Path, ih: &Path, agent: &str) -> String {
    format!("[root] no workspace: {}. {}", why.unwrap_or("this session has none"), ws_rule(home, ih, agent))
}

/// The owner's notice for a session's access (after `workspace.set`).
fn session_text(st: &AccessState, refused_linux: bool, why: Option<&str>, home: &Path, ih: &Path, agent: &str) -> String {
    match (&st.workspace, st.full) {
        (Some(w), true) => format!("[root] workspace: {} · access FULL: {FULL_TEXT}", w.display()),
        (None, true) => format!("{} · access FULL: {FULL_TEXT}", no_ws_text(why, home, ih, agent)),
        (Some(w), false) => format!("[root] workspace: {}", w.display()),
        (None, false) if refused_linux => NO_WS_LINUX.to_string(),
        (None, false) => format!("{} In workspace mode it cannot edit your files either.", no_ws_text(why, home, ih, agent)),
    }
}

fn snap_notice(root: &Root, r: &Result<Value, String>) {
    root.notice_code(if r.is_ok() { "info" } else { "warn" }, "snapshot", &snap_text(r));
}

/// R-UNDO (v0.5.2): "(/undo restores it)" was false once INIT had been reopened (`/undo` restores the newest snapshot
/// that differs from the folder, and at launch that is never the one just taken). An unchanged folder takes none.
fn snap_text(r: &Result<Value, String>) -> String {
    let s = match r {
        Ok(s) => s,
        Err(why) => return format!("[root] no snapshot of the workspace: {why}"),
    };
    let (id, ws) = (s["id"].as_str().unwrap_or(""), s["workspace"].as_str().unwrap_or(""));
    if s["unchanged"] == true {
        format!("[root] {ws} is unchanged since snapshot {id}, taken {} {}, so no new snapshot was taken (/undo list shows what /undo can restore)",
            local_of(&s["at"]), trigger(s))
    } else {
        format!("[root] snapshot {id} of {ws} taken {} (/undo list shows what /undo can restore)", trigger(s))
    }
}

/// `workspace.set{cwd, mode?}` (spec items 2, 4, 5): a new owner session. Never an error for an invalid cwd.
/// ADR-012: the session takes the charter's default access, or workspace mode with `mode: "workspace"` (`init-agent --workspace`).
pub fn workspace_set(root: &Root, a: &Value) -> Result<Value, Err3> {
    let cwd = a["cwd"].as_str().ok_or(("root.args", "workspace.set needs cwd (a string)".to_string(), Value::Null))?;
    let session_full = match a.get("mode").and_then(Value::as_str) {
        None if a.get("mode").is_none_or(Value::is_null) => None,
        Some("workspace") => Some(false),
        _ => return Err(("root.args", "workspace.set mode must be \"workspace\" (or absent: the charter's sandbox.mode)".into(), Value::Null)),
    };
    let own = fixed_of(&root.p);
    let new = pa::validate_workspace(cwd, &own.as_fixed(), &ws_deny(root));
    let ws = new.as_ref().ok().cloned();
    let why = new.as_ref().err().map(|r| refusal_text(*r, cwd));
    let before = state(root);
    {
        let mut st = lock(&root.access);
        (st.workspace, st.full, st.confirmed, st.why) = (ws.clone(), session_full, false, why.clone());
    }
    persist(root);
    let after = state(root);
    crate::twin::session_started(root, None); // X9: a new owner session; its first line is replayable only with a snapshot
    crate::sentinel::session_started(root); // X3: the sentinel's units never span owner sessions
    if let Some(w) = &after.workspace {
        let r = take(root, w, "session");
        snap_notice(root, &r);
        crate::twin::session_started(root, r.as_ref().ok());
    }
    let change = (before.workspace.clone(), before.full) != (after.workspace.clone(), after.full);
    // LINUX: a valid cwd gives no workspace while the session is not full (workspace mode is capped off there)
    let linux_refused = new.is_ok() && after.workspace.is_none();
    let text = session_text(&after, linux_refused, why.as_deref(), &own.0, &root.p.ih, &root.p.agent_cmd(""));
    if change {
        changed(root, "workspace", &text);
    } else {
        root.notice_code("info", "access", &text);
    }
    let refused = match new {
        Err(r) => Some(json!({"code": r.code(), "message": refusal_text(r, cwd)})),
        Ok(_) if linux_refused => Some(json!({"code": "platform", "message": NO_WS_LINUX.trim_start_matches("[root] ")})),
        Ok(_) => None,
    };
    Ok(json!({"access": view(root), "changed": change, "refused": refused}))
}

fn confirm(a: &Value) -> bool {
    a["confirm"].as_str() == Some(PHRASE)
}

/// `full.set{on, confirm?}` (spec item 3; ADR-012). Off: the session is confined to workspace mode. On: with the
/// charter's default full (`sandbox.mode = "full"`) at once; with `"workspace"` after the typed confirmation, once per
/// owner session. Refused while `sandbox.allow_full = false`.
pub fn full_set(root: &Root, a: &Value) -> Result<Value, Err3> {
    let on = a["on"].as_bool().ok_or(("root.args", "full.set needs on (a boolean)".to_string(), Value::Null))?;
    let c = charter(root);
    let (full, ws) = { let st = state(root); (st.full, st.workspace) };
    if !on {
        lock(&root.access).full = Some(false);
        persist(root);
        if full {
            changed(root, "full_off", &format!("[root] full access off for this session: the agent works in its workspace (workspace mode) \
                from its next restart at an idle point. A new init-agent launch starts with the charter's default again; to keep workspace \
                mode, launch with {} or set sandbox.mode = \"workspace\" in the charter", root.p.agent_cmd("--workspace")));
        }
        return Ok(json!({"access": view(root), "changed": full, "snapshot": null}));
    }
    if !c.allow_full {
        return Err(("root.mode", "full access is disabled by the charter (sandbox.allow_full = false)".into(), json!({"reason": "charter"})));
    }
    if full {
        return Ok(json!({"access": view(root), "changed": false, "snapshot": null}));
    }
    let confirmed = lock(&root.access).confirmed;
    if !c.default_full && !confirmed && !confirm(a) {
        let snap = ws.as_ref().map_or_else(|| " There is no workspace, so no snapshot covers it.".to_string(),
            |w| format!(" {} is snapshotted first.", w.display()));
        let m = format!("Full access lets the agent {FULL_TEXT}.{snap} Type `{PHRASE}` to turn it on.");
        return Err(("root.confirm", m, json!({"phrase": PHRASE})));
    }
    // the session's workspace becomes visible again on Linux once full access is in effect
    let ws = ws.or_else(|| lock(&root.access).workspace.clone());
    let snap = ws.as_ref().map(|w| take(root, w, "full_on"));
    {
        let mut st = lock(&root.access);
        // the charter's default needs no session override (a later charter switch to "workspace" then applies)
        st.full = if c.default_full { None } else { Some(true) };
        st.confirmed |= !c.default_full;
    }
    persist(root);
    if let Some(r) = &snap {
        snap_notice(root, r);
    }
    let tail = if ws.is_some() { "" } else { " No workspace is set, so nothing has /undo." };
    changed(root, "full_on", &format!("[root] full access on: the agent may {FULL_TEXT} (from its next restart).{tail}"));
    Ok(json!({"access": view(root), "changed": true, "snapshot": snap.and_then(Result::ok)}))
}

/// ROOT-MSG (v0.5.1): `/undo` with no workspace: why there is none, what `/undo` covers, and how to get one.
fn undo_no_ws(root: &Root) -> String {
    let why = lock(&root.access).why.clone();
    format!("/undo has nothing to restore: this session has no workspace ({}). {}", why.as_deref().unwrap_or("none was set"),
        ws_rule(&home(&root.p), &root.p.ih, &root.p.agent_cmd("")))
}

/// `undo{confirm?}` (spec item 4; R-UNDO, v0.5.2): restore the newest snapshot of the workspace that differs from the
/// folder as it is now and holds no state an `/undo` replaced (v0.5.3), saving the current state first.
pub fn undo(root: &Root, a: &Value) -> Result<Value, Err3> {
    undo_to(root, None, confirm(a))
}

/// `/undo` (`id` None: the newest snapshot that differs from the folder now, so a launch snapshot of a folder the agent
/// already changed is passed over, and that holds no state an `/undo` replaced, [`replaced`]) or `/undo <id>` (R-UNDO,
/// v0.5.2), asked (`root.confirm`) or confirmed. The state the restore replaces is saved first (`pre_undo`), so a
/// restore can itself be undone, by `/undo <id>`. v0.5.3: the restore is all or nothing ([`restore`]); one that does
/// not complete leaves the folder as it was and drops the save, unless it could not put everything back
/// ([`unrestored`]).
fn undo_to(root: &Root, id: Option<&str>, confirmed: bool) -> Result<Value, Err3> {
    let ws = state(root).workspace.ok_or_else(|| ("root.mode", undo_no_ws(root), json!({"reason": "no_workspace"})))?;
    let all = snaps(root, &ws);
    if all.is_empty() {
        return Err(("root.not_found", format!("no snapshot of {}", ws.display()), Value::Null));
    }
    if let Some(id) = id.filter(|id| !all.iter().any(|s| s["id"] == *id)) {
        return Err(unknown_id(id, &ws));
    }
    // While a turn runs the body may be writing the workspace: refuse (the owner retries when it is idle).
    if root.sup.kernel().is_some_and(|k| k.busy.load(std::sync::atomic::Ordering::SeqCst)) {
        return Err(body_busy());
    }
    if !confirmed {
        let now = now_digest(root, &ws);
        let target = choose(root, &all, id, now.as_deref(), &ws)?;
        if id.is_none() {
            lock(&root.access).offered = target["id"].as_str().map(String::from);
        }
        let passed = passed_over(root, &all, &target, now.as_deref());
        return Err(("root.confirm", ask_text(&ws, &target, id, passed), json!({"phrase": PHRASE, "snapshot": target})));
    }
    let internal = |m: String| ("root.internal", m, Value::Null);
    let _one = lock(&RESTORE);
    // lane U (v0.5.3): from here to the last move no owner line goes to the agent ([`restoring`]); and the agent must
    // still be idle, with no line sent to it since its last heartbeat (a turn it starts shows only in the next one)
    let _hold = Holding::start(root);
    // A line sent since the last heartbeat may have started a turn that only the next heartbeat shows: wait for it, at
    // most one heartbeat interval and 2 s (new lines stay queued meanwhile), so an /undo typed right after a short
    // turn goes ahead instead of being refused (the v0.5.3 release CI's wire9 e2e).
    let hb_s = lock(&root.st).charter.as_ref().map_or(5, |l| l.ch.root.hb_interval_s).max(1) as u64;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(hb_s + 2);
    while let Some(k) = root.sup.kernel() {
        let (busy, hb, sent) = (k.busy.load(std::sync::atomic::Ordering::SeqCst), *lock(&k.last_hb), *lock(&k.delivered));
        if busy || (in_turn(false, hb, sent) && std::time::Instant::now() >= until) {
            return Err(body_busy());
        }
        if !in_turn(false, hb, sent) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    revalidate(root, &ws).map_err(|m| internal(format!("the workspace no longer validates ({m}); nothing changed")))?;
    let saved = take(root, &ws, "pre_undo").map_err(|m| internal(format!("could not save the current state ({m}); nothing changed")))?;
    let sid = saved["id"].as_str().unwrap_or("").to_string();
    let now = snap_digest(root, &saved);
    let target = match choose(root, &all, id, now.as_deref(), &ws) {
        Ok(t) => t,
        Err(e) => {
            drop_snap(root, &sid); // nothing restored, so nothing to save
            return Err(e);
        }
    };
    // plain `/undo`: the folder changed since the question, so it now names another snapshot; ask again
    if id.is_none() {
        let offered = lock(&root.access).offered.take();
        if offered.is_some_and(|o| target["id"] != o.as_str()) {
            drop_snap(root, &sid);
            lock(&root.access).offered = target["id"].as_str().map(String::from);
            let passed = passed_over(root, &all, &target, now.as_deref());
            return Err(("root.confirm", format!("The folder changed since /undo asked. {}", ask_text(&ws, &target, None, passed)),
                json!({"phrase": PHRASE, "snapshot": target})));
        }
    }
    let tid = target["id"].as_str().unwrap_or("").to_string();
    let stage = stage_dir(root);
    if let Err(u) = restore(&snap_dir(root).join(&tid), &ws, &stage, &mut crate::lineage::env_fault) {
        return Err(unrestored(root, &ws, &sid, &stage, u));
    }
    let saved = keep_replaced(root, &saved, &stage, &target);
    prune(root, &ws, Some(&tid)); // the snapshot just restored is kept, even when it is the oldest of six
    let st = state(root);
    root.append("access", json!({"access": st.level(), "event": "undo", "full": st.full, "workspace": true}));
    root.notice_code("info", "snapshot", &format!("[root] /undo restored {} to snapshot {tid}, taken {} {}; the folder as it was is saved as \
        snapshot {sid}, so /undo {sid} brings it back (/undo list shows every snapshot)", ws.display(), local_of(&target["at"]), trigger(&target)));
    Ok(json!({"restored": target, "saved": saved}))
}

/// R-UNDO (v0.5.3): one confirmed restore at a time: its save, its pick and its moves.
static RESTORE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lane U (v0.5.3): set while a confirmed `/undo` holds [`RESTORE`], from its second idle check to its last move.
/// The busy check came before the confirmation and before the lock, and the owner socket serves one thread per
/// client, so a line typed on another attached client (or one redelivered) could start a turn that wrote the
/// workspace while its entries moved. Meanwhile owner lines stay in the owner queue (link.rs `deliver`, whose notice
/// says why), and go to the agent when the restore ends.
static RESTORING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A confirmed `/undo` is restoring ([`RESTORING`]).
pub fn restoring() -> bool {
    RESTORING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Sets [`RESTORING`] for its life; when it ends, the lines held meanwhile go to the agent.
struct Holding<'a>(&'a Root);

impl<'a> Holding<'a> {
    fn start(root: &'a Root) -> Self {
        RESTORING.store(true, std::sync::atomic::Ordering::SeqCst);
        Holding(root)
    }
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        RESTORING.store(false, std::sync::atomic::Ordering::SeqCst);
        self.0.redeliver();
    }
}

/// The live body may be in a turn: its last heartbeat (at `hb`) said so, or an owner line went to it at or after
/// that heartbeat (`sent`), so a turn it started is not in a heartbeat yet.
fn in_turn(busy: bool, hb: std::time::Instant, sent: Option<std::time::Instant>) -> bool {
    busy || sent.is_some_and(|t| t >= hb)
}

fn body_busy() -> Err3 {
    ("root.busy", "the agent is in a turn; /undo restores only while it is idle (wait for the turn to end, or interrupt it, then /undo again)".into(),
        json!({"reason": "body_busy"}))
}

/// The answer to a restore that did not complete ([`Unrestored`]), and what is kept. Refused or rolled back, the folder
/// is as it was, so its save and the staging tree go. Stuck, both stay: they hold what the folder now lacks.
fn unrestored(root: &Root, ws: &Path, sid: &str, stage: &Path, u: Unrestored) -> Err3 {
    match u {
        Unrestored::Refused(why) => {
            drop_snap(root, sid);
            remove_clone(root, stage);
            ("root.mode", format!("/undo did not restore {}: {why}. Nothing changed.", ws.display()), json!({"reason": "restore_refused"}))
        }
        Unrestored::RolledBack(why) => {
            drop_snap(root, sid);
            remove_clone(root, stage);
            ("root.internal", format!("/undo could not restore {}: {why}, so every entry was put back and the folder is exactly as it was \
                before /undo. Nothing was lost: /undo again, or /undo list for another snapshot.", ws.display()), json!({"reason": "rolled_back"}))
        }
        Unrestored::Stuck(why) => {
            let m = format!("/undo failed midway and could not put everything back: {why}. The folder as it was before /undo is snapshot \
                {sid} (/undo {sid} confirm restores it), and what was not put back is in {}", tilde(stage, &root.p.home));
            root.notice_code("alert", "snapshot", &format!("[root] {m}"));
            ("root.internal", m, json!({"reason": "partial", "saved": sid}))
        }
    }
}

/// What `/undo` restores: snapshot `id`, else the newest that differs from `now` (the digest of the folder as it is
/// now; None: unknown, so every snapshot differs) and holds no state an `/undo` replaced ([`replaced`], v0.5.3). One
/// that equals the folder is never restored: nothing would change. When only replaced states differ, the refusal
/// (`data.reason = "replaced"`) names the `/undo <id>` that brings the newest of them back.
fn choose(root: &Root, all: &[Value], id: Option<&str>, now: Option<&str>, ws: &Path) -> Result<Value, Err3> {
    let same = |s: &Value| now.is_some() && snap_digest(root, s).as_deref() == now;
    let unchanged = |m: String| ("root.not_found", m, json!({"reason": "unchanged"}));
    match id {
        Some(id) => match all.iter().find(|s| s["id"] == id) {
            Some(s) if same(s) => Err(unchanged(format!("/undo {id}: that snapshot is the same as {} now, so there is nothing to restore; \
                /undo list shows the others", ws.display()))),
            Some(s) => Ok(s.clone()),
            None => Err(unknown_id(id, ws)),
        },
        None => {
            let gone = replaced(root, all);
            let was_replaced = |s: &Value| snap_digest(root, s).is_some_and(|d| gone.contains(&d));
            if let Some(s) = all.iter().rev().find(|s| !same(s) && !was_replaced(s)) {
                return Ok(s.clone());
            }
            Err(match all.iter().rev().find(|s| !same(s)) {
                Some(r) => ("root.not_found", format!("/undo has nothing to restore: the snapshots that differ from {} hold only what an /undo \
                    replaced, which plain /undo never brings back. /undo {} brings back the newest of them, taken {} {}; /undo list shows every \
                    snapshot.", ws.display(), r["id"].as_str().unwrap_or(""), local_of(&r["at"]), trigger(r)), json!({"reason": "replaced"})),
                None => {
                    let newest = &all[all.len() - 1];
                    unchanged(format!("/undo has nothing to restore: {} is the same as every snapshot of it, so nothing changed since the newest \
                        ({}, taken {} {}); /undo list shows them", ws.display(), newest["id"].as_str().unwrap_or(""), local_of(&newest["at"]),
                        trigger(newest)))
                }
            })
        }
    }
}

/// R-UNDO (v0.5.3, REL-052 §7.5): the content, by digest, of the states an `/undo` replaced that no later restore
/// brought back. Each `pre_undo` snapshot, oldest first, adds its own digest and takes away the digest of the snapshot
/// its restore put in place (`restored`, recorded by [`keep_replaced`]; a `pre_undo` of v0.5.2 has none). Plain `/undo`
/// never restores one of these ([`choose`]): after an `/undo` and new edits, v0.5.2's newest differing snapshot was the
/// `pre_undo` save, so `/undo` brought back the edits the owner had just undone. `/undo <id>` restores any snapshot,
/// and restoring one makes its content a default target again.
fn replaced(root: &Root, all: &[Value]) -> std::collections::BTreeSet<String> {
    let mut gone = std::collections::BTreeSet::new();
    for s in all.iter().filter(|s| s["reason"] == "pre_undo") {
        gone.extend(snap_digest(root, s));
        if let Some(d) = s["restored"]["digest"].as_str() {
            gone.remove(d);
        }
    }
    gone
}

/// How many snapshots newer than `target`, other than the folder as it is now, the pick passed over because an
/// `/undo` replaced their state (for the question's text).
fn passed_over(root: &Root, all: &[Value], target: &Value, now: Option<&str>) -> usize {
    let gone = replaced(root, all);
    all.iter().filter(|s| s["id"].as_str() > target["id"].as_str())
        .filter_map(|s| snap_digest(root, s)).filter(|d| now != Some(d.as_str()) && gone.contains(d)).count()
}

fn unknown_id(id: &str, ws: &Path) -> Err3 {
    let id: String = id.chars().take(80).collect();
    ("root.not_found", format!("/undo: {id} is not a snapshot of {}; /undo list shows the ones /undo can restore", ws.display()),
        json!({"reason": "unknown_id"}))
}

/// The confirmation of `/undo` (`id` None) or `/undo <id>`: what is restored, from when, and that it can be undone.
/// `passed`: the newer snapshots plain `/undo` passed over because an `/undo` replaced their state ([`passed_over`]).
fn ask_text(ws: &Path, s: &Value, id: Option<&str>, passed: usize) -> String {
    let (sid, when) = (s["id"].as_str().unwrap_or(""), format!("taken {} {}", local_of(&s["at"]), trigger(s)));
    let over = if passed == 0 { String::new() } else { format!(", passing over {passed} that an /undo replaced") };
    let what = match id {
        Some(id) => format!("/undo {id} restores {} to that snapshot, {when}.", ws.display()),
        None => format!("/undo restores {} to snapshot {sid}, {when}: the newest snapshot that differs from the folder now{over} (/undo list \
            shows the others).", ws.display()),
    };
    format!("{what} Everything changed in the folder since then, by you or the agent, is replaced; the folder as it is now is saved \
        first, so this can be undone. Type `{PHRASE}` to restore it.")
}

/// `/undo list` (R-UNDO, v0.5.2): the workspace's snapshots, newest first, one line each (id, local time, trigger,
/// folder), marking the one `/undo` restores and those the folder equals now.
fn list_text(root: &Root) -> (&'static str, String) {
    let Some(ws) = state(root).workspace else { return ("warn", format!("[root] {}", undo_no_ws(root))) };
    let all = snaps(root, &ws);
    if all.is_empty() {
        return ("info", format!("[root] /undo list: no snapshot of {} yet, so /undo has nothing to restore. INIT takes one each time init-agent \
            starts in this folder, unless the folder is over 2 GiB, not on APFS or on another volume than INIT's files.", ws.display()));
    }
    let now = now_digest(root, &ws);
    let def = choose(root, &all, None, now.as_deref(), &ws).ok().map(|s| s["id"].clone());
    let head = if def.is_some() {
        "/undo restores the one marked \"← /undo\"; /undo <id> restores another"
    } else if all.iter().any(|s| now.is_none() || snap_digest(root, s) != now) {
        "the ones that differ from the folder now hold what an /undo replaced, so plain /undo has nothing to restore; /undo <id> restores one"
    } else {
        "each is the same as the folder now, so /undo has nothing to restore"
    };
    let n = all.len();
    let mut t = format!("[root] /undo list: {n} snapshot{} of {}, newest first (INIT keeps the last {SNAP_KEEP}). {head}.", if n == 1 { "" } else { "s" },
        ws.display());
    let folder = tilde(&ws, &home(&root.p));
    let gone = replaced(root, &all);
    for s in all.iter().rev() {
        let d = snap_digest(root, s);
        let mark = if def.as_ref() == Some(&s["id"]) {
            " · ← /undo"
        } else if now.is_some() && d == now {
            " · same as the folder now"
        } else if d.is_some_and(|d| gone.contains(&d)) {
            " · an /undo replaced it" // v0.5.3: plain /undo passes over it; /undo <id> restores it
        } else {
            ""
        };
        t.push_str(&format!("\n  {} · {} · {} · {folder}{mark}", s["id"].as_str().unwrap_or(""), local_of(&s["at"]), trigger(s)));
    }
    ("info", t)
}

/// A snapshot's trigger, as it reads after "taken <time>".
fn trigger(s: &Value) -> &'static str {
    match s["reason"].as_str() {
        Some("session") => "at launch",
        Some("full_on") => "when full access was turned on",
        Some("pre_undo") => "before an /undo (the folder it replaced)",
        _ => "(another trigger)",
    }
}

/// An RFC 3339 time in the Root's local time zone (the owner's clock, not UTC).
fn local_of(at: &Value) -> String {
    at.as_str().map_or_else(|| "?".into(), |a| util::parse_rfc3339(a).map_or_else(|| a.to_string(), local))
}

/// `ms` as the owner's clock shows it: `2026-10-28 18:19:10 +07:00`.
fn local(ms: i64) -> String {
    local_at(ms, crate::sys::utc_offset_s(ms.div_euclid(1000)))
}

fn local_at(ms: i64, off_s: i64) -> String {
    let s = rfc3339(ms + off_s * 1000);
    let a = off_s.abs();
    format!("{} {} {}{:02}:{:02}", &s[..10], &s[11..19], if off_s < 0 { '-' } else { '+' }, a / 3600, a / 60 % 60)
}

/// The owner lines of build/status/SBX-009.md B. False: not an access line.
pub fn line(root: &Root, t: &str) -> bool {
    let show = |root: &Root| {
        let v = view(root);
        let ws = v["workspace"].as_str().unwrap_or("none");
        let full = if v["full"] == true { " · FULL" } else { "" };
        // ROOT-MSG: "workspace none" says why and how to get one
        let rule = if v["workspace"].is_null() {
            let why = lock(&root.access).why.clone();
            format!(" ({}. {})", why.as_deref().unwrap_or("this session has none"), ws_rule(&home(&root.p), &root.p.ih, &root.p.agent_cmd("")))
        } else {
            String::new()
        };
        root.notice_code("info", "access", &format!("[root] access {}{full} · workspace {ws}{rule} · default {} · read_home {} · allow_full {}",
            v["access"].as_str().unwrap_or(""), v["mode"].as_str().unwrap_or(""), v["read_home"], v["allow_full"]));
    };
    let reply = |root: &Root, r: Result<Value, Err3>, again: &str| match r {
        Ok(_) => {}
        Err(("root.confirm", m, _)) => root.notice_code("info", "access", &format!("[root] {}", m.replace(&format!("Type `{PHRASE}`"), &format!("Type {again}")))),
        Err((_, m, _)) => root.notice_code("warn", "access", &format!("[root] {m}")),
    };
    match t {
        "/workspace" | "/full" => show(root),
        "/full on" => reply(root, full_set(root, &json!({"on": true})), "/full on confirm"),
        "/full on confirm" => reply(root, full_set(root, &json!({"on": true, "confirm": PHRASE})), ""),
        "/full off" => reply(root, full_set(root, &json!({"on": false})), ""),
        "/undo" => reply(root, undo_to(root, None, false), "/undo confirm"),
        "/undo confirm" => reply(root, undo_to(root, None, true), ""),
        // R-UNDO (v0.5.2): `/undo list`; `/undo <id>` asks, `/undo <id> confirm` restores that snapshot
        _ => match t.strip_prefix("/undo ").map(|r| r.split_whitespace().collect::<Vec<_>>()).as_deref() {
            Some(["list"]) => {
                let (level, text) = list_text(root);
                root.notice_code(level, "snapshot", &text);
            }
            Some([c]) if *c == PHRASE => reply(root, undo_to(root, None, true), ""),
            Some([id]) => reply(root, undo_to(root, Some(id), false), &format!("/undo {id} confirm")),
            Some([id, c]) if *c == PHRASE => reply(root, undo_to(root, Some(id), true), ""),
            Some(_) => root.notice_code("warn", "access", "[root] /undo takes nothing, list, or a snapshot id: /undo, /undo list, /undo <id> \
                (the ids are in /undo list)"),
            None => return false,
        },
    }
    true
}

/// After `charter.apply`: a changed `[sandbox]` is an access change (`sandbox.mode` is the session's default unless the
/// owner chose otherwise this session); `allow_full = false` ends full access (the state computes it).
pub fn charter_applied(root: &Root) {
    changed(root, "charter", &format!("[root] the charter's sandbox settings changed: access {}", state(root).level()));
}

// ---------------------------------------------------------------------------------------------
// Snapshots: ROOT_HOME/snapshots/<id>/ (an APFS clone of the workspace) and <id>.json (its Snap)

fn revalidate(root: &Root, ws: &Path) -> Result<(), String> {
    let own = fixed_of(&root.p);
    match pa::validate_workspace(&ws.to_string_lossy(), &own.as_fixed(), &ws_deny(root)) {
        Ok(w) if w == ws => Ok(()),
        Ok(w) => Err(format!("it now resolves to {}", w.display())),
        Err(r) => Err(r.message().into()),
    }
}

/// Every snapshot of `ws`, oldest first (ids start with their UTC time).
fn snaps(root: &Root, ws: &Path) -> Vec<Value> {
    let Ok(rd) = fs::read_dir(snap_dir(root)) else { return vec![] };
    let mut v: Vec<Value> = rd.flatten().filter_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        let id = n.strip_suffix(".json")?;
        let s: Value = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
        (s["id"] == id && safe_id(id) && s["workspace"].as_str() == ws.to_str() && snap_dir(root).join(id).is_dir()).then_some(s)
    }).collect();
    v.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    v
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('.') && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Sum of the regular files' sizes under `p`, never following links; stops past `limit`.
fn size_over(p: &Path, limit: u64) -> std::io::Result<bool> {
    let (mut total, mut stack) = (0u64, vec![p.to_path_buf()]);
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let md = fs::symlink_metadata(e.path())?;
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total += md.len();
                if total > limit {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Clone `ws` into a new snapshot. Err: why there is none. R-UNDO (v0.5.2): a `session` or `full_on` snapshot of a
/// folder whose content equals the newest snapshot's is not kept; that one is returned with `"unchanged": true`, so
/// relaunches never fill the 5 kept with copies. The state an /undo replaces (`pre_undo`) is never skipped for its
/// size (a clone is cheap, and the undo must not lose it), never folded into an older snapshot, and not pruned here:
/// `undo_to` prunes after the restore, keeping the snapshot it restored.
fn take(root: &Root, ws: &Path, reason: &str) -> Result<Value, String> {
    let save = reason == "pre_undo";
    revalidate(root, ws)?;
    if !save && size_over(ws, SNAP_MAX_BYTES).map_err(|e| format!("cannot measure it: {e}"))? {
        return Err("the workspace is over 2 GiB".into());
    }
    let dir = snap_dir(root);
    fs::create_dir_all(&dir).and_then(|()| fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))).map_err(|e| format!("snapshots/: {e}"))?;
    if crate::sys::fs_type(ws).map_err(|e| e.to_string())? != "apfs" {
        return Err("the workspace is not on APFS".into());
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).map_err(|e| e.to_string());
    if dev(ws)? != dev(&dir)? {
        return Err("the workspace is on another volume than ROOT_HOME".into());
    }
    let at = now_ms();
    let name: String = ws.file_name().map(|n| n.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).take(48).collect())
        .unwrap_or_default();
    let base = format!("{}-{}", rfc3339(at).replace(['-', ':'], ""), name.trim_start_matches('.'));
    let id = (1..100).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|id| !dir.join(id).exists() && !dir.join(format!("{id}.json")).exists()).ok_or("no free snapshot name")?;
    crate::sys::clonefile(ws, &dir.join(&id)).map_err(|e| format!("clonefile: {e}"))?;
    let digest = digest(&dir.join(&id));
    if !save && let Some(d) = digest.as_deref() && let Some(mut prev) = snaps(root, ws).pop().filter(|p| snap_digest(root, p).as_deref() == Some(d)) {
        remove_clone(root, &dir.join(&id)); // exact path, cloned above: the folder is unchanged since `prev`
        prev["unchanged"] = json!(true);
        return Ok(prev);
    }
    let s = json!({"at": rfc3339(at), "digest": digest, "id": id, "reason": reason, "workspace": ws});
    write_atomic(&dir.join(format!("{id}.json")), format!("{s}\n").as_bytes(), 0o600).map_err(|e| format!("{id}.json: {e}"))?;
    if !save {
        prune(root, ws, None);
    }
    Ok(s)
}

/// Keep 5 snapshots of `ws`: the newest, and `keep` (the one an /undo just restored) even when it is the oldest; the
/// others are removed by their exact paths.
fn prune(root: &Root, ws: &Path, keep: Option<&str>) {
    let all = snaps(root, ws);
    let excess = all.len().saturating_sub(SNAP_KEEP);
    for s in all.iter().filter(|s| keep.is_none_or(|k| s["id"] != k)).take(excess) {
        drop_snap(root, s["id"].as_str().unwrap_or(""));
    }
}

/// Remove snapshot `id` (its tree and its record) by exact path: a snapshot this Root made. v0.5.3: a tree that cannot
/// be removed keeps its record, so it stays a listed snapshot (and prune tries again) instead of an orphan.
fn drop_snap(root: &Root, id: &str) {
    if safe_id(id) && remove_clone(root, &snap_dir(root).join(id)) {
        let _ = fs::remove_file(snap_dir(root).join(format!("{id}.json")));
    }
}

/// R-UNDO (v0.5.3, REL-052 §7.5 findings 1 and 2): remove a tree this Root made under ROOT_HOME/snapshots (a snapshot,
/// a probe clone, a staging tree) by its exact path ([`crate::snapshot::remove_tree`]). In v0.5.2 a read-only folder
/// or a locked file in a clone made its removal fail: the snapshots of such a folder were never pruned, an unchanged
/// launch left its clone, and the shared `.probe` stayed for good. A failure is logged and the tree stays (false).
fn remove_clone(root: &Root, p: &Path) -> bool {
    match crate::snapshot::remove_tree(p) {
        Ok(()) => true,
        Err(e) => {
            util::log(&root.p.rh, "warn", &format!("could not remove {}: {e}", p.display()));
            false
        }
    }
}

/// A new staging directory's path for one restore: `snapshots/.undo-<pid>-<n>` (R-UNDO, v0.5.3). One a crashed Root
/// left is never removed by name: it may hold entries the workspace lacks.
fn stage_dir(root: &Root) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    snap_dir(root).join(format!(".undo-{}-{n}-{}", std::process::id(), now_ms()))
}

/// After a restore (R-UNDO, v0.5.3): a clone of the entries it moved out of the workspace (`stage/old`) becomes the
/// `pre_undo` snapshot's tree, in place of the clone taken before the pick, so a write made in between is kept too;
/// that clone and `old` go with the staging tree. The record gets the tree's digest and `restored`: the snapshot the
/// restore put in place, with its digest ([`replaced`]). Returns the record. Every step that fails leaves the clone
/// taken before the pick as the save.
///
/// A clone, never `old` itself (lane U, v0.5.3, class 1): the files in `old` are the workspace's own inodes, which the
/// zone may still reach through a hard link kept outside the folder or a descriptor it holds, so their content can
/// change after the Root records the digest, and `/undo <id>` would restore content that no longer matches its
/// record. The clone shares no inode with anything the zone can reach. Removing `old` by its path is safe: the zone
/// reaches entries there only as inodes it already held, and tail Z1 denies every write by a path under ROOT_HOME, so
/// it can change a file's bytes there but cannot add, rename or swap an entry.
fn keep_replaced(root: &Root, saved: &Value, stage: &Path, target: &Value) -> Value {
    let (dir, sid) = (snap_dir(root), saved["id"].as_str().unwrap_or(""));
    let (tree, old, clone) = (dir.join(sid), stage.join("old"), stage.join("clone"));
    let mut rec = saved.clone();
    let mv = |a: &Path, b: &Path| crate::snapshot::move_entry(&End::Path(a.into()), &End::Path(b.into()), false);
    let mut kept = true; // `tree` holds a whole save, and `stage` nothing that is not saved
    if safe_id(sid) && mv(&tree, &clone).is_ok() {
        match crate::sys::clonefile(&old, &tree) {
            Ok(()) => rec["digest"] = json!(digest(&tree)),
            Err(e) => {
                util::log(&root.p.rh, "warn", &format!("snapshot {sid}: what the restore replaced could not be cloned ({e}); the clone taken \
                    before the pick is kept"));
                // a clone that failed partway is removed by its exact path, made just now; then the first clone goes back
                kept = remove_clone(root, &tree) && mv(&clone, &tree).inspect_err(|e| util::log(&root.p.rh, "error",
                    &format!("snapshot {sid}: its tree is in {} ({e})", clone.display()))).is_ok();
            }
        }
    }
    rec["restored"] = json!({"digest": snap_digest(root, target), "id": target["id"]});
    if let Err(e) = write_atomic(&dir.join(format!("{sid}.json")), format!("{rec}\n").as_bytes(), 0o600) {
        util::log(&root.p.rh, "warn", &format!("{sid}.json: {e}"));
    }
    if kept && tree.is_dir() {
        remove_clone(root, stage); // exact path, made for this restore
    }
    rec
}

/// R-UNDO (v0.5.2): the content digest of a tree the Root owns (a snapshot or the probe clone, never the live
/// workspace: R-F2), to tell an unchanged folder from a changed one. sha256 over every entry below `dir`, never
/// following a link: its kind, permission bits, path, and a file's length and bytes or a link's target. Times are not
/// in it, so a `touch` alone is no change. None past 2 GiB of file bytes or on an error: such a tree is never
/// called unchanged.
fn digest(dir: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    let (mut h, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let mut names: Vec<std::ffi::OsString> = fs::read_dir(dir.join(&rel)).ok()?.map(|e| e.map(|e| e.file_name())).collect::<Result<_, _>>().ok()?;
        names.sort();
        for n in names {
            let (r, p) = (rel.join(&n), dir.join(&rel).join(&n));
            let md = fs::symlink_metadata(&p).ok()?;
            let kind = if md.is_dir() { 'd' } else if md.is_file() { 'f' } else if md.file_type().is_symlink() { 'l' } else { 'o' };
            h.update(format!("{kind} {:o} {} ", md.mode() & 0o7777, if kind == 'f' { md.len() } else { 0 }).as_bytes());
            h.update(r.as_os_str().as_bytes());
            h.update(b"\0");
            match kind {
                'd' => stack.push(r),
                'l' => h.update(fs::read_link(&p).ok()?.as_os_str().as_bytes()),
                'f' => {
                    total += md.len();
                    if total > SNAP_MAX_BYTES {
                        return None;
                    }
                    let mut f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&p).ok()?;
                    let mut left = md.len();
                    while left > 0 {
                        let k = f.read(&mut buf).ok()?;
                        if k == 0 {
                            return None; // shorter than its size: not a stable tree
                        }
                        let k = k.min(usize::try_from(left).unwrap_or(usize::MAX));
                        h.update(&buf[..k]);
                        left -= k as u64;
                    }
                }
                _ => {}
            }
        }
    }
    Some(util::hex(&h.finalize()))
}

/// A snapshot's digest: its record's, else computed from its tree and recorded (snapshots before v0.5.2 have none).
fn snap_digest(root: &Root, s: &Value) -> Option<String> {
    if let Some(d) = s.get("digest") {
        return d.as_str().map(String::from);
    }
    let id = s["id"].as_str().filter(|id| safe_id(id))?;
    let d = digest(&snap_dir(root).join(id));
    let mut rec = s.clone();
    rec["digest"] = json!(d);
    if let Some(o) = rec.as_object_mut() {
        o.remove("unchanged");
    }
    let _ = write_atomic(&snap_dir(root).join(format!("{id}.json")), format!("{rec}\n").as_bytes(), 0o600);
    d
}

/// The digest of the workspace as it is now, hashed from a clone the Root removes after, so the Root reads the live
/// workspace only as the snapshot rules allow (the size walk and the clone, §3.17.4). R-UNDO (v0.5.3, REL-052 §7.5
/// finding 2): each check clones into a probe of its own, `snapshots/.probe-<pid>-<n>`, removed on every path; the
/// probes a crashed call left (v0.5.2's shared `.probe` among them) go first, and one that cannot be removed is
/// logged and passed by, never fatal to the check. v0.5.2's one `.probe`, once it could not be removed (a read-only
/// folder in it), failed every later check with EEXIST. None: no clone (over 2 GiB, another volume, an error), and
/// every snapshot then counts as different from the folder.
fn now_digest(root: &Root, ws: &Path) -> Option<String> {
    static PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let _one = lock(&PROBE); // so no probe of this process is in use while the old ones go
    revalidate(root, ws).ok()?;
    if size_over(ws, SNAP_MAX_BYTES).ok()? {
        return None;
    }
    let dir = snap_dir(root);
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let n = e.file_name();
        if n == ".probe" || n.to_string_lossy().starts_with(".probe-") {
            remove_clone(root, &e.path()); // exact path, from this Root's own directory: a probe a crashed call left
        }
    }
    let p = dir.join(format!(".probe-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let d = crate::sys::clonefile(ws, &p).ok().and_then(|()| digest(&p));
    remove_clone(root, &p); // exact path, cloned above (also after a clone that failed partway)
    d
}

/// R-UNDO (v0.5.3, REL-052 §7.5 finding 1, FG-1): replace the workspace's entries with the snapshot's, completely or not
/// at all. v0.5.2 removed the entries first and cloned the snapshot's after, so a read-only folder (EACCES on an
/// unlink inside it) stopped it with the files already gone. Here nothing is removed. `stage` is a new directory
/// under ROOT_HOME/snapshots, so on the workspace's volume (`take` checked it): `stage/new` is a clone of the
/// snapshot, and `stage/old` receives the workspace's entries. First every entry that will move is checked: the
/// workspace folder is writable, no entry is locked (an immutable or append-only flag), and a read-only folder is the
/// owner's. Then the workspace's entries move to `old` and `new`'s into the workspace, each by a rename that never
/// replaces ([`crate::sys::rename_excl_at`]) and never follows a link ([`move_entry`]); a move that fails puts back,
/// newest first, every move made. On success `old` holds the replaced entries and `new` is gone. The workspace
/// directory itself stays (an owner shell inside it keeps a valid cwd). `fault` runs before each move (test builds:
/// the INIT_FAULT points `root.undo.out` and `root.undo.in`). The moves and their put-back are R2's too
/// ([`crate::snapshot::swap`]); here without `force`, so the owner's own lock flags refuse the restore.
///
/// Lane U (v0.5.3): the workspace is opened once, never following a link at any component, and listed, checked and
/// moved out of and into only through that descriptor ([`End::Under`]), never by its path. In full access the agent
/// can write the workspace's parent, so during an `/undo` it could rename the workspace away and put a link at its
/// path; the Root, which is not sandboxed, then moved entries out of (and the snapshot's into) whatever the link
/// named. Now a link at the path when the restore starts refuses it, and one swapped in later is never followed: the
/// restore completes in the folder it opened, wherever that folder now is.
fn restore(src: &Path, ws: &Path, stage: &Path, fault: &mut dyn FnMut(&str) -> std::io::Result<()>) -> Result<(), Unrestored> {
    use Unrestored::{Refused, RolledBack, Stuck};
    use crate::snapshot::Op;
    use std::os::fd::AsFd;
    if !fs::symlink_metadata(src).is_ok_and(|m| m.is_dir()) {
        return Err(Refused("that snapshot's tree is missing; /undo list shows the others".into()));
    }
    let wsd = crate::sys::open_dir_nofollow(None, ws).map_err(|e| Refused(match fs::symlink_metadata(ws) {
        Ok(m) if m.file_type().is_symlink() => format!("{} is now a link, not the folder, and /undo never follows one", ws.display()),
        Ok(m) if !m.is_dir() => format!("{} is no longer a folder", ws.display()),
        _ => format!("{} cannot be opened without following a link ({e})", ws.display()),
    }))?;
    let base = wsd.as_fd();
    let mode = fs::File::from(wsd.try_clone().map_err(|e| Refused(e.to_string()))?).metadata().map_err(|e| Refused(e.to_string()))?.mode() & 0o7777;
    let shown = util::sh_word(&ws.to_string_lossy());
    if !crate::sys::may_write_at(base) {
        return Err(Refused(format!("{} itself is read-only (mode {mode:o}), so what is in it cannot be replaced; make it writable (chmod u+w \
            {shown}) and /undo again", ws.display())));
    }
    let (new, old) = (stage.join("new"), stage.join("old"));
    let mk = |d: &Path| fs::DirBuilder::new().mode(0o700).create(d);
    mk(stage).and_then(|()| mk(&old)).map_err(|e| Refused(format!("{}: {e}", stage.display())))?;
    crate::sys::clonefile(src, &new).map_err(|e| Refused(format!("the snapshot could not be copied ({e})")))?;
    let sorted = |mut v: Vec<std::ffi::OsString>| {
        v.sort();
        v
    };
    let outs = crate::sys::list_at(base).map(sorted).map_err(|e| Refused(format!("it cannot be listed ({e})")))?;
    let ins = fs::read_dir(&new).and_then(|d| d.map(|e| e.map(|e| e.file_name())).collect::<std::io::Result<Vec<_>>>()).map(sorted)
        .map_err(|e| Refused(format!("the snapshot cannot be listed ({e})")))?;
    for n in &outs {
        let p = ws.join(n);
        let q = util::sh_word(&p.to_string_lossy());
        match unmovable(Some(base), Path::new(n)) {
            Some(Stop::Locked) => return Err(Refused(format!("{} is locked (an immutable or append-only flag; ls -lO shows it), so it cannot \
                be moved; unlock it (chflags nouchg {q}) and /undo again", p.display()))),
            Some(Stop::Foreign) => return Err(Refused(format!("{} is a read-only folder of another user, so it cannot be moved aside; make it \
                writable for you, or move it out of {}, and /undo again", p.display(), ws.display()))),
            None => {}
        }
    }
    if let Some(n) = ins.iter().find(|n| unmovable(None, &new.join(n)).is_some()) {
        return Err(Refused(format!("the snapshot holds {}, which is locked (an immutable or append-only flag), so it cannot be moved into \
            the folder; /undo list shows the other snapshots", n.to_string_lossy())));
    }
    let inside = |n: &std::ffi::OsString| End::Under { base, rel: n.into(), shown: ws.join(n) };
    let ops: Vec<Op> = outs.iter().map(|n| Op::Move { from: inside(n), to: End::Path(old.join(n)), point: "root.undo.out" })
        .chain(ins.iter().map(|n| Op::Move { from: End::Path(new.join(n)), to: inside(n), point: "root.undo.in" })).collect();
    let mut guard = |op: &Op| match op {
        Op::Move { point, .. } => fault(point),
        Op::MkDir(_) => Ok(()),
    };
    if let Err((i, e, left)) = crate::snapshot::swap(&ops, false, &mut guard) {
        let (n, out) = if i < outs.len() { (&outs[i], true) } else { (&ins[i - outs.len()], false) };
        let what = format!("moving {} {} failed ({e})", n.to_string_lossy(), if out { "aside" } else { "in from the snapshot" });
        return Err(if left.is_empty() { RolledBack(what) } else { Stuck(format!("{what}, and putting back {} failed", left.join(", "))) });
    }
    let _ = fs::remove_dir(&new); // empty now
    Ok(())
}

/// What stops an entry from moving ([`restore`]'s check before any move).
enum Stop {
    /// An immutable or append-only flag: a rename is refused even to the owner.
    Locked,
    /// A read-only folder of another user: a move to another parent needs write permission on the folder itself.
    Foreign,
}

/// Entry `name` of `dir` (None: `name` is a path), never following a link.
fn unmovable(dir: Option<std::os::fd::BorrowedFd>, name: &Path) -> Option<Stop> {
    let st = crate::sys::stat_at(dir, name).ok()?; // gone meanwhile: its move fails, and every move is put back
    if crate::sys::locked_flags(st.flags) {
        return Some(Stop::Locked);
    }
    (st.is_dir() && st.mode & 0o200 == 0 && st.uid != crate::sys::euid()).then_some(Stop::Foreign)
}

#[cfg(test)]
mod msg_tests {
    use super::*;

    /// ROOT-MSG (v0.5.1): with no workspace, the notice states the rule (a folder inside the home, not the home itself),
    /// why this start folder was refused, where the agent works instead, that `/undo` has nothing, and how to get one.
    #[test]
    fn no_workspace_says_the_rule_where_the_agent_works_and_the_fix() {
        let (home, ih) = (Path::new("/Users/o"), Path::new("/Users/o/.init"));
        let why = refusal_text(pa::Refusal::OutsideHome, "/private/tmp/demo");
        assert_eq!(why, "/private/tmp/demo is outside your home folder");
        for full in [true, false] {
            let t = session_text(&AccessState { read_home: true, workspace: None, full }, false, Some(&why), home, ih, "~/.local/bin/init-agent");
            assert!(t.starts_with("[root] no workspace: /private/tmp/demo is outside your home folder. "), "{t}");
            // v0.5.3 (REL-052 §7.5 finding 4): the launcher by its path; a fresh Mac has no ~/.local/bin on PATH
            for want in ["must be a folder inside your home (/Users/o), not your home itself", "the agent works in ~/.init/work",
                "/undo has nothing to restore", "cd into a project folder inside your home, and run ~/.local/bin/init-agent there"] {
                assert!(t.contains(want), "{want:?} in {t}");
            }
            assert!(!t.contains("cd into a project and run init-agent to let me edit it"), "the old advice: {t}");
            assert_eq!(t.contains("access FULL"), full, "{t}");
        }
        assert_eq!(refusal_text(pa::Refusal::Home, "/Users/o"), "/Users/o is your home folder itself");
        let w = session_text(&AccessState { read_home: true, workspace: Some("/Users/o/p".into()), full: false }, false, None, home, ih, "x");
        assert_eq!(w, "[root] workspace: /Users/o/p");
    }

    /// R-UNDO (v0.5.2): snapshot times on the owner's clock, with the offset; the triggers in words.
    #[test]
    fn snapshot_times_are_local_and_triggers_are_words() {
        let ms = util::parse_rfc3339("2026-10-28T11:19:10.728Z").unwrap();
        assert_eq!(local_at(ms, 7 * 3600), "2026-10-28 18:19:10 +07:00");
        assert_eq!(local_at(ms, -(3 * 3600 + 1800)), "2026-10-28 07:49:10 -03:30");
        assert_eq!(local_at(ms, 0), "2026-10-28 11:19:10 +00:00");
        let s = |r: &str| json!({"at": "2026-10-28T11:19:10.728Z", "id": "x", "reason": r, "workspace": "/w"});
        assert_eq!([trigger(&s("session")), trigger(&s("full_on")), trigger(&s("pre_undo"))],
            ["at launch", "when full access was turned on", "before an /undo (the folder it replaced)"]);
        assert_eq!(snap_text(&Ok(s("session"))), "[root] snapshot x of /w taken at launch (/undo list shows what /undo can restore)");
        assert!(!snap_text(&Ok(s("session"))).contains("/undo restores it"), "the v0.5.1 claim that a relaunch made false");
    }

    /// R-UNDO (v0.5.2): the digest sees content, permission bits, names and link targets, not times.
    #[test]
    fn the_digest_sees_content_not_times() {
        use std::os::unix::fs::symlink;
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/undo-digest-{}", std::process::id()));
        let (a, b) = (top.join("a"), top.join("b"));
        for d in [&a, &b] {
            fs::create_dir_all(d.join("src")).unwrap();
            fs::write(d.join("src/x.txt"), "v1\n").unwrap();
            fs::write(d.join("README"), "r\n").unwrap();
            symlink("src/x.txt", d.join("link")).unwrap();
        }
        let same = |a: &Path, b: &Path| digest(a).is_some() && digest(a) == digest(b);
        let f = fs::File::options().write(true).open(b.join("README")).unwrap();
        f.set_modified(std::time::SystemTime::UNIX_EPOCH).unwrap();
        assert!(same(&a, &b), "a time alone is no change");
        fs::write(b.join("src/x.txt"), "v2\n").unwrap();
        assert!(!same(&a, &b), "content");
        fs::write(b.join("src/x.txt"), "v1\n").unwrap();
        assert!(same(&a, &b));
        fs::set_permissions(b.join("README"), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!same(&a, &b), "permission bits");
        fs::set_permissions(b.join("README"), fs::metadata(a.join("README")).unwrap().permissions()).unwrap();
        fs::remove_file(b.join("link")).unwrap();
        symlink("README", b.join("link")).unwrap();
        assert!(!same(&a, &b), "a link's target");
        fs::remove_file(b.join("link")).unwrap();
        symlink("src/x.txt", b.join("link")).unwrap();
        fs::rename(b.join("README"), b.join("README2")).unwrap();
        assert!(!same(&a, &b), "a name");
        fs::rename(b.join("README2"), b.join("README")).unwrap();
        fs::write(b.join("src/empty"), "").unwrap();
        assert!(!same(&a, &b), "a new empty file");
        fs::remove_dir_all(&top).unwrap(); // exact path, created above
    }

    /// Lane U (v0.5.3): the idle check a confirmed `/undo` repeats once it holds the restore lock: busy by the last
    /// heartbeat, or an owner line sent at or after it (a turn it starts shows only in the next heartbeat).
    #[test]
    fn a_line_sent_since_the_last_heartbeat_counts_as_a_turn() {
        use std::time::{Duration, Instant};
        let hb = Instant::now();
        let (before, after) = (hb.checked_sub(Duration::from_secs(1)).unwrap(), hb + Duration::from_secs(1));
        assert!(!in_turn(false, hb, None), "idle, nothing sent");
        assert!(!in_turn(false, hb, Some(before)), "a line the last heartbeat already saw");
        assert!(in_turn(false, hb, Some(hb)) && in_turn(false, hb, Some(after)), "a line sent since");
        assert!(in_turn(true, hb, None) && in_turn(true, hb, Some(before)), "busy by the heartbeat");
    }

    /// Lane U: a snapshot (`snap`), the workspace edited since (`ws`), and `canary`, a folder the zone cannot write,
    /// holding the same names, so a restore that followed a link to it would move every entry.
    fn swap_fixture(tag: &str) -> PathBuf {
        let top = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/undo-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top); // exact path, a test home of this test
        for (d, v) in [("snap", "snapshot"), ("w/ws", "edited"), ("canary", "canary")] {
            fs::create_dir_all(top.join(d).join("src")).unwrap();
            fs::write(top.join(d).join("src/a.txt"), format!("{v}\n")).unwrap();
            fs::write(top.join(d).join("README"), format!("{v}\n")).unwrap();
        }
        fs::canonicalize(&top).unwrap()
    }

    /// Lane U (v0.5.3, class 1): the restore works on the workspace's descriptor, never on its path. In full access the
    /// agent can write the workspace's parent, so during an `/undo` it renames the workspace away and puts a link to
    /// another folder at its path (here before the first move, through the fault hook). c4797cc listed and moved
    /// `ws.join(name)`: the Root moved the canary folder's entries out and the snapshot's into it. Now the canary
    /// folder is untouched and the workspace, where it moved to, holds the snapshot.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_restore_never_follows_a_link_swapped_in_at_the_workspace() {
        let top = swap_fixture("swap");
        let (snap, ws, moved, canary, stage) = (top.join("snap"), top.join("w/ws"), top.join("w/ws.moved"), top.join("canary"), top.join("stage"));
        let (want, canary0) = (digest(&snap), digest(&canary));
        let mut swapped = 0;
        let mut fault = |_: &str| {
            if swapped == 0 {
                fs::rename(&ws, &moved)?;
                std::os::unix::fs::symlink(&canary, &ws)?;
            }
            swapped += 1;
            Ok(())
        };
        let r = restore(&snap, &ws, &stage, &mut fault);
        assert!(r.is_ok(), "the restore completes in the folder it opened");
        assert!(swapped > 0, "the fault hook ran");
        assert_eq!(digest(&canary), canary0, "the folder the link names is untouched");
        assert_eq!(fs::read_to_string(canary.join("README")).unwrap(), "canary\n");
        assert_eq!(digest(&moved), want, "the workspace itself holds the snapshot");
        assert_eq!(fs::read_to_string(stage.join("old/src/a.txt")).unwrap(), "edited\n", "what it replaced is the workspace's");
        assert!(fs::symlink_metadata(&ws).unwrap().file_type().is_symlink(), "the link stays a link");
        fs::remove_dir_all(&top).unwrap(); // exact path, created above
    }

    /// Lane U: a link at the workspace's path, or at a folder above it, when the restore starts refuses it before
    /// anything is made or moved; c4797cc followed it and restored the folder it named.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_restore_refuses_a_workspace_that_is_a_link() {
        let top = swap_fixture("link");
        let (snap, ws, canary, stage) = (top.join("snap"), top.join("w/ws"), top.join("canary"), top.join("stage"));
        let canary0 = digest(&canary);
        // the workspace's path is a link to the canary folder; and `up` is a link above a path to it (`up/canary`)
        fs::rename(&ws, top.join("w/ws.moved")).unwrap();
        std::os::unix::fs::symlink(&canary, &ws).unwrap();
        std::os::unix::fs::symlink(&top, top.join("up")).unwrap();
        for (ws, why) in [(ws, "is now a link, not the folder, and /undo never follows one"), (top.join("up/canary"), "cannot be opened without following a link")] {
            let e = match restore(&snap, &ws, &stage, &mut |_| Ok(())) {
                Err(Unrestored::Refused(m)) => m,
                Err(Unrestored::RolledBack(m) | Unrestored::Stuck(m)) => panic!("not refused: {m}"),
                Ok(()) => panic!("{} was restored through a link", ws.display()),
            };
            assert!(e.starts_with(&format!("{} ", ws.display())) && e.contains(why), "{e}");
            assert!(!stage.exists(), "nothing is made before the workspace is open");
        }
        assert_eq!(digest(&canary), canary0, "the folder the links name is untouched");
        assert_eq!(fs::read_to_string(top.join("w/ws.moved/README")).unwrap(), "edited\n");
        fs::remove_dir_all(&top).unwrap(); // exact path, created above
    }
}
