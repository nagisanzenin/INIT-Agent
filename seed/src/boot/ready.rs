//! `state/run/ready` (body/boot.md §3.11, §4.2; base format kernel/supervise.md §4.4): written
//! atomically (temp `state/run/.ready.<pid>`, `sync_all`, `rename`) only after the `boot` event is
//! durable and, in probe mode, the probe passed. The heartbeat is the loop's (`loop::status`).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub fn run_dir(home: &Path) -> PathBuf {
    home.join("state").join("run")
}

pub struct ReadyIn<'a> {
    pub boot_event_id: &'a str,
    pub boot_id: &'a str,
    pub leaf: &'a str,
    pub mode: &'a str,
    pub prefix_hash: &'a str,
    /// `ProbeReport::to_value()` in probe mode; null otherwise.
    pub probe: Value,
    pub session: &'a str,
    pub sha: &'a str,
}

pub fn ready_value(r: &ReadyIn) -> Value {
    json!({
        "boot_event_id": r.boot_event_id,
        "boot_id": r.boot_id,
        "leaf": r.leaf,
        "mode": r.mode,
        "pid": std::process::id(),
        "prefix_hash": r.prefix_hash,
        "probe": r.probe,
        "session": r.session,
        "sha": r.sha,
        "ts": crate::log::clock::now_rfc3339_ms(),
        "v": 1,
    })
}

/// Temp file, `sync_all`, rename (a reader never sees partial JSON).
pub fn write_ready(home: &Path, v: &Value) -> std::io::Result<()> {
    let dir = run_dir(home);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".ready.{}", std::process::id()));
    let mut line = crate::log::icj::to_string(v).map_err(std::io::Error::from)?;
    line.push('\n');
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(line.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join("ready"))
}
