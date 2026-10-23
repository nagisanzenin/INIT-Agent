//! ADR-011 §4 (lane X4): reversibility. The system snapshot of the agent's state and its retention (§4.2), the R2
//! restore with the zone stopped, the Root-owned probe roots and the A4 rollback probe (§4.3), and the `settled`
//! pointer (§4.1). The clone and restore mechanics are ADR-009's workspace snapshots and `/undo` (access.rs,
//! root.md §3.17.4), applied to `S_A`; the probe roots are sealed run roots (sealed/run.rs, root.md §3.3.6).
//!
//! **What this makes reversible** (§4.4): agent state (`S_A` = `INIT_HOME/body/`, `state/` without `sessions/` and
//! `spill/`, `vault/` without `traces/`) back to the point before a group went live (R2), and, through A4, the
//! guarantee that the group's parent body still boots on the state a candidate writes at boot.
//!
//! **Limits, stated plainly** (§4.4):
//! - A4 covers only state written at boot. State a candidate migrates later, at first use, is covered by R2 alone.
//! - R2 loses the agent state written since the snapshot from the live tree. It is kept in `pre-restore-<n>`, so a
//!   restore can itself be undone, but nothing merges the two.
//! - A snapshot is crash-consistent (taken while the zone may be writing, like a power cut), not
//!   transaction-consistent.
//! - No snapshot above `snap_max_gib` (16 GiB logical), on an unsupported filesystem, or on another volume than
//!   `ROOT_HOME` (A5 refuses the change instead). Linux uses fd-pinned, byte-bounded reflink/copy on explicit local
//!   filesystems; unsupported metadata and special files are refused rather than silently weakened.
//! - The conversation (`state/sessions/`), spill files and request traces are never rolled back.
//!
//! **What nothing here can undo** (§4.6): money spent with the provider; data sent to the provider (the owner's files
//! the agent read included); executed external actions (`git-push`, `email`, non-GET `http`); writes outside the
//! workspace in full mode (no home snapshot exists); deferred execution the owner later runs (build files, scripts,
//! editor configs); the owner's own actions on the agent's output; processes that escaped with `setsid`.
//!
//! File rules (§4.2, exceptions to R-F1/R-F2 beside ADR-009's): a snapshot reads the named `INIT_HOME` trees only by
//! one `clonefile(2)` each (`CLONE_NOFOLLOW`: a tree swapped for a link is cloned as the link and refused, never
//! followed); a restore writes them only while no zone process exists, never following a link and never touching
//! `state/sessions/`, `state/spill/` or `vault/traces/`. Everything else lives under `ROOT_HOME/snapshots/sys/` and
//! `ROOT_HOME/sealed/run/`, which the zone can never read (tail Z1). Deletions are by exact path.

use crate::link::{Root, lock};
use crate::memwatch::{self, Limits};
use crate::sealed::{Env, RunRoot};
use crate::util::{self, now_ms, rfc3339, write_atomic};
use crate::zone::Err3;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `S_A` (§4.2): each tree with the entries that are never snapshotted and never restored.
const TREES: [(&str, &[&str]); 3] = [("body", &[]), ("state", &["sessions", "spill"]), ("vault", &["traces"])];
/// §4.2 retention: at most 16 system snapshots.
const KEEP_MAX: usize = 16;
/// A5: the disk floor plus 2 GiB.
const ROOM: u64 = 2 << 30;
/// A snapshot younger than this is never pruned: its group's `change_open` may not be appended yet.
const FRESH_MS: i64 = 600_000;
/// §4.3 step 2: each arm must write `ready` and exit 0 within 90 s.
pub const ARM_WALL: Duration = Duration::from_secs(90);
const SETTLE_MS: i64 = 7 * 86_400_000;

/// One snapshot, restore or prune at a time.
static OP: Mutex<()> = Mutex::new(());

pub fn dir(rh: &Path) -> PathBuf {
    rh.join("snapshots/sys")
}

fn safe_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && !id.starts_with('.') && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

fn ioe(e: std::io::Error) -> String {
    e.to_string()
}

/// Removes `p` by its exact path: a real directory recursively (`remove_dir_all` never follows links), anything else
/// (a file, a link) itself. Missing is fine.
pub(crate) fn remove(p: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Sum of the regular files' sizes under `p`, never following links.
fn logical(p: &Path) -> u64 {
    let (mut total, mut stack) = (0u64, vec![p.to_path_buf()]);
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            match fs::symlink_metadata(e.path()) {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) if m.is_file() => total += m.len(),
                _ => {}
            }
        }
    }
    total
}

/// Clones `S_A` into a fresh Root-owned destination. APFS uses CLONE_NOFOLLOW and removes exclusions from the clone.
/// Linux pins ancestors, skips excluded trees without opening them and reserves each file/link's logical bytes
/// before copying. Replaced tree roots, unpreserved metadata and concurrent file changes are refused.
fn clone_sa(ih: &Path, dst: &Path, max: u64) -> Result<(), (&'static str, String)> {
    let internal = |message| ("root.internal", message);
    fs::DirBuilder::new().mode(0o700).create(dst).map_err(|e| internal(format!("{}: {e}", dst.display())))?;
    #[cfg(target_os = "linux")]
    let mut remaining = max;
    #[cfg(not(target_os = "linux"))]
    let _ = max; // APFS's clone is checked against the logical quota below.
    for (tree, skip) in TREES {
        let (src, to) = (ih.join(tree), dst.join(tree));
        if fs::symlink_metadata(&src).is_err() {
            continue; // no such tree: nothing to keep (a restore leaves the live one empty)
        }
        #[cfg(target_os = "linux")]
        {
            let copied = crate::sys::clonefile_bounded(&src, &to, remaining, skip).map_err(|e| {
                let code = if e.kind() == std::io::ErrorKind::FileTooLarge { "extend.snapshot_too_large" } else { "root.internal" };
                (code, format!("clonefile INIT_HOME/{tree}: {e}"))
            })?;
            remaining -= copied;
        }
        #[cfg(not(target_os = "linux"))]
        crate::sys::clonefile(&src, &to).map_err(|e| internal(format!("clonefile INIT_HOME/{tree}: {e}")))?;
        if !fs::symlink_metadata(&to).is_ok_and(|m| m.is_dir()) {
            return Err(internal(format!("INIT_HOME/{tree} is not a directory (a link?); it is never followed")));
        }
        for s in skip {
            remove(&to.join(s)).map_err(|e| internal(format!("{tree}/{s}: {e}")))?;
        }
    }
    Ok(())
}

fn snapshot_filesystem_supported(os: &str, filesystem: &str) -> bool {
    match os {
        "macos" => filesystem == "apfs",
        "linux" => matches!(filesystem, "ext2/3/4" | "btrfs" | "xfs" | "overlayfs" | "tmpfs"),
        _ => false,
    }
}

/// Takes snapshot `id` of `ih`'s `S_A` into `dir(rh)/<id>/` plus `<id>.json`. Err: (code, why); nothing is left.
pub fn take_at(rh: &Path, ih: &Path, id: &str, kind: &str, group: Option<u64>, max: u64) -> Result<Value, (&'static str, String)> {
    let big = |m: String| ("extend.snapshot_too_large", m);
    let int = |m: String| ("root.internal", m);
    if !safe_id(id) {
        return Err(int(format!("bad snapshot id {id:?}")));
    }
    let d = dir(rh);
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).map_err(|e| int(format!("snapshots/sys: {e}")))?;
    let filesystem = crate::sys::fs_type(ih).map_err(|e| int(e.to_string()))?;
    if !snapshot_filesystem_supported(std::env::consts::OS, &filesystem) {
        return Err(big(format!("INIT_HOME filesystem {filesystem:?} has no supported system snapshot backend")));
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).map_err(|e| int(e.to_string()));
    if dev(ih)? != dev(&d)? {
        return Err(big("INIT_HOME is on another volume than ROOT_HOME: no system snapshot can be taken".into()));
    }
    let dst = d.join(id);
    if fs::symlink_metadata(&dst).is_ok() || fs::symlink_metadata(d.join(format!("{id}.json"))).is_ok() {
        return Err(int(format!("snapshot {id} exists")));
    }
    let at = now_ms();
    if let Err((code, message)) = clone_sa(ih, &dst, max) {
        let _ = remove(&dst); // exact path, created by clone_sa
        return Err((code, message));
    }
    let bytes = logical(&dst);
    if bytes > max {
        let _ = remove(&dst); // exact path, created above
        return Err(big(format!("agent state is {} MiB, over snap_max_gib = {} GiB", bytes >> 20, max >> 30)));
    }
    let meta = json!({"at": rfc3339(at), "at_ms": at, "group": group, "id": id, "kind": kind, "logical_bytes": bytes, "v": 1});
    if let Err(e) = write_atomic(&d.join(format!("{id}.json")), format!("{meta}\n").as_bytes(), 0o600) {
        let _ = remove(&dst); // exact path, created above
        return Err(int(format!("{id}.json: {e}")));
    }
    Ok(meta)
}

/// Every system snapshot, oldest first.
pub fn metas(rh: &Path) -> Vec<Value> {
    let d = dir(rh);
    let mut v: Vec<Value> = fs::read_dir(&d).into_iter().flatten().flatten().filter_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        let id = n.strip_suffix(".json")?;
        let m: Value = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
        (m["id"] == id && safe_id(id) && fs::symlink_metadata(d.join(id)).is_ok_and(|x| x.is_dir())).then_some(m)
    }).collect();
    v.sort_by_key(|m| (m["at_ms"].as_i64().unwrap_or(0), m["id"].as_str().unwrap_or("").to_string()));
    v
}

/// §4.2 retention, pure: the ids to keep. The snapshots of open groups (`change_open` without `change_close`: their
/// `snapshot` field, or a snapshot taken for that group), the newest snapshot at or before the last `lkg`, `settled`
/// and `twin` moves, the newest `pre_restore`, and anything younger than 10 minutes.
pub fn keep_set(metas: &[Value], records: &[Value], now: i64) -> BTreeSet<String> {
    let mut open: BTreeMap<u64, Option<String>> = BTreeMap::new();
    let mut moved: BTreeMap<&str, i64> = BTreeMap::new();
    let mut twin = None; // X9: the frozen twin's own snapshot, named by its last `twin` record, is pinned by id
    for r in records {
        let (k, d) = (r["kind"].as_str().unwrap_or(""), &r["data"]);
        if k == "twin" {
            twin = d["snapshot"].as_str().map(String::from);
        }
        match k {
            "change_open" => {
                if let Some(g) = d["group"].as_u64() {
                    let e = open.entry(g).or_default();
                    if let Some(s) = d["snapshot"].as_str() {
                        *e = Some(s.to_string());
                    }
                }
            }
            "change_close" => drop(d["group"].as_u64().map(|g| open.remove(&g))),
            "lkg" | "settled" | "twin" => drop(r["at"].as_str().and_then(util::parse_rfc3339).map(|t| moved.insert(k, t))),
            _ => {}
        }
    }
    let id = |m: &Value| m["id"].as_str().unwrap_or("").to_string();
    let sys: Vec<&Value> = metas.iter().filter(|m| m["kind"] == "sys").collect();
    let mut keep: BTreeSet<String> = open.values().flatten().cloned().collect();
    keep.extend(twin);
    keep.extend(sys.iter().filter(|m| m["group"].as_u64().is_some_and(|g| open.contains_key(&g))).map(|m| id(m)));
    for t in moved.values() {
        if let Some(m) = sys.iter().rev().find(|m| m["at_ms"].as_i64().is_some_and(|a| a <= *t)) {
            keep.insert(id(m));
        }
    }
    keep.extend(metas.iter().rev().find(|m| m["kind"] == "pre_restore").map(id));
    keep.extend(metas.iter().filter(|m| now - m["at_ms"].as_i64().unwrap_or(0) < FRESH_MS).map(id));
    keep
}

/// Removes, by exact path, every snapshot `keep_set` does not keep. Returns the ids removed.
fn prune_at(rh: &Path, records: &[Value], now: i64) -> Vec<String> {
    let all = metas(rh);
    let keep = keep_set(&all, records, now);
    let d = dir(rh);
    let mut gone = Vec::new();
    for m in &all {
        let id = m["id"].as_str().unwrap_or("");
        if safe_id(id) && !keep.contains(id) {
            let _ = remove(&d.join(id)); // exact path: a snapshot this Root made
            let _ = fs::remove_file(d.join(format!("{id}.json")));
            gone.push(id.to_string());
        }
    }
    gone
}

/// §4.2 retention on the live Root (X3 calls it after `change_close` and `lkg` moves, sentinel `after_close`).
pub fn prune(root: &Root) {
    let _g = lock(&OP);
    prune_at(&root.p.rh, &crate::lineage::records(&root.p.rh).unwrap_or_default(), now_ms());
}

/// Charter `extend.snap_max_gib` (§9 default 16, C17 bounds [1, 256]) with a live owner grant on top (X7).
fn snap_max(root: &Root) -> u64 {
    crate::grant::snap_max_gib(root) << 30
}

/// §4.2: the system snapshot at a group's first `change_open`, after A4 and before the change goes live. Ok:
/// `{"id","kind":"sys","group","logical_mib"}`, recorded as `snapshot`. Refusals (A5): `extend.snapshot_too_large`
/// (with an alert), `root.quota` (no room: 16 snapshots pinned, or free disk below the floor plus 2 GiB).
pub fn take(root: &Root, group: Option<u64>) -> Result<Value, Err3> {
    let _g = lock(&OP);
    let (rh, ih) = (&root.p.rh, &root.p.ih);
    let floor = lock(&root.st).charter.as_ref().map_or(0, |l| Limits::of(&l.ch).floor);
    let free = crate::sys::free_bytes(ih).unwrap_or(u64::MAX);
    if free < floor + ROOM {
        return Err(("root.quota", format!("free disk {} MiB is below the workload floor plus 2 GiB: no system snapshot, no change", free >> 20),
            json!({"reason": "disk"})));
    }
    let records = crate::lineage::records(rh).unwrap_or_default();
    prune_at(rh, &records, now_ms());
    let all = metas(rh);
    // a group-less snapshot (X9's twin freeze) is `t<ms>`, or `t<ms>-<n>` when that id is taken (same ms, fake clock)
    let free_id = |b: String| (1..100).map(|n| if n == 1 { b.clone() } else { format!("{b}-{n}") }).find(|i| !all.iter().any(|m| m["id"] == i.as_str()))
        .unwrap_or(b);
    let id = group.map_or_else(|| free_id(format!("t{}", now_ms())), |g| g.to_string());
    let keep = keep_set(&all, &records, i64::MAX / 2); // what stays pinned whatever its age (nothing counts as fresh)
    if keep.len() >= KEEP_MAX {
        return Err(("root.quota", format!("{KEEP_MAX} system snapshots are pinned (open groups and pointers): close a group first"),
            json!({"reason": "snapshots"})));
    }
    if all.iter().any(|m| m["id"] == id.as_str()) && !keep.contains(&id) {
        let _ = remove(&dir(rh).join(&id)); // an orphan of a refused change: exact path
        let _ = fs::remove_file(dir(rh).join(format!("{id}.json")));
    }
    match take_at(rh, ih, &id, "sys", group, snap_max(root)) {
        Ok(m) => {
            let mib = m["logical_bytes"].as_u64().unwrap_or(0).div_ceil(1 << 20);
            root.append("snapshot", json!({"group": group, "id": id, "kind": "sys", "logical_mib": mib, "skipped": null}));
            Ok(json!({"group": group, "id": id, "kind": "sys", "logical_mib": mib}))
        }
        Err((code, m)) => {
            if code == "extend.snapshot_too_large" {
                root.append("snapshot", json!({"group": group, "id": null, "kind": "sys", "logical_mib": null, "skipped": "too_large"}));
                root.alert("snapshot_too_large", &format!("[root] no system snapshot: {m}; self-changes are refused until it fits (or grant extend.snap_max_gib)"));
            }
            Err((code, m, json!({"group": group})))
        }
    }
}

/// R2 on paths: restores snapshot `id` into `ih` while `live()` stays false. The current `S_A` is first cloned to
/// `pre-restore-<n>`. Ok: `{"restored","saved","saved_bytes"}`.
pub fn restore_at(rh: &Path, ih: &Path, id: &str, live: &dyn Fn() -> bool) -> Result<Value, Err3> {
    let busy = |m: String| ("root.busy", m, json!({"reason": "zone_live"}));
    let int = |m: String| ("root.internal", m, Value::Null);
    if live() {
        return Err(busy("a zone process is alive: R2 restores only with the zone stopped (stop, then sweep)".into()));
    }
    let d = dir(rh);
    let src = d.join(id);
    if !safe_id(id) || !metas(rh).iter().any(|m| m["id"] == id) {
        return Err(("root.not_found", format!("no system snapshot {id:?}"), Value::Null));
    }
    let n = metas(rh).iter().filter_map(|m| m["id"].as_str()?.strip_prefix("pre-restore-")?.parse::<u64>().ok()).max().unwrap_or(0) + 1;
    let saved = format!("pre-restore-{n}");
    let sm = take_at(rh, ih, &saved, "pre_restore", None, u64::MAX).map_err(|(_, m)| int(format!("could not save the current state ({m}); nothing changed")))?;
    let partial = |m: String| format!("{m}; the state before the restore is snapshot {saved}");
    for (tree, skip) in TREES {
        if live() {
            return Err(busy(partial(format!("a zone process started during the restore (before {tree}/)"))));
        }
        let (t, s) = (ih.join(tree), src.join(tree));
        let has = fs::symlink_metadata(&s).is_ok_and(|m| m.is_dir());
        match fs::symlink_metadata(&t) {
            Ok(m) if m.is_dir() => {
                for e in fs::read_dir(&t).map_err(|e| int(partial(format!("{tree}/: {e}"))))? {
                    let e = e.map_err(|e| int(partial(e.to_string())))?;
                    if !skip.iter().any(|x| e.file_name() == *x) {
                        remove(&e.path()).map_err(|x| int(partial(format!("{tree}/{}: {x}", e.file_name().to_string_lossy()))))?;
                    }
                }
            }
            Ok(_) => {
                fs::remove_file(&t).map_err(|e| int(partial(format!("{tree}: {e}"))))?; // a link or a file in its place
                fs::DirBuilder::new().mode(0o700).create(&t).map_err(|e| int(partial(format!("{tree}: {e}"))))?;
            }
            Err(_) if has => fs::DirBuilder::new().mode(0o700).create(&t).map_err(|e| int(partial(format!("{tree}: {e}"))))?,
            Err(_) => continue,
        }
        if has {
            for e in fs::read_dir(&s).map_err(|e| int(partial(e.to_string())))? {
                let e = e.map_err(|e| int(partial(e.to_string())))?;
                if !skip.iter().any(|x| e.file_name() == *x) {
                    crate::sys::clonefile(&e.path(), &t.join(e.file_name())).map_err(|x| int(partial(format!("clone {tree}/{}: {x}", e.file_name().to_string_lossy()))))?;
                }
            }
        }
    }
    Ok(json!({"restored": id, "saved": saved, "saved_bytes": sm["logical_bytes"]}))
}

/// R2 (§4.2): restore snapshot `id` with the zone stopped; refused `root.busy` while any zone instance lives. The
/// caller stops and sweeps the zone first (root.md §3.7.3 step 1) and starts the target tuple after (step 5).
pub fn restore(root: &Root, id: &str, reason: &str) -> Result<Value, Err3> {
    let _g = lock(&OP);
    let live = || root.zone.count(None) > 0;
    let r = restore_at(&root.p.rh, &root.p.ih, id, &live)?;
    let mib = r["saved_bytes"].as_u64().unwrap_or(0).div_ceil(1 << 20);
    root.append("snapshot", json!({"group": null, "id": r["saved"], "kind": "pre_restore", "logical_mib": mib, "skipped": null}));
    root.append("restore", json!({"kept": ["sessions", "spill", "traces"], "reason": reason, "saved": r["saved"], "snapshot": id}));
    root.notice_code("alert", "snapshot", &format!("[root] agent state restored to system snapshot {id} ({reason}); the state before it is {}",
        r["saved"].as_str().unwrap_or("")));
    prune_at(&root.p.rh, &crate::lineage::records(&root.p.rh).unwrap_or_default(), now_ms());
    Ok(r)
}

/// `sentinel::Hooks.restore` (X3's H1 escalation, between root.md §3.7.3 steps 1 and 5).
fn restore_hook(root: &Root, id: &str, reason: &str) -> Result<(), String> {
    restore(root, id, reason).map(drop).map_err(|(c, m, _)| format!("{c}: {m}"))
}

// ---------------------------------------------------------------------------------------------
// A4: the Root-run rollback probe (§4.3) in a Root-owned probe root (a sealed run root)

/// One arm of the probe: a tuple's body binary, commit, optional `t0` blob and ext components (also X9's replay arms).
pub(crate) struct Arm {
    pub(crate) binary: String,
    pub(crate) sha: String,
    t0: Option<String>,
    pub(crate) ext: Vec<(String, String)>,
}

fn hex(s: &str, n: &[usize]) -> bool {
    n.contains(&s.len()) && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn arm_of(t: &Value) -> Result<Arm, String> {
    let b = &t["body"];
    let binary = b["binary"].as_str().filter(|s| hex(s, &[64])).ok_or("body.binary: the body binary's sha256 (no stored body binary)")?;
    let sha = b["sha"].as_str().filter(|s| hex(s, &[40, 64])).ok_or("body.sha: a 40- or 64-hex commit")?;
    let t0 = match &b["t0"] {
        Value::Null => None,
        v => Some(v.as_str().filter(|s| hex(s, &[64])).ok_or("body.t0: a sha256")?.to_string()),
    };
    let mut ext = Vec::new();
    for c in t["ext"]["components"].as_array().into_iter().flatten() {
        let name = c["name"].as_str().filter(|n| safe_id(n) && !n.contains('.')).ok_or("ext component name: ^[A-Za-z0-9_-]+$")?;
        let sha = c["sha256"].as_str().filter(|s| hex(s, &[64])).ok_or("ext component sha256")?;
        ext.push((name.to_string(), sha.to_string()));
    }
    Ok(Arm { binary: binary.into(), sha: sha.into(), t0, ext })
}

pub(crate) fn store_blob(rh: &Path, sha: &str) -> Result<PathBuf, String> {
    crate::tuple::blob(rh, sha).filter(|b| b.is_file()).ok_or_else(|| format!("blob {sha} is not in the store"))
}

/// The arm's T0 into `body/` (replacing the previous arm's): its `t0` blob (tuple v2), else the kernel-exported
/// tree `INIT_HOME/lineage/trees/<sha12>/` (as sealed body runs take theirs; never the agent-writable `body/`).
pub(crate) fn place_t0(ih: &Path, rh: &Path, r: &Path, a: &Arm) -> Result<(), String> {
    let body = r.join("body");
    remove(&body).map_err(ioe)?;
    match &a.t0 {
        Some(t0) => crate::sealed::extract(&store_blob(rh, t0)?, &body)?,
        None => {
            let tree = ih.join("lineage/trees").join(&a.sha[..12]);
            fs::DirBuilder::new().mode(0o700).create(&body).map_err(ioe)?;
            for f in ["GENESIS.md", "SELF.md", "self.json"] {
                if fs::symlink_metadata(tree.join(f)).is_ok_and(|m| m.is_file()) {
                    fs::copy(tree.join(f), body.join(f)).map_err(ioe)?;
                }
            }
            if fs::symlink_metadata(tree.join("skills")).is_ok_and(|m| m.is_dir()) {
                let ok = Command::new("/bin/cp").arg("-R").arg(tree.join("skills")).arg(body.join("skills")).status().is_ok_and(|s| s.success());
                ok.then_some(()).ok_or(format!("copy {}/skills failed", tree.display()))?;
            }
        }
    }
    let ok = ["GENESIS.md", "SELF.md"].iter().all(|f| body.join(f).is_file());
    ok.then_some(()).ok_or_else(|| format!("no T0 for body {} (no t0 blob, no exported tree)", &a.sha[..12]))
}

/// The body's session id rule (`^[0-9A-Za-z._-]{1,64}$`).
fn session_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Runs one arm in mode `check` on the probe root's state. Ok: `{"exit":0,"ms"}`; Err: why (for the refusal).
fn run_arm(env: &Env, run: &RunRoot, a: &Arm, session: &str, wall: Duration) -> Result<Value, (&'static str, String, Value)> {
    let r = &run.dir;
    let int = |m: String| ("root.internal", m, Value::Null);
    let store = |m: String| ("root.store", m, Value::Null);
    place_t0(&env.ih, &env.rh, r, a).map_err(store)?;
    let bin = r.join("bin/init-body");
    remove(&bin).map_err(|e| int(e.to_string()))?;
    fs::copy(store_blob(&env.rh, &a.binary).map_err(store)?, &bin).and_then(|_| fs::set_permissions(&bin, fs::Permissions::from_mode(0o500)))
        .map_err(|e| int(format!("bin/init-body: {e}")))?;
    let ext = r.join("bin/ext");
    remove(&ext).map_err(|e| int(e.to_string()))?;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&ext).map_err(|e| int(e.to_string()))?;
    for (name, sha) in &a.ext {
        fs::copy(store_blob(&env.rh, sha).map_err(store)?, ext.join(format!("{name}.wasm"))).map_err(|e| int(format!("bin/ext/{name}.wasm: {e}")))?;
    }
    for d in ["kernel", "state/kernel", "state/sessions", "state/run", "work/tmp", "work/home", "run"] {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(r.join(d)).map_err(|e| int(e.to_string()))?;
    }
    for f in ["CONSTITUTION.md", "config.toml"] {
        let src = env.ih.join("kernel").join(f);
        if !r.join("kernel").join(f).exists() && fs::symlink_metadata(&src).is_ok_and(|m| m.is_file()) {
            fs::copy(&src, r.join("kernel").join(f)).map_err(|e| int(format!("kernel/{f}: {e}")))?;
        }
    }
    let boot_id = util::hex(&util::random::<8>().map_err(|e| int(e.to_string()))?);
    let bj = r.join("state/kernel/boot.json");
    let mut b: Value = fs::read(&bj).ok().and_then(|x| serde_json::from_slice(&x).ok()).filter(Value::is_object).unwrap_or_else(|| json!({"v": 1}));
    for (k, v) in [("body_id", json!("a4")), ("boot_id", json!(boot_id)), ("boot_seq", json!(0)), ("branch", Value::Null), ("cause", Value::Null),
        ("crash", Value::Null), ("from_sha", Value::Null), ("mode", json!("check")), ("reason", json!("cold")), ("provisional", Value::Null),
        ("safe", Value::Null), ("session", json!(session)), ("sha", json!(a.sha)), ("spawned_at", json!(rfc3339(now_ms()))),
        ("update_failed", Value::Null)] {
        b[k] = v;
    }
    b["env"]["init_home"] = json!(r);
    let kv = b["env"]["kernel_version"].as_str().unwrap_or("0.1.0").to_string();
    write_atomic(&bj, format!("{b}\n").as_bytes(), 0o600).map_err(|e| int(format!("boot.json: {e}")))?;
    let ready = r.join("state/run/ready");
    remove(&ready).map_err(|e| int(e.to_string()))?;
    let lim = Limits::of(&env.ch);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut c = crate::sealed::sandboxed(env, run, None, &[bin.clone().into()]);
    c.envs([("INIT_HOME", s(r)), ("INIT_SESSION", session.to_string()), ("INIT_SHA", a.sha.clone()), ("INIT_BOOT_ID", boot_id.clone()),
        ("INIT_BOOT_REASON", "cold".into()), ("INIT_BODY_ID", "a4".into()), ("INIT_MODE", "check".into()), ("INIT_UI", "stdio-jsonl/1".into()),
        ("INIT_KERNEL_VERSION", kv), ("INIT_KERNEL_SOCK", s(&env.rh.join("run").join(format!("sk-{}.sock", run.id)))),
        ("HOME", s(&r.join("work/home"))), ("TMPDIR", format!("{}/", s(&r.join("work/tmp"))))])
        .envs(memwatch::body_env(&lim, &run.events_path())).current_dir(r.join("work"));
    let t0 = Instant::now();
    let (code, why, _) = crate::sealed::supervise(c, run, &lim, wall, None, || None).map_err(|m| ("root.quota", m, json!({"reason": "disk"})))?;
    let rd: Value = fs::read(&ready).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default();
    let wrote = rd["boot_id"] == boot_id.as_str() && rd["mode"] == "check";
    if why == "done" && code == Some(0) && wrote {
        return Ok(json!({"exit": 0, "ms": t0.elapsed().as_millis() as u64}));
    }
    let what = if why != "done" { why.to_string() } else if code != Some(0) { format!("exit {code:?}") } else { "no ready".into() };
    Err(("extend.rollback_incompatible", what, json!({"exit": code, "ready": wrote, "why": why})))
}

/// A4 on a sealed `Env` (no Root needed): the candidate, then the parent, each in mode `check` on an isolated copy of
/// the live `state/` without `spill/` (the parent on the state the candidate left). The probe root is a sealed run
/// root, removed by its exact path when this returns.
pub fn probe(env: &Env, cand: &Value, parent: &Value, wall: Duration) -> Result<Value, Err3> {
    let refuse = |arm: &str, (code, m, mut d): Err3| {
        if code == "extend.rollback_incompatible" {
            d["arm"] = json!(arm);
            let who = if arm == "candidate" { "the candidate body does not boot in mode check" } else { "the parent body cannot boot on the state the candidate leaves" };
            return (code, format!("A4: {who} ({m}); a rollback could not restore a working system"), d);
        }
        (code, format!("A4 {arm}: {m}"), d)
    };
    let (c, p) = (arm_of(cand).map_err(|m| ("root.args", format!("A4 candidate: {m}"), Value::Null))?,
        arm_of(parent).map_err(|m| ("root.store", format!("A4 parent: {m}"), Value::Null))?);
    let run = RunRoot::new(&env.rh).map_err(|m| ("root.internal", m, Value::Null))?;
    let st = run.dir.join("state");
    if fs::symlink_metadata(env.ih.join("state")).is_ok() {
        crate::sys::clonefile(&env.ih.join("state"), &st).map_err(|e| ("root.internal", format!("clonefile INIT_HOME/state: {e}"), Value::Null))?;
    }
    if !fs::symlink_metadata(&st).is_ok_and(|m| m.is_dir()) {
        remove(&st).map_err(|e| ("root.internal", e.to_string(), Value::Null))?; // a link cloned as a link: never followed
        fs::DirBuilder::new().mode(0o700).create(&st).map_err(|e| ("root.internal", e.to_string(), Value::Null))?;
    }
    for x in ["spill", "run/ready", "run/heartbeat"] {
        remove(&st.join(x)).map_err(|e| ("root.internal", e.to_string(), Value::Null))?;
    }
    let session = fs::read_to_string(st.join("session")).ok().map(|s| s.trim().to_string()).filter(|s| session_ok(s))
        .map_or_else(|| util::random::<6>().map(|b| format!("a4-{}", util::hex(&b))), Ok).map_err(|e| ("root.internal", e.to_string(), Value::Null))?;
    let rc = run_arm(env, &run, &c, &session, wall).map_err(|e| refuse("candidate", e))?;
    let rp = run_arm(env, &run, &p, &session, wall).map_err(|e| refuse("parent", e))?;
    Ok(json!({"candidate": rc, "parent": rp}))
}

/// A4 on the live Root (`extend.rollback_incompatible`, `root.busy` while a sealed operation runs, `root.store`).
pub fn a4(root: &Root, cand: &Value, parent: &Value) -> Result<Value, Err3> {
    let env = Env::of(root).map_err(|m| ("root.internal", m, Value::Null))?;
    let Some(_g) = crate::sealed::try_begin(&root.p.rh) else {
        return Err(("root.busy", "a sealed operation is running; retry the change after it".into(), json!({"reason": "sealed"})));
    };
    probe(&env, cand, parent, ARM_WALL)
}

/// `sentinel::Hooks.admit`: A4 for `t2` and `dep` (the candidate body against the group's parent), then the system
/// snapshot when the change opens a group. Ok: `{"A4": "pass"|"skip", "snapshot": id|null}`.
pub fn admit(root: &Root, a: &Value, first: bool) -> Result<Value, Err3> {
    let a4v = if matches!(a["scope"].as_str(), Some("t2" | "dep")) {
        let pid = a["parent_tuple"].as_str().map(String::from).or_else(|| lock(&root.sup.ptrs).current.clone())
            .ok_or(("root.store", "A4: no parent tuple".to_string(), Value::Null))?;
        let parent = crate::tuple::load(&root.p.rh, &pid).ok_or(("root.store", format!("A4: tuple {pid} is not in the store"), Value::Null))?;
        let mut cand = parent.clone();
        cand["body"] = a["candidate"]["body"].clone();
        if a["candidate"]["ext"].is_object() {
            cand["ext"] = a["candidate"]["ext"].clone();
        }
        a4(root, &cand, &parent)?;
        "pass"
    } else {
        "skip"
    };
    let snap = if first { take(root, a["group"].as_u64())?["id"].clone() } else { Value::Null };
    Ok(json!({"A4": a4v, "snapshot": snap}))
}

/// Registers the A4/snapshot admission hook and R2 with the sentinel (X3).
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut h = lock(crate::sentinel::hooks());
        h.admit.push(admit);
        h.restore = Some(restore_hook);
    });
}

// ---------------------------------------------------------------------------------------------
// The `settled` pointer (§4.1)

/// `settled`, pure: the newest tuple that became `lkg` at least 7 days before `now` with no `direct` rollback
/// (`change_close{attribution:"direct"}`) of one of its groups and no quarantine (`cap_state{to:"quarantined"}`) of a
/// cap version one of its groups added or modified. A tuple's groups are those whose `change_open` named it.
pub fn settled_of(records: &[Value], now: i64) -> Option<String> {
    let mut groups: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    let mut caps: BTreeMap<u64, BTreeSet<(String, String)>> = BTreeMap::new();
    let (mut direct, mut quarantined) = (BTreeSet::new(), BTreeSet::new());
    let mut lkg: Vec<(String, i64)> = Vec::new();
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    for r in records {
        let d = &r["data"];
        match r["kind"].as_str() {
            Some("change_open") => {
                let Some(g) = d["group"].as_u64() else { continue };
                groups.entry(s(&d["tuple"])).or_default().insert(g);
                for c in d["caps"].as_array().into_iter().flatten().filter(|c| c["op"] != "remove") {
                    caps.entry(g).or_default().insert((s(&c["id"]), s(&c["ver"])));
                }
            }
            Some("change_close") if d["attribution"] == "direct" => drop(d["group"].as_u64().map(|g| direct.insert(g))),
            Some("cap_state") if d["to"] == "quarantined" => drop(quarantined.insert((s(&d["cap"]), s(&d["ver"])))),
            Some("lkg") => {
                if let (Some(t), Some(at)) = (d["tuple"].as_str(), r["at"].as_str().and_then(util::parse_rfc3339)) {
                    lkg.push((t.to_string(), at));
                }
            }
            _ => {}
        }
    }
    let none = BTreeSet::new();
    lkg.iter().rev().find(|(t, at)| {
        let gs = groups.get(t).unwrap_or(&none);
        now - at >= SETTLE_MS && !gs.iter().any(|g| direct.contains(g) || caps.get(g).is_some_and(|c| !c.is_disjoint(&quarantined)))
    }).map(|(t, _)| t.clone())
}

/// Appends `settled{tuple}` when the pointer moves. X3 calls it after each `change_close` and `lkg` move (sentinel
/// `after_close`).
pub fn settled_tick(root: &Root) -> Option<String> {
    let s = settled_of(&crate::lineage::records(&root.p.rh).unwrap_or_default(), now_ms())?;
    if lock(&root.sup.ptrs).settled.as_deref() != Some(s.as_str()) {
        root.append("settled", json!({"tuple": s}));
    }
    Some(s)
}

#[cfg(test)]
mod tests;
