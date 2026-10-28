//! R1: panic and resume (root/root.md §3.9): the owner-socket and latch-file channels, the watch
//! paths, the latch, the records (panic demotes one level, CH-2), resume, the CLI fallback latch.
//! `egress::panic` and `egress::resume` (WP22). WP21: the signal channel (SIGUSR1), the kill of every
//! instance the Root holds, the panic and its demotion under one lineage lock (so a resume can never land
//! between them), the rollback to audited-good and the `panic_resume` start. WP23 adds the mirror-ref
//! channel, WP24 `sealed::cancel_all`. Live-system safety: panic signals only held pids and process groups
//! (ADR-008); in the held-only build the result says so (`sweep`), and zone.rs reports any escapee.

use crate::lineage::Lineage;
use crate::link::{Refusal, Root, lock};
use crate::sup::{Ev, INTEGRITY, Plan, Slot};
use crate::util::{self, icj, now_ms, rfc3339};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// ROOT-MSG (v0.5.1): how the owner restarts after a panic, runnable as printed (`init-root` is not on PATH).
pub fn resume_hint(p: &crate::install::Paths) -> String {
    format!("To restart: type /resume in init-agent, or run {}.", p.cmd("resume"))
}

pub fn latch(root_home: &Path) -> PathBuf {
    root_home.join("run/panic")
}

/// Creates the latch `{"at","channel","v":1}` (fsynced) unless one exists; a hand-made latch is kept.
pub fn write_latch(root_home: &Path, at: i64, channel: &str) -> std::io::Result<()> {
    let l = latch(root_home);
    if fs::symlink_metadata(&l).is_ok() {
        return Ok(());
    }
    let body = icj(&json!({"at": rfc3339(at), "channel": channel, "v": 1}))? + "\n";
    util::write_atomic(&l, body.as_bytes(), 0o600)
}

/// §3.9.2. A second panic while panicked reports the first and does nothing else. The lineage lock is
/// taken first and held until the demotion is recorded, so `resume` (which takes it first too) waits.
pub fn panic(root: &Arc<Root>, channel: &str) -> Value {
    let at = now_ms();
    let mut lg = lock(&root.lin);
    {
        let mut st = lock(&root.st);
        if let Slot::Panicked { at, channel } = &st.slot {
            return json!({"already": true, "at": rfc3339(*at), "channel": channel});
        }
        st.slot = Slot::Panicked { at, channel: channel.into() }; // 3. the broker refuses every spawn from here
    }
    if let Err(e) = write_latch(&root.p.rh, at, channel) {
        util::log(&root.p.rh, "error", &format!("panic latch: {e}"));
    }
    if let Some(e) = root.egress.get() {
        e.panic(); // 2. refuse new requests, drop in-flight ones
    }
    crate::sealed::cancel_all(&root.p.rh); // 3. the sealed instances, by pid; their run roots by exact path
    // 4. every instance the Root holds, by pid and process group
    *lock(&root.sup.k) = None; // the supervisor ignores this kernel's exit
    let (instances, processes) = root.zone.kill_all();
    let (from, to) = record(root, &mut lg, channel, instances as u64, processes as u64);
    drop(lg);
    crate::sentinel::panicked(root); // X3: H3, open groups go as collateral
    root.sync_egress();
    // ADR-008 (review F5): say what this build's kill reached; escapees are reported (zone.rs), never killed
    let sweep = if cfg!(feature = "live-sweep") { "d1" } else { "held_only" };
    let note = if sweep == "held_only" { " (held-only build: a process that left its group is reported, not killed)" } else { "" };
    let msg = format!("PANIC at {} via {channel}: killed {processes} processes in {instances} instances; egress closed; \
        level {from}→{to}. {}{note}", rfc3339(at), resume_hint(&root.p));
    root.alert("panic", &msg);
    root.publish();
    json!({"at": rfc3339(at), "channel": channel, "killed": {"instances": instances, "processes": processes},
        "level": {"from": from, "to": to}, "message": msg, "sweep": sweep})
}

/// Step 5: `panic{channel, killed}`, then `ladder down` by one level (L0 stays 0→0), under the caller's
/// lineage lock. Returns (from, to).
fn record(root: &Root, lg: &mut Option<Lineage>, channel: &str, instances: u64, processes: u64) -> (i64, i64) {
    let from = lock(&root.st).level.level;
    let data = json!({"channel": channel, "killed": {"instances": instances, "processes": processes}});
    let Ok(p) = root.append_locked(lg, "root", "panic", data) else {
        return (from, from);
    };
    let to = (from - 1).max(0);
    let down = json!({"cause": "demotion", "direction": crate::ledger::down_dir(from), "from": from, "to": to, "trigger": {"kind": "panic", "seq": p["seq"]}});
    let _ = root.append_locked(lg, "root", "ladder", down);
    (from, to)
}

/// §3.6.5 step 6: a latch at start means `Panicked`; a panic not yet in the lineage (a hand-made
/// latch, the CLI fallback, a crash after the latch) is recorded now, exactly once.
pub fn cold_start(root: &Arc<Root>, records: &[Value]) {
    let l = latch(&root.p.rh);
    if fs::symlink_metadata(&l).is_err() {
        return;
    }
    let body: Value = fs::read_to_string(&l).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let at = body["at"].as_str().and_then(util::parse_rfc3339).unwrap_or_else(now_ms);
    let channel = body["channel"].as_str().unwrap_or("latch_file").to_string();
    lock(&root.st).slot = Slot::Panicked { at, channel: channel.clone() };
    let last = records.iter().rev().find_map(|r| r["kind"].as_str().filter(|k| *k == "panic" || *k == "resume"));
    if last != Some("panic") {
        record(root, &mut lock(&root.lin), &channel, 0, 0);
        root.alert("panic", &format!("PANIC (latched at {}, via {channel}) found at Root start. {}", rfc3339(at), resume_hint(&root.p)));
    }
}

/// §3.9.5 (and `/resume` while Stopped or Halted, §3.6.6). Refused while a watch path exists, or while the
/// mirror's `refs/heads/panic` does (the mirror-ref channel on; L5). After a
/// panic: the latch goes, the zone rolls back to audited-good (the `rollback` record), `resume` is recorded,
/// and the supervisor starts the kernel with reason `panic_resume` in the mode of the panic.
pub fn resume(root: &Arc<Root>) -> Result<Value, Refusal> {
    let watch = lock(&root.st).charter.as_ref().map(|l| l.ch.panic.watch_paths.clone()).unwrap_or_default();
    if let Some(w) = watch.iter().find(|w| fs::symlink_metadata(w).is_ok()) {
        return Err(("root.mode", format!("resume refused: the panic watch path {w} still exists; remove it first")));
    }
    // §3.9.5 step 1, the mirror ref (build-deviations §5 L5): as the mirror thread's last poll saw it
    let mirror = lock(&root.st).charter.as_ref().and_then(|l| crate::lineage::mirror::Cfg::of(&root.p, &l.ch));
    if let Some(id) = crate::lineage::mirror::panic_ref_seen(mirror.as_ref()) {
        return Err(("root.mode", format!("resume refused: the mirror's refs/heads/panic still exists (commit {}); delete that \
            branch first; the Root looks again within a second", id.get(..12).unwrap_or(&id))));
    }
    let mut lg = lock(&root.lin);
    let (mode, level, at, channel) = {
        let mut st = lock(&root.st);
        let (at, channel) = match &st.slot {
            Slot::Panicked { at, channel } => (*at, channel.clone()),
            Slot::Halted { reason, .. } if INTEGRITY.contains(reason) => {
                return Err(("root.mode", format!("Halted{{{reason}}} is an integrity failure, which /resume cannot clear: fix the cause the \
                    Root's log names (logs/root.jsonl in ROOT_HOME), then restart the Root: {}, then {}", crate::install::stop_hint(&root.p),
                    crate::install::start_hint(&root.p))));
            }
            s @ (Slot::Halted { .. } | Slot::Stopped | Slot::Backoff { .. }) => {
                let name = s.name();
                drop(st);
                drop(lg);
                root.sup.send(Ev::Start(Plan::plain("cold")));
                return Ok(json!({"slot": name}));
            }
            s => return Ok(json!({"slot": s.name()})),
        };
        // the latch goes before the slot changes, under the lock, so the watcher never re-panics on it
        let gone = fs::remove_file(latch(&root.p.rh)).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) });
        if let Err(e) = gone {
            return Err(("root.internal", format!("could not remove the latch: {e}")));
        }
        st.slot = Slot::Stopped;
        (st.mode, st.level.level, at, channel)
    };
    if let Some(e) = root.egress.get() {
        e.resume();
    }
    if let Ok((_, d)) = crate::tuple::rollback(root, crate::ledger::anchor(root), "panic") { // ADR-011 §4.1: `settled` on the free path
        let _ = root.append_locked(&mut lg, "root", "rollback", d);
    }
    let tuple = lock(&root.sup.ptrs).current.clone();
    let _ = root.append_locked(&mut lg, "root", "resume", json!({"level": level, "mode": mode, "tuple": tuple}));
    drop(lg);
    let panic = json!({"at": rfc3339(at), "channel": channel});
    root.sup.send(Ev::Start(Plan { target: None, reason: "panic_resume", cause: Value::Null, panic, mode: None }));
    root.alert("resume", &format!("[root] resumed after panic at level L{level}; the zone restarts on the audited-good tuple"));
    root.publish();
    Ok(json!({"level": level, "mode": mode, "slot": "starting", "tuple": tuple}))
}

/// The signal channel (§3.9.1) and orderly shutdown: SIGUSR1 panics; SIGTERM and SIGINT stop the zone and exit.
pub fn signals(root: Arc<Root>) {
    let r = crate::sys::signal_pipe(&[libc::SIGUSR1, libc::SIGTERM, libc::SIGINT]);
    let Ok(mut r) = r.map_err(|e| util::log(&root.p.rh, "error", &format!("signal handlers: {e}"))) else { return };
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        while r.read_exact(&mut b).is_ok() {
            if i32::from(b[0]) == libc::SIGUSR1 {
                panic(&root, "signal");
            } else {
                root.sup.send(Ev::Shutdown);
            }
        }
    });
}

/// The watcher's panic channels (§3.9.1): the latch file and the owner's watch paths, every second.
pub fn watch(root: Arc<Root>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let (panicked, watch) = {
            let st = lock(&root.st);
            (matches!(st.slot, Slot::Panicked { .. }), st.charter.as_ref().map(|l| l.ch.panic.watch_paths.clone()).unwrap_or_default())
        };
        if panicked {
            continue;
        }
        if fs::symlink_metadata(latch(&root.p.rh)).is_ok() {
            panic(&root, "latch_file");
        } else if watch.iter().any(|w| fs::symlink_metadata(w).is_ok()) {
            panic(&root, "watch_path");
        }
    }
}
