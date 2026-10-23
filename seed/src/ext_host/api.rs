//! The seam to the `ext` tool and the loop: `ExtHost` implements WP11's `ExtHostApi`
//! (`crate::tools::ext`, which mirrors host.md §4.1). The registry snapshot crosses the seam
//! type-erased (`Snapshot = Arc<dyn Any + Send + Sync>`) and is downcast back here.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{CallCtx, ExtHost, ExtOutcome, Registry, ReloadKind, ReloadReport};
use crate::tools::ext::{ExtHostApi, Snapshot};

impl ExtHostApi for ExtHost {
    fn begin_reload(&self, kind: ReloadKind) {
        ExtHost::begin_reload(self, kind)
    }

    fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport> {
        ExtHost::commit_reload(self, max_wait)
    }

    fn reload_pending(&self) -> bool {
        ExtHost::reload_pending(self)
    }

    fn reset_trials(&self) {
        ExtHost::reset_trials(self)
    }

    fn snapshot(&self) -> Snapshot {
        let reg: Arc<Registry> = ExtHost::snapshot(self);
        reg
    }

    fn call(&self, reg: &Snapshot, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome {
        match reg.downcast_ref::<Registry>() {
            Some(r) => ExtHost::call(self, r, name, input, cx),
            // A snapshot from another host type (e.g. the stub): use our own current one.
            None => ExtHost::call(self, &ExtHost::snapshot(self), name, input, cx),
        }
    }

    fn shutdown(&self) {
        ExtHost::shutdown(self)
    }
}

/// The constructor the loop/boot uses: `INIT_HOME` and `CARGO_TARGET_DIR` from the environment,
/// boot reload queued. `Arc<ExtHost>` coerces to `Arc<dyn ExtHostApi>`.
pub fn from_env() -> Result<Arc<dyn ExtHostApi>, String> {
    let host = ExtHost::new(super::ExtPaths::from_env()?)?;
    Ok(host)
}
