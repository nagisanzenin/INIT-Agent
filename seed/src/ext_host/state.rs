//! Per-call store state, the WASI p2 context, and the `init:ext/process` host glue
//! (ext/host.md §3.6, §3.13; ext/wit.md §3.4).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use wasmtime::component::ResourceTable;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::bindings::v1::init::ext::process::{
    Host, ProcessError, ReadResult, RunOutput, RunRequest, SpawnRequest,
};
use super::engine::CallLimiter;
use super::process::ProcHost;

/// What a store may reach: `Full` for `call`, `None` for `describe` (D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caps {
    Full,
    None,
}

/// Host paths of `/work` and `/data`.
#[derive(Clone, Debug)]
pub(crate) struct GuestRoots {
    pub work: PathBuf,
    pub data: PathBuf,
}

pub(crate) struct CallState {
    wasi: WasiCtx,
    table: ResourceTable,
    pub limits: CallLimiter,
    /// Extension name.
    pub ext: Arc<str>,
    /// Wall-clock end of this call.
    pub deadline: Instant,
    /// Set by the dispatcher on owner abort (§3.10).
    pub cancel: Arc<AtomicBool>,
    pub caps: Caps,
    /// None for `describe`.
    pub roots: Option<GuestRoots>,
    pub procs: Arc<ProcHost>,
}

impl CallState {
    pub(crate) fn new(
        wasi: WasiCtx,
        ext: Arc<str>,
        deadline: Instant,
        cancel: Arc<AtomicBool>,
        caps: Caps,
        roots: Option<GuestRoots>,
        procs: Arc<ProcHost>,
    ) -> Self {
        CallState {
            wasi,
            table: ResourceTable::new(),
            limits: CallLimiter::new(),
            ext,
            deadline,
            cancel,
            caps,
            roots,
            procs,
        }
    }

    fn full(&self) -> Result<&GuestRoots, ProcessError> {
        match (self.caps, &self.roots) {
            (Caps::Full, Some(r)) => Ok(r),
            _ => Err(ProcessError::Denied("describe has no capabilities".into())),
        }
    }
}

impl WasiView for CallState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// §3.6. Defaults: stdin closed, no env, no args, no preopens. Network is off three ways: TCP,
/// UDP and name lookup disabled, and the default socket-address check denies every address.
pub(crate) fn wasi_ctx(
    roots: Option<&GuestRoots>,
    log: &MemoryOutputPipe,
) -> wasmtime::Result<WasiCtx> {
    let mut b = WasiCtxBuilder::new();
    b.stdout(log.clone())
        .stderr(log.clone())
        .max_random_size(1 << 20)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false)
        .allow_blocking_current_thread(true);
    if let Some(r) = roots {
        b.preopened_dir(&r.work, "/work", FsPerms::ReadWrite)?
            .preopened_dir(&r.data, "/data", FsPerms::ReadWrite)?
            .initial_cwd("/work");
    }
    Ok(b.build())
}

impl Host for CallState {
    fn run(&mut self, req: RunRequest) -> Result<RunOutput, ProcessError> {
        let roots = self.full()?.clone();
        self.procs.run(&roots, req, self.deadline, &self.cancel)
    }

    fn spawn(&mut self, req: SpawnRequest) -> Result<u32, ProcessError> {
        let roots = self.full()?.clone();
        self.procs.spawn(&self.ext, &roots, req)
    }

    fn write(&mut self, session: u32, data: Vec<u8>) -> Result<(), ProcessError> {
        self.full()?;
        self.procs.write(&self.ext, session, data)
    }

    fn read(
        &mut self,
        session: u32,
        wait_ms: u32,
        until: Option<String>,
    ) -> Result<ReadResult, ProcessError> {
        self.full()?;
        self.procs.read(
            &self.ext,
            session,
            wait_ms,
            until,
            self.deadline,
            &self.cancel,
        )
    }

    fn close(&mut self, session: u32) -> Result<(), ProcessError> {
        self.full()?;
        self.procs.close(&self.ext, session)
    }

    fn sessions(&mut self) -> Vec<u32> {
        if self.full().is_err() {
            return Vec::new();
        }
        self.procs.sessions(&self.ext)
    }
}
