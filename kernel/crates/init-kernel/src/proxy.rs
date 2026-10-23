//! The `kernel-proxy` adapter behind supervise's [`ProxyCtl`] seam (kernel/supervise.md §3.2;
//! kernel/proxy.md Q-SV1). Lives in the binary so the lane crates stay independent.

use init_proto::serde_json::{self, Value};
use kernel_proxy::{Alert, AlertSink, GateKind, Label, Level, OpenScope, Proxy, ProxyConfig, ProxyHandle, ScopeKind, StartOptions, Visibility};
use kernel_supervise::promote::proxy::LaneKind;
use kernel_supervise::promote::{GateProxy, RequestView};
use kernel_supervise::proxyctl::{ProxyAlertSink, ProxyCtl, ProxyLabel};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The kernel's proxy, started by supervise after `hello`.
#[derive(Default)]
pub struct KernelProxy {
    h: Mutex<Option<ProxyHandle>>,
}

fn label(l: &ProxyLabel) -> Label {
    Label { sha: l.sha.clone(), boot_reason: l.boot_reason.clone(), model: l.model.clone(), level: Level::from_u8(l.level) }
}

impl KernelProxy {
    fn with(&self, f: impl FnOnce(&ProxyHandle)) {
        if let Some(h) = self.h.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            f(h);
        }
    }
}

impl ProxyCtl for KernelProxy {
    fn start(&self, cfg: &Value, home: &Path, egress_sock: &Path, l: &ProxyLabel, alerts: ProxyAlertSink) -> Result<(), Vec<String>> {
        let pc: ProxyConfig = serde_json::from_value(cfg.clone()).map_err(|e| vec![format!("proxy config: {e}")])?;
        let sink: AlertSink = Arc::new(move |a: Alert| alerts(&a.code, &a.message));
        let opts = StartOptions { home: home.to_path_buf(), egress_sock: egress_sock.to_path_buf(), live_socket: None, label: label(l), judge: false };
        let h = Proxy::start(&pc, opts, sink)?;
        *self.h.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
        Ok(())
    }

    fn set_level(&self, level: u8) {
        self.with(|h| h.set_level(Level::from_u8(level)));
    }

    fn set_ceilings(&self, c: &Value) {
        self.with(|h| h.set_ceilings(c.clone()));
    }

    fn set_label(&self, scope: &str, l: &ProxyLabel) {
        self.with(|h| {
            let _ = h.set_label(scope, label(l));
        });
    }

    fn set_body(&self, scope: &str, instance: &str) {
        self.with(|h| {
            let _ = h.set_body_id(scope, instance);
        });
    }

    fn shutdown(&self, grace: Duration) {
        self.with(|h| h.shutdown(grace));
    }

    fn stats(&self, scope: &str) -> Option<Value> {
        self.handle()?.stats(scope)
    }
}

impl KernelProxy {
    fn handle(&self) -> Option<ProxyHandle> {
        self.h.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The gate's view of the kernel proxy (`promote::GateProxy`, kernel/proxy.md §4.4): per-gate
/// budgets, probe and run lanes, scope summaries and the trace reader. WP8b's `TestProxy` is the
/// reference; this one labels lanes with the configured model and the current level.
pub struct KernelGate {
    proxy: Arc<KernelProxy>,
    home: PathBuf,
    provider: String,
    model: String,
    sup: kernel_supervise::supervisor::Handle,
}

impl KernelGate {
    pub fn new(proxy: Arc<KernelProxy>, home: &Path, provider: &str, model: &str, sup: kernel_supervise::supervisor::Handle) -> KernelGate {
        KernelGate { proxy, home: home.to_path_buf(), provider: provider.into(), model: model.into(), sup }
    }
}

impl GateProxy for KernelGate {
    fn open_gate(&self, job: &str, kind: &str, cap: u64) -> Result<(), String> {
        let h = self.proxy.handle().ok_or("the proxy is not running")?;
        h.open_gate(job, if kind == "quick" { GateKind::Quick } else { GateKind::Promote }, cap).map_err(|e| format!("{e:?}"))
    }
    fn close_gate(&self, job: &str) -> u64 {
        self.proxy.handle().and_then(|h| h.close_gate(job).ok()).map(|s| s.spent_pusd / 1_000_000).unwrap_or(0)
    }
    fn open_scope(&self, id: &str, kind: LaneKind, job: &str, kernel_only: bool, cap: u64, sha: &str, reason: &str, socket: &Path) -> Result<(), String> {
        let h = self.proxy.handle().ok_or("the proxy is not running")?;
        let kind = match kind {
            LaneKind::Probe => ScopeKind::Probe,
            LaneKind::G3 => ScopeKind::G3,
            LaneKind::Run => ScopeKind::Run,
        };
        let level = self.sup.latest().map(|s| s.level).unwrap_or(0);
        let label = Label { sha: sha.into(), boot_reason: reason.into(), model: self.model.clone(), level: Level::from_u8(level) };
        h.open_scope(OpenScope { scope_id: id.into(), kind, job_id: Some(job.into()), visibility: if kernel_only { Visibility::Kernel } else { Visibility::Agent }, cap_musd: cap, label, socket: socket.into() })
            .map_err(|e| format!("{e:?}"))
    }
    fn close_scope(&self, id: &str) -> Value {
        self.proxy.handle().and_then(|h| h.close_scope(id).ok()).unwrap_or(Value::Null)
    }
    fn scope_summary(&self, id: &str) -> Value {
        self.proxy.handle().and_then(|h| h.scope_summary(id).ok()).unwrap_or(Value::Null)
    }
    fn request_reservation_musd(&self) -> Option<u64> {
        self.proxy.handle()?.request_reservation_musd(&self.provider, &self.model)
    }
    fn set_run_caps(&self, id: &str, c: kernel_gate::caps::RunCaps) -> Result<(), String> {
        let h = self.proxy.handle().ok_or("the proxy is not running")?;
        let caps = kernel_proxy::RunCaps { max_turns: Some(c.max_turns), max_output_tokens: Some(c.max_output_tokens), max_input_tokens: Some(c.max_input_tokens) };
        h.set_run_caps(id, caps).map_err(|e| format!("{e:?}"))
    }
    fn request(&self, req_id: &str) -> Option<RequestView> {
        let family = if self.provider == "anthropic" { kernel_proxy::tap::Family::Anthropic } else { kernel_proxy::tap::Family::OpenAiChat };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok((bytes, elements)) = kernel_proxy::trace::reconstruct(&self.home.join("vault/traces/v1"), req_id) {
                let head = serde_json::from_slice::<Value>(&bytes).ok().map(|v| kernel_proxy::hash::head_object(&self.provider, family, &v).to_string().into_bytes());
                return Some(RequestView { elements, head });
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
