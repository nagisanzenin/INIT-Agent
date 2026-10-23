//! SBX-009, ADR-009 (build/specs/ADR-009-spec.md, root/root.md §3.17): the owner's workspace, full mode, the
//! grant of each body spawn, workspace snapshots and `/undo`. The wire protocol is build/status/SBX-009.md
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

/// The Root's access state (ROOT_HOME/state/access.json holds the workspace; full mode never survives a restart).
#[derive(Default)]
pub struct St {
    workspace: Option<PathBuf>,
    full: bool,
    /// Full mode was confirmed in this owner session (each `workspace.set` starts one).
    confirmed: bool,
    /// The live body last spawned: instance id, `INIT_ACCESS`, `INIT_WORKSPACE`.
    live: Option<(String, &'static str, Option<PathBuf>)>,
    /// Fleet members granted the workspace write (`fleet.workspace_write`): instance id → when it was granted.
    writers: std::collections::BTreeMap<String, i64>,
}

/// A member's write slot counts from its grant until the broker records the instance (or the spawn failed).
const WRITER_PENDING_MS: i64 = 60_000;

/// At Root start: the recorded workspace, if it still validates.
pub fn load(p: &crate::install::Paths) -> St {
    let v: Value = fs::read_to_string(p.r("state/access.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let fixed = fixed_of(p);
    let deny = pa::read_deny(&fixed.0, &[]);
    let workspace = v["workspace"].as_str().and_then(|w| pa::validate_workspace(w, &fixed.as_fixed(), &deny).ok()).filter(|w| w.as_os_str() == v["workspace"].as_str().unwrap_or(""));
    St { workspace, ..St::default() }
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

/// The charter's `[sandbox]` ADR-009 keys: (read_home, allow_full, read_deny entries).
fn charter(root: &Root) -> (bool, bool, Vec<PathBuf>) {
    let st = lock(&root.st);
    st.charter.as_ref().map_or((true, true, vec![]), |l| {
        let s = &l.ch.sandbox;
        (s.read_home, s.allow_full, s.read_deny.iter().filter_map(|x| pa::charter_entry(x)).collect())
    })
}

/// The read deny list (`Z_DENY_<i>`): the built-in list, then the charter's.
pub fn deny(root: &Root) -> Vec<PathBuf> {
    pa::read_deny(&home(&root.p), &charter(root).2)
}

/// The write deny list (`Z_WDENY_<i>`, tail Z3a): the built-in list, then INIT's bin directory (`install.json`).
pub fn write_deny(root: &Root) -> Vec<PathBuf> {
    let bin: Vec<PathBuf> = root.info["bin_dir"].as_str().map(PathBuf::from).into_iter().collect();
    pa::write_deny(&home(&root.p), &bin)
}

/// What a workspace may not lie in: both deny lists.
fn ws_deny(root: &Root) -> Vec<PathBuf> {
    let mut d = deny(root);
    d.extend(write_deny(root));
    d
}

fn state(root: &Root) -> AccessState {
    let (read_home, allow_full, _) = charter(root);
    let a = lock(&root.access);
    AccessState { read_home, workspace: a.workspace.clone(), full: a.full && allow_full }
}

/// The grant of a `body`-profile spawn `inst` (kind `body` or `probe`; `live`: its INIT_HOME is the real one).
/// ADR-011 §4.5, §7.4 (INTEG-A): a fleet member (kind `body`, not live) reads the home only, unless an owner
/// `fleet.workspace_write` grant of N is live and a workspace is set: then at most N running members at once (first
/// come) also write the workspace (`INIT_ACCESS=workspace`, never the home in full mode). No grant: unchanged.
pub fn spawn_grant(root: &Root, inst: &str, kind: &str, live: bool) -> SpawnGrant {
    let st = state(root);
    let g = pa::grant_for(kind, live, &st, &home(&root.p));
    let (n, Some(ws)) = (if kind == "body" && !live { crate::grant::workspace_writers(root) } else { 0 }, st.workspace) else { return g };
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
    SpawnGrant { grant: pa::Grant { read_home: g.grant.read_home, write: Some(ws.clone()) }, access: "workspace", workspace: Some(ws) }
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

/// `AccessView` (build/status/SBX-009.md).
pub fn view(root: &Root) -> Value {
    let st = state(root);
    let (read_home, allow_full, _) = charter(root);
    let snap = st.workspace.as_ref().and_then(|w| snaps(root, w).pop());
    json!({"access": st.level(), "allow_full": allow_full, "full": st.full, "read_home": read_home,
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

fn no_ws_text() -> &'static str {
    "[root] no workspace: cd into a project and run init to let me edit it"
}

fn snap_notice(root: &Root, r: &Result<Value, String>) {
    match r {
        Ok(s) => root.notice_code("info", "snapshot", &format!("[root] snapshot {} of {} taken (/undo restores it)", s["id"].as_str().unwrap_or(""), s["workspace"].as_str().unwrap_or(""))),
        Err(why) => root.notice_code("warn", "snapshot", &format!("[root] no snapshot of the workspace: {why}")),
    }
}

/// `workspace.set{cwd}` (spec items 2, 4, 5): a new owner session. Never an error for an invalid cwd.
pub fn workspace_set(root: &Root, a: &Value) -> Result<Value, Err3> {
    let cwd = a["cwd"].as_str().ok_or(("root.args", "workspace.set needs cwd (a string)".to_string(), Value::Null))?;
    let own = fixed_of(&root.p);
    let new = pa::validate_workspace(cwd, &own.as_fixed(), &ws_deny(root));
    let ws = new.as_ref().ok().cloned();
    let (old, was_full) = {
        let mut st = lock(&root.access);
        let old = (st.workspace.clone(), st.full);
        (st.workspace, st.full, st.confirmed) = (ws.clone(), false, false);
        old
    };
    let rec = json!({"v": 1, "workspace": ws, "set_at": rfc3339(now_ms())});
    if let Err(e) = write_atomic(&root.p.r("state/access.json"), format!("{rec}\n").as_bytes(), 0o600) {
        util::log(&root.p.rh, "warn", &format!("state/access.json: {e}"));
    }
    crate::twin::session_started(root, None); // X9: a new owner session; its first line is replayable only with a snapshot
    crate::sentinel::session_started(root); // X3: the sentinel's units never span owner sessions
    if let Some(w) = &ws {
        let r = take(root, w, "session", true);
        snap_notice(root, &r);
        crate::twin::session_started(root, r.as_ref().ok());
    }
    let change = old != ws || was_full;
    let text = ws.as_ref().map_or_else(|| no_ws_text().to_string(), |w| format!("[root] workspace: {}", w.display()));
    if change {
        changed(root, "workspace", &text);
    } else {
        root.notice_code("info", "access", &text);
    }
    let refused = new.err().map(|r| json!({"code": r.code(), "message": r.message()}));
    Ok(json!({"access": view(root), "changed": change, "refused": refused}))
}

fn confirm(a: &Value) -> bool {
    a["confirm"].as_str() == Some(PHRASE)
}

/// `full.set{on, confirm?}` (spec item 3).
pub fn full_set(root: &Root, a: &Value) -> Result<Value, Err3> {
    let on = a["on"].as_bool().ok_or(("root.args", "full.set needs on (a boolean)".to_string(), Value::Null))?;
    let (_, allow_full, _) = charter(root);
    let (full, confirmed, ws) = { let st = lock(&root.access); (st.full && allow_full, st.confirmed, st.workspace.clone()) };
    if !on {
        lock(&root.access).full = false;
        if full {
            changed(root, "full_off", "[root] full mode off: the body goes back to its workspace grant when it restarts at its next idle point");
        }
        return Ok(json!({"access": view(root), "changed": full, "snapshot": null}));
    }
    if !allow_full {
        return Err(("root.mode", "full mode is disabled by the charter (sandbox.allow_full = false)".into(), json!({"reason": "charter"})));
    }
    if full {
        return Ok(json!({"access": view(root), "changed": false, "snapshot": null}));
    }
    if !confirmed && !confirm(a) {
        let snap = ws.as_ref().map_or_else(|| " There is no workspace, so no snapshot covers it.".to_string(),
            |w| format!(" {} is snapshotted first.", w.display()));
        let m = format!("Full mode lets the agent write anywhere in your home except your secrets and the Root's files.{snap} \
            Type `{PHRASE}` to turn it on.");
        return Err(("root.confirm", m, json!({"phrase": PHRASE})));
    }
    let snap = ws.as_ref().map(|w| take(root, w, "full_on", true));
    {
        let mut st = lock(&root.access);
        (st.full, st.confirmed) = (true, true);
    }
    if let Some(r) = &snap {
        snap_notice(root, r);
    }
    let tail = if ws.is_some() { "" } else { " No workspace is set, so no snapshot covers full mode." };
    changed(root, "full_on", &format!("[root] full mode on: the agent may write your whole home except secrets and the Root (from its next restart).{tail}"));
    Ok(json!({"access": view(root), "changed": true, "snapshot": snap.and_then(Result::ok)}))
}

/// `undo{confirm?}` (spec item 4): restore the latest snapshot of the workspace, saving the current state first.
pub fn undo(root: &Root, a: &Value) -> Result<Value, Err3> {
    let ws = lock(&root.access).workspace.clone().ok_or(("root.mode", "no workspace is set".to_string(), json!({"reason": "no_workspace"})))?;
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
    root.append("access", json!({"access": state(root).level(), "event": "undo", "full": state(root).full, "workspace": true}));
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
        root.notice_code("info", "access", &format!("[root] access {}{full} · workspace {ws} · read_home {} · allow_full {}", v["access"].as_str().unwrap_or(""),
            v["read_home"], v["allow_full"]));
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

/// After `charter.apply`: a changed `[sandbox]` is an access change; `allow_full = false` ends full mode.
pub fn charter_applied(root: &Root) {
    let (_, allow_full, _) = charter(root);
    if !allow_full {
        lock(&root.access).full = false;
    }
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
