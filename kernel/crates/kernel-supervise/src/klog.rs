//! Kernel diagnostics. The Root sends the kernel's stderr to `ROOT_HOME/logs/kernel-<seq>.stderr`
//! (root/root.md §3.5.1), so one ICJ line per message on stderr is the kernel log.

/// Log one line `{"c":component,"msg":…,"ts":…}` on stderr.
pub fn log(component: &str, msg: &str) {
    let v = serde_json::json!({"c": component, "msg": msg, "ts": crate::clock::now_rfc3339()});
    eprintln!("{v}");
}
