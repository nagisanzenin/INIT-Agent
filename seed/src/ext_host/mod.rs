//! The extension host (docs/ext/host.md): runs T1 extensions, WebAssembly components built for
//! `wasm32-wasip2` against WIT `init:ext@1.0.0`, behind the fixed `ext` tool.
//!
//! - One Wasmtime `Engine` per body process: Cranelift only (Winch is not compiled in), the
//!   pooling allocator (32 instances, 256 MiB memories), epoch interruption, an in-memory
//!   incremental cache and no `unsafe` deserialization anywhere (§3.3, §3.14).
//! - A fresh `Store` per call with `StoreLimits`, an epoch callback every 10 ms tick that enforces
//!   the deadline and the owner-abort flag, and a wall-clock watchdog (§3.10–§3.12).
//! - A registry swapped atomically at turn boundaries (`begin_reload` / `commit_reload`); a call
//!   holds its `Arc<ExtVersion>`, so old versions drain (§3.7–§3.9).
//! - The `init:ext/process` host: `run` and pipe sessions, every child spawned and owned by the
//!   host and killed only by the process group it holds (§3.13).
//!
//! Entry point for the loop and the `ext` tool: [`ExtHost::new`] returns `Arc<ExtHost>`, which
//! implements `crate::tools::ExtHostApi` (see `api.rs`).

mod api;
pub(crate) mod bindings;
pub mod cli;
mod dispatch;
mod engine;
mod loader;
mod process;
mod registry;
mod state;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use wasmtime::Engine;
use wasmtime::component::Linker;

#[allow(unused_imports)]
pub use crate::tools::ext::{CallCtx, ExtOutcome, OutcomeKind, ReloadKind, ReloadReport};
#[allow(unused_imports)]
pub use api::from_env;
#[allow(unused_imports)]
pub use registry::{ExtVersion, Fingerprint, Origin, Registry, ToolInfoV};

use dispatch::{StuckTable, Ticker};
use engine::MemCacheStore;
use loader::LoaderShared;
use process::ProcHost;
use state::CallState;

/// `init:ext` majors this host supports, newest first (boot.md prints it as `ext_api_majors`).
pub const SUPPORTED_MAJORS: &[u32] = &[1];

// Constants (host.md §4.1, one table).
pub const EPOCH_TICK: Duration = Duration::from_millis(10);
pub const CALL_TIMEOUT_DEFAULT_MS: u32 = 30_000;
pub const CALL_TIMEOUT_MIN_MS: u32 = 100;
pub const CALL_TIMEOUT_MAX_MS: u32 = 600_000;
pub const DESCRIBE_TIMEOUT: Duration = Duration::from_millis(1_000);
pub const WATCHDOG_GRACE: Duration = Duration::from_millis(2_000);
pub const ABORT_GRACE: Duration = Duration::from_millis(200);
pub const MAX_STUCK: usize = 8;
pub const INPUT_MAX: usize = 1 << 20;
pub const OUTPUT_MAX: usize = 1 << 20;
pub const LOG_CAP: usize = 1 << 20;
pub const LOG_TAIL: usize = 4096;
pub const LOG_ROTATE: u64 = 1 << 20;
pub const WASM_MAX_BYTES: u64 = 64 << 20;
pub const INCR_CACHE_BYTES: usize = 32 << 20;
pub const MAX_MEMORY_BYTES: usize = 256 << 20;
pub const TABLE_ELEMENTS: usize = 20_000;
pub const RELOAD_WAIT_REQUESTED: Duration = Duration::from_secs(10);
pub const RELOAD_WAIT_BEFORE_EXT: Duration = Duration::from_secs(60);

/// host.md §4.1.
#[derive(Clone, Debug)]
pub struct ExtPaths {
    /// `$INIT_HOME`
    pub home: PathBuf,
    /// `$INIT_HOME/body/ext`
    pub src: PathBuf,
    /// `$INIT_HOME/bin/ext` (kernel-published symlink to an immutable set)
    pub kept: PathBuf,
    /// `$CARGO_TARGET_DIR` or `$INIT_HOME/work/target`
    pub target: PathBuf,
    /// `$INIT_HOME/state/ext`
    pub state: PathBuf,
}

impl ExtPaths {
    pub fn from_home(home: &Path, cargo_target_dir: Option<&Path>) -> Self {
        ExtPaths {
            home: home.to_path_buf(),
            src: home.join("body/ext"),
            kept: home.join("bin/ext"),
            target: cargo_target_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.join("work/target")),
            state: home.join("state/ext"),
        }
    }

    /// §4.6: `INIT_HOME`, and `CARGO_TARGET_DIR` (else `$INIT_HOME/work/target`).
    pub fn from_env() -> Result<Self, String> {
        let home = std::env::var_os("INIT_HOME")
            .filter(|h| !h.is_empty())
            .ok_or("INIT_HOME is not set")?;
        let tgt = std::env::var_os("CARGO_TARGET_DIR").filter(|t| !t.is_empty());
        Ok(Self::from_home(
            Path::new(&home),
            tgt.as_deref().map(Path::new),
        ))
    }

    /// A path for the catalog and the report: relative to `INIT_HOME` when inside it.
    pub(crate) fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.home)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    }

    pub(crate) fn data_dir(&self, name: &str) -> PathBuf {
        self.state.join(name).join("data")
    }
}

/// Session clock for the trial/kept rule (§3.8): `floor(n) = max(session_start, kept_at[n])`.
pub(crate) struct TrialClock {
    pub session_start: SystemTime,
    pub kept_at: BTreeMap<String, SystemTime>,
}

pub struct ExtHost {
    /// Self-reference for the loader thread (which must not keep the host alive).
    pub(crate) me: Weak<ExtHost>,
    pub(crate) paths: ExtPaths,
    pub(crate) engine: Engine,
    pub(crate) incr: Arc<MemCacheStore>,
    pub(crate) linker_v1: Linker<CallState>,
    pub(crate) current: Mutex<Arc<Registry>>,
    pub(crate) loader: Arc<LoaderShared>,
    pub(crate) ticker: Arc<Ticker>,
    pub(crate) stuck: Arc<StuckTable>,
    pub(crate) procs: Arc<ProcHost>,
    pub(crate) clock: Mutex<TrialClock>,
    /// Serializes `catalog.json` writes.
    pub(crate) catalog_lock: Mutex<()>,
    pub(crate) created: Instant,
}

impl ExtHost {
    /// Engine, linkers, ticker and loader; queues the `Boot` reload at once, so compiling the
    /// kept set overlaps the boot model round trip (§3.9 rule 4).
    pub fn new(paths: ExtPaths) -> Result<Arc<ExtHost>, String> {
        let h = Self::build(paths)?;
        h.begin_reload(ReloadKind::Boot);
        Ok(h)
    }

    /// Without the boot reload (`init-body ext …` and tests).
    pub(crate) fn build(paths: ExtPaths) -> Result<Arc<ExtHost>, String> {
        let incr = Arc::new(MemCacheStore::default());
        let engine = engine::new_engine(incr.clone()).map_err(|e| format!("engine: {e:#}"))?;
        let linker_v1 = bindings::new_linker_v1(&engine).map_err(|e| format!("linker: {e:#}"))?;
        let ticker = Arc::new(Ticker::default());
        dispatch::start_ticker(&engine, ticker.clone()).map_err(|e| format!("ticker: {e}"))?;
        let host = Arc::new_cyclic(|me| ExtHost {
            me: me.clone(),
            paths,
            engine,
            incr,
            linker_v1,
            current: Mutex::new(Arc::new(Registry::default())),
            loader: Arc::new(LoaderShared::default()),
            ticker,
            stuck: Arc::new(StuckTable::default()),
            procs: Arc::new(ProcHost::new()),
            clock: Mutex::new(TrialClock {
                session_start: SystemTime::now(),
                kept_at: BTreeMap::new(),
            }),
            catalog_lock: Mutex::new(()),
            created: Instant::now(),
        });
        Ok(host)
    }

    /// The active registry; called once per tool batch.
    pub fn snapshot(&self) -> Arc<Registry> {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Session start: only kept components (and trials built from now on) load.
    pub fn reset_trials(&self) {
        self.clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .session_start = SystemTime::now();
        self.begin_reload(ReloadKind::SessionStart);
    }

    /// Kills every process session; called on every orderly exit, including exit 75.
    pub fn shutdown(&self) {
        self.procs.shutdown();
    }

    pub fn paths(&self) -> &ExtPaths {
        &self.paths
    }
}

impl Drop for ExtHost {
    fn drop(&mut self) {
        self.loader.stop();
        self.procs.shutdown();
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
