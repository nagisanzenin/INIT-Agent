//! The extension-host seam at boot (body/boot.md §3.5) and `init-body ext …` argv dispatch
//! (ext/host.md §4.5). WP14 owns the host (`crate::ext_host`); boot only starts it and routes
//! the subcommands.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::env::BootEnv;
use crate::tools::{
    CallCtx, ExtHostApi, ExtOutcome, Mode, OutcomeKind, ReloadKind, ReloadReport, Snapshot,
};

/// Normal and probe mode start WP14's Wasmtime host (`ext_host::from_env`: `INIT_HOME`, and
/// `CARGO_TARGET_DIR` else `$INIT_HOME/work/target`). It queues the `Boot` load of `bin/ext/*.wasm`
/// without waiting for it (host.md §3.9 rule 4: compiling overlaps the first model round trip; the
/// loop commits the report at a boundary, or waits for it before an `ext` batch). Safe mode has no
/// host (`ext_disabled`). Returns the host, the boot facts' `ext` object and an optional warning.
pub fn start(env: &BootEnv) -> (Option<Arc<dyn ExtHostApi>>, Value, Option<String>) {
    if env.mode == Mode::Safe {
        return (None, json!({"status": "disabled"}), None);
    }
    match crate::ext_host::from_env() {
        Ok(host) => (
            Some(Arc::new(BootQuiet::new(host))),
            json!({"status": "loading"}),
            None,
        ),
        // The body still serves; every `ext` call says why there are no extensions.
        Err(e) => (
            Some(Arc::new(Unavailable(e.clone()))),
            json!({"error": e, "status": "unavailable"}),
            Some(format!("extension host unavailable: {e}")),
        ),
    }
}

/// `init-body ext describe|call|inspect …` (exit 0 ran / 1 load error / 2 usage).
pub fn cli(args: &[OsString]) -> ExitCode {
    crate::ext_host::cli::main(args)
}

/// The real host, minus one kind of report: a load that boot itself queued (`Boot` at process
/// start, `SessionStart` from `reset_trials`) and that changed nothing (`ok`, no components, no
/// `·` lines: the host's `(no changes)` text) is not reported, so a body without extensions does
/// not log `[init:reload] ext generation 1 (no changes)` at every boot (it would also sit after
/// the probe instruction in a probe body's first request). The registry swap itself still happens
/// inside the host's `commit_reload`. Once the agent or the kernel asked for a reload (`Request`,
/// `Kept`), every report is passed on, so `reload_done` and testing.md A2's order are unchanged.
struct BootQuiet {
    inner: Arc<dyn ExtHostApi>,
    explicit: AtomicBool,
}

impl BootQuiet {
    fn new(inner: Arc<dyn ExtHostApi>) -> BootQuiet {
        BootQuiet {
            inner,
            explicit: AtomicBool::new(false),
        }
    }
}

fn no_changes(r: &ReloadReport) -> bool {
    r.ok && r.components.is_empty() && r.text.lines().skip(1).all(|l| l == "(no changes)")
}

impl ExtHostApi for BootQuiet {
    fn begin_reload(&self, kind: ReloadKind) {
        if matches!(kind, ReloadKind::Request | ReloadKind::Kept(_)) {
            self.explicit.store(true, Ordering::SeqCst); // before the host queues it
        }
        self.inner.begin_reload(kind)
    }
    fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport> {
        let r = self.inner.commit_reload(max_wait)?;
        // Read after the commit: a report of an explicit load implies the flag was already set.
        (self.explicit.load(Ordering::SeqCst) || !no_changes(&r)).then_some(r)
    }
    fn reload_pending(&self) -> bool {
        self.inner.reload_pending()
    }
    fn reset_trials(&self) {
        self.inner.reset_trials()
    }
    fn snapshot(&self) -> Snapshot {
        self.inner.snapshot()
    }
    fn call(&self, reg: &Snapshot, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome {
        self.inner.call(reg, name, input, cx)
    }
    fn shutdown(&self) {
        self.inner.shutdown()
    }
}

/// The host when Wasmtime could not start (engine, linker or ticker error): no components, and
/// each call reports the cause as a host error (tools.md §3.6 `ext_host`).
struct Unavailable(String);

impl ExtHostApi for Unavailable {
    fn begin_reload(&self, _kind: ReloadKind) {}
    fn commit_reload(&self, _max_wait: Duration) -> Option<ReloadReport> {
        None
    }
    fn reload_pending(&self) -> bool {
        false
    }
    fn reset_trials(&self) {}
    fn snapshot(&self) -> Snapshot {
        Arc::new(())
    }
    fn call(&self, _reg: &Snapshot, name: &str, _input: &Value, _cx: &CallCtx) -> ExtOutcome {
        ExtOutcome {
            text: format!(
                "ext {name}: host error: the extension host did not start at boot ({}); no extensions are loaded",
                self.0
            ),
            is_error: true,
            kind: OutcomeKind::Host,
            version: None,
            elapsed_ms: 0,
            truncated: false,
        }
    }
    fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn unavailable_host_reports_the_cause() {
        let h = Unavailable("engine: boom".into());
        let abort = AtomicBool::new(false);
        let cx = CallCtx {
            call_id: "c1",
            workspace: std::path::Path::new("/"),
            abort: &abort,
        };
        let o = h.call(&h.snapshot(), "count-lines", &json!({}), &cx);
        assert!(o.is_error);
        assert_eq!(o.kind.code(), "ext_host");
        assert!(o.text.contains("engine: boom"), "{}", o.text);
        assert!(h.commit_reload(Duration::ZERO).is_none());
    }

    /// A host that returns its queued reports in order.
    struct Fake {
        reports: std::sync::Mutex<Vec<ReloadReport>>,
    }

    impl ExtHostApi for Fake {
        fn begin_reload(&self, _kind: ReloadKind) {}
        fn commit_reload(&self, _max_wait: Duration) -> Option<ReloadReport> {
            let mut q = self.reports.lock().unwrap();
            (!q.is_empty()).then(|| q.remove(0))
        }
        fn reload_pending(&self) -> bool {
            !self.reports.lock().unwrap().is_empty()
        }
        fn reset_trials(&self) {}
        fn snapshot(&self) -> Snapshot {
            Arc::new(())
        }
        fn call(&self, _: &Snapshot, _: &str, _: &Value, _: &CallCtx) -> ExtOutcome {
            unreachable!()
        }
        fn shutdown(&self) {}
    }

    fn report(generation: u64, components: &[&str], text: &str) -> ReloadReport {
        ReloadReport {
            ok: true,
            generation,
            components: components.iter().map(|c| c.to_string()).collect(),
            text: text.into(),
        }
    }

    #[test]
    fn boot_quiet_drops_only_boot_initiated_no_change_reports() {
        let empty = |g| {
            report(
                g,
                &[],
                &format!("[init:reload] ext generation {g}\n(no changes)"),
            )
        };
        let added = report(
            2,
            &["count-lines"],
            "[init:reload] ext generation 2\n+ count-lines 0.1.0 (kept)",
        );
        let not_built = report(
            3,
            &[],
            "[init:reload] ext generation 3\n· not built: draft (looked for x)",
        );
        let fake = Arc::new(Fake {
            reports: std::sync::Mutex::new(vec![
                empty(1),
                added.clone(),
                not_built.clone(),
                empty(4),
            ]),
        });
        let q = BootQuiet::new(fake.clone());
        q.begin_reload(ReloadKind::SessionStart);
        assert_eq!(
            q.commit_reload(Duration::ZERO),
            None,
            "Boot, nothing kept: no event"
        );
        assert_eq!(
            q.commit_reload(Duration::ZERO),
            Some(added),
            "a kept component is reported"
        );
        assert_eq!(
            q.commit_reload(Duration::ZERO),
            Some(not_built),
            "a not-built line is news"
        );
        q.begin_reload(ReloadKind::Request);
        assert_eq!(
            q.commit_reload(Duration::ZERO),
            Some(empty(4)),
            "initctl reload always reports"
        );
    }

    /// `--manifest` prints the majors the built-in host really accepts.
    #[test]
    fn manifest_majors_are_the_hosts() {
        assert_eq!(
            &super::super::manifest::EXT_API_MAJORS[..],
            crate::ext_host::SUPPORTED_MAJORS
        );
    }
}
