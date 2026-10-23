//! WP11 test helpers. Every test home lives under `<target dir>/test-homes/wp11/` (`homes_root`) and
//! is removed by its exact path when the test ends (`INIT_WP11_KEEP=1` keeps it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::{Limits, Mode, PathCx, Ran, ToolCx, Tools, ToolsConfig};
use crate::model::{Abort, ToolCall};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

use crate::tools::ext::{CallCtx, ExtOutcome, OutcomeKind, ReloadKind, ReloadReport, Snapshot};
use crate::tools::{ExtHostApi, Signal, kill_group};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `<target dir>/test-homes/wp11`: `$CARGO_TARGET_DIR` when the environment sets it to an absolute
/// path (the gate's G0 does: the slot's `out/target`, because the source export is read-only), else
/// `<crate>/target`.
pub fn homes_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("test-homes")
        .join("wp11")
}

pub struct TempHome {
    pub path: PathBuf,
}

impl TempHome {
    pub fn new(name: &str) -> TempHome {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = homes_root().join(format!("{name}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        for d in ["state/sessions", "work", "userhome"] {
            std::fs::create_dir_all(path.join(d)).unwrap();
        }
        TempHome { path }
    }
    pub fn work(&self) -> PathBuf {
        self.path.join("work")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        if std::env::var("INIT_WP11_KEEP").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// The process environment as a map, with `overrides` applied (`None` removes a key).
pub fn base_env(overrides: &[(&str, Option<&str>)]) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = std::env::vars().collect();
    for k in [
        "CARGO_TARGET_DIR",
        "BASH_ENV",
        "INIT_TEST",
        "INIT_TEST_TOOL_TIMEOUT_S",
        "INIT_MEM_EVENTS",
        "INIT_MEM_CAP_MIB",
    ] {
        m.remove(k);
    }
    for (k, v) in overrides {
        match v {
            Some(v) => {
                m.insert(k.to_string(), v.to_string());
            }
            None => {
                m.remove(*k);
            }
        }
    }
    m
}

pub fn kill_hook(pg: i32) {
    kill_group(pg, Signal::Kill);
}

/// A tool test bench: a temp home, `Tools`, an `Abort`.
pub struct Bench {
    pub home: TempHome,
    pub tools: Tools,
    pub abort: Arc<Abort>,
    pub images: bool,
    pub mode: Mode,
    pub spill: PathBuf,
    /// The tools' cwd (`PathCx.cwd`); None = `work()`. ADR-009: boot sets it to the workspace.
    pub cwd: Option<PathBuf>,
}

impl Bench {
    pub fn new(name: &str) -> Bench {
        Bench::with(name, None, &[])
    }

    pub fn with(
        name: &str,
        ext: Option<Arc<dyn ExtHostApi>>,
        env: &[(&str, Option<&str>)],
    ) -> Bench {
        let home = TempHome::new(name);
        let tools = Tools::new(ToolsConfig {
            bash: super::find_bash(std::env::var("PATH").ok().as_deref()),
            ext,
            provider: "mock".into(),
            model: "mock-frontier-1".into(),
            init_home: Some(home.path.clone()),
            base_env: Some(base_env(env)),
            limits: Limits::default(),
        });
        let spill = home.path.join("state/spill/test-session");
        Bench {
            home,
            tools,
            abort: Arc::new(Abort::new(kill_hook)),
            images: true,
            mode: Mode::Normal,
            spill,
            cwd: None,
        }
    }

    pub fn work(&self) -> PathBuf {
        self.home.work()
    }

    pub fn call(&self, name: &str, input: Value) -> ToolCall {
        let raw = input.to_string();
        ToolCall {
            id: format!("call_{}", COUNTER.fetch_add(1, Ordering::Relaxed)),
            name: name.into(),
            input: input.as_object().cloned().unwrap_or_default(),
            raw_arguments: Some(raw),
        }
    }

    pub fn run_call(&self, c: &ToolCall) -> Ran {
        let work = self.cwd.clone().unwrap_or_else(|| self.work());
        let uhome = self.home.path.join("userhome");
        let pcx = PathCx {
            cwd: &work,
            home: Some(&uhome),
            init_home: Some(&self.home.path),
        };
        let p = self.tools.prepare(0, c, &pcx);
        let reg = self.tools.ext().map(|x| x.snapshot());
        let cx = ToolCx {
            paths: pcx,
            spill_dir: &self.spill,
            call_id: &c.id,
            abort: &self.abort,
            images: self.images,
            model_id: "mock-frontier-1",
            mode: self.mode,
            reg: reg.as_ref(),
        };
        self.tools.run(&p, &cx)
    }

    pub fn run(&self, name: &str, input: Value) -> Ran {
        let c = self.call(name, input);
        self.run_call(&c)
    }
}

/// The text of a result (its text blocks joined).
pub fn text(r: &Ran) -> String {
    r.result
        .content
        .iter()
        .filter_map(|b| match b {
            crate::model::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A host that records calls and answers with a scripted kind.
pub struct FakeHost {
    pub calls: AtomicUsize,
    pub kind: std::sync::Mutex<OutcomeKind>,
    pub workspace: std::sync::Mutex<Option<std::path::PathBuf>>,
    pub reloads: std::sync::Mutex<Vec<ReloadKind>>,
    pub shutdowns: AtomicUsize,
    pub resets: AtomicUsize,
    pub report: std::sync::Mutex<Option<ReloadReport>>,
    /// Moved into `report` by `begin_reload` (a load that completes with the next commit).
    pub next_report: std::sync::Mutex<Option<ReloadReport>>,
    /// `spin`: the call loops until the abort flag is set, then returns Aborted.
    pub spin: AtomicBool,
}

impl Default for FakeHost {
    fn default() -> Self {
        FakeHost {
            calls: AtomicUsize::new(0),
            kind: std::sync::Mutex::new(OutcomeKind::Ok),
            workspace: std::sync::Mutex::new(None),
            reloads: std::sync::Mutex::new(Vec::new()),
            shutdowns: AtomicUsize::new(0),
            resets: AtomicUsize::new(0),
            report: std::sync::Mutex::new(None),
            next_report: std::sync::Mutex::new(None),
            spin: AtomicBool::new(false),
        }
    }
}

impl ExtHostApi for FakeHost {
    fn begin_reload(&self, kind: ReloadKind) {
        self.reloads.lock().unwrap().push(kind);
        if let Some(r) = self.next_report.lock().unwrap().take() {
            *self.report.lock().unwrap() = Some(r);
        }
    }
    fn commit_reload(&self, _max_wait: Duration) -> Option<ReloadReport> {
        self.report.lock().unwrap().take()
    }
    fn reload_pending(&self) -> bool {
        self.report.lock().unwrap().is_some()
    }
    fn reset_trials(&self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
    }
    fn snapshot(&self) -> Snapshot {
        Arc::new(7u64)
    }
    fn call(&self, reg: &Snapshot, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(reg.downcast_ref::<u64>(), Some(&7));
        *self.workspace.lock().unwrap() = Some(cx.workspace.to_path_buf());
        let t0 = Instant::now();
        if self.spin.load(Ordering::SeqCst) {
            while !cx.abort.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            return ExtOutcome {
                text: format!("ext {name} 0.1.0: aborted by the owner"),
                is_error: true,
                kind: OutcomeKind::Aborted,
                version: Some("0.1.0".into()),
                elapsed_ms: t0.elapsed().as_millis() as u64,
                truncated: false,
            };
        }
        let kind = *self.kind.lock().unwrap();
        let (text, is_error) = match kind {
            OutcomeKind::Ok => (format!("{name} got {input}"), false),
            OutcomeKind::Unknown => (
                format!("no extension named \"{name}\". Loaded: count-lines (Count lines.)"),
                true,
            ),
            k => (format!("ext {name} 0.1.0: {k:?}"), true),
        };
        ExtOutcome {
            text,
            is_error,
            kind,
            version: Some("0.1.0".into()),
            elapsed_ms: 1,
            truncated: false,
        }
    }
    fn shutdown(&self) {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}
