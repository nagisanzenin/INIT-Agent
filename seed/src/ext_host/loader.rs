//! Loading and hot swap: artifact resolution, the load pipeline, the `ext-loader` thread,
//! `begin_reload` / `commit_reload`, the reload report and `state/ext/catalog.json`
//! (ext/host.md §3.8, §3.9, §4.2, §4.3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use wasmtime::component::Component;

use super::bindings::v1;
use super::dispatch::chain;
use super::registry::{ExtVersion, Fingerprint, Origin, Pre, Registry, hash_bytes, validate};
use super::{ExtHost, ReloadKind, ReloadReport, WASM_MAX_BYTES, lock};

/// The result of one load: the full desired state, computed from scratch.
pub(crate) struct Staged {
    pub exts: BTreeMap<String, Arc<ExtVersion>>,
    pub failed: BTreeMap<String, String>,
    /// (name, what was looked for)
    pub not_built: Vec<(String, String)>,
    /// name → notes for the report (timeout clamping)
    pub notes: BTreeMap<String, Vec<String>>,
}

#[derive(Default)]
struct LoadQ {
    running: bool,
    queued: Option<ReloadKind>,
    done: Option<Result<Staged, String>>,
    thread_alive: bool,
    stop: bool,
}

#[derive(Default)]
pub(crate) struct LoaderShared {
    q: Mutex<LoadQ>,
    cv: Condvar,
}

impl LoaderShared {
    pub(crate) fn stop(&self) {
        lock(&self.q).stop = true;
        self.cv.notify_all();
    }
}

/// `file(n) = n.replace('-', '_') + ".wasm"`
pub(crate) fn file_name(n: &str) -> String {
    format!("{}.wasm", n.replace('-', "_"))
}

fn mtime_ns(m: &fs::Metadata) -> u128 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

struct Candidate {
    name: String,
    artifact: Option<(PathBuf, Origin)>,
    looked_for: String,
}

impl ExtHost {
    /// Returns at once; at most one load runs and at most one waits (a newer request replaces
    /// the queued one). `Kept(names)` records `kept_at` immediately.
    pub fn begin_reload(&self, kind: ReloadKind) {
        self.procs.reap_idle(Instant::now());
        if let ReloadKind::Kept(names) = &kind {
            let now = SystemTime::now();
            let mut c = lock(&self.clock);
            for n in names {
                c.kept_at.insert(n.clone(), now);
            }
        }
        let spawn = {
            let mut q = lock(&self.loader.q);
            q.queued = Some(kind);
            let spawn = !q.thread_alive;
            q.thread_alive = true;
            spawn
        };
        self.loader.cv.notify_all();
        if spawn {
            let weak = self.me.clone();
            let shared = self.loader.clone();
            let r = std::thread::Builder::new()
                .name("ext-loader".into())
                .spawn(move || loader_loop(weak, shared));
            if r.is_err() {
                let mut q = lock(&self.loader.q);
                q.thread_alive = false;
                q.queued = None;
                q.done = Some(Err("the ext-loader thread could not be started".into()));
            }
        }
    }

    /// True while a load is queued, running, or finished but not committed.
    pub fn reload_pending(&self) -> bool {
        let q = lock(&self.loader.q);
        q.running || q.queued.is_some() || q.done.is_some()
    }

    /// Waits up to `max_wait` for the pending load, then swaps the registry and returns the
    /// report. None if nothing is pending, or if the load is still running at `max_wait` (it
    /// stays pending for the next boundary).
    pub fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport> {
        let end = Instant::now() + max_wait;
        let done = {
            let mut q = lock(&self.loader.q);
            loop {
                if let Some(d) = q.done.take() {
                    break d;
                }
                if !q.running && q.queued.is_none() {
                    return None;
                }
                let now = Instant::now();
                if now >= end {
                    return None;
                }
                q = self
                    .loader
                    .cv
                    .wait_timeout(q, end - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        };
        Some(match done {
            Ok(staged) => self.apply(staged),
            Err(msg) => {
                let g = self.snapshot().generation;
                ReloadReport {
                    ok: false,
                    generation: g,
                    components: Vec::new(),
                    text: format!("[init:reload] ext generation {g} (unchanged)\n! loader: {msg}"),
                }
            }
        })
    }

    /// The registry the next load builds on: the staged set if one is waiting, else the active one.
    fn load_base(&self) -> BTreeMap<String, Arc<ExtVersion>> {
        if let Some(Ok(st)) = &lock(&self.loader.q).done {
            return st.exts.clone();
        }
        self.snapshot().exts.clone()
    }

    fn resolve(&self, kind: &ReloadKind) -> Vec<Candidate> {
        let p = &self.paths;
        let mut out = Vec::new();
        if matches!(kind, ReloadKind::Boot) {
            let mut files: Vec<PathBuf> = fs::read_dir(&p.kept)
                .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
                .unwrap_or_default();
            files.sort();
            for f in files {
                if f.extension().and_then(|x| x.to_str()) != Some("wasm") || !f.is_file() {
                    continue;
                }
                let Some(stem) = f.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                out.push(Candidate {
                    name: stem.replace('_', "-"),
                    looked_for: p.rel(&f),
                    artifact: Some((f.clone(), Origin::Kept)),
                });
            }
            return out;
        }
        let mut dirs: Vec<String> = fs::read_dir(&p.src)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().join("Cargo.toml").is_file())
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        dirs.sort();
        let (session_start, kept_at) = {
            let c = lock(&self.clock);
            (c.session_start, c.kept_at.clone())
        };
        for n in dirs {
            let file = file_name(&n);
            let floor = match kept_at.get(&n) {
                Some(k) if *k > session_start => *k,
                _ => session_start,
            };
            let mut trial: Option<(PathBuf, SystemTime)> = None;
            for prof in ["debug", "release"] {
                let f = p.target.join("wasm32-wasip2").join(prof).join(&file);
                if let Ok(m) = fs::metadata(&f)
                    && m.is_file()
                    && let Ok(t) = m.modified()
                    && t > floor
                    && trial.as_ref().is_none_or(|(_, bt)| t > *bt)
                {
                    trial = Some((f, t));
                }
            }
            let kept = p.kept.join(&file);
            let artifact = match trial {
                Some((f, _)) => Some((f, Origin::Trial)),
                None if kept.is_file() => Some((kept, Origin::Kept)),
                None => None,
            };
            let looked_for = format!(
                "{}/wasm32-wasip2/{{debug,release}}/{file}",
                p.rel(&p.target)
            );
            out.push(Candidate {
                name: n,
                artifact,
                looked_for,
            });
        }
        out
    }

    /// One reload, from scratch (§3.8). Candidates in name order, so reports are deterministic.
    pub(crate) fn load_all(&self, kind: &ReloadKind) -> Staged {
        let base = self.load_base();
        let mut st = Staged {
            exts: BTreeMap::new(),
            failed: BTreeMap::new(),
            not_built: Vec::new(),
            notes: BTreeMap::new(),
        };
        for c in self.resolve(kind) {
            let active = base.get(&c.name);
            let Some((path, origin)) = c.artifact else {
                st.not_built.push((c.name, c.looked_for));
                continue;
            };
            match self.load_one(Some(&c.name), &path, origin, active) {
                Ok((v, notes)) => {
                    if !notes.is_empty() {
                        st.notes.insert(c.name.clone(), notes);
                    }
                    st.exts.insert(c.name, v);
                }
                Err(e) => {
                    if let Some(old) = active {
                        st.exts.insert(c.name.clone(), old.clone());
                    }
                    st.failed.insert(c.name, e);
                }
            }
        }
        for name in st.exts.keys() {
            let _ = fs::create_dir_all(self.paths.data_dir(name));
        }
        st
    }

    /// §3.8 steps 1–7 for one artifact. `name: None` skips the directory-name check.
    pub(crate) fn load_one(
        &self,
        name: Option<&str>,
        path: &Path,
        origin: Origin,
        active: Option<&Arc<ExtVersion>>,
    ) -> Result<(Arc<ExtVersion>, Vec<String>), String> {
        let shown = self.paths.rel(path);
        let meta = fs::metadata(path).map_err(|e| format!("{shown}: {e}"))?;
        let mt = mtime_ns(&meta);
        if let Some(a) = active
            && a.artifact == path
            && a.fp.len == meta.len()
            && a.fp.mtime_ns == mt
        {
            return Ok((a.clone(), Vec::new())); // unchanged
        }
        if meta.len() > WASM_MAX_BYTES {
            return Err(format!(
                "{shown} is {} bytes; the limit is 64 MiB",
                meta.len()
            ));
        }
        let bytes = fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
        let fp = Fingerprint {
            len: bytes.len() as u64,
            mtime_ns: mt,
            hash: hash_bytes(&bytes),
        };
        if let Some(a) = active
            && a.fp.len == fp.len
            && a.fp.hash == fp.hash
        {
            // Same bytes at a new path or origin: no recompile.
            let mut v = (**a).clone();
            v.artifact = path.to_path_buf();
            v.origin = origin;
            v.fp = fp;
            return Ok((Arc::new(v), Vec::new()));
        }
        let t0 = Instant::now();
        let component = Component::from_binary(&self.engine, &bytes)
            .map_err(|e| format!("compile failed: {}", chain(&e)))?;
        let compile_ms = t0.elapsed().as_millis().min(u32::MAX as u128) as u32;
        // Newest supported major first (ext/wit.md §3.6, D8); only v1 today.
        let pre = self
            .linker_v1
            .instantiate_pre(&component)
            .and_then(v1::ToolPre::new)
            .map(Pre::V1)
            .map_err(|e| format!("does not match init:ext@1: {}", chain(&e)))?;
        let label = name.unwrap_or("describe");
        let a = self.describe_pre(&pre, label)?;
        let b = self.describe_pre(&pre, label)?;
        if a != b {
            return Err(
                "describe is not deterministic (two calls returned different values)".into(),
            );
        }
        let (info, notes) = validate(name, &a)?;
        Ok((
            Arc::new(ExtVersion {
                name: info.name.clone(),
                info,
                api: 1,
                origin,
                artifact: path.to_path_buf(),
                fp,
                compile_ms,
                loaded_at: SystemTime::now(),
                pre,
            }),
            notes,
        ))
    }

    /// The swap: one assignment under the registry mutex, then the catalog (§3.9 rule 7).
    fn apply(&self, st: Staged) -> ReloadReport {
        let (report, cat) = {
            let mut cur = lock(&self.current);
            let old = cur.clone();
            let generation = old.generation + 1;
            let new = Arc::new(Registry {
                generation,
                exts: st.exts.clone(),
                failed: st.failed.clone(),
                not_built: st.not_built.iter().map(|(n, _)| n.clone()).collect(),
            });
            let report = self.render(&old, &new, &st);
            *cur = new.clone();
            (report, new)
        };
        let mut report = report;
        if let Err(e) = self.write_catalog(&cat) {
            report.text.push_str(&format!(
                "\n· warning: state/ext/catalog.json not written: {e}"
            ));
        }
        report
    }

    fn render(&self, old: &Registry, new: &Registry, st: &Staged) -> ReloadReport {
        let mut lines = vec![format!("[init:reload] ext generation {}", new.generation)];
        let mut components = Vec::new();
        let names: BTreeSet<&String> = old
            .exts
            .keys()
            .chain(new.exts.keys())
            .chain(st.failed.keys())
            .collect();
        for n in names {
            let (o, v) = (old.exts.get(n), new.exts.get(n));
            match (o, v) {
                (None, Some(v)) => {
                    components.push(n.clone());
                    lines.push(format!(
                        "+ {n} {} ({}, compiled in {} ms): {}",
                        v.info.version,
                        v.origin.as_str(),
                        v.compile_ms,
                        v.info.description
                    ));
                    lines.push(format!("  input_schema: {}", v.info.schema_text()));
                }
                (Some(o), Some(v))
                    if !Arc::ptr_eq(o, v)
                        && (o.fp.hash != v.fp.hash || o.origin != v.origin || o.info != v.info) =>
                {
                    components.push(n.clone());
                    let ver = if o.info.version != v.info.version {
                        format!("{} → {}", o.info.version, v.info.version)
                    } else {
                        v.info.version.clone()
                    };
                    let how = if o.fp.hash == v.fp.hash {
                        v.origin.as_str().to_string()
                    } else {
                        format!("{}, compiled in {} ms", v.origin.as_str(), v.compile_ms)
                    };
                    lines.push(format!("~ {n} {ver} ({how})"));
                    if o.info.description != v.info.description {
                        lines.push(format!("  description: {}", v.info.description));
                    }
                    if o.info.input_schema != v.info.input_schema {
                        lines.push(format!("  input_schema: {}", v.info.schema_text()));
                    }
                }
                (Some(_), None) if !st.failed.contains_key(n) => {
                    components.push(n.clone());
                    lines.push(format!("- {n} (removed)"));
                }
                _ => {}
            }
            if let Some(e) = st.failed.get(n) {
                let prev = match new.exts.get(n) {
                    Some(v) => format!("version {} still serving", v.info.version),
                    None => "no previous version".into(),
                };
                lines.push(format!("! {n}: {e} ({prev})"));
            }
            if let Some(ns) = st.notes.get(n) {
                for note in ns {
                    lines.push(format!("  note: {note}"));
                }
            }
        }
        for (n, looked) in &st.not_built {
            lines.push(format!("· not built: {n} (looked for {looked})"));
        }
        for (n, c) in self.stuck.snapshot() {
            lines.push(format!(
                "· stuck: {n} ({c} call{} still blocked in a host call; calls to it return busy)",
                if c == 1 { "" } else { "s" }
            ));
        }
        if lines.len() == 1 {
            lines.push("(no changes)".into());
        }
        ReloadReport {
            ok: st.failed.is_empty(),
            generation: new.generation,
            components,
            text: lines.join("\n"),
        }
    }

    /// The catalog as a `Value` (sorted keys, integers and strings only; §4.2).
    pub(crate) fn catalog_value(&self, reg: &Registry) -> Value {
        let mut exts = Map::new();
        for (n, v) in &reg.exts {
            exts.insert(
                n.clone(),
                json!({
                    "api": format!("init:ext@{}", v.api),
                    "artifact": self.paths.rel(&v.artifact),
                    "compile_ms": v.compile_ms,
                    "description": v.info.description,
                    "input_schema": v.info.input_schema,
                    "origin": v.origin.as_str(),
                    "timeout_ms": v.info.timeout_ms,
                    "version": v.info.version,
                }),
            );
        }
        let failed: Map<String, Value> = reg
            .failed
            .iter()
            .map(|(k, e)| (k.clone(), Value::String(e.clone())))
            .collect();
        let stuck: Map<String, Value> = self
            .stuck
            .snapshot()
            .into_iter()
            .map(|(k, c)| (k, json!(c)))
            .collect();
        json!({
            "extensions": exts,
            "failed": failed,
            "generation": reg.generation,
            "not_built": reg.not_built,
            "stuck": stuck,
        })
    }

    /// Write-to-temp plus `rename` (atomic); informational, never read back.
    fn write_catalog(&self, reg: &Registry) -> std::io::Result<()> {
        let _g = lock(&self.catalog_lock);
        fs::create_dir_all(&self.paths.state)?;
        let mut bytes =
            serde_json::to_vec_pretty(&self.catalog_value(reg)).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let tmp = self
            .paths
            .state
            .join(format!(".catalog.json.tmp-{}", std::process::id()));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, self.paths.state.join("catalog.json"))
    }
}

fn loader_loop(weak: Weak<ExtHost>, shared: Arc<LoaderShared>) {
    loop {
        let kind = {
            let mut q = lock(&shared.q);
            loop {
                if q.stop {
                    q.thread_alive = false;
                    return;
                }
                if let Some(k) = q.queued.take() {
                    q.running = true;
                    break k;
                }
                q = shared
                    .cv
                    .wait_timeout(q, Duration::from_secs(5))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                if weak.strong_count() == 0 {
                    q.thread_alive = false;
                    return;
                }
            }
        };
        let result = match weak.upgrade() {
            Some(host) => {
                let r =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| host.load_all(&kind)));
                drop(host);
                r.map_err(|p| {
                    let msg = p
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".into());
                    format!("the loader panicked: {msg}; the registry is unchanged")
                })
            }
            None => {
                lock(&shared.q).thread_alive = false;
                return;
            }
        };
        let mut q = lock(&shared.q);
        q.running = false;
        q.done = Some(result);
        drop(q);
        shared.cv.notify_all();
    }
}
