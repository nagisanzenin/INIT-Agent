//! M7 S v1 (m7-design.md §3.1.2, §3.1.5; PROOF-LEAN D-L1): a pack installed read-only into
//! `ROOT_HOME/sealed/packs/<pack>@<version>/`; the draw (seeds from `/dev/urandom` kept in `sealed/seeds/`,
//! generators run in sealed instances with no network, model-free validation); the partitions in `sealed/S.json`
//! (G4, a reserve on held-out templates, 4 canaries); throw-away instances for the cost probe. Records carry
//! counts only.

use super::run::{self, Env, RunRoot};
use super::stats::Rng;
use super::task::{self, Task};
use crate::link::{Root, lock};
use crate::util::{self, icj, now_ms, rfc3339, write_atomic};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

const FAMILIES: [&str; 4] = ["E", "B", "C", "T"];
/// D-L1: G4 32 (E 14, C 9, T 9) and a reserve of 20 held-out templates (E 10, C 5, T 5); family B is dropped.
/// A pack's `composition` (`{"E": {"g4": 14, "reserve": 10}, …}`) overrides it.
const LEAN: [(&str, usize, usize); 3] = [("E", 14, 10), ("C", 9, 5), ("T", 9, 5)];
/// Fewer valid G4 tasks than this and S is `sealed_none` (m7-design.md §3.1.5).
pub const G4_MIN: usize = 24;
const GEN_WALL: Duration = Duration::from_secs(120);

pub struct Tpl {
    pub fam: String,
    pub id: String,
    pub dir: PathBuf,
    pub lang: String,
    pub licence: String,
    pub difficulty: i64,
    /// `canary/<kind>/`: health, impossible, tamper or leak.
    pub canary: Option<String>,
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

fn mkdir(p: &Path) -> Result<(), String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn cp(src: &Path, dst: &Path) -> Result<(), String> {
    let ok = Command::new("/bin/cp").arg("-R").arg(src).arg(dst).status().is_ok_and(|s| s.success());
    ok.then_some(()).ok_or(format!("copying {} failed", src.display()))
}

/// Files read-only (0400, executables 0500), directories 0700 so the owner can still delete by exact path.
fn seal_modes(p: &Path) {
    for e in fs::read_dir(p).into_iter().flatten().flatten() {
        let (q, Ok(m)) = (e.path(), e.metadata()) else { continue };
        if m.is_dir() {
            seal_modes(&q);
            let _ = fs::set_permissions(&q, fs::Permissions::from_mode(0o700));
        } else if m.is_file() {
            let _ = fs::set_permissions(&q, fs::Permissions::from_mode(if m.permissions().mode() & 0o100 != 0 { 0o500 } else { 0o400 }));
        }
    }
}

/// `sealed draw --pack-dir <dir>`: the pack (a directory with `pack.json`) into `sealed/packs/<pack>@<version>/`.
/// An installed version is reused only if its content digest matches; a changed pack needs a new version.
pub fn install(rh: &Path, src: &Path) -> Result<PathBuf, String> {
    let pj = read_json(&src.join("pack.json")).ok_or("pack.json: missing or not JSON")?;
    let (name, ver) = (pj["pack"].as_str().unwrap_or(""), pj["version"].as_str().unwrap_or(""));
    if !name_ok(name) || !name_ok(ver) {
        return Err("pack.json: pack and version must match ^[A-Za-z0-9._-]{1,64}$".into());
    }
    let packs = rh.join("sealed/packs");
    mkdir(&packs)?;
    let (dst, dig) = (packs.join(format!("{name}@{ver}")), packs.join(format!("{name}@{ver}.digest")));
    let want = run::tree_digest(src);
    if dst.is_dir() {
        return match fs::read_to_string(&dig) {
            Ok(d) if d.trim() == want => Ok(dst),
            _ => Err(format!("pack {name}@{ver} is installed with different content; give the changed pack a new version")),
        };
    }
    let tmp = packs.join(format!(".inst-{}", util::hex(&util::random::<4>().map_err(|e| e.to_string())?)));
    cp(src, &tmp).and_then(|()| {
        seal_modes(&tmp);
        write_atomic(&dig, format!("{want}\n").as_bytes(), 0o600).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &dst).map_err(|e| e.to_string())
    }).inspect_err(|_| drop(fs::remove_dir_all(&tmp)))?; // exact path, created above
    Ok(dst)
}

/// The installed `<pack>@<version>` (the newest one installed).
pub fn installed(rh: &Path, pack: &str) -> Result<PathBuf, String> {
    let pre = format!("{pack}@");
    fs::read_dir(rh.join("sealed/packs")).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&pre) && e.path().is_dir())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok()).map(|e| e.path())
        .ok_or_else(|| format!("no pack {pack} is installed: {}", util::root_cmd(rh, "sealed draw --pack-dir <dir>")))
}

/// Every template in the pack: `<family>/<template>/template.json` and `canary/<kind>/template.json`.
pub fn templates(pack: &Path) -> Vec<Tpl> {
    let lic = read_json(&pack.join("pack.json")).unwrap_or_default();
    let mut v = Vec::new();
    for fam in FAMILIES.iter().chain(&["canary"]) {
        // `_`-prefixed directories are a family's shared helpers, never templates (FORMAT.md)
        let mut dirs: Vec<PathBuf> = fs::read_dir(pack.join(fam)).into_iter().flatten().flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with(['_', '.'])).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let Some(t) = read_json(&d.join("template.json")) else { continue };
            let Some(id) = t["id"].as_str().filter(|s| name_ok(s)) else { continue };
            let kind = t["canary"].as_str().map(String::from).or_else(|| d.file_name().map(|n| n.to_string_lossy().to_string()));
            let licence = t["licence"].as_str().or(lic["licences"][id].as_str()).unwrap_or("INIT").to_string();
            v.push(Tpl { fam: fam.to_string(), id: id.into(), dir: d.clone(), lang: t["lang"].as_str().unwrap_or("any").into(), licence,
                difficulty: t["difficulty"]["default"].as_i64().unwrap_or(3), canary: (*fam == "canary").then_some(kind).flatten() });
        }
    }
    v
}

pub fn s_json(rh: &Path) -> Option<Value> {
    read_json(&rh.join("sealed/S.json"))
}

pub fn save_s(rh: &Path, s: &Value) -> Result<(), String> {
    write_atomic(&rh.join("sealed/S.json"), format!("{}\n", icj(s).map_err(|e| e.to_string())?).as_bytes(), 0o600).map_err(|e| e.to_string())
}

/// The reserve's instance ids (`task::store` leaves them out).
pub fn reserve_ids(rh: &Path) -> Vec<String> {
    s_json(rh).map(|s| ids(&s["reserve"])).unwrap_or_default()
}

pub fn ids(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// The tasks of an S.json list, in its order.
pub fn tasks(rh: &Path, list: &Value) -> Vec<Task> {
    ids(list).iter().filter_map(|id| task::load(&rh.join("sealed/S").join(id)).ok()).collect()
}

/// This install's GUID (16 random bytes, hex), created on first use.
pub fn guid(rh: &Path) -> Result<String, String> {
    let p = rh.join("sealed/guid");
    match fs::read_to_string(&p) {
        Ok(g) if g.trim().len() == 32 => Ok(g.trim().to_string()),
        _ => {
            let g = util::hex(&util::random::<16>().map_err(|e| e.to_string())?);
            write_atomic(&p, format!("{g}\n").as_bytes(), 0o600).map_err(|e| e.to_string())?;
            Ok(g)
        }
    }
}

/// Runs `f` over `items` on `n` threads, results in item order.
pub fn par<T: Sync, R: Send>(items: &[T], n: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let (next, out) = (AtomicUsize::new(0), Mutex::new((0..items.len()).map(|_| None).collect::<Vec<Option<R>>>()));
    std::thread::scope(|s| {
        for _ in 0..n.clamp(1, items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, SeqCst);
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                lock(&out)[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().flatten().collect()
}

/// `gen/run <seed> <outdir>` for `tpl` in a sealed instance with no network. The sealed profile reads only the run
/// root, so the pack's layout is rebuilt there first: `lib/`, `runner/`, the family's `_*` helpers and the template,
/// at their relative paths (FORMAT.md). Returns the instance directory.
fn generate(env: &Env, run: &RunRoot, pack: &Path, tpl: &Tpl, seed: &str, guid: &str) -> Result<PathBuf, String> {
    let p = run.dir.join("pack");
    mkdir(&p.join(&tpl.fam))?;
    for d in ["lib", "runner"] {
        if pack.join(d).is_dir() {
            cp(&pack.join(d), &p.join(d))?;
        }
    }
    for e in fs::read_dir(pack.join(&tpl.fam)).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('_')) {
        cp(&e.path(), &p.join(&tpl.fam).join(e.file_name()))?; // the family's shared helpers (`_lib/`, …)
    }
    let tdir = p.join(&tpl.fam).join(tpl.dir.file_name().ok_or("template dir")?);
    cp(&tpl.dir, &tdir)?;
    let out = run.dir.join("out/inst");
    let mut c = run::sandboxed(env, run, None, &[tdir.join("gen/run").into(), seed.into(), out.clone().into()]);
    c.env("INIT_S1_GUID", guid).current_dir(run.dir.join("tmp"));
    let (code, why, _) = run::supervise(c, run, &crate::memwatch::Limits::of(&env.ch), GEN_WALL, None, || None)?;
    if why != "done" || code != Some(0) || !out.join("task.json").is_file() {
        return Err(format!("{}: gen/run ended {why} (exit {code:?})", tpl.id));
    }
    Ok(out)
}

/// §3.1.5 step 3, no model call: the solution scores 1000 on 3 runs; the no-op workspace scores the same on 3 runs
/// and at most 500; an empty workspace scores below 1000; every check finishes in time and leaves its dir intact.
/// Canaries: the no-op is stable and below 1000, the honest solution is stable (1000 for `health`).
fn validate(env: &Env, t: &Task, canary: Option<&str>) -> Result<(), String> {
    let chk = |tar: Option<&Path>| -> Result<i64, String> {
        let c = run::check_tar(env, t, tar)?;
        if c.timeout || c.void.is_some() {
            return Err("the check timed out or changed its own directory".into());
        }
        Ok(c.score)
    };
    let three = |tar: &str| (0..3).map(|_| chk(Some(&t.dir.join(tar)))).collect::<Result<Vec<i64>, String>>();
    let (sol, noop) = (three("solution.tar")?, three("workspace.tar")?);
    let same = |v: &[i64]| v.iter().all(|x| *x == v[0]);
    let ok = match canary {
        None => sol.iter().all(|s| *s == 1000) && same(&noop) && noop[0] <= 500 && chk(None)? < 1000,
        Some("health") => sol.iter().all(|s| *s == 1000) && same(&noop) && noop[0] < 1000,
        Some(_) => same(&sol) && same(&noop) && noop[0] < 1000,
    };
    ok.then_some(()).ok_or(format!("validation failed: solution {sol:?}, no-op {noop:?}"))
}

struct Drawn {
    id: String,
    fam: String,
    part: String,
    canary: Option<String>,
}

/// One instance of `tpl` into `sealed/S/<id>/`: generated, given a random 12-hex id and `provenance.json` v2,
/// validated, its seed kept in `sealed/seeds/<id>.json` (0600). The template id never leaves `ROOT_HOME`.
fn draw_one(env: &Env, pack: &Path, pj: &Value, tpl: &Tpl, part: &str, s_version: &str, guid: &str) -> Result<Drawn, String> {
    let run = RunRoot::new(&env.rh)?;
    let seed = util::hex(&util::random::<32>().map_err(|e| e.to_string())?);
    let inst = generate(env, &run, pack, tpl, &seed, guid)?;
    let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
    let mut tj = read_json(&inst.join("task.json")).ok_or("task.json: not JSON")?;
    tj["id"] = json!(id);
    fs::write(inst.join("task.json"), icj(&tj).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let prov = json!({"v": 2, "kind": if tpl.canary.is_some() { "canary" } else { "task" }, "canary_kind": tpl.canary, "pack": pj["pack"],
        "pack_version": pj["version"], "family": tpl.fam, "template": tpl.id, "lang": tpl.lang, "difficulty": tpl.difficulty,
        "licence": tpl.licence, "partition": part, "drawn_at": rfc3339(now_ms()), "s_version": s_version,
        "check_digest": run::tree_digest(&inst.join("check")), "source": "s1", "weight": 1});
    fs::write(inst.join("provenance.json"), icj(&prov).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let t = task::load(&inst)?;
    validate(env, &t, tpl.canary.as_deref()).map_err(|e| format!("{}: {e}", tpl.id))?;
    mkdir(&env.rh.join("sealed/seeds"))?;
    let sd = json!({"difficulty": tpl.difficulty, "seed": seed, "template": tpl.id});
    write_atomic(&env.rh.join("sealed/seeds").join(format!("{id}.json")), icj(&sd).map_err(|e| e.to_string())?.as_bytes(), 0o600)
        .map_err(|e| e.to_string())?;
    mkdir(&env.rh.join("sealed/S"))?;
    fs::rename(&inst, env.rh.join("sealed/S").join(&id)).map_err(|e| e.to_string())?;
    Ok(Drawn { id, fam: tpl.fam.clone(), part: part.into(), canary: tpl.canary.clone() })
}

/// Templates of one family in a seeded order that alternates its languages (E: Python, Rust), so each partition
/// gets a balanced share.
fn balanced<'a>(ts: Vec<&'a Tpl>, rng: &mut Rng) -> Vec<&'a Tpl> {
    let mut by: BTreeMap<&str, Vec<&Tpl>> = BTreeMap::new();
    for t in ts {
        by.entry(t.lang.as_str()).or_default().push(t);
    }
    let mut groups: Vec<Vec<&Tpl>> = by.into_values().collect();
    groups.iter_mut().for_each(|g| rng.shuffle(g));
    rng.shuffle(&mut groups);
    let mut out = Vec::new();
    for i in 0..groups.iter().map(Vec::len).max().unwrap_or(0) {
        out.extend(groups.iter().filter_map(|g| g.get(i).copied()));
    }
    out
}

/// `init-root sealed draw [--pack s1] [--pack-dir DIR]` (m7-design.md §3.1.5): S v1 once per install (rotation is
/// cut by D-L2). Each template serves one partition; a template that fails is replaced by a spare of its family when
/// there is one. Appends `sealed_draw` (counts only) and prints counts only.
pub fn draw(root: &Root, a: &Value) -> Result<Value, String> {
    let env = Env::of(root)?;
    let _op = run::begin(&env.rh);
    if s_json(&env.rh).is_some() {
        return Err("S v1 is already drawn (sealed/S.json); the lean build has no rotation".into());
    }
    let name = a["pack"].as_str().unwrap_or(&env.ch.sealed.pack).to_string();
    let pack = match a["pack_dir"].as_str() {
        Some(d) => install(&env.rh, Path::new(d))?,
        None => installed(&env.rh, &name)?,
    };
    let pj = read_json(&pack.join("pack.json")).ok_or("pack.json")?;
    let guid = guid(&env.rh)?;
    let all = templates(&pack);
    let comp: Vec<(String, usize, usize)> = match pj["composition"].as_object() {
        Some(o) => o.iter().filter(|(f, _)| FAMILIES.contains(&f.as_str()))
            .map(|(f, c)| (f.clone(), c["g4"].as_u64().unwrap_or(0) as usize, c["reserve"].as_u64().unwrap_or(0) as usize)).collect(),
        None => LEAN.iter().map(|(f, g, r)| (f.to_string(), *g, *r)).collect(),
    };
    let mut rng = Rng(u64::from_be_bytes(util::random::<8>().map_err(|e| e.to_string())?));
    #[allow(clippy::type_complexity)]
    let (mut slots, mut spares): (Vec<(&Tpl, &str)>, BTreeMap<String, Vec<&Tpl>>) = (Vec::new(), BTreeMap::new());
    for (fam, g, r) in &comp {
        let mut ts = balanced(all.iter().filter(|t| &t.fam == fam && t.canary.is_none()).collect(), &mut rng);
        let rest = ts.split_off((*g + *r).min(ts.len()));
        let res = ts.split_off((*g).min(ts.len()));
        slots.extend(ts.into_iter().map(|t| (t, "g4")).chain(res.into_iter().map(|t| (t, "reserve"))));
        spares.insert(fam.clone(), rest);
    }
    slots.extend(all.iter().filter(|t| t.canary.is_some()).map(|t| (t, "canary")));
    let s_version = format!("{}.0", pj["pack"].as_str().unwrap_or(&name));
    let par_n = env.ch.sealed.parallel.max(1) as usize;
    let mut got: Vec<Result<Drawn, String>> = par(&slots, par_n, |(t, p)| draw_one(&env, &pack, &pj, t, p, &s_version, &guid));
    let mut skipped = Vec::new();
    for (i, (t, p)) in slots.iter().enumerate() {
        while let Err(e) = &got[i] {
            skipped.push(e.clone());
            util::log(&env.rh, "warn", &format!("sealed draw: {e}"));
            match spares.get_mut(&t.fam).and_then(Vec::pop) {
                Some(s) => got[i] = draw_one(&env, &pack, &pj, s, p, &s_version, &guid),
                None => break,
            }
        }
    }
    if !skipped.is_empty() {
        root.alert_from("sealed", "sealed_template", &format!("[root] sealed draw skipped {} template instance(s); see logs/root.jsonl", skipped.len()));
    }
    let ok: Vec<Drawn> = got.into_iter().flatten().collect();
    let list = |p: &str| ok.iter().filter(|d| d.part == p).map(|d| d.id.clone()).collect::<Vec<_>>();
    let canary: BTreeMap<String, String> = ok.iter().filter_map(|d| d.canary.clone().map(|k| (k, d.id.clone()))).collect();
    let mut counts: BTreeMap<String, Value> = BTreeMap::new();
    for (fam, _, _) in &comp {
        let n = |p: &str| ok.iter().filter(|d| &d.fam == fam && d.part == p).count();
        counts.insert(fam.clone(), json!({"g4": n("g4"), "reserve": n("reserve")}));
    }
    counts.insert("canary".into(), json!(canary.len()));
    let s = json!({"v": 1, "pack": pj["pack"], "pack_version": pj["version"], "s_version": s_version, "drawn_at": rfc3339(now_ms()),
        "g4": list("g4"), "reserve": list("reserve"), "canary": canary, "selected": false, "out_of_band": [], "reserve_used": false});
    save_s(&env.rh, &s)?;
    let data = json!({"pack": pj["pack"], "pack_version": pj["version"], "s_version": s_version, "counts": counts,
        "skipped_templates": skipped.len(), "spent_musd": 0});
    let rec = root.append("sealed_draw", data.clone()).ok_or("the lineage is unavailable")?;
    let none = list("g4").len() < G4_MIN || canary.len() < 4;
    Ok(json!({"sealed_draw": data, "seq": rec["seq"], "sealed_none": none}))
}

/// A throw-away instance of `tpl` for the cost probe (m7-design.md §3.6 P): generated like a draw (not validated,
/// never an S member); it lives in its run root, which is deleted by exact path when the caller drops it.
pub fn throwaway(env: &Env, pack: &Path, tpl: &Tpl) -> Result<(RunRoot, Task), String> {
    let run = RunRoot::new(&env.rh)?;
    let seed = util::hex(&util::random::<32>().map_err(|e| e.to_string())?);
    let inst = generate(env, &run, pack, tpl, &seed, &guid(&env.rh)?)?;
    let id = util::hex(&util::random::<6>().map_err(|e| e.to_string())?);
    let mut tj = read_json(&inst.join("task.json")).ok_or("task.json: not JSON")?;
    tj["id"] = json!(id);
    fs::write(inst.join("task.json"), icj(&tj).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let prov = json!({"v": 2, "kind": "task", "family": tpl.fam, "lang": tpl.lang, "partition": "probe", "source": "s1"});
    fs::write(inst.join("provenance.json"), icj(&prov).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let t = task::load(&inst)?;
    Ok((run, t))
}
