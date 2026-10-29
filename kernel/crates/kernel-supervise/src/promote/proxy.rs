//! The gate's view of the in-process proxy (kernel/proxy.md §4.4), behind a trait so this crate
//! does not link kernel-proxy (lane independence, as WP4's `ProxyCtl`). `init-kernel` implements
//! it with `kernel_proxy::ProxyHandle`; the tests do the same with the real proxy.

use crate::probe::ProbeScope;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// `ScopeKind` of a gate lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    /// G2 arms.
    Probe,
    /// The G3 probe.
    G3,
    /// A G4 task run.
    Run,
}

/// A reconstructed request (proxy.md §3.13 reader).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestView {
    /// The message-list elements, as sent.
    pub elements: Vec<Vec<u8>>,
    /// The ICJ head object (log-context.md §3.10.4), for the `prefix_hash` byte offset.
    pub head: Option<Vec<u8>>,
}

/// What the gate asks of the proxy.
pub trait GateProxy: Send + Sync {
    /// `open_gate(job, kind, cap)`: the per-gate soft budget over all of the job's lanes.
    fn open_gate(&self, job: &str, kind: &str, cap_musd: u64) -> Result<(), String>;
    /// `close_gate(job)`: spent µUSD.
    fn close_gate(&self, job: &str) -> u64;
    /// `open_scope` on `socket` (the probe's `run/proxy.sock`). `kernel_only` = `Visibility::Kernel`.
    #[allow(clippy::too_many_arguments)]
    fn open_scope(&self, scope_id: &str, kind: LaneKind, job: &str, kernel_only: bool, cap_musd: u64, sha: &str, boot_reason: &str, socket: &Path) -> Result<(), String>;
    /// `close_scope`: the final `ScopeSummary`.
    fn close_scope(&self, scope_id: &str) -> Value;
    /// `scope_summary` (`"live"` is the primary's lane).
    fn scope_summary(&self, scope_id: &str) -> Value;
    /// The trace store's reconstruction of one request.
    fn request(&self, req_id: &str) -> Option<RequestView>;
    /// One model request's reservation floor for the routed model, in µUSD (G4 refuses a task
    /// whose cap is below it: `gate.g4.task_cost_cap`, K-GATE). `None`: unknown, not checked.
    fn request_reservation_musd(&self) -> Option<u64> {
        None
    }
    /// Whether the proxy refuses requests for money (`ProxyHandle::money_enforced`). Under the Root's observe-only
    /// policy a task's cost cap refuses nothing, so G4's task-cap check does not apply (D11). The default (test
    /// proxies) enforces.
    fn money_enforced(&self) -> bool {
        true
    }
    /// Per-run caps on actuals on a `Run` lane (m7-design.md §3.1.3, §3.2.8; GATE-V2b), set right
    /// after `open_scope`, before the run's body spawns. The default (test proxies) enforces nothing.
    fn set_run_caps(&self, scope_id: &str, caps: kernel_gate::caps::RunCaps) -> Result<(), String> {
        let _ = (scope_id, caps);
        Ok(())
    }
}

/// No proxy: scopes open, summaries are null (G2 then fails `probe_no_round_trip`, honestly).
pub struct NoGateProxy;

impl GateProxy for NoGateProxy {
    fn open_gate(&self, _: &str, _: &str, _: u64) -> Result<(), String> {
        Ok(())
    }
    fn close_gate(&self, _: &str) -> u64 {
        0
    }
    fn open_scope(&self, _: &str, _: LaneKind, _: &str, _: bool, _: u64, _: &str, _: &str, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn close_scope(&self, _: &str) -> Value {
        Value::Null
    }
    fn scope_summary(&self, _: &str) -> Value {
        Value::Null
    }
    fn request(&self, _: &str) -> Option<RequestView> {
        None
    }
}

/// One probe's lane: a [`ProbeScope`] over a [`GateProxy`].
pub struct GateScope {
    pub proxy: Arc<dyn GateProxy>,
    pub scope_id: String,
    pub kind: LaneKind,
    pub job: String,
    pub kernel_only: bool,
    pub cap_musd: u64,
    pub sha: String,
    pub boot_reason: String,
    /// Per-run caps for a G4 or screen run's lane ([`GateScope::set_caps`]).
    caps: std::sync::OnceLock<kernel_gate::caps::RunCaps>,
    closed: Mutex<Option<Value>>,
}

impl GateScope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(proxy: Arc<dyn GateProxy>, scope_id: &str, kind: LaneKind, job: &str, kernel_only: bool, cap_musd: u64, sha: &str, boot_reason: &str) -> Arc<GateScope> {
        Arc::new(GateScope {
            proxy,
            scope_id: scope_id.into(),
            kind,
            job: job.into(),
            kernel_only,
            cap_musd,
            sha: sha.into(),
            boot_reason: boot_reason.into(),
            caps: std::sync::OnceLock::new(),
            closed: Mutex::new(None),
        })
    }

    /// The lane's per-run caps, applied when the lane opens (once; a second call is ignored).
    pub fn set_caps(&self, caps: kernel_gate::caps::RunCaps) {
        let _ = self.caps.set(caps);
    }
}

impl ProbeScope for GateScope {
    fn open(&self, socket: &Path, _probe_id: &str) -> Result<(), String> {
        self.proxy.open_scope(&self.scope_id, self.kind, &self.job, self.kernel_only, self.cap_musd, &self.sha, &self.boot_reason, socket)?;
        match self.caps.get() {
            Some(c) => self.proxy.set_run_caps(&self.scope_id, *c),
            None => Ok(()),
        }
    }
    fn summary(&self) -> Value {
        if let Some(v) = self.closed.lock().unwrap().as_ref() {
            return v.clone();
        }
        self.proxy.scope_summary(&self.scope_id)
    }
    fn close(&self) -> Value {
        let mut g = self.closed.lock().unwrap();
        if let Some(v) = g.as_ref() {
            return v.clone();
        }
        let v = self.proxy.close_scope(&self.scope_id);
        *g = Some(v.clone());
        v
    }
}
