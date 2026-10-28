//! SBX-009, ADR-009 (build/specs/ADR-009-spec.md, root/root.md §3.17): the owner's workspace, full mode, the
//! grant of each body spawn, workspace snapshots and `/undo`. ADR-012 (v0.3.1): full access is the charter's default
//! (`sandbox.mode`); the owner's session can confine it (`init-agent --workspace`, `/full off`) or turn it back on. The wire protocol is build/status/SBX-009.md
//! ("Protocol for WIRE-009"). What a grant allows is the sandbox crate's (`init_sandbox_profiles::access`).
//! Lock order: `access` is a leaf; nothing is appended, emitted or sent while it is held.

use crate::link::{Root, lock};
use crate::util::{self, now_ms, rfc3339, write_atomic};
use crate::zone::Err3;
use init_sandbox_profiles::access::{self as pa, AccessState, Fixed, SpawnGrant};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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

/// The read deny list (`Z_DENY_<i>`): the built-in list, then the charter's.
pub fn deny(root: &Root) -> Vec<PathBuf> {
    pa::read_deny(&home(&root.p), &charter(root).deny)
}

/// The write deny list (`Z_WDENY_<i>`, tail Z3a): the built-in list, then INIT's bin directory (`install.json`).
pub fn write_deny(root: &Root) -> Vec<PathBuf> {
    let bin: Vec<PathBuf> = root.info["bin_dir"].as_str().map(PathBuf::from).into_iter().collect();
    pa::write_deny(&home(&root.p), &bin)
}

/// ADR-012: a full grant's read deny list: INIT's key directory, then the charter's `sandbox.read_deny`.
pub fn deny_full(root: &Root) -> Vec<PathBuf> {
    pa::full_read_deny(&home(&root.p), &charter(root).deny)
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
/// come) also write the workspace (`INIT_ACCESS=workspace`, never the home in full mode). No grant: unchanged.
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
    SpawnGrant { grant: pa::Grant { read_home: g.grant.read_home, write: Some(ws.clone()), full: false }, access: "workspace", workspace: Some(ws) }
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

/// ROOT-MSG (v0.5.1): the workspace rule, where the agent works without one, and how to get one.
fn ws_rule(home: &Path, ih: &Path) -> String {
    format!("A workspace is the folder you start init-agent in, and it must be a folder inside your home ({}), not your home \
        itself and not one of INIT's own or protected folders. Without one the agent works in {}, and /undo has nothing to \
        restore. To get one: quit, cd into a project folder inside your home, and run init-agent there.",
        home.display(), tilde(&ih.join("work"), home))
}

fn no_ws_text(why: Option<&str>, home: &Path, ih: &Path) -> String {
    format!("[root] no workspace: {}. {}", why.unwrap_or("this session has none"), ws_rule(home, ih))
}

/// The owner's notice for a session's access (after `workspace.set`).
fn session_text(st: &AccessState, refused_linux: bool, why: Option<&str>, home: &Path, ih: &Path) -> String {
    match (&st.workspace, st.full) {
        (Some(w), true) => format!("[root] workspace: {} · access FULL: {FULL_TEXT}", w.display()),
        (None, true) => format!("{} · access FULL: {FULL_TEXT}", no_ws_text(why, home, ih)),
        (Some(w), false) => format!("[root] workspace: {}", w.display()),
        (None, false) if refused_linux => NO_WS_LINUX.to_string(),
        (None, false) => format!("{} In workspace mode it cannot edit your files either.", no_ws_text(why, home, ih)),
    }
}

fn snap_notice(root: &Root, r: &Result<Value, String>) {
    match r {
        Ok(s) => root.notice_code("info", "snapshot", &format!("[root] snapshot {} of {} taken (/undo restores it)", s["id"].as_str().unwrap_or(""), s["workspace"].as_str().unwrap_or(""))),
        Err(why) => root.notice_code("warn", "snapshot", &format!("[root] no snapshot of the workspace: {why}")),
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
        let r = take(root, w, "session", true);
        snap_notice(root, &r);
        crate::twin::session_started(root, r.as_ref().ok());
    }
    let change = (before.workspace.clone(), before.full) != (after.workspace.clone(), after.full);
    // LINUX: a valid cwd gives no workspace while the session is not full (workspace mode is capped off there)
    let linux_refused = new.is_ok() && after.workspace.is_none();
    let text = session_text(&after, linux_refused, why.as_deref(), &own.0, &root.p.ih);
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
            changed(root, "full_off", "[root] full access off for this session: the agent works in its workspace (workspace mode) from its \
                next restart at an idle point. A new init-agent launch starts with the charter's default again; to keep workspace mode, \
                launch with init-agent --workspace or set sandbox.mode = \"workspace\" in the charter");
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
    let snap = ws.as_ref().map(|w| take(root, w, "full_on", true));
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
        ws_rule(&home(&root.p), &root.p.ih))
}

/// `undo{confirm?}` (spec item 4): restore the latest snapshot of the workspace, saving the current state first.
pub fn undo(root: &Root, a: &Value) -> Result<Value, Err3> {
    let ws = state(root).workspace.ok_or_else(|| ("root.mode", undo_no_ws(root), json!({"reason": "no_workspace"})))?;
    let latest = snaps(root, &ws).pop().ok_or(("root.not_found", format!("no snapshot of {}", ws.display()), Value::Null))?;
    // While a turn runs the body may be writing the workspace: refuse (the owner retries when it is idle).
    if root.sup.kernel().is_some_and(|k| k.busy.load(std::sync::atomic::Ordering::SeqCst)) {
        return Err(("root.busy", "the agent is in a turn; /undo restores only while it is idle (wait for the turn to end, or interrupt it, then /undo again)".into(),
            json!({"reason": "body_busy"})));
    }
    if !confirm(a) {
        let m = format!("/undo restores {} to the snapshot taken {} (the current state is saved first, so this can be undone). \
            Type `{PHRASE}` to restore it.", ws.display(), latest["at"].as_str().unwrap_or("?"));
        return Err(("root.confirm", m, json!({"phrase": PHRASE, "snapshot": latest})));
    }
    let internal = |m: String| ("root.internal", m, Value::Null);
    revalidate(root, &ws).map_err(|m| internal(format!("the workspace no longer validates ({m}); nothing changed")))?;
    let saved = take(root, &ws, "pre_undo", false).map_err(|m| internal(format!("could not save the current state ({m}); nothing changed")))?;
    let src = snap_dir(root).join(latest["id"].as_str().unwrap_or(""));
    if let Err(e) = restore(&src, &ws) {
        let m = format!("the restore failed midway ({e}); the state before /undo is snapshot {}", saved["id"].as_str().unwrap_or(""));
        root.notice_code("alert", "snapshot", &format!("[root] {m}"));
        return Err(internal(m));
    }
    let st = state(root);
    root.append("access", json!({"access": st.level(), "event": "undo", "full": st.full, "workspace": true}));
    root.notice_code("info", "snapshot", &format!("[root] /undo restored {} to the snapshot taken {}; the state before it is snapshot {}",
        ws.display(), latest["at"].as_str().unwrap_or("?"), saved["id"].as_str().unwrap_or("")));
    Ok(json!({"restored": latest, "saved": saved}))
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
            format!(" ({}. {})", why.as_deref().unwrap_or("this session has none"), ws_rule(&home(&root.p), &root.p.ih))
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
        "/undo" => reply(root, undo(root, &json!({})), "/undo confirm"),
        "/undo confirm" => reply(root, undo(root, &json!({"confirm": PHRASE})), ""),
        _ => return false,
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

/// Clone `ws` into a new snapshot. `limit`: skip over 2 GiB (a `pre_undo` save has no limit: a clone is cheap,
/// and the undo must not lose the current state). Err: why there is no snapshot.
fn take(root: &Root, ws: &Path, reason: &str, limit: bool) -> Result<Value, String> {
    revalidate(root, ws)?;
    if limit && size_over(ws, SNAP_MAX_BYTES).map_err(|e| format!("cannot measure it: {e}"))? {
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
    let s = json!({"at": rfc3339(at), "id": id, "reason": reason, "workspace": ws});
    write_atomic(&dir.join(format!("{id}.json")), format!("{s}\n").as_bytes(), 0o600).map_err(|e| format!("{id}.json: {e}"))?;
    prune(root, ws);
    Ok(s)
}

/// Keep the last 5 snapshots of `ws`; older ones are removed by their exact paths.
fn prune(root: &Root, ws: &Path) {
    let all = snaps(root, ws);
    let dir = snap_dir(root);
    for s in &all[..all.len().saturating_sub(SNAP_KEEP)] {
        let id = s["id"].as_str().unwrap_or("");
        if safe_id(id) {
            let _ = fs::remove_dir_all(dir.join(id)); // exact path: a snapshot this Root made
            let _ = fs::remove_file(dir.join(format!("{id}.json")));
        }
    }
}

/// Replace the workspace's entries with clones of the snapshot's. The workspace directory itself stays (an
/// owner shell inside it keeps a valid cwd). Its current entries were cloned into a `pre_undo` snapshot first.
fn restore(src: &Path, ws: &Path) -> std::io::Result<()> {
    if !fs::symlink_metadata(src)?.is_dir() {
        return Err(util::bad("the snapshot is not a directory"));
    }
    for e in fs::read_dir(ws)? {
        let p = e?.path();
        if fs::symlink_metadata(&p)?.is_dir() { fs::remove_dir_all(&p)? } else { fs::remove_file(&p)? } // links removed, never followed
    }
    for e in fs::read_dir(src)? {
        let e = e?;
        crate::sys::clonefile(&e.path(), &ws.join(e.file_name()))?;
    }
    Ok(())
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
            let t = session_text(&AccessState { read_home: true, workspace: None, full }, false, Some(&why), home, ih);
            assert!(t.starts_with("[root] no workspace: /private/tmp/demo is outside your home folder. "), "{t}");
            for want in ["must be a folder inside your home (/Users/o), not your home itself", "the agent works in ~/.init/work",
                "/undo has nothing to restore", "cd into a project folder inside your home, and run init-agent there"] {
                assert!(t.contains(want), "{want:?} in {t}");
            }
            assert!(!t.contains("cd into a project and run init-agent to let me edit it"), "the old advice: {t}");
            assert_eq!(t.contains("access FULL"), full, "{t}");
        }
        assert_eq!(refusal_text(pa::Refusal::Home, "/Users/o"), "/Users/o is your home folder itself");
        let w = session_text(&AccessState { read_home: true, workspace: Some("/Users/o/p".into()), full: false }, false, None, home, ih);
        assert_eq!(w, "[root] workspace: /Users/o/p");
    }
}
