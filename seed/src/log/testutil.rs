//! Test helpers shared by the log and context tests. Every test home lives under
//! `<target>/test-homes/wp12/` (`$CARGO_TARGET_DIR` when set, as in G0; else `seed/target`) and is
//! removed by its exact path when the test ends.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::event::{BootEv, LogRepair};
use super::session::{OpenParams, OpenReport, SessionLog};

pub const SHA: &str = "3f2a1c0e3f2a1c0e3f2a1c0e3f2a1c0e3f2a1c0e";
pub const SID: &str = "20261019-203015-9f3a2c";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Under the build's target dir: `$CARGO_TARGET_DIR` when set (the gate's G0 sets it to the build
/// slot's `out/target`, the only writable tree there; the source export is read-only), else
/// `<crate>/target`.
pub fn homes_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("test-homes")
        .join("wp12")
}

/// A temporary `INIT_HOME`, deleted (exact path) on drop unless `INIT_WP12_KEEP=1`.
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
        std::fs::create_dir_all(path.join("state").join("sessions")).unwrap();
        TempHome { path }
    }

    pub fn session_file(&self, sid: &str) -> PathBuf {
        super::session::session_file(&self.path, sid)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        if std::env::var("INIT_WP12_KEEP").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// xorshift64*: deterministic test randomness without a crate.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn hex16(&mut self) -> String {
        format!("{:016x}", self.next() | 1)
    }
    pub fn text(&mut self, len: usize) -> String {
        const W: [&str; 12] = [
            "alpha", "beta", "gamma", "delta", "état", "→", "line\n", "tab\t", "\"q\"", "/path",
            "x", "🙂",
        ];
        let mut s = String::new();
        while s.len() < len {
            s.push_str(W[self.below(W.len() as u64) as usize]);
            s.push(' ');
        }
        s
    }
}

pub fn open(home: &Path, sid: &str) -> (SessionLog, OpenReport) {
    SessionLog::open(&OpenParams {
        home,
        env_session: sid,
        sha: SHA,
        lock_wait: Duration::from_secs(2),
    })
    .unwrap()
}

/// A schema-valid boot event (WP13 builds the real one from boot.json and awareness.md).
pub fn boot_ev(text: &str) -> BootEv {
    BootEv {
        boot_id: "00000000000000b1".into(),
        reason: "cold".into(),
        mode: "normal".into(),
        sha: SHA.into(),
        from_sha: None,
        model: "mock-frontier-1".into(),
        provider: "mock".into(),
        effort: "high".into(),
        context_window: 131_072,
        effective_window: 131_072,
        body_tokens: 1000,
        prefix_hash: format!("sha256:{}", "0".repeat(64)),
        env: json!({}),
        facts: json!({}),
        log_repair: LogRepair::default(),
        text: text.into(),
        extra: Map::new(),
    }
}

pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}
