//! File helpers: atomic ICJ writes (supervise.md K-F4), JSONL appends, the kernel log.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` atomically: a new temp file in the same directory (`O_NOFOLLOW`,
/// `create_new`), `fsync`, `rename`, `fsync` of the directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.tmp.{}.{n}", name.to_string_lossy(), std::process::id()));
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o644).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp); // exact path: our own temp file
    }
    res
}

/// Serialize `x` as one ICJ line and write it atomically.
pub fn write_json<T: serde::Serialize>(path: &Path, x: &T) -> io::Result<()> {
    let bytes = init_proto::icj::to_line(x).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_atomic(path, &bytes)
}

/// Append one line (`O_APPEND`, created 0644, `O_NOFOLLOW`).
pub fn append_line(path: &Path, line: &[u8]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = OpenOptions::new().append(true).create(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let mut buf = line.to_vec();
    if !buf.ends_with(b"\n") {
        buf.push(b'\n');
    }
    f.write_all(&buf)
}

/// `state/kernel/logs/kernel.jsonl` (kernel/initctl.md §3.3 step 4).
pub const KERNEL_LOG: &str = "state/kernel/logs/kernel.jsonl";

/// Log one ICJ line `{"c":"serve","ev":…,"ts":…, <fields>}` to `<home>/state/kernel/logs/kernel.jsonl`.
/// Logging never fails the caller.
pub fn klog(home: &Path, ev: &str, fields: serde_json::Value) {
    let mut m = match fields {
        serde_json::Value::Object(m) => m,
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            let mut m = serde_json::Map::new();
            m.insert("data".into(), other);
            m
        }
    };
    m.insert("c".into(), "serve".into());
    m.insert("ev".into(), ev.into());
    m.insert("ts".into(), crate::clock::now_rfc3339().into());
    if let Ok(line) = init_proto::icj::to_line(&serde_json::Value::Object(m)) {
        let _ = append_line(&home.join(KERNEL_LOG), &line);
    }
}
