//! R1: whole-system tuples, the blob store, the `current`/`lkg`/`audited` pointers and the rollback
//! target choice (root/root.md §3.7). Pointers are folded from the lineage; `state/root.json` caches them.
//! ADR-011 §4.1 (X4): the `settled` pointer, moved by `snapshot::settled_tick`.

use crate::install::{Paths, VERSION};
use crate::link::{Root, lock};
use crate::util::{self, bad, hex, icj, write_atomic};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAX_BLOB: u64 = 512 << 20;

/// The three pointers of §3.7.2, plus the first tuple ever formed (the genesis fallback).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ptrs {
    pub current: Option<String>,
    pub lkg: Option<String>,
    pub audited: Option<String>,
    pub genesis: Option<String>,
    /// ADR-011 §4.1: the newest tuple `lkg` ≥ 7 days with no direct rollback or quarantine since (X4).
    pub settled: Option<String>,
    /// ADR-011 Evaluation plan: the frozen twin, moved only by a `twin` record (X9: freeze, owner re-freeze).
    pub twin: Option<String>,
}

impl Ptrs {
    /// Folds one lineage record: `tuple` (formed, now current), `rollback.to`, `lkg`, `audited`.
    pub fn apply(&mut self, r: &Value) {
        let (d, s) = (&r["data"], |v: &Value| v.as_str().map(String::from));
        match r["kind"].as_str() {
            Some("tuple") => {
                self.current = s(&d["id"]);
                self.genesis = self.genesis.take().or_else(|| self.current.clone());
            }
            Some("rollback") => self.current = s(&d["to"]),
            Some("lkg") => self.lkg = s(&d["tuple"]),
            Some("audited") => self.audited = s(&d["tuple"]),
            Some("settled") => self.settled = s(&d["tuple"]),
            Some("twin") => self.twin = s(&d["tuple"]),
            _ => {}
        }
    }
    pub fn json(&self) -> Value {
        json!({"audited": self.audited, "current": self.current, "lkg": self.lkg, "settled": self.settled, "twin": self.twin})
    }
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn blob(rh: &Path, sha: &str) -> Option<PathBuf> {
    hex64(sha).then(|| rh.join("store/blobs").join(sha))
}

pub(crate) fn sha_of(p: &Path) -> Option<String> {
    let mut f = File::open(p).ok()?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop {
        match f.read(&mut buf).ok()? {
            0 => return Some(hex(&h.finalize())),
            n => h.update(&buf[..n]),
        }
    }
}

/// §3.7.1: reads `r` to EOF while hashing into a temp file, compares with `want`, stores `blobs/<sha256>`
/// (0444, R-F3). The same bytes are never stored twice. `store_max` bounds the whole store.
pub fn put(rh: &Path, r: &mut dyn Read, want: Option<&str>, store_max: u64) -> Result<String, String> {
    if want.is_some_and(|w| !hex64(w)) {
        return Err("sha256 must be 64 lowercase hex".into());
    }
    let dir = rh.join("store/blobs");
    let used: u64 = fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    let tmp = dir.join(format!(".put.{}", hex(&util::random::<6>().map_err(|e| e.to_string())?)));
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o444).custom_flags(libc::O_NOFOLLOW).open(&tmp).map_err(|e| e.to_string())?;
        let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
        loop {
            let k = r.read(&mut buf).map_err(|e| e.to_string())?;
            if k == 0 {
                break;
            }
            n += k as u64;
            if n > MAX_BLOB || used + n > store_max {
                return Err("too large, or the store is full".to_string());
            }
            h.update(&buf[..k]);
            f.write_all(&buf[..k]).map_err(|e| e.to_string())?;
        }
        f.sync_all().map_err(|e| e.to_string())?;
        let sha = hex(&h.finalize());
        if want.is_some_and(|w| w != sha) {
            return Err(format!("hash mismatch: the bytes hash to {sha}"));
        }
        Ok(sha)
    })();
    let dst = res.as_ref().ok().and_then(|s| blob(rh, s));
    match dst {
        Some(d) if !d.exists() => {
            fs::rename(&tmp, &d).map_err(|e| e.to_string())?;
            File::open(&dir).and_then(|d| d.sync_all()).map_err(|e| e.to_string())?;
        }
        _ => drop(fs::remove_file(&tmp)), // exact path, created above
    }
    res
}

/// `put` for a file the Root itself holds (the installed kernel binaries): an APFS clone of it (`fs::copy`)
/// into a temp file, hashed after the copy, then stored like `put` (0444, never twice). The store then costs
/// no second copy of a 150 MB debug kernel.
pub fn put_file(rh: &Path, src: &Path, store_max: u64) -> Result<String, String> {
    let dir = rh.join("store/blobs");
    let used: u64 = fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    let tmp = dir.join(format!(".put.{}", hex(&util::random::<6>().map_err(|e| e.to_string())?)));
    if fs::symlink_metadata(&tmp).is_ok() {
        return Err("temp name in use".into());
    }
    let res = (|| {
        let n = fs::copy(src, &tmp).map_err(|e| format!("{}: {e}", src.display()))?;
        if n > MAX_BLOB || used + n > store_max {
            return Err("too large, or the store is full".to_string());
        }
        fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o444)).map_err(|e| e.to_string())?;
        File::open(&tmp).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        sha_of(&tmp).ok_or_else(|| "cannot read the copy".to_string())
    })();
    match res.as_ref().ok().and_then(|s| blob(rh, s)) {
        Some(d) if !d.exists() => {
            fs::rename(&tmp, &d).map_err(|e| e.to_string())?;
            File::open(&dir).and_then(|d| d.sync_all()).map_err(|e| e.to_string())?;
        }
        _ => drop(fs::remove_file(&tmp)), // exact path, created above
    }
    res
}

/// The tuple id: `t` + the first 12 hex of SHA-256(ICJ(tuple)).
pub fn id_of(t: &Value) -> String {
    format!("t{}", &util::sha256_hex(icj(t).unwrap_or_default().as_bytes())[..12])
}

pub fn save(rh: &Path, t: &Value) -> io::Result<String> {
    let id = id_of(t);
    let f = rh.join("store/tuples").join(format!("{id}.json"));
    if !f.exists() {
        write_atomic(&f, format!("{}\n", icj(t)?).as_bytes(), 0o444)?;
    }
    Ok(id)
}

pub fn load(rh: &Path, id: &str) -> Option<Value> {
    let t: Value = serde_json::from_str(&fs::read_to_string(rh.join("store/tuples").join(format!("{id}.json"))).ok()?).ok()?;
    (id_of(&t) == id).then_some(t)
}

/// Every blob a tuple names, as (sha256, required).
fn shas(t: &Value) -> Vec<(&str, bool)> {
    fn s(v: &Value) -> &str {
        v.as_str().unwrap_or("")
    }
    let mut v = vec![(s(&t["kernel"]["init_kernel"]), true), (s(&t["kernel"]["initctl"]), false), (s(&t["body"]["binary"]), false)];
    v.extend(t["ext"]["components"].as_array().into_iter().flatten().map(|c| (s(&c["sha256"]), true)));
    v
}

/// §3.7.3 step 2: every blob present and intact (an optional blob may be empty).
pub fn verify(rh: &Path, t: &Value) -> bool {
    shas(t).into_iter().all(|(sha, req)| (!req && sha.is_empty()) || blob(rh, sha).and_then(|b| sha_of(&b)).as_deref() == Some(sha))
}

/// §3.7.3 step 3: `init-kernel` and `initctl` from the store into `INIT_HOME/kernel/bin`, each a new inode.
pub fn install_kernel(p: &Paths, t: &Value) -> io::Result<()> {
    for (name, key) in [("init-kernel", "init_kernel"), ("initctl", "initctl")] {
        let sha = t["kernel"][key].as_str().unwrap_or("");
        let dst = p.i("kernel/bin").join(name);
        if sha.is_empty() || sha_of(&dst).as_deref() == Some(sha) {
            continue;
        }
        let src = blob(&p.rh, sha).ok_or_else(|| bad("bad sha256"))?;
        util::copy_atomic(&src, &dst, 0o755)?;
    }
    Ok(())
}

/// The kernel half of a maintainer tuple: the Root-written `kernel/bin` binaries, stored as blobs, and the
/// kernel source commit (`lineage/kernel.git` `main`, §3.8.1; "" when no source bundle was imported).
/// `None`: no kernel installed.
pub fn kernel_part(root: &Root) -> Result<Option<Value>, String> {
    let (kb, kc) = (root.p.i("kernel/bin/init-kernel"), root.p.i("kernel/bin/initctl"));
    if !kb.is_file() {
        return Ok(None);
    }
    let max = store_max(root);
    let (k, c) = (put_file(&root.p.rh, &kb, max)?, if kc.is_file() { put_file(&root.p.rh, &kc, max)? } else { String::new() });
    let git = root.info["git"].as_str().unwrap_or("/usr/bin/git");
    let built_from = crate::lineage::kgit::get(&root.p, git, "refs/heads/main").unwrap_or_default();
    Ok(Some(json!({"built_from": built_from, "init_kernel": k, "initctl": c, "source": "maintainer", "version": VERSION})))
}

/// The first tuple when none exists yet and no seed was installed (homes made without `--seed-source`):
/// the Root-written kernel binaries plus an empty body (the kernel then keeps its own `bin/init-body`,
/// kernel-supervise `restore_from_tuple`). `None`: no kernel installed. With a seed, install forms the
/// genesis tuple from the install-mode kernel's promotion instead (`on_promotion`).
pub fn genesis(root: &Root) -> Result<Option<String>, String> {
    let Some(kernel) = kernel_part(root)? else { return Ok(None) };
    let t = json!({"body": {"binary": "", "sha": ""}, "ext": {"components": [], "set": null}, "kernel": kernel, "v": 1});
    let id = save(&root.p.rh, &t).map_err(|e| e.to_string())?;
    root.append("tuple", json!({"id": id, "tuple": t}));
    root.append("lkg", json!({"tuple": id}));
    root.append("audited", json!({"tuple": id}));
    Ok(Some(id))
}

pub fn store_max(root: &Root) -> u64 {
    (lock(&root.st).charter.as_ref().map_or(8, |l| l.ch.quotas.store_max_gib).max(0) as u64) << 30
}

/// §3.7.3 steps 2–3: the first of `want`, LKG, settled (ADR-011 §4.1), audited-good, genesis whose blobs verify, with its kernel
/// installed. Returns the id and the `rollback` record data; the caller appends it (and so moves `current`).
pub fn rollback(root: &Root, want: &'static str, reason: &str) -> Result<(String, Value), String> {
    let p = lock(&root.sup.ptrs).clone();
    let get = |w: &str| match w { "lkg" => p.lkg.clone(), "settled" => p.settled.clone(), "audited" => p.audited.clone(),
        "genesis" => p.genesis.clone(), _ => p.current.clone() };
    for target in ORDER.map(|t| if t.is_empty() { want } else { t }) {
        let Some(id) = get(target) else { continue };
        let Some(t) = load(&root.p.rh, &id).filter(|t| verify(&root.p.rh, t)) else { continue };
        install_kernel(&root.p, &t).map_err(|e| format!("install {id}: {e}"))?;
        return Ok((id.clone(), json!({"from": p.current, "reason": reason, "target": target, "to": id})));
    }
    Err(none_usable(want))
}

/// The rollback order after the wanted pointer (`""`).
const ORDER: [&str; 5] = ["", "lkg", "settled", "audited", "genesis"];

/// R-UNDO (v0.5.2, the post-release review of v0.5.1): the refusal names every pointer `rollback` tried, `settled`
/// included, in its order.
fn none_usable(want: &str) -> String {
    let name = |t: &str| match t { "lkg" => "LKG", "audited" => "audited-good", "" | "current" => "current", other => other }.to_string();
    let mut tried: Vec<String> = Vec::new();
    for t in ORDER.map(|t| if t.is_empty() { want } else { t }) {
        if !tried.contains(&name(t)) {
            tried.push(name(t));
        }
    }
    let last = tried.pop().unwrap_or_default();
    format!("no usable tuple ({} and {last} all fail to verify)", tried.join(", "))
}

/// An owner upgrade's commit (upgrade.rs; §3.7.2): the rollback anchors of a demotion and a panic resume (`audited`, and
/// `settled` on the free path; `ledger::anchor`) and the owner's `/rollback twin` target (`twin`) keep their body and
/// ext and take the kernel of the upgraded tuple `to`, so no later rollback reinstalls the kernel from before the
/// upgrade under the new Root. Each pointer whose kernel is not `to`'s moves (also when the upgrade kept the kernel: an
/// anchor a v0.5.0 commit left on an older kernel moves then too) and gets an `audited`, `settled` or `twin` record
/// `{tuple, from, upgrade}`; the twin's also keeps its freeze (`snapshot`, `reason`, `frozen_at`; twin.rs `pin_of`). A
/// pointer whose tuple file cannot be read is never skipped silently: its record is `{tuple: null, from, skipped,
/// upgrade}`, which clears it, because it cannot serve a rollback and its file coming back later must not bring the old
/// kernel back. Returns this upgrade's moves, `anchors_of`.
pub fn reanchor(root: &Root, to: &str, upgrade: &Value) -> Result<Vec<Value>, String> {
    let p = lock(&root.sup.ptrs).clone();
    let new = load(&root.p.rh, to).ok_or_else(|| format!("the upgraded tuple {to} is not in the store"))?;
    let pin = crate::twin::pin_of(&crate::lineage::records(&root.p.rh).map_err(|e| format!("lineage: {e}"))?);
    for (name, from) in [("audited", p.audited), ("settled", p.settled), ("twin", p.twin)] {
        let Some(id) = from else { continue };
        let mut d = match load(&root.p.rh, &id) {
            Some(t) if t["kernel"] == new["kernel"] => continue,
            Some(mut t) => {
                t["kernel"] = new["kernel"].clone();
                json!({"from": id, "tuple": save(&root.p.rh, &t).map_err(|e| e.to_string())?, "upgrade": upgrade})
            }
            None => {
                let file = root.p.rh.join("store/tuples").join(format!("{id}.json"));
                let why = if fs::symlink_metadata(&file).is_ok() { "its tuple file is damaged: it does not parse or does not hash to its id" }
                    else { "its tuple file is missing from the store" };
                json!({"from": id, "skipped": why, "tuple": null, "upgrade": upgrade})
            }
        };
        // the twin's replays, weekly reports and re-freeze clock read its freeze, which the new kernel does not change
        if name == "twin" && d["tuple"].is_string() && let Some(pin) = pin.as_ref().filter(|x| x.tuple == id) {
            (d["snapshot"], d["reason"], d["frozen_at"]) = (json!(pin.snapshot), json!(pin.reason), json!(util::rfc3339(pin.at)));
        }
        root.append_by("root", name, d).map_err(|e| format!("lineage: {e}"))?;
    }
    Ok(anchors_of(&crate::lineage::records(&root.p.rh).map_err(|e| format!("lineage: {e}"))?, upgrade))
}

/// The pointer moves of the upgrade `upgrade`'s commit, read from the lineage (so a commit re-run after a crash reports
/// what an earlier run recorded): `[{name, from, tuple, skipped}]`. A rolled-back upgrade's restores are not moves.
pub fn anchors_of(recs: &[Value], upgrade: &Value) -> Vec<Value> {
    recs.iter().filter(|r| matches!(r["kind"].as_str(), Some("audited" | "settled" | "twin")) && r["data"]["upgrade"] == *upgrade
        && r["data"]["rolled_back"] != true)
        .map(|r| json!({"from": r["data"]["from"], "name": r["kind"], "skipped": r["data"]["skipped"], "tuple": r["data"]["tuple"]})).collect()
}

/// `promotion{lkg:true}` (§3.7.1): the body binary and the components must already be stored.
pub fn check_promotion(rh: &Path, d: &Value) -> Result<(), String> {
    let comps = d["components"].as_array().ok_or("components must be a list")?;
    let mut need = vec![d["body_binary"].as_str().ok_or("body_binary is required")?];
    need.extend(comps.iter().map(|c| c["sha256"].as_str().unwrap_or("")));
    if d["lkg"] != true {
        return d["lkg"].is_boolean().then_some(()).ok_or_else(|| "lkg must be a boolean".into());
    }
    match need.into_iter().find(|s| !blob(rh, s).is_some_and(|b| b.is_file())) {
        Some(s) => Err(format!("blob {s:?} is not in the store: store.put it first")),
        None => Ok(()),
    }
}

/// After the record: forms {current kernel, new body, new ext} and appends `tuple`, then `lkg`. At install
/// (§4.3 step 8) there is no current tuple yet: the seed's promotion forms the **genesis** tuple from the
/// installed maintainer kernel (`sup.install`), and the Root appends `tuple`, `lkg` and `audited`.
pub fn on_promotion(root: &Root, d: &Value) {
    let cur = lock(&root.sup.ptrs).current.clone();
    let (kernel, genesis) = match cur.and_then(|c| load(&root.p.rh, &c)) {
        Some(base) => (base["kernel"].clone(), false),
        None => match lock(&root.sup.install).clone() {
            Some(k) => (k, true),
            None => return,
        },
    };
    let t = json!({"body": {"binary": d["body_binary"], "sha": d["sha"]}, "ext": {"components": d["components"], "set": d["ext_set"]},
        "kernel": kernel, "v": 1});
    if let Ok(id) = save(&root.p.rh, &t) {
        root.append("tuple", json!({"id": id, "tuple": t}));
        root.append("lkg", json!({"tuple": id}));
        if genesis {
            root.append("audited", json!({"tuple": id}));
        }
    }
}

/// ADR-011 §2.3: check the executable against Root-held admission records. Lane labels are not
/// authority. Missing records or unreadable executables fail closed; only an explicit, entirely
/// body-less bootstrap tuple set permits the pre-seed install path. This checks bytes at admission,
/// not at exec: immutable executable staging is still required to close the hash/exec race.
pub fn unwatched_body(root: &Root, program: &Path) -> Result<(), String> {
    let p = lock(&root.sup.ptrs).clone();
    let mut ids: Vec<String> = [p.lkg, p.current, p.settled].into_iter().flatten().collect();
    let v = crate::sentinel::view(root);
    for g in v["groups"].as_array().into_iter().flatten() {
        ids.extend(["tuple", "parent_tuple"].iter().filter_map(|k| g[*k].as_str().map(str::to_string)));
    }
    ids.extend(v["rollback"]["to"].as_str().map(str::to_string));
    ids.sort();
    ids.dedup();
    let tuples: Vec<Option<Value>> = ids.iter().map(|i| load(&root.p.rh, i)).collect();
    let bodies = admitted_bodies(&tuples)?;
    if bodies.is_empty() {
        return Ok(()); // explicit no-seed bootstrap, not a missing/corrupt admission record
    }
    let sha = sha_of(program).ok_or("cannot read the live body executable")?;
    if bodies.contains(&sha.as_str()) {
        Ok(())
    } else {
        Err(format!("live body executable {sha} is not in an admitted tuple"))
    }
}

fn admitted_bodies(tuples: &[Option<Value>]) -> Result<Vec<&str>, String> {
    if tuples.is_empty() {
        return Err("no body admission records are available".into());
    }
    let bodies: Vec<&str> = tuples.iter().map(|t| {
        t.as_ref().and_then(|t| t["body"]["binary"].as_str())
            .ok_or_else(|| "a body admission tuple is missing or malformed".to_string())
    }).collect::<Result<_, _>>()?;
    if bodies.iter().any(|b| !b.is_empty() && !hex64(b)) {
        return Err("an admitted body digest is malformed".into());
    }
    // An empty historical genesis must not disable enforcement after the first seeded promotion.
    Ok(bodies.into_iter().filter(|b| !b.is_empty()).collect())
}

/// `tuple.get` (§4.1.3): ids and objects.
pub fn get(root: &Root) -> Value {
    let p = lock(&root.sup.ptrs).clone();
    let one = |id: &Option<String>| id.as_ref().map_or(Value::Null, |i| json!({"id": i, "tuple": load(&root.p.rh, i)}));
    json!({"audited": one(&p.audited), "current": one(&p.current), "lkg": one(&p.lkg)})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-UNDO (v0.5.2): the refusal names what `rollback` tried, `settled` included (the v0.5.1 text left it out).
    #[test]
    fn the_rollback_refusal_names_every_pointer_it_tried() {
        assert_eq!(none_usable("current"), "no usable tuple (current, LKG, settled, audited-good and genesis all fail to verify)");
        assert_eq!(none_usable("lkg"), "no usable tuple (LKG, settled, audited-good and genesis all fail to verify)");
        assert_eq!(none_usable("settled"), "no usable tuple (settled, LKG, audited-good and genesis all fail to verify)");
    }

    #[test]
    fn pointers_fold_and_ids_are_stable() {
        let r = |kind: &str, data: Value| json!({"kind": kind, "data": data});
        let mut p = Ptrs::default();
        for rec in [r("tuple", json!({"id": "tA"})), r("lkg", json!({"tuple": "tA"})), r("audited", json!({"tuple": "tA"})),
            r("tuple", json!({"id": "tB"})), r("lkg", json!({"tuple": "tB"})), r("rollback", json!({"to": "tA"}))] {
            p.apply(&rec);
        }
        assert_eq!(p, Ptrs { current: Some("tA".into()), lkg: Some("tB".into()), audited: Some("tA".into()), genesis: Some("tA".into()), settled: None, twin: None });
        p.apply(&r("settled", json!({"tuple": "tA"})));
        assert_eq!(p.settled.as_deref(), Some("tA"));
        let t = json!({"b": 1, "a": [2]});
        assert_eq!(id_of(&t), id_of(&json!({"a": [2], "b": 1})));
        assert_eq!(id_of(&t).len(), 13);
    }

    #[test]
    fn body_admission_fails_closed_but_preserves_explicit_bootstrap() {
        let a = "a".repeat(64);
        let t = |b: &str| Some(json!({"body": {"binary": b, "sha": "x"}}));
        assert_eq!(admitted_bodies(&[t(&a), t("")]).unwrap(), vec![a.as_str()]);
        assert!(admitted_bodies(&[t("")]).unwrap().is_empty());
        assert!(admitted_bodies(&[]).is_err());
        assert!(admitted_bodies(&[t(&a), None]).is_err());
        assert!(admitted_bodies(&[Some(json!({"body": {}}))]).is_err());
        assert!(admitted_bodies(&[t("not-a-sha")]).is_err());
    }

    /// ADR-011 §2.3 on a test home: the admitted body passes, any other binary is named; an `lkg` without a body
    /// binary cannot disable an already seeded current tuple; a missing tuple fails closed.
    #[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
    #[test]
    fn unwatched_body_on_a_test_home() {
        let (dir, root) = crate::grant::tests::test_home("uw", "");
        let (a, b) = (dir.join("body-a"), dir.join("body-b"));
        fs::write(&a, "admitted body\n").unwrap();
        fs::write(&b, "unannounced body\n").unwrap();
        let (sa, sb) = (sha_of(&a).unwrap(), sha_of(&b).unwrap());
        let point = |kind: &str, t: &Value| {
            let id = save(&root.p.rh, t).unwrap();
            let rec = if kind == "tuple" { json!({"kind": kind, "data": {"id": id}}) } else { json!({"kind": kind, "data": {"tuple": id}}) };
            lock(&root.sup.ptrs).apply(&rec);
            id
        };
        let t = json!({"body": {"binary": sa, "sha": "a".repeat(40)}, "ext": {"components": [], "set": null}, "v": 1});
        point("tuple", &t);
        point("lkg", &t);
        assert!(unwatched_body(&root, &a).is_ok());
        assert!(unwatched_body(&root, &b).unwrap_err().contains(&sb));
        assert!(unwatched_body(&root, &dir.join("missing")).is_err());
        assert!(unwatched_body(&root, Path::new("/bin/sh")).is_err(), "an interpreter is not the admitted body");
        // An old body-less genesis must not bypass the seeded current tuple.
        point("lkg", &json!({"body": {"binary": "", "sha": ""}, "ext": {"components": [], "set": null}, "v": 1}));
        assert!(unwatched_body(&root, &b).is_err());
        lock(&root.sup.ptrs).lkg = Some("t000000000000".into());
        assert!(unwatched_body(&root, &b).is_err(), "a tuple that does not load");
        drop(root);
        let _ = fs::remove_dir_all(&dir); // exact path, created by test_home
    }

    #[test]
    fn store_hashes_dedups_and_refuses_mismatch() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp21/unit-store");
        let _ = fs::remove_dir_all(&d); // exact path, created by this test
        fs::create_dir_all(d.join("store/blobs")).unwrap();
        let want = util::sha256_hex(b"kernel bytes");
        assert_eq!(put(&d, &mut &b"kernel bytes"[..], Some(&want), 1 << 20), Ok(want.clone()));
        assert_eq!(put(&d, &mut &b"kernel bytes"[..], None, 1 << 20), Ok(want.clone()), "stored once");
        assert!(put(&d, &mut &b"other"[..], Some(&want), 1 << 20).unwrap_err().contains("mismatch"));
        assert!(put(&d, &mut &b"x"[..], Some("../etc"), 1 << 20).is_err());
        assert!(put(&d, &mut &[0u8; 64][..], None, 16).unwrap_err().contains("full"));
        let names: Vec<String> = fs::read_dir(d.join("store/blobs")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, std::slice::from_ref(&want), "no temp file is left");
        let t = json!({"body": {"binary": "", "sha": ""}, "ext": {"components": []}, "kernel": {"init_kernel": want, "initctl": ""}});
        assert!(verify(&d, &t));
        let broken = json!({"kernel": {"init_kernel": "0".repeat(64)}, "body": {"binary": ""}, "ext": {"components": []}});
        assert!(!verify(&d, &broken));
        fs::remove_dir_all(&d).unwrap();
    }
}
