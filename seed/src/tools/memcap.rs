//! MEM-RUNAWAY, the body's side (tools.md §3.5.8). The Root enforces: it kills a tool's process tree whose memory
//! footprint (resident + compressed) passes the charter's `sandbox.mem_cap_mib`, and every workload while free disk
//! is below `sandbox.disk_floor_gib`. It records each kill, before the signal, in a file this body may read and the
//! agent may not write (`INIT_MEM_EVENTS`, set by the Root at spawn, with the cap in `INIT_MEM_CAP_MIB`). This module
//! only reads that file: after a tool process died by a signal, to tell the model why; and before a bash call, to
//! refuse it while the Root reports the disk below the floor. Nothing here measures, limits or signals anything.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// How much of the events file's end is read (it is rotated by the Root at 256 KiB).
const TAIL: u64 = 64 << 10;
/// A record counts for a call stamped from this long before the call's spawn (clock steps, rounding).
const SLACK_MS: u64 = 2_000;
/// The Root repeats `disk_low` every 5 s while the disk stays low; older than this, it no longer counts.
const DISK_FRESH_MS: u64 = 15_000;

/// The Root's side of the watchdog as this body got it from its environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub events: Option<PathBuf>,
    pub cap_mib: Option<u64>,
}

impl Config {
    /// From `INIT_MEM_EVENTS` and `INIT_MEM_CAP_MIB` (both Root-set; absent outside a Root-spawned instance).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Config {
        Config {
            events: get("INIT_MEM_EVENTS")
                .filter(|s| s.starts_with('/'))
                .map(PathBuf::from),
            cap_mib: get("INIT_MEM_CAP_MIB").and_then(|s| s.parse().ok()),
        }
    }

    pub fn from_env() -> Config {
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    /// The records at the end of the events file, oldest first (unreadable lines and files are skipped).
    fn records(&self) -> Vec<Value> {
        let Some(p) = &self.events else {
            return Vec::new();
        };
        let Ok(mut f) = std::fs::File::open(p) else {
            return Vec::new();
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let from = len.saturating_sub(TAIL);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(from)).is_err() || f.take(TAIL).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.lines().collect();
        if from > 0 && !lines.is_empty() {
            lines.remove(0); // a cut line
        }
        lines
            .into_iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    /// The Root's kill record for the process group `pgid` (a tool call), stamped at or after `since_ms` (the call's
    /// spawn, wall clock) less a small slack; the newest one if several match.
    pub fn lookup(&self, pgid: i32, since_ms: u64) -> Option<Kill> {
        let floor = since_ms.saturating_sub(SLACK_MS);
        self.records().into_iter().rev().find_map(|r| {
            let ours = r["pgid"].as_i64() == Some(i64::from(pgid))
                && r["wall_ms"].as_u64().is_some_and(|t| t >= floor);
            if !ours {
                return None;
            }
            let n = |k: &str| r[k].as_u64().unwrap_or(0);
            match r["event"].as_str() {
                Some("mem_cap") => Some(Kill::MemCap {
                    cap_mib: r["cap_mib"].as_u64().or(self.cap_mib).unwrap_or(0),
                    footprint_mib: n("footprint_mib"),
                }),
                Some("disk_floor") => Some(Kill::DiskFloor {
                    free_mib: n("free_mib"),
                    floor_mib: n("floor_mib"),
                }),
                _ => None,
            }
        })
    }

    /// While the Root's newest disk record says `disk_low` and is at most 15 s old: (free MiB, floor MiB).
    pub fn disk_low(&self, now_ms: u64) -> Option<(u64, u64)> {
        let r = self
            .records()
            .into_iter()
            .rev()
            .find(|r| matches!(r["event"].as_str(), Some("disk_low") | Some("disk_ok")))?;
        let fresh = r["wall_ms"]
            .as_u64()
            .is_some_and(|t| now_ms.saturating_sub(t) <= DISK_FRESH_MS);
        (r["event"] == "disk_low" && fresh).then(|| {
            (
                r["free_mib"].as_u64().unwrap_or(0),
                r["floor_mib"].as_u64().unwrap_or(0),
            )
        })
    }
}

/// Why the Root killed a tool's processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kill {
    MemCap { cap_mib: u64, footprint_mib: u64 },
    DiskFloor { free_mib: u64, floor_mib: u64 },
}

impl Kill {
    /// The model-facing sentence (tools.md §3.5.8).
    pub fn text(&self) -> String {
        match self {
            Kill::MemCap {
                cap_mib,
                footprint_mib,
            } => format!(
                "process exceeded memory cap {cap_mib} MiB, killed (its process tree reached {footprint_mib} MiB)"
            ),
            Kill::DiskFloor {
                free_mib,
                floor_mib,
            } => format!(
                "free disk ({free_mib} MiB) fell below the workload floor ({floor_mib} MiB), process killed"
            ),
        }
    }

    /// The `tool_end` summary.
    pub fn summary(&self) -> &'static str {
        match self {
            Kill::MemCap { .. } => "memory cap",
            Kill::DiskFloor { .. } => "disk floor",
        }
    }
}

/// Now, in wall-clock ms (the Root stamps its records the same way).
pub fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testutil::{Bench, text};
    use crate::tools::{Signal, kill_group};
    use serde_json::json;
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// One line as the Root writes it (the shape `root/crates/init-root/tests/memwatch.rs` reads back from a live kill).
    fn root_line(event: &str, pgid: i32, wall_ms: u64) -> String {
        let free = if event == "mem_cap" {
            Value::Null
        } else {
            json!(3000)
        };
        json!({"cap_mib": 256, "event": event, "floor_mib": 5120, "footprint_mib": 801, "free_mib": free, "instance": "0e6d0cdfb7e2",
            "kind": "body", "pgid": pgid, "pids": [pgid], "scope": "unit", "v": 1, "wall_ms": wall_ms})
        .to_string()
    }

    fn append(p: &Path, line: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    /// A bench whose tools read `events` (as `INIT_MEM_EVENTS` would make them).
    fn bench(name: &str) -> (Bench, PathBuf) {
        let mut b = Bench::new(name);
        let ev = b.home.path.join("mem-events.jsonl");
        std::fs::write(&ev, "").unwrap();
        b.tools.mem = Config {
            events: Some(ev.clone()),
            cap_mib: Some(256),
        };
        (b, ev)
    }

    /// Plays the Root's part for one bash call: waits for the call's process group id in `pidf`, then (when `record`)
    /// appends the kill record first and SIGKILLs the group, the order the Root's watchdog keeps. The group is this
    /// test's own child's (the bench's bash call), signalled by its id through the body's own `kill_group`.
    fn root_kills(
        pidf: PathBuf,
        ev: PathBuf,
        record: Option<&'static str>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let t = Instant::now();
            let pg = loop {
                if let Some(pg) = std::fs::read_to_string(&pidf)
                    .ok()
                    .and_then(|s| s.trim().parse::<i32>().ok())
                {
                    break pg;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(10),
                    "the call never started"
                );
                std::thread::sleep(Duration::from_millis(20));
            };
            if let Some(event) = record {
                append(&ev, &root_line(event, pg, wall_ms()));
            }
            assert!(kill_group(pg, Signal::Kill));
        })
    }

    /// The tool error the model sees when the Root's watchdog kills a bash call over the cap.
    #[test]
    fn a_root_memory_kill_is_a_tool_error_naming_the_cap() {
        let (b, ev) = bench("memcap-kill");
        let pidf = b.work().join("pg");
        let root = root_kills(pidf.clone(), ev, Some("mem_cap"));
        let r = b.run("bash", json!({"command": format!("echo $$ > '{}'; echo started; sleep 30", pidf.display()), "timeout": 20}));
        root.join().unwrap();
        let t = text(&r);
        assert!(r.result.is_error, "{t}");
        assert!(t.contains("started\n"), "{t}");
        assert!(t.contains("[process exceeded memory cap 256 MiB, killed (its process tree reached 801 MiB) wall_time="), "{t}");
        assert_eq!(
            (
                r.summary.as_str(),
                r.result.meta.signal,
                r.result.meta.exit_code
            ),
            ("memory cap", Some(9), None)
        );
    }

    /// A disk-floor kill reads as such; a kill with no record stays a plain signal.
    #[test]
    fn a_disk_floor_kill_and_a_plain_kill() {
        let (b, ev) = bench("memcap-disk");
        let pidf = b.work().join("pg");
        let root = root_kills(pidf.clone(), ev.clone(), Some("disk_floor"));
        let r = b.run(
            "bash",
            json!({"command": format!("echo $$ > '{}'; sleep 30", pidf.display()), "timeout": 20}),
        );
        root.join().unwrap();
        assert!(text(&r).contains("[free disk (3000 MiB) fell below the workload floor (5120 MiB), process killed wall_time="), "{}", text(&r));
        assert_eq!(r.summary, "disk floor");
        std::fs::remove_file(&pidf).unwrap(); // exact path, written by the call above
        let root = root_kills(pidf.clone(), ev, None);
        let r = b.run(
            "bash",
            json!({"command": format!("echo $$ > '{}'; sleep 30", pidf.display()), "timeout": 20}),
        );
        root.join().unwrap();
        let t = text(&r);
        assert!(
            t.contains("[exit_code=none signal=9 wall_time=") && !t.contains("memory cap"),
            "{t}"
        );
    }

    /// Only a record for this call's process group, stamped since its spawn, counts; the newest wins.
    #[test]
    fn only_this_calls_fresh_record_counts() {
        let (b, ev) = bench("memcap-lookup");
        let now = wall_ms();
        append(&ev, &root_line("mem_cap", 4242, now - 60_000)); // an older call that had the same pgid
        append(&ev, &root_line("mem_cap", 4243, now)); // another call
        append(&ev, "not json");
        assert_eq!(b.tools.mem.lookup(4242, now), None);
        assert_eq!(
            b.tools.mem.lookup(4243, now),
            Some(Kill::MemCap {
                cap_mib: 256,
                footprint_mib: 801
            })
        );
        append(&ev, &root_line("disk_floor", 4243, now));
        assert_eq!(
            b.tools.mem.lookup(4243, now),
            Some(Kill::DiskFloor {
                free_mib: 3000,
                floor_mib: 5120
            })
        );
        assert_eq!(
            Config::default().lookup(4243, now),
            None,
            "no events file: nothing to say"
        );
        let c = Config::from_lookup(|k| match k {
            "INIT_MEM_EVENTS" => Some("relative/path".into()),
            "INIT_MEM_CAP_MIB" => Some("4096".into()),
            _ => None,
        });
        assert_eq!(
            c,
            Config {
                events: None,
                cap_mib: Some(4096)
            },
            "a relative events path is ignored"
        );
    }

    /// While the Root's newest disk record is a fresh `disk_low`, bash starts nothing; `disk_ok` or a stale record ends it.
    #[test]
    fn bash_is_refused_while_the_disk_is_below_the_floor() {
        let (b, ev) = bench("memcap-floor");
        let marker = b.work().join("ran");
        let cmd = json!({"command": format!("touch '{}'", marker.display())});
        let low = json!({"event": "disk_low", "free_mib": 3000, "floor_mib": 5120, "v": 1, "wall_ms": wall_ms()}).to_string();
        append(&ev, &low);
        let r = b.run("bash", cmd.clone());
        assert!(r.result.is_error && !marker.exists(), "{}", text(&r));
        assert!(
            text(&r).starts_with(
                "error: disk_floor: free disk (3000 MiB) is below the workload floor (5120 MiB)"
            ),
            "{}",
            text(&r)
        );
        append(
            &ev,
            &json!({"event": "disk_ok", "v": 1, "wall_ms": wall_ms()}).to_string(),
        );
        let r = b.run("bash", cmd.clone());
        assert!(!r.result.is_error && marker.exists(), "{}", text(&r));
        std::fs::remove_file(&marker).unwrap(); // exact path, created by the call above
        let stale = json!({"event": "disk_low", "free_mib": 1, "floor_mib": 5120, "v": 1, "wall_ms": wall_ms() - 60_000}).to_string();
        append(&ev, &stale);
        let r = b.run("bash", cmd);
        assert!(
            !r.result.is_error && marker.exists(),
            "a stale disk_low no longer refuses: {}",
            text(&r)
        );
    }
}
