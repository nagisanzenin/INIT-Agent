//! ADR-011 §5.1: capability identity and the cap table. The Root computes the `root`-evidence caps of a change
//! from the blobs in its store: `skill/<n>` and `agent/<n>` (sha256 over the directory's sorted (path, content)
//! pairs in the `t0` tar), `prompt/GENESIS.md` and `prompt/SELF.md` (file sha256), `ext/<name>` (the component's
//! sha256). The table is a fold over `change_open`, `change_close`, `cap_state` and `cap_use`.

use super::usage::Day;
use crate::link::{Root, lock};
use crate::zone::Err3;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A change naming more caps than this is refused (`root.args`): its `change_open` must fit the lineage's 16 KiB.
const CHANGE_MAX: usize = 96;
/// A `t0` blob is at most 8 MiB (root.md §3.7.1).
const T0_MAX: u64 = 8 << 20;

/// One row of the cap table and of `caps.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CapRow {
    pub id: String,
    pub ver: Option<String>,
    pub prev_ver: Option<String>,
    /// `active`, `removed`, `observed` (seen in use only: seed components and trials, which never decay), or a
    /// `cap_state` state (`cold`, `archive_due`, `archived`, `quarantined`, `core`).
    pub state: String,
    pub evidence: String,
    /// Introduced by a `change_open`.
    pub registered: bool,
    /// The open group that last changed it.
    pub group: Option<u64>,
    pub turns: u64,
    pub uses: u64,
    pub harm_turns: u64,
    pub last_turn_seq: Option<u64>,
    pub last_day: Option<String>,
}

#[derive(Default)]
pub struct Table {
    rows: BTreeMap<String, CapRow>,
    /// Per open group, each cap's row as it was before the group first changed it (the undo of a rollback).
    undo: BTreeMap<u64, BTreeMap<String, Option<CapRow>>>,
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

impl Table {
    pub fn apply(&mut self, kind: &str, d: &Value) {
        match kind {
            "change_open" => {
                let Some(g) = d["group"].as_u64() else { return };
                for c in d["caps"].as_array().into_iter().flatten() {
                    let Some(id) = c["id"].as_str().filter(|i| super::valid_id(i)) else { continue };
                    let before = self.rows.get(id).cloned();
                    self.undo.entry(g).or_default().entry(id.to_string()).or_insert(before);
                    let r = self.rows.entry(id.to_string()).or_insert_with(|| CapRow { id: id.to_string(), ..CapRow::default() });
                    let remove = c["op"] == "remove";
                    (r.ver, r.prev_ver) = (if remove { None } else { s(&c["ver"]) }, s(&c["prev_ver"]));
                    r.state = if remove { "removed" } else { "active" }.into();
                    r.evidence = if c["evidence"] == "root" { "root" } else { "zone" }.into();
                    (r.registered, r.group) = (true, (c["op"] != "baseline").then_some(g)); // F3: a seed cap has no group
                }
            }
            "change_close" => {
                let Some(g) = d["group"].as_u64() else { return };
                let undone = matches!(d["outcome"].as_str(), Some("rolled_back" | "collateral"));
                for (id, before) in self.undo.remove(&g).unwrap_or_default() {
                    let counters = self.rows.get(&id).map(|r| (r.turns, r.uses, r.harm_turns, r.last_turn_seq, r.last_day.clone()));
                    match (undone, before) {
                        (true, Some(mut b)) => {
                            if let Some(c) = counters {
                                (b.turns, b.uses, b.harm_turns, b.last_turn_seq, b.last_day) = c;
                            }
                            b.group = None;
                            self.rows.insert(id, b);
                        }
                        (true, None) => {
                            if let Some(r) = self.rows.get_mut(&id) {
                                (r.state, r.ver, r.group) = ("removed".into(), None, None);
                            }
                        }
                        (false, _) => {
                            if let Some(r) = self.rows.get_mut(&id).filter(|r| r.group == Some(g)) {
                                r.group = None;
                            }
                        }
                    }
                }
            }
            "cap_state" => {
                if let (Some(id), Some(to)) = (d["cap"].as_str().filter(|i| super::valid_id(i)), d["to"].as_str()) {
                    let r = self.rows.entry(id.to_string()).or_insert_with(|| CapRow { id: id.to_string(), ..CapRow::default() });
                    r.state = to.to_string();
                }
            }
            "cap_use" => {
                let day = s(&d["day"]);
                for c in d["caps"].as_array().into_iter().flatten() {
                    let Some(id) = c["id"].as_str().filter(|i| super::valid_id(i)) else { continue };
                    let r = self.rows.entry(id.to_string()).or_insert_with(|| CapRow { id: id.to_string(), state: "observed".into(),
                        evidence: "zone".into(), ..CapRow::default() });
                    let n = |k: &str| c[k].as_u64().unwrap_or(0);
                    (r.turns, r.uses, r.harm_turns) = (r.turns + n("turns"), r.uses + n("uses"), r.harm_turns + n("harm_turns"));
                    r.last_turn_seq = r.last_turn_seq.max(c["last_turn_seq"].as_u64());
                    r.last_day = r.last_day.take().max(day.clone());
                    if c["evidence"] == "root" {
                        r.evidence = "root".into();
                    }
                }
            }
            _ => {}
        }
    }

    /// The rows with the undigested days' counters added (a cap first seen in use is `observed`).
    pub fn rows(&self, pending: &BTreeMap<String, Day>) -> Vec<CapRow> {
        let mut rows = self.rows.clone();
        for (d, day) in pending {
            for (id, k) in day {
                let r = rows.entry(id.clone()).or_insert_with(|| CapRow { id: id.clone(), state: "observed".into(), evidence: "zone".into(), ..CapRow::default() });
                (r.turns, r.uses, r.harm_turns) = (r.turns + k.turns, r.uses + k.uses, r.harm_turns + k.harm_turns);
                r.last_turn_seq = r.last_turn_seq.max(Some(k.last_turn_seq));
                r.last_day = r.last_day.take().max(Some(d.clone()));
                if k.root {
                    r.evidence = "root".into();
                }
            }
        }
        rows.into_values().collect()
    }
}

/// The regular files of a (pax/ustar) tar: path → content, relative paths only; links and specials are skipped.
pub fn tar_files(b: &[u8]) -> Result<BTreeMap<String, &[u8]>, String> {
    let (mut out, mut off, mut long): (BTreeMap<String, &[u8]>, usize, Option<String>) = (BTreeMap::new(), 0, None);
    let cstr = |x: &[u8]| String::from_utf8_lossy(&x[..x.iter().position(|&c| c == 0).unwrap_or(x.len())]).into_owned();
    loop {
        // the archive must end with its zero block: a tar cut anywhere is refused
        let h = b.get(off..off + 512).ok_or("t0: truncated tar")?;
        if h.iter().all(|&c| c == 0) {
            break;
        }
        let size = if h[124] & 0x80 != 0 {
            h[125..136].iter().fold(0u64, |a, &c| a.saturating_mul(256).saturating_add(u64::from(c)))
        } else {
            u64::from_str_radix(cstr(&h[124..136]).trim(), 8).map_err(|_| "t0: bad tar size".to_string())?
        } as usize;
        let data = b.get(off + 512..off + 512 + size).ok_or("t0: truncated tar")?;
        let mut name = cstr(&h[0..100]);
        if &h[257..263] == b"ustar\0" && h[345] != 0 {
            name = format!("{}/{name}", cstr(&h[345..500]));
        }
        match h[156] {
            b'x' => {
                // pax records: "<len> <key>=<value>\n"
                let text = String::from_utf8_lossy(data);
                long = text.lines().filter_map(|l| l.split_once(' ')).filter_map(|(_, kv)| kv.strip_prefix("path=")).next_back().map(String::from).or(long);
            }
            b'L' => long = Some(cstr(data)),
            b'0' | 0 => {
                let p = long.take().unwrap_or(name);
                let p = p.strip_prefix("./").unwrap_or(&p).to_string();
                if !p.starts_with('/') && !p.split('/').any(|c| c == "..") {
                    out.insert(p, data);
                }
            }
            _ => long = None,
        }
        off += 512 + size.div_ceil(512) * 512;
    }
    Ok(out)
}

/// The `root`-evidence versions of a `t0` tar: `skill/<n>` and `agent/<n>` (sha256 over the directory's files
/// sorted by their path inside it, each as `path NUL len(u64 BE) content`) and the two prompt files.
pub fn t0_caps(files: &BTreeMap<String, &[u8]>) -> BTreeMap<String, String> {
    let mut dirs: BTreeMap<String, Sha256> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (p, c) in files {
        if matches!(p.as_str(), "GENESIS.md" | "SELF.md") {
            out.insert(format!("prompt/{p}"), crate::util::sha256_hex(c));
            continue;
        }
        let Some((kind, rest)) = p.strip_prefix("skills/").map(|r| ("skill", r)).or_else(|| p.strip_prefix("agents/").map(|r| ("agent", r))) else { continue };
        let Some((n, rel)) = rest.split_once('/').filter(|(n, rel)| super::valid_name(n) && !n.contains('/') && !rel.is_empty()) else { continue };
        let h = dirs.entry(format!("{kind}/{n}")).or_default();
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update((c.len() as u64).to_be_bytes());
        h.update(c);
    }
    out.extend(dirs.into_iter().map(|(id, h)| (id, crate::util::hex(&h.finalize()))));
    out
}

/// `ext/<name>` → component sha256 of a tuple's (or a candidate's) `ext.components`.
fn ext_caps(ext: &Value) -> BTreeMap<String, String> {
    ext["components"].as_array().into_iter().flatten()
        .filter_map(|c| Some((c["name"].as_str().filter(|n| super::valid_name(n))?, c["sha256"].as_str()?)))
        .map(|(n, s)| (format!("ext/{n}"), s.to_string())).collect()
}

fn t0_of(rh: &std::path::Path, sha: Option<&str>) -> Result<BTreeMap<String, String>, Err3> {
    let Some(sha) = sha.filter(|s| !s.is_empty()) else { return Ok(BTreeMap::new()) };
    let p = crate::tuple::blob(rh, sha).ok_or_else(|| ("root.args", format!("t0 {sha:?} is not a sha256"), Value::Null))?;
    let missing = || ("root.store", format!("t0 blob {sha} is not in the store"), json!({"sha256": sha}));
    if std::fs::metadata(&p).map_err(|_| missing())?.len() > T0_MAX {
        return Err(("root.args", format!("t0 blob {sha} is over 8 MiB"), Value::Null));
    }
    let b = std::fs::read(&p).map_err(|_| missing())?;
    Ok(t0_caps(&tar_files(&b).map_err(|e| ("root.args", e, Value::Null))?))
}

/// The cap changes between two version maps.
pub(super) fn diff(cand: &BTreeMap<String, String>, parent: &BTreeMap<String, String>, out: &mut Vec<Value>) {
    for (id, v) in cand {
        match parent.get(id) {
            None => out.push(json!({"evidence": "root", "id": id, "op": "add", "prev_ver": null, "ver": v})),
            Some(p) if p != v => out.push(json!({"evidence": "root", "id": id, "op": "modify", "prev_ver": p, "ver": v})),
            _ => {}
        }
    }
    for (id, p) in parent.iter().filter(|(id, _)| !cand.contains_key(*id)) {
        out.push(json!({"evidence": "root", "id": id, "op": "remove", "prev_ver": p, "ver": null}));
    }
}

/// `Hooks.caps` (X3): the caps of a `change.open{candidate{body{t0}, ext{components}}, caps}` against the current
/// tuple, `[{id, ver, prev_ver, op, evidence}]` sorted by id. The `root` entries are computed here from the store;
/// the zone's `caps` entries are kept for the other kinds only (`tool`, `mod`, `mem`, `loop`, `kernel`, `dep`).
pub fn change_caps(root: &Root, args: &Value) -> Result<Value, Err3> {
    let rh = &root.p.rh;
    let cur = lock(&root.sup.ptrs).current.clone();
    let parent = cur.as_deref().and_then(|id| crate::tuple::load(rh, id)).unwrap_or(Value::Null);
    let cand = &args["candidate"];
    let mut out = Vec::new();
    let cand_t0 = t0_of(rh, cand["body"]["t0"].as_str())?;
    if parent["body"]["t0"].as_str().is_none_or(str::is_empty) {
        // F3 (demo 2026-10-25): the parent tuple holds no t0 blob (the install's genesis tuple, or a gated-path
        // promotion), so the Root cannot tell the seed's T0 from this change's. Each T0 cap is `baseline`: the cap
        // table learns its version, but no group is attributed with it, so a rollback never quarantines a seed file.
        out.extend(cand_t0.iter().map(|(id, v)| json!({"evidence": "root", "id": id, "op": "baseline", "prev_ver": null, "ver": v})));
    } else {
        diff(&cand_t0, &t0_of(rh, parent["body"]["t0"].as_str())?, &mut out);
    }
    if cand["ext"].is_object() {
        diff(&ext_caps(&cand["ext"]), &ext_caps(&parent["ext"]), &mut out);
    }
    zone_entries(&args["caps"], &mut out)?;
    out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    if out.len() > CHANGE_MAX {
        return Err(("root.args", format!("a change may name at most {CHANGE_MAX} caps ({} here)", out.len()), Value::Null));
    }
    Ok(Value::Array(out))
}

/// The zone's `change.open.caps` (evidence `zone`): validated; kinds the Root computes are dropped.
fn zone_entries(caps: &Value, out: &mut Vec<Value>) -> Result<(), Err3> {
    let ver = |v: &Value| v.is_null() || v.as_str().is_some_and(|s| (1..=80).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c)));
    for c in caps.as_array().into_iter().flatten() {
        let id = c["id"].as_str().filter(|i| super::valid_id(i));
        let op = c["op"].as_str().filter(|o| matches!(*o, "add" | "modify" | "remove"));
        let (Some(id), Some(op)) = (id, op) else {
            return Err(("root.args", "change.open caps: each entry needs a valid id (ADR-011 §5.1) and op add|modify|remove".into(), c.clone()));
        };
        if !ver(&c["ver"]) || !ver(&c["prev_ver"]) {
            return Err(("root.args", format!("change.open caps: {id}: ver and prev_ver are null or 1-80 of [A-Za-z0-9._:-]"), Value::Null));
        }
        let computed = ["skill/", "agent/", "prompt/", "ext/"].iter().any(|k| id.starts_with(k));
        if !computed && !out.iter().any(|o| o["id"] == id) {
            out.push(json!({"evidence": "zone", "id": id, "op": op, "prev_ver": c["prev_ver"], "ver": c["ver"]}));
        }
    }
    Ok(())
}
