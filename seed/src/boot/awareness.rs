//! The awareness texts the body renders (docs/awareness.md v0.3): the environment snapshot
//! (§3.5, §4.2), the boot block (§3.6, §3.7.1 capability line) and the self-signal line (§3.8).
//! Rendered once, stored in the event's `text` (log-context P2), so every text is a pure function
//! of its inputs (the date comes from `env.probed.date_utc`). Integer formatting only: money and
//! thousands separators come from integer division; no float exists anywhere (ICJ).
//!
//! Missing inputs never panic (§5): a value the text needs but `boot.json` lacks renders `?`, and
//! an optional line whose source is missing is left out. `boot_json_missing` lists the gaps for
//! the boot's `error{code:"boot_json_field"}`.

#[cfg_attr(not(test), allow(dead_code))]
mod lint;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::context::SignalSource;
use crate::context::model::Thresholds;
use crate::log::clock::{civil_from_days, utc_parts};
use crate::log::event::SignalEv;
use crate::provider::http::{ChunkedReader, Framing, body_framing, read_head};

/// The fixed CLI candidate list (§3.5.1), sorted. The agent extends it through T2.
pub const CLI_CANDIDATES: [&str; 25] = [
    "awk",
    "bash",
    "cargo",
    "cc",
    "curl",
    "find",
    "gh",
    "git",
    "grep",
    "jq",
    "make",
    "node",
    "npm",
    "patch",
    "perl",
    "python3",
    "rg",
    "rustc",
    "rustup",
    "sed",
    "sqlite3",
    "tar",
    "uv",
    "wasm-tools",
    "wasmtime",
];

/// A `disk_free_mb` move smaller than this does not count as an environment change: free space
/// moves a little at every boot, and the full environment text would otherwise never be skipped.
pub const DISK_CHANGE_MB: u64 = 1024;

// ---------------------------------------------------------------------------------------------
// Small helpers: JSON access, integer formatting, UTC minutes
// ---------------------------------------------------------------------------------------------

/// The value at `path`, or None when absent or null.
fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    (!cur.is_null()).then_some(cur)
}

fn get_u(v: &Value, path: &[&str]) -> Option<u64> {
    get(v, path)?.as_u64()
}

fn get_s<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    get(v, path)?.as_str()
}

/// A string or integer as text (ids, versions, triggers); None for anything else.
fn scalar(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}

fn q(x: Option<impl ToString>) -> String {
    x.map_or_else(|| "?".to_string(), |v| v.to_string())
}

/// `1234567` → `1,234,567` (integer division only).
pub fn thousands(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    format!("{},{:03}", thousands(n / 1000), n % 1000)
}

/// Micro-USD as dollars, `{musd/1_000_000}.{(musd%1_000_000)/10_000:02}` (§3.5.2): `1230000` →
/// `$1.23`.
pub fn dollars(musd: u64) -> String {
    format!("${}.{:02}", musd / 1_000_000, (musd % 1_000_000) / 10_000)
}

fn q_dollars(x: Option<u64>) -> String {
    x.map_or_else(|| "?".to_string(), dollars)
}

fn signed(d: i128) -> String {
    let sign = if d < 0 { '-' } else { '+' };
    format!("{sign}{}", thousands(d.unsigned_abs() as u64))
}

/// The first `n` characters (a sha's `<x7>`, a hash's 12).
fn short(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn sha7(s: &str) -> &str {
    short(s, 7)
}

/// Howard Hinnant's `days_from_civil` (the inverse of `civil_from_days`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(s: &[u8], i: usize, n: usize) -> Option<u32> {
    let b = s.get(i..i + n)?;
    b.iter()
        .all(u8::is_ascii_digit)
        .then(|| b.iter().fold(0u32, |a, &d| a * 10 + (d - b'0') as u32))
}

/// A UTC RFC 3339 time (`2026-10-19T20:30Z`, `…T20:30:15Z`, `…T20:30:15.123Z`) as minutes since
/// the epoch.
fn parse_utc_minute(t: &str) -> Option<i64> {
    let s = t.as_bytes();
    let (y, mo, d) = (num(s, 0, 4)?, num(s, 5, 2)?, num(s, 8, 2)?);
    let (h, mi) = (num(s, 11, 2)?, num(s, 14, 2)?);
    let seps = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':')];
    if seps.iter().any(|&(i, c)| s.get(i) != Some(&c))
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || h > 23
        || mi > 59
        || s.last() != Some(&b'Z')
        || !(s.len() == 17 || s.get(16) == Some(&b':'))
    {
        return None;
    }
    Some(days_from_civil(y as i64, mo, d) * 1440 + (h * 60 + mi) as i64)
}

/// Minutes since the epoch → `2026-10-19 20:30`.
fn minute_text(m: i64) -> String {
    let (y, mo, d) = civil_from_days(m.div_euclid(1440));
    let mod_ = m.rem_euclid(1440);
    format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}", mod_ / 60, mod_ % 60)
}

fn utc_text(t: &str) -> Option<String> {
    Some(format!("{} UTC", minute_text(parse_utc_minute(t)?)))
}

/// `+07:00` → 420.
fn parse_offset(o: &str) -> Option<i64> {
    let s = o.as_bytes();
    let sign = match s.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = (num(s, 1, 2)?, num(s, 4, 2)?);
    (s.len() == 6 && s[3] == b':' && h <= 14 && m <= 59).then_some(sign * (h * 60 + m) as i64)
}

fn offset_text(min: i64) -> String {
    let sign = if min < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", min.abs() / 60, min.abs() % 60)
}

// ---------------------------------------------------------------------------------------------
// Environment snapshot: the probed half (§3.5.1)
// ---------------------------------------------------------------------------------------------

/// Inputs of the probed half of the environment snapshot (§3.5.1).
pub struct ProbeIn<'a> {
    /// `$INIT_HOME`.
    pub home: &'a Path,
    /// The body cwd (boot facts `cwd`).
    pub cwd: &'a Path,
    /// Boot facts `bash`: `{"path","version"}`, or null when none was found.
    pub bash: &'a Value,
    /// `PATH` as the body sees it (CLI lookup).
    pub path_var: Option<&'a str>,
    /// log-context §3.12.1 `W_eff`.
    pub effective_window: u64,
    /// The effective effort (`boot.json.model.effort` or the model default).
    pub effort: &'a str,
    /// `state/kernel/boot.json` verbatim (the owner's day boundary lives under `env`).
    pub boot: &'a Value,
    pub now: SystemTime,
    /// Per-process probe timeout (2 s in production).
    pub timeout: Duration,
}

/// The first `PATH` entry holding a regular file `name` with an execute bit (no process spawned).
fn which(name: &str, path_var: Option<&str>) -> Option<PathBuf> {
    path_var?
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// `ETXTBSY`: the executable is open for writing somewhere, typically a just-written file whose
/// write descriptor a concurrent fork in this multi-threaded process still holds for an instant.
const ETXTBSY: i32 = 26;

/// Time a successful probe's output may take to arrive after the process exited (the reader
/// thread may not have run yet on a loaded machine; the pipe is already at EOF).
const PROBE_OUTPUT_GRACE: Duration = Duration::from_millis(500);

/// Runs one probe process (our own child) and returns its stdout, or None on a spawn failure, a
/// non-zero exit or the timeout (then the child is killed by its handle and reaped). A transient
/// `ETXTBSY` at spawn is retried until the deadline.
fn run_probe(
    prog: &Path,
    args: &[&OsStr],
    path_var: Option<&str>,
    timeout: Duration,
) -> Option<String> {
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = path_var {
        cmd.env("PATH", p);
    }
    let deadline = Instant::now() + timeout;
    let mut child = loop {
        match cmd.spawn() {
            Ok(c) => break c,
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    };
    let Some(out) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    // A reader thread, so a grandchild holding the pipe open cannot block the probe past its
    // timeout.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.take(64 * 1024).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    let buf = rx.recv_timeout(left.max(PROBE_OUTPUT_GRACE)).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn first_line(s: String) -> Option<String> {
    let l = s.lines().next()?.trim();
    (!l.is_empty()).then(|| l.to_string())
}

/// `df -Pk <home>`: the 4th field of line 2, in MiB.
fn disk_free_mb(home: &Path, path_var: Option<&str>, timeout: Duration) -> Option<u64> {
    let df = which("df", path_var).or_else(|| {
        ["/bin/df", "/usr/bin/df"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })?;
    let out = run_probe(
        &df,
        &[OsStr::new("-Pk"), home.as_os_str()],
        path_var,
        timeout,
    )?;
    let kib: u64 = out
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kib / 1024)
}

/// `2026-10-19T20:30Z` (to the minute).
fn date_utc(now: SystemTime) -> Value {
    match now.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let (y, mo, da, h, mi, _, _) = utc_parts(d.as_millis() as u64);
            Value::String(format!("{y:04}-{mo:02}-{da:02}T{h:02}:{mi:02}Z"))
        }
        Err(_) => Value::Null,
    }
}

/// The owner's UTC offset. The Root's `ceilings.day_starts_at_utc_offset` when present; else
/// derived from the money day's start, which is the owner's local midnight in UTC
/// (`boot.json.env.budget.day_starts_at`, e.g. `…T17:00:00Z` → `+07:00`). Offsets are taken in
/// `[-11:00, +13:00]`, so `…T10:00:00Z` reads as Hawaii (`-10:00`), not `+14:00`.
fn owner_offset(boot: &Value) -> Value {
    if let Some(o) = get_s(
        boot,
        &["env", "root", "ceilings", "day_starts_at_utc_offset"],
    ) && let Some(m) = parse_offset(o)
    {
        return Value::String(offset_text(m));
    }
    let Some(start) = get_s(boot, &["env", "budget", "day_starts_at"]).and_then(parse_utc_minute)
    else {
        return Value::Null;
    };
    let mut off = (1440 - start.rem_euclid(1440)) % 1440;
    if off > 13 * 60 {
        off -= 1440;
    }
    Value::String(offset_text(off))
}

/// The `probed` object of §4.2 (sorted keys, integers only; `?`-able fields are null when unknown).
pub fn probe_env(p: &ProbeIn) -> Value {
    let (clis, missing): (Vec<&str>, Vec<&str>) = CLI_CANDIDATES
        .iter()
        .partition(|n| which(n, p.path_var).is_some());
    let (rustc, cargo, git) = (
        which("rustc", p.path_var),
        which("cargo", p.path_var),
        which("git", p.path_var),
    );
    let run1 = |prog: &Option<PathBuf>, args: &[&str]| -> Option<String> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        run_probe(prog.as_ref()?, &args, p.path_var, p.timeout).and_then(first_line)
    };
    // Four short processes in parallel (`rustc -V` and `rustc --print sysroot` share a thread).
    let (rustc_v, sysroot, cargo_v, git_v, disk) = std::thread::scope(|s| {
        let r = s.spawn(|| {
            let v = run1(&rustc, &["-V"]);
            let sys = v
                .as_ref()
                .and_then(|_| run1(&rustc, &["--print", "sysroot"]));
            (v, sys)
        });
        let c = s.spawn(|| run1(&cargo, &["-V"]));
        let g = s.spawn(|| run1(&git, &["--version"]));
        let d = s.spawn(|| disk_free_mb(p.home, p.path_var, p.timeout));
        let (rv, sys) = r.join().unwrap_or((None, None));
        (
            rv,
            sys,
            c.join().ok().flatten(),
            g.join().ok().flatten(),
            d.join().ok().flatten(),
        )
    });
    let wasm = sysroot.map(|s| {
        Path::new(&s)
            .join("lib")
            .join("rustlib")
            .join("wasm32-wasip2")
            .is_dir()
    });
    let cpus = std::thread::available_parallelism()
        .ok()
        .map(|n| n.get() as u64);
    json!({
        "bash": p.bash.clone(),
        "clis": clis,
        "clis_missing": missing,
        "cpus": cpus,
        "cwd": p.cwd.to_string_lossy(),
        "date_utc": date_utc(p.now),
        "disk_free_mb": disk,
        "effective_window": p.effective_window,
        "effort": p.effort,
        "owner_offset": owner_offset(p.boot),
        "versions": {"cargo": cargo_v, "git": git_v, "rustc": rustc_v},
        "wasm32_wasip2": wasm,
    })
}

/// `{"kernel": boot.json.env verbatim (null if absent), "probed": probed, "v": 1}` (§4.2).
pub fn snapshot(boot: &Value, probed: Value) -> Value {
    json!({"kernel": boot.get("env").cloned().unwrap_or(Value::Null), "probed": probed, "v": 1})
}

/// Snapshot fields that changed since the previous boot's `env` (log-context `boot.env_changed`),
/// excluding `date_utc` and `budget`; sorted. `prev = None` (first boot of the session): empty.
/// Names are the top-level keys of `kernel` and `probed` (`os`, `clis`, `root`, …). A
/// `disk_free_mb` move below `DISK_CHANGE_MB` does not count.
pub fn env_changed(env: &Value, prev_env: Option<&Value>) -> Vec<String> {
    let Some(prev) = prev_env else {
        return Vec::new();
    };
    let empty = Map::new();
    let mut out = BTreeSet::new();
    for part in ["kernel", "probed"] {
        let a = env.get(part).and_then(Value::as_object).unwrap_or(&empty);
        let b = prev.get(part).and_then(Value::as_object).unwrap_or(&empty);
        for k in a.keys().chain(b.keys()) {
            if k == "date_utc" || k == "budget" {
                continue;
            }
            let (x, y) = (a.get(k), b.get(k));
            let changed = match (
                k.as_str(),
                x.and_then(Value::as_u64),
                y.and_then(Value::as_u64),
            ) {
                ("disk_free_mb", Some(x), Some(y)) => x.abs_diff(y) >= DISK_CHANGE_MB,
                _ => x != y,
            };
            if changed {
                out.insert(k.clone());
            }
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------------------------
// Environment text (§3.5.2)
// ---------------------------------------------------------------------------------------------

/// Fields the environment text does not show; a change in them alone keeps the short form.
const NOT_IN_ENV_TEXT: [&str; 1] = ["root"];

fn list(v: Option<&Value>) -> String {
    match v.and_then(Value::as_array) {
        None => "?".into(),
        Some(a) if a.is_empty() => "none".into(),
        Some(a) => a
            .iter()
            .map(|x| q(scalar(Some(x))))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// `GNU bash, version 5.3.20(1)-release (…)` → `GNU 5.3.20`.
fn bash_text(bash: Option<&Value>) -> String {
    let Some(b) = bash.filter(|b| b.is_object()) else {
        return "none".into();
    };
    let path = q(get_s(b, &["path"]));
    let ver = match get_s(b, &["version"]) {
        None => "?".to_string(),
        Some(v) => {
            let n = v
                .split_once("version ")
                .map_or(v, |(_, r)| r)
                .split(|c: char| c == '(' || c.is_whitespace())
                .next()
                .unwrap_or(v);
            if v.starts_with("GNU bash") {
                format!("GNU {n}")
            } else {
                n.to_string()
            }
        }
    };
    format!("{ver} ({path})")
}

/// `rustc 1.97.1 (8bab26f4f 2026-07-14)` → `1.97.1` (the first word that starts with a digit).
fn version_number(v: Option<&str>) -> String {
    match v {
        None => "?".into(),
        Some(v) => v
            .split_whitespace()
            .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
            .unwrap_or(v)
            .to_string(),
    }
}

fn date_part(p: &Value) -> Option<(i64, Option<i64>)> {
    let m = get_s(p, &["date_utc"]).and_then(parse_utc_minute)?;
    Some((m, get_s(p, &["owner_offset"]).and_then(parse_offset)))
}

/// The full form (§3.5.2).
fn env_full(env: &Value) -> String {
    let k = env.get("kernel").unwrap_or(&Value::Null);
    let p = env.get("probed").unwrap_or(&Value::Null);
    let ks = |path: &[&str]| q(get_s(k, path));
    let date = match date_part(p) {
        None => "?".to_string(),
        Some((m, None)) => format!("{} UTC", minute_text(m)),
        Some((m, Some(off))) => format!(
            "{} UTC (owner's time: {}, {})",
            minute_text(m),
            minute_text(m + off),
            offset_text(off)
        ),
    };
    let network = match get_s(k, &["network", "mode"]) {
        None => "?".to_string(),
        Some("proxy-only") => {
            "through the Root's egress only (model providers; dependencies via `initctl vendor`)"
                .to_string()
        }
        Some(m) => m.to_string(),
    };
    let day_starts = get_s(k, &["budget", "day_starts_at"])
        .and_then(parse_utc_minute)
        .map(|m| short(&minute_text(m)[11..], 5).to_string());
    let wasm = match get(p, &["wasm32_wasip2"]).and_then(Value::as_bool) {
        None => "?",
        Some(true) => "installed",
        Some(false) => "not installed",
    };
    [
        "Environment (\"probed\" = measured inside your sandbox at boot; the rest comes from the kernel):"
            .to_string(),
        format!("- date: {date}"),
        format!(
            "- os: {} {} {} · {} MB · {} cpus (probed)",
            ks(&["os", "family"]),
            ks(&["os", "version"]),
            ks(&["os", "arch"]),
            q(get_u(k, &["mem_total_mb"])),
            q(get_u(p, &["cpus"]))
        ),
        format!(
            "- home: {} (INIT_HOME) · cwd: {} · bash: {}",
            ks(&["init_home"]),
            q(get_s(p, &["cwd"])),
            bash_text(get(p, &["bash"]))
        ),
        format!(
            "- sandbox: {} · write: {} · read-only: {} · hidden: {}",
            ks(&["sandbox", "kind"]),
            list(get(k, &["sandbox", "write"])),
            list(get(k, &["sandbox", "read_only"])),
            list(get(k, &["sandbox", "hidden"]))
        ),
        format!("- network: {network}"),
        format!(
            "- model: {}/{} (effort {}) · window {} · compaction plans for {} · max output {}",
            ks(&["model", "provider"]),
            ks(&["model", "id"]),
            q(get_s(p, &["effort"])),
            q(get_u(k, &["model", "context_window"]).map(thousands)),
            q(get_u(p, &["effective_window"]).map(thousands)),
            q(get_u(k, &["model", "max_output"]).map(thousands))
        ),
        format!(
            "- budget: {} of {} spent today (day starts {} UTC; your soft day budget, clamped under the Root's {} ceiling)",
            q_dollars(get_u(k, &["budget", "today_musd"])),
            q_dollars(get_u(k, &["budget", "cap_musd"])),
            q(day_starts),
            q_dollars(get_u(k, &["root", "ceilings", "day_musd"]))
        ),
        format!("- kernel: {}", q(scalar(get(k, &["kernel_version"])))),
        format!("- on PATH (probed): {}", list(get(p, &["clis"]))),
        format!("- not found (probed): {}", list(get(p, &["clis_missing"]))),
        format!(
            "- versions (probed): rustc {} · cargo {} · git {} · wasm32-wasip2 target: {wasm}",
            version_number(get_s(p, &["versions", "rustc"])),
            version_number(get_s(p, &["versions", "cargo"])),
            version_number(get_s(p, &["versions", "git"]))
        ),
        format!(
            "- disk free (probed): {} MB",
            q(get_u(p, &["disk_free_mb"]).map(thousands))
        ),
    ]
    .join("\n")
}

/// The short form (§3.5.2), naming the previous boot event.
fn env_short(env: &Value, prev_id: &str) -> String {
    let k = env.get("kernel").unwrap_or(&Value::Null);
    let p = env.get("probed").unwrap_or(&Value::Null);
    let date = match date_part(p) {
        None => "?".to_string(),
        Some((m, None)) => format!("{} UTC", minute_text(m)),
        Some((m, Some(off))) => format!(
            "{} UTC (owner {} {})",
            minute_text(m),
            &minute_text(m + off)[11..],
            offset_text(off)
        ),
    };
    format!(
        "Environment: unchanged since boot {prev_id}, except date {date} · budget {} of {}.",
        q_dollars(get_u(k, &["budget", "today_musd"])),
        q_dollars(get_u(k, &["budget", "cap_musd"]))
    )
}

/// Full on a cold boot, on the first boot of a session, or when a field the text shows changed
/// since the previous boot on the path; else short.
fn env_text(b: &BlockIn) -> String {
    let prev = match b.prev_boot {
        Some((id, p)) if b.reason != "cold" => p.get("env").map(|e| (id, e)),
        _ => None,
    };
    match prev {
        Some((id, pe))
            if env_changed(b.env, Some(pe))
                .iter()
                .all(|k| NOT_IN_ENV_TEXT.contains(&k.as_str())) =>
        {
            env_short(b.env, id)
        }
        _ => env_full(b.env),
    }
}

// ---------------------------------------------------------------------------------------------
// Boot block (§3.6)
// ---------------------------------------------------------------------------------------------

/// What the boot block needs besides `boot.json` (§3.6).
pub struct BlockIn<'a> {
    /// `state/kernel/boot.json` verbatim.
    pub boot: &'a Value,
    /// `INIT_SHA`.
    pub sha: &'a str,
    /// `normal` · `probe` · `safe`.
    pub mode: &'a str,
    /// `cold` · `promote` · `rollback` · `crash` · `handoff`.
    pub reason: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    /// The effective effort.
    pub effort: &'a str,
    pub session: &'a str,
    /// The session file was created by this boot (a new session).
    pub session_created: bool,
    pub rehydrated_events: u64,
    pub interrupted_calls: u64,
    /// `sha256:<64 hex>` of this boot's head.
    pub prefix_hash: &'a str,
    /// The previous `boot` event on the path: its id (16 hex) and its JSON object (all fields).
    pub prev_boot: Option<(&'a str, &'a Value)>,
    /// `snapshot(..)`.
    pub env: &'a Value,
    /// boot.md §3.8 `git_sync` object.
    pub git_sync: &'a Value,
    /// `boot.json.update_failed` when its `seq` is not yet in the log; else None.
    pub update_failed: Option<&'a Value>,
    /// `live.capability` from `$INIT_HOME/state/kernel/self.json`, when readable.
    pub capability: Option<&'a Value>,
    /// `INIT_ACCESS` (ADR-009); None (an older kernel): no access line.
    pub access: Option<&'a str>,
    /// `INIT_WORKSPACE` (ADR-009); None: no workspace.
    pub workspace: Option<&'a Path>,
}

/// The boot event's `text` (§3.6 structure, reason and mode lines, optional lines, environment).
pub fn boot_text(b: &BlockIn) -> String {
    let mut lines = vec![format!("[init:boot] {}", reason_line(b))];
    lines.extend(mode_line(b));
    lines.push(you_line(b));
    lines.push(autonomy_line(b.boot));
    lines.push(ceilings_line(b.boot));
    lines.extend(access_line(b));
    lines.push(prefix_line(b));
    lines.push(session_line(b));
    lines.extend(extend_line(b));
    lines.extend(level_change_line(b));
    lines.extend(update_failed_line(b));
    lines.extend(audit_lines(b));
    lines.extend(capability_line(b));
    lines.extend(retire_line(b.boot));
    lines.extend(thin_line(b));
    lines.extend(upstream_line(b.boot));
    lines.extend(predict_line(b.boot));
    lines.extend(engine_line(b.boot));
    lines.extend(evolution_state_line(b.boot));
    lines.push(env_text(b));
    lines.join("\n")
}

/// `boot.json` fields the boot block reads but found absent (or null where the reason needs a
/// value), as dotted paths, for `error{code:"boot_json_field"}` (§5). `env.root` is reported here
/// too; the caller may raise `root_status_missing` for it instead.
pub fn boot_json_missing(boot: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for k in [
        "sha",
        "from_sha",
        "lkg_sha",
        "reason",
        "mode",
        "provisional",
        "restarts_since_lkg",
        "update_failed",
        "env",
        "selfmap",
        "evolution",
    ] {
        if boot.get(k).is_none() {
            out.push(k.to_string());
        }
    }
    let mut need: Vec<&[&str]> = vec![&["selfmap", "body_tokens"][..], &["env", "root"][..]];
    match get_s(boot, &["reason"]) {
        Some("promote") => need.push(&["branch"]),
        Some("crash") => need.push(&["crash"]),
        Some("handoff") => need.push(&["cause"]),
        Some("rollback") if get(boot, &["cause"]).is_none() => need.push(&["update_failed"]),
        _ => {}
    }
    for path in need {
        if get(boot, path).is_none() {
            out.push(path.join("."));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `boot.json.cause` kind: an object's `kind`, or the cause itself when it is a string.
fn cause_kind(boot: &Value) -> Option<&str> {
    let c = get(boot, &["cause"])?;
    c.as_str().or_else(|| get_s(c, &["kind"]))
}

/// A trigger as text: a string, or an object's `kind`.
fn trigger_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(o) => q(get_s(o, &["kind"])),
        None => "?".into(),
    }
}

fn kernel_upgrade_text(ku: &Value) -> String {
    let source = match get_s(ku, &["source"]) {
        Some("maintainer") => "maintainer release".to_string(),
        Some("agent") => match scalar(get(ku, &["branch"]))
            .or_else(|| get_s(ku, &["commit"]).map(|c| sha7(c).to_string()))
        {
            Some(r) => format!("your commit {r}"),
            None => "your commit".to_string(),
        },
        other => q(other),
    };
    format!(
        "Your kernel changed from {} to {} ({source}). The Root audits the new system before it counts as audited-good; take no external actions until then.",
        q(scalar(get(ku, &["from"]))),
        q(scalar(get(ku, &["to"])))
    )
}

fn reason_line(b: &BlockIn) -> String {
    let j = b.boot;
    let sha = sha7(b.sha);
    let from = q(get_s(j, &["from_sha"]).map(sha7));
    let stash = get_s(b.git_sync, &["stash"])
        .map(|s| format!("; uncommitted work is in {s}"))
        .unwrap_or_default();
    let mut t = match b.reason {
        "cold" => {
            let mut t = String::from("Cold start: no running predecessor handed over to you.");
            if b.session_created {
                t.push_str(" New session.");
            } else {
                t.push_str(&format!(" Continuing session {}.", b.session));
            }
            if let Some(p) = get(j, &["panic"]) {
                t.push_str(&format!(
                    " A panic stopped everything at {} ({}). You restart from your audited-good system {sha}, one level lower.",
                    q(get_s(p, &["at"]).and_then(utc_text)),
                    q(get_s(p, &["channel"]))
                ));
            }
            t
        }
        "promote" => format!(
            "You were promoted: {from} → {sha} (branch {}). G0–G3 passed; G4 is running. Until it passes you are provisional, so take no external actions. This is the same conversation: continue where you left off.",
            q(get_s(j, &["branch"]))
        ),
        "rollback" if matches!(cause_kind(j), Some("demotion" | "rollback_system")) => {
            let c = get(j, &["cause"]).unwrap_or(&Value::Null);
            format!(
                "The Root rolled your whole system back to audited-good {sha} from {from}. Cause: {}: {}. Lineage record {}. Your rolled-back commits are kept at refs/init/rolled-back/{from}{stash}. The conversation continues.",
                trigger_text(get(c, &["trigger"])),
                q(scalar(get(c, &["evidence"]))),
                q(get_u(c, &["seq"]))
            )
        }
        "rollback" => {
            let uf = get(j, &["update_failed"]).unwrap_or(&Value::Null);
            format!(
                "You were rolled back to your LKG {sha} from {from}. Cause: {}: {}. Report: state/kernel/update-failed.json. Your rolled-back commits are kept at refs/init/rolled-back/{from}{stash}. The conversation continues.",
                q(get_s(uf, &["stage"])),
                q(get_s(uf, &["reason"]))
            )
        }
        "crash" => {
            let how = match (
                get(j, &["crash", "exit_code"]).and_then(Value::as_i64),
                scalar(get(j, &["crash", "signal"])),
            ) {
                (Some(c), _) => format!("exit code {c}"),
                (None, Some(s)) => format!("signal {s}"),
                (None, None) => "?".to_string(),
            };
            format!(
                "You restarted after {from} crashed ({how}). Restarts since LKG: {}. The last stderr lines are in this boot event's crash field. Check what any interrupted tool call did before retrying it.",
                q(get_u(j, &["restarts_since_lkg"]))
            )
        }
        "handoff" => format!(
            "The kernel restarted you without a body change ({}). The conversation continues.",
            handoff_cause(b)
        ),
        other => format!("Boot reason {other}."),
    };
    if let Some(ku) = get(j, &["kernel_upgrade"]) {
        t.push(' ');
        t.push_str(&kernel_upgrade_text(ku));
    }
    t
}

fn handoff_cause(b: &BlockIn) -> String {
    let c = get(b.boot, &["cause"]).unwrap_or(&Value::Null);
    match cause_kind(b.boot) {
        Some("clear_safe" | "safe_clear") => {
            let h = get_u(c, &["hours"]).or_else(|| get_u(c, &["after_s"]).map(|s| s / 3600));
            format!("safe mode cleared after {} hours without a crash", q(h))
        }
        Some("model_change") => {
            let old = b.prev_boot.and_then(|(_, p)| {
                Some(format!(
                    "{}/{}",
                    get_s(p, &["provider"])?,
                    get_s(p, &["model"])?
                ))
            });
            format!(
                "your main model changed from {} to {}/{}",
                q(old),
                b.provider,
                b.model
            )
        }
        Some("owner_restart") => "your owner restarted you".to_string(),
        Some("workspace_change") => "your owner's workspace changed".to_string(),
        Some("kernel_upgrade") => "your kernel was upgraded".to_string(),
        Some(k) => k.to_string(),
        None if get(b.boot, &["kernel_upgrade"]).is_some() => {
            "your kernel was upgraded".to_string()
        }
        None => "?".to_string(),
    }
}

fn mode_line(b: &BlockIn) -> Option<String> {
    match b.mode {
        "safe" => Some(format!(
            "SAFE MODE: your LKG crash-looped, so the Root rolled you back to your audited-good system {} and lowered your level. promote, gate --quick and vendor are disabled until safe mode clears itself after 24 hours without a crash; T0/T1 trials still work. Diagnose from the crash tails before changing your core.",
            sha7(b.sha)
        )),
        "probe" => Some(
            "GATE PROBE: you are a staging copy being checked by the kernel. Follow the probe instruction below and change nothing."
                .to_string(),
        ),
        _ => None,
    }
}

fn audited_good(boot: &Value) -> Option<&str> {
    get_s(boot, &["env", "root", "audited_good"])
        .or_else(|| get_s(boot, &["evolution", "audited_good"]))
}

fn you_line(b: &BlockIn) -> String {
    let j = b.boot;
    let prov = if get(j, &["provisional"]).is_some() {
        " (provisional)"
    } else {
        ""
    };
    let audit = if get(j, &["evolution", "audit_required"]).and_then(Value::as_bool) == Some(true) {
        " (audit required)"
    } else {
        ""
    };
    let tokens = get_u(j, &["selfmap", "body_tokens"]);
    let delta = match (
        tokens,
        get_u(j, &["selfmap", "body_tokens_prev"]),
        get_s(j, &["from_sha"]),
    ) {
        (Some(t), Some(p), Some(f)) => {
            format!(" ({} vs {})", signed(t as i128 - p as i128), sha7(f))
        }
        _ => String::new(),
    };
    // Unknown body size (no selfmap count yet, e.g. a genesis boot): the part is left out (§5).
    let size = match tokens {
        Some(t) => format!(" · body {} tokens{delta}", thousands(t)),
        None => String::new(),
    };
    format!(
        "- you: body {}{prov} · LKG {}{audit} · audited-good {} · model {}/{}{size}",
        sha7(b.sha),
        q(get_s(j, &["lkg_sha"]).map(sha7)),
        q(audited_good(j).map(sha7)),
        b.provider,
        b.model,
    )
}

/// `<criterion> (<have> of <need>)`, `(met)` or `(none yet)`; unmet ones first.
fn criteria_text(next: &Value) -> String {
    let Some(cs) = get(next, &["criteria"]).and_then(Value::as_array) else {
        return "?".into();
    };
    let mut items: Vec<(bool, String)> = cs
        .iter()
        .map(|c| {
            let name = q(get_s(c, &["text"]).or_else(|| get_s(c, &["id"])));
            let met = get(c, &["met"]).and_then(Value::as_bool) == Some(true);
            let status = match (met, get_u(c, &["have"]), get_u(c, &["need"])) {
                (true, _, _) => "met".to_string(),
                (false, Some(h), Some(n)) => format!("{} of {}", thousands(h), thousands(n)),
                (false, Some(h), None) => thousands(h),
                (false, None, _) => "none yet".to_string(),
            };
            (met, format!("{name} ({status})"))
        })
        .collect();
    if items.is_empty() {
        return "?".into();
    }
    items.sort_by_key(|(met, _)| *met); // stable: unmet first, Root order kept within each group
    items
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The cap comes from `env.root` as `rootview::render_boot` filled it: the kernel's copy, else the
/// Root's published view (`root/ledger.json` `cap`, or min(`max_level`, `supported_max_level`)
/// from `ledger.json`/`charter.json`). When none of them gives it, ` (cap L?)` is left out rather
/// than printed as a placeholder, and so are unknown parts of the at-cap text.
fn autonomy_line(boot: &Value) -> String {
    let Some(r) = get(boot, &["env", "root"]).filter(|r| r.is_object()) else {
        return "- autonomy: L? · ?".to_string();
    };
    let level = get_u(r, &["level"]);
    let (max, sup) = (get_u(r, &["max_level"]), get_u(r, &["supported_max_level"]));
    let cap = get_u(r, &["cap"]).or_else(|| Some(max?.min(sup?)));
    // The Root's ledger has `next: null` exactly at the cap (`get` maps null to None).
    let at_cap = matches!((level, cap), (Some(l), Some(c)) if l >= c)
        || r.get("next").is_some_and(Value::is_null);
    let progress = match get(r, &["next"]).filter(|n| n.is_object()) {
        Some(n) if !at_cap => format!(
            "next: L{} needs {}",
            q(get_u(n, &["level"])),
            criteria_text(n)
        ),
        _ if !at_cap => "?".to_string(),
        _ => {
            let parts: Vec<String> = [
                max.map(|m| format!("charter max L{m}")),
                sup.map(|s| format!("this Root supports up to L{s}")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if parts.is_empty() {
                "at your cap".to_string()
            } else {
                format!("at your cap: {}", parts.join(", "))
            }
        }
    };
    let cap_text = cap.map(|c| format!(" (cap L{c})")).unwrap_or_default();
    format!("- autonomy: L{}{cap_text} · {progress}", q(level))
}

fn class_name(id: &str) -> String {
    match id {
        "models" | "model-providers" | "model_providers" | "providers" => {
            "model providers".to_string()
        }
        other => other.replace(['-', '_'], " "),
    }
}

fn actions_text(r: &Value) -> String {
    let Some(cs) = get(r, &["classes"]).and_then(Value::as_array) else {
        return "?".into();
    };
    let names: Vec<String> = cs.iter().map(|c| class_name(&q(scalar(Some(c))))).collect();
    match names.as_slice() {
        [] => "none".into(),
        [only] if only == "model providers" => "model providers only".into(),
        _ => names.join(", "),
    }
}

fn ceilings_line(boot: &Value) -> String {
    let Some(r) = get(boot, &["env", "root"]) else {
        return "- ceilings (Root): ? a day · ? total, ? used · ? live bodies · actions: ?".into();
    };
    let c = |k: &str| get_u(r, &["ceilings", k]);
    let bodies = match c("parallel_bodies") {
        Some(1) => "1 live body".to_string(),
        n => format!("{} live bodies", q(n)),
    };
    format!(
        "- ceilings (Root): {} a day · {} total, {} used · {bodies} · actions: {}",
        q_dollars(c("day_musd")),
        q_dollars(c("total_musd")),
        q_dollars(c("total_used_musd")),
        actions_text(r)
    )
}

/// One line of text: control characters (a newline in a directory name) become `?`.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// ADR-009, ADR-012: what this body may write and read (`INIT_ACCESS` = `none` · `read_home` · `workspace` ·
/// `full`, set by the Root on every body spawn; SBX-009 §D). It is fixed for the body's lifetime
/// (a change restarts the body), so it lives in the boot block, never in the head or the signal
/// line. Absent `INIT_ACCESS` (an older kernel): no line. A value this body does not know, or a
/// value that contradicts the workspace, renders verbatim.
fn access_line(b: &BlockIn) -> Option<String> {
    let access = one_line(b.access?);
    let raw = b.workspace.map(|w| w.to_string_lossy());
    let ws = raw.as_deref().map(one_line);
    let ws_or_none = ws.clone().unwrap_or_else(|| "none".into());
    Some(match (access.as_str(), ws) {
        ("workspace", Some(w)) => {
            // The probed cwd differs only when the workspace was not a directory at boot (a boot
            // facts warning); an unknown cwd counts as the workspace.
            let start = match get_s(b.env, &["probed", "cwd"]) {
                Some(cwd) if Some(cwd) != raw.as_deref() => {
                    format!("it is not a directory now: your tools start in {}", one_line(cwd))
                }
                _ => "your tools start in the workspace".to_string(),
            };
            format!(
                "- access: write the workspace {w} and INIT_HOME ({start}) · read the owner's home, secret stores denied"
            )
        }
        ("none", None) => {
            "- access: INIT_HOME only (no home access; the v0.1 sandbox)".to_string()
        }
        ("read_home", None) => "- access: no workspace (init was launched outside a project): write INIT_HOME only · read the owner's home, secret stores denied".to_string(),
        // ADR-012: full is the default; the Root keeps only its own files out of reach.
        ("full", _) => format!(
            "- access: FULL: read and write anywhere the owner can, network allowed · ~ is the owner's home · denied: the Root's files (~/.init-root, its service, INIT's launchers) · only the workspace has /undo · workspace {ws_or_none}"
        ),
        (other, _) => format!("- access: {other} · workspace {ws_or_none}"),
    })
}

fn prefix_line(b: &BlockIn) -> String {
    let h = b
        .prefix_hash
        .strip_prefix("sha256:")
        .unwrap_or(b.prefix_hash);
    let status = match b.prev_boot {
        None => "first boot of this session",
        Some((_, p)) if get_s(p, &["prefix_hash"]) == Some(b.prefix_hash) => "same as before",
        Some(_) => "changed, so one cache miss",
    };
    format!("- prompt prefix: {} ({status})", short(h, 12))
}

fn session_line(b: &BlockIn) -> String {
    let ev = if b.rehydrated_events == 1 {
        "event"
    } else {
        "events"
    };
    let mut s = format!(
        "- session: {} · {} {ev} rehydrated",
        b.session,
        thousands(b.rehydrated_events)
    );
    match b.interrupted_calls {
        0 => {}
        1 => s.push_str(" · 1 tool call was interrupted; its result says whether it ran"),
        k => s.push_str(&format!(
            " · {} tool calls were interrupted; their results say whether they ran",
            thousands(k)
        )),
    }
    s
}

/// `n/m` with `?` for an unknown side.
fn frac(n: Option<u64>, m: Option<u64>) -> String {
    format!("{}/{}", q(n), q(m))
}

/// At most 4 cap ids (Root-validated, rendered on one line), then `+N more`.
fn cap_list(v: Option<&Value>) -> Option<String> {
    let a: Vec<String> = v?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(|s| one_line(short(s, 64)))
        .collect();
    if a.is_empty() {
        return None;
    }
    let mut s = a.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if a.len() > 4 {
        s.push_str(&format!(" +{} more", a.len() - 4));
    }
    Some(s)
}

/// The level part of the extend line: `L0→L1 4d` (days still to serve at this level), else the
/// first unmet criterion in the Root's order, `L0→L1 ready` when every one is met, `L2 at cap`.
fn extend_level(r: &Value) -> String {
    let Some(level) = get_u(r, &["level"]) else {
        return "L?".into();
    };
    let cap = get_u(r, &["cap"])
        .or_else(|| Some(get_u(r, &["max_level"])?.min(get_u(r, &["supported_max_level"])?)));
    if cap.is_some_and(|c| level >= c) || r.get("next").is_some_and(Value::is_null) {
        return format!("L{level} at cap");
    }
    let Some(next) = get(r, &["next"]).filter(|n| n.is_object()) else {
        return format!("L{level}→L{} ?", level + 1);
    };
    let to = get_u(next, &["level"]).unwrap_or(level + 1);
    let cs = get(next, &["criteria"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let unmet: Vec<&Value> = cs
        .iter()
        .filter(|c| get(c, &["met"]).and_then(Value::as_bool) != Some(true))
        .collect();
    let left = |c: &Value| Some(get_u(c, &["need"])?.saturating_sub(get_u(c, &["have"])?));
    let rest = if cs.is_empty() {
        "?".to_string()
    } else if let Some(d) = unmet
        .iter()
        .find(|c| get_s(c, &["id"]) == Some("days_at_level"))
        .and_then(|c| left(c))
    {
        format!("{d}d")
    } else {
        match unmet.first() {
            None => "ready".into(),
            Some(c) => match (get_s(c, &["id"]), left(c)) {
                (Some("owner_turns"), Some(n)) => plural(n, "owner turn", "owner turns"),
                (Some("clean_groups"), Some(n)) => plural(n, "clean group", "clean groups"),
                (id, _) => {
                    let have = get_u(c, &["have"]).map(thousands);
                    let need = get_u(c, &["need"]).map(thousands);
                    format!(
                        "{} ({} of {})",
                        one_line(short(id.unwrap_or("?"), 64)),
                        q(have),
                        q(need)
                    )
                }
            },
        }
    };
    format!("L{level}→L{to} {rest}")
}

/// ADR-011 X11: one line on the free path, from `env.root.extend` (`rootview::normalize_extend`),
/// the ADR's `- extend · groups 2/4 · window 7/20 · caps 31/64 · L0→L1 4d`: open change groups of
/// the level's budget, the newest group's window (owner turns seen of the turns it needs), live
/// self-made caps of the bloat budget, and the way to the next level. Debts and blocks follow, each
/// with what to do. No line on the gated path (no `extend`).
fn extend_line(b: &BlockIn) -> Option<String> {
    let r = get(b.boot, &["env", "root"])?;
    let x = get(r, &["extend"]).filter(|x| x.is_object())?;
    let window = match get(x, &["window"]).filter(|w| w.is_object()) {
        None => "window none".to_string(),
        Some(w) => {
            let kernel = if get(w, &["kernel"]).and_then(Value::as_bool) == Some(true) {
                " (kernel)"
            } else {
                ""
            };
            format!(
                "window {}{kernel}",
                frac(get_u(w, &["turns"]), get_u(w, &["of"]))
            )
        }
    };
    let (caps, caps_max) = (get_u(x, &["caps"]), get_u(x, &["caps_max"]));
    let mut parts = vec![
        format!(
            "groups {}",
            frac(get_u(x, &["groups"]), get_u(x, &["groups_max"]))
        ),
        window,
        format!("caps {}", frac(caps, caps_max)),
        extend_level(r),
    ];
    if let Some(l) = cap_list(get(x, &["quarantine_debt"])) {
        parts.push(format!(
            "QUARANTINED {l}: move to quarantine/ or remove; every other change is refused until then"
        ));
    }
    if let Some(l) = cap_list(get(x, &["archive_due"])) {
        parts.push(format!(
            "archive due {l}: move to archive/ within 7 days or adding caps is refused"
        ));
    }
    if matches!((caps, caps_max), (Some(n), Some(m)) if n > m) {
        parts.push("over the caps budget: archive or remove before adding".into());
    }
    if let Some(t) = get_s(x, &["freeze_until"]) {
        let now = get_s(b.env, &["probed", "date_utc"]).and_then(parse_utc_minute);
        match parse_utc_minute(t) {
            Some(m) if now.is_some_and(|n| m <= n) => {}
            Some(m) => parts.push(format!(
                "changes frozen until {} UTC (money burst)",
                minute_text(m)
            )),
            None => parts.push("changes frozen (money burst)".into()),
        }
    }
    if let Some(rb) = get(x, &["rollback"]) {
        let why = rb.as_str().map(|s| one_line(short(s, 64)));
        parts.push(match why {
            Some(w) => format!("Root rollback pending ({w})"),
            None => "Root rollback pending".into(),
        });
    }
    Some(format!("- extend · {}", parts.join(" · ")))
}

/// What a level adds (evolution.md §3.15.1, kernel surface column).
fn new_for_level(level: Option<u64>) -> &'static str {
    match level {
        Some(1) => {
            "initctl task template, task submit, task list, task show, budget set and model set"
        }
        Some(2) => "initctl fleet spawn, fleet send and fleet stop",
        Some(3) => "initctl kernel promote",
        Some(4) => "every action class your charter allows",
        _ => "?",
    }
}

/// Shown once: when `root.last_change` is newer (by R4 seq) than the previous boot's, or on the
/// first boot of a session (nothing in this conversation has shown it yet).
fn level_change_line(b: &BlockIn) -> Option<String> {
    let lc = get(b.boot, &["env", "root", "last_change"])?;
    let prev = b
        .prev_boot
        .and_then(|(_, p)| get(p, &["env", "kernel", "root", "last_change"]));
    let newer = match (get_u(lc, &["seq"]), prev.and_then(|p| get_u(p, &["seq"]))) {
        (Some(s), Some(p)) => s > p,
        _ => prev != Some(lc),
    };
    if !newer {
        return None;
    }
    let (from, to) = (get_u(lc, &["from"]), get_u(lc, &["to"]));
    let dir = get_s(lc, &["direction"]).or(match (from, to) {
        (Some(f), Some(t)) if t > f => Some("up"),
        (Some(f), Some(t)) if t < f => Some("down"),
        _ => None,
    });
    let (a, z) = (q(from), q(to));
    Some(match dir {
        Some("up") => {
            let crit = match get(lc, &["criteria"]).and_then(Value::as_array) {
                Some(cs) if !cs.is_empty() => cs
                    .iter()
                    .map(|c| match c {
                        Value::String(s) => s.clone(),
                        o => q(get_s(o, &["text"]).or_else(|| get_s(o, &["id"]))),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => "?".to_string(),
            };
            format!(
                "- level up: L{a} → L{z} ({crit}). New for you: {}.",
                new_for_level(to)
            )
        }
        Some("down") => format!(
            "- level down: L{a} → L{z} after {} (lineage record {}). Your hold timers restarted; see evolution.md §3.15.3 for what was cancelled.",
            trigger_text(get(lc, &["trigger"])),
            q(get_u(lc, &["seq"]))
        ),
        Some("cap") => format!("- level capped: L{a} → L{z} by a charter edit."),
        _ => format!("- level change: L{a} → L{z}."),
    })
}

fn update_failed_line(b: &BlockIn) -> Option<String> {
    if b.reason == "rollback" {
        return None; // the reason line already names the cause
    }
    let uf = b.update_failed?;
    let exit = get(uf, &["exit_code"])
        .and_then(Value::as_i64)
        .map(|c| format!(" (exit {c})"))
        .unwrap_or_default();
    Some(format!(
        "- last update failed: {} at {}: {}{exit}. Report: state/kernel/update-failed.json.",
        q(get_s(uf, &["sha"]).map(sha7)),
        q(get_s(uf, &["stage"])),
        q(get_s(uf, &["reason"]))
    ))
}

fn audit_lines(b: &BlockIn) -> Vec<String> {
    let j = b.boot;
    let mut out = Vec::new();
    if get(j, &["evolution", "audit_required"]).and_then(Value::as_bool) == Some(true) {
        out.push(format!(
            "- your LKG {} waits for a sealed audit: its gain rests on tasks you wrote.",
            q(get_s(j, &["lkg_sha"]).map(sha7))
        ));
    }
    let prev = b
        .prev_boot
        .and_then(|(_, p)| get_s(p, &["env", "kernel", "root", "audited_good"]));
    if let (Some(cur), Some(prev)) = (audited_good(j), prev)
        && cur != prev
    {
        let on = get_s(j, &["env", "root", "audited_good_at"])
            .and_then(parse_utc_minute)
            .map(|m| format!(" on {}", &minute_text(m)[..10]))
            .unwrap_or_default();
        out.push(format!(
            "- sealed audit{on} passed: audited-good is now {}.",
            sha7(cur)
        ));
    }
    out
}

/// One row per category for the view: `_all` and `low_n` rows are dropped (a `low_n` row says
/// nothing and is never named); among several rows of a category the `pool` row wins, then
/// `holdout`, then `agent` (awareness.md §3.7.1 rule of use), then the one with more tasks.
fn capability_rows(cap: &Value) -> BTreeMap<String, (u64, Option<u64>)> {
    let rank = |s: Option<&str>| match s {
        Some("pool") => 0,
        Some("holdout") => 1,
        Some("agent") => 2,
        _ => 3,
    };
    let mut best: BTreeMap<String, (u8, u64, u64, Option<u64>)> = BTreeMap::new();
    for row in get(cap, &["categories"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(cat), Some(p)) = (get_s(row, &["category"]), get_u(row, &["pass_permille"]))
        else {
            continue;
        };
        let tasks = get_u(row, &["tasks"]);
        // `low_n` absent: fewer than 3 tasks (selfmap.md's rule), unknown counts as low.
        let low = get(row, &["low_n"])
            .and_then(Value::as_bool)
            .unwrap_or_else(|| tasks.is_none_or(|t| t < 3));
        if cat == "_all" || low {
            continue;
        }
        let key = (rank(get_s(row, &["source"])), u64::MAX - tasks.unwrap_or(0));
        match best.get(cat) {
            Some(&(r, t, _, _)) if (r, t) <= key => {}
            _ => {
                best.insert(cat.to_string(), (key.0, key.1, p, tasks));
            }
        }
    }
    best.into_iter()
        .map(|(c, (_, _, p, t))| (c, (p, t)))
        .collect()
}

/// What the boot event stores so the next boot can tell whether the capability line is due
/// (§3.7.1); goes into the boot event as `capability`: `{category: pass_permille}` over the
/// nameable rows. None: nothing to store.
pub fn capability_digest(capability: Option<&Value>) -> Option<Value> {
    let rows = capability_rows(capability?);
    if rows.is_empty() {
        return None;
    }
    Some(Value::Object(
        rows.into_iter().map(|(c, (p, _))| (c, json!(p))).collect(),
    ))
}

/// Shown on a cold boot, and on any boot where a nameable category appeared or its
/// `pass_permille` moved by 100 or more since the previous boot event (§3.7.1).
fn capability_line(b: &BlockIn) -> Option<String> {
    let rows = capability_rows(b.capability?);
    if rows.is_empty() {
        return None;
    }
    let prev = b
        .prev_boot
        .and_then(|(_, p)| get(p, &["capability"]))
        .and_then(Value::as_object);
    let due = b.reason == "cold"
        || match prev {
            None => true,
            Some(pv) => rows.iter().any(|(c, (p, _))| {
                pv.get(c)
                    .and_then(Value::as_u64)
                    .is_none_or(|o| p.abs_diff(o) >= 100)
            }),
        };
    if !due {
        return None;
    }
    let mut sorted: Vec<(&String, &(u64, Option<u64>))> = rows.iter().collect();
    sorted.sort_by_key(|(c, (p, _))| (*p, c.as_str()));
    let item = |(c, (p, t)): (&String, &(u64, Option<u64>))| {
        format!("{c} {}% (n {})", p / 10, q(t.map(thousands)))
    };
    let n = sorted.len();
    let weak_n = if n >= 3 { 2 } else { 1 };
    let weakest = sorted[..weak_n]
        .iter()
        .map(|&r| item(r))
        .collect::<Vec<_>>()
        .join(" · ");
    let strongest = if n >= 2 {
        format!("; strongest {}", item(sorted[n - 1]))
    } else {
        String::new()
    };
    Some(format!(
        "- capability (pass rate by task category, kernel-measured): weakest {weakest}{strongest}. Full view: jq '.live.capability' state/kernel/self.json"
    ))
}

fn evolution_items<'a>(boot: &'a Value, key: &str) -> Vec<&'a Value> {
    get(boot, &["evolution", key])
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn retire_line(boot: &Value) -> Option<String> {
    let items = evolution_items(boot, "retire_candidates");
    if items.is_empty() {
        return None;
    }
    let list = items
        .iter()
        .take(10)
        .map(|it| {
            format!(
                "{} ({}, {} tasks, {} tokens)",
                q(scalar(get(it, &["id"]))),
                q(get_s(it, &["class"])),
                q(get_u(it, &["tasks_since_use"]).map(thousands)),
                q(get_u(it, &["tokens"]).map(thousands))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "- unused growth (retirement candidates): {list}. Delete what you no longer need with a retire commit."
    ))
}

fn thin_line(b: &BlockIn) -> Option<String> {
    let items: Vec<&Value> = evolution_items(b.boot, "thin_candidates")
        .into_iter()
        .filter(|it| get_s(it, &["status"]) == Some("pending"))
        .collect();
    if items.is_empty() {
        return None;
    }
    let list = items
        .iter()
        .map(|it| {
            format!(
                "{} ({}, {} tokens)",
                q(scalar(get(it, &["id"]))),
                q(get_s(it, &["tag"])),
                q(get_u(it, &["tokens"]).map(thousands))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "- your model changed to {}/{}. Scaffolds that may now be unneeded: {list}. Try one thin commit per component; the gate decides.",
        b.provider, b.model
    ))
}

fn upstream_line(boot: &Value) -> Option<String> {
    let u = get(boot, &["evolution", "upstream"])?;
    Some(format!(
        "- seed release {} ({}) is available upstream. Merge it as an upstream commit when convenient.",
        q(scalar(get(u, &["tag"]))),
        q(get_s(u, &["sha"]).map(sha7))
    ))
}

fn predict_line(boot: &Value) -> Option<String> {
    let p = get_u(boot, &["evolution", "predict_hit_rate"])?;
    Some(format!(
        "- your Predicts: claims were right {}% of the time (last 20 resolved).",
        p / 10
    ))
}

/// A kernel-written word (a friction signature, a proposal state) on one line, at most 64 chars.
fn kernel_word(s: &str) -> String {
    one_line(short(s, 64))
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// The evolution engine (evolution-v2 §3.4.4, §4.6), from the kernel's `boot.json.evolution`
/// fields `frictions_eligible`, `frictions_top {signature, instances}`, `proposals_open` and
/// `proposals_by_state {state: count}`. No line when nothing is eligible and nothing is open.
fn engine_line(boot: &Value) -> Option<String> {
    let eligible = get_u(boot, &["evolution", "frictions_eligible"]).unwrap_or(0);
    let open = get_u(boot, &["evolution", "proposals_open"]).unwrap_or(0);
    if eligible == 0 && open == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if eligible > 0 {
        let mut s = plural(eligible, "eligible friction", "eligible frictions");
        if let Some(top) = get(boot, &["evolution", "frictions_top"]).filter(|t| t.is_object()) {
            s.push_str(&format!(
                " (top {} ×{})",
                q(get_s(top, &["signature"]).map(kernel_word)),
                q(get_u(top, &["instances"]).map(thousands))
            ));
        }
        parts.push(s);
    }
    if open > 0 {
        let states: BTreeMap<String, u64> = get(boot, &["evolution", "proposals_by_state"])
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((kernel_word(k), v.as_u64().filter(|&n| n > 0)?)))
                    .collect()
            })
            .unwrap_or_default();
        let s = match states.iter().next() {
            Some((state, &n)) if states.len() == 1 && n == open => {
                format!("{} {state}", plural(open, "proposal", "proposals"))
            }
            Some(_) => {
                let list = states
                    .iter()
                    .map(|(k, n)| format!("{} {k}", thousands(*n)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} open ({list})", plural(open, "proposal", "proposals"))
            }
            None => format!("{} open", plural(open, "proposal", "proposals")),
        };
        parts.push(s);
    }
    Some(format!("- evolution: {}", parts.join("; ")))
}

fn evolution_state_line(boot: &Value) -> Option<String> {
    let s = get_s(boot, &["evolution", "state"])
        .or_else(|| get_s(boot, &["evolution", "evolution_state"]))?;
    (s != "normal").then(|| format!("- evolution state: {s}."))
}

// ---------------------------------------------------------------------------------------------
// Self-signals (§3.8)
// ---------------------------------------------------------------------------------------------

/// Kernel and Root values the signal line shows (from `boot.json` at boot; updated by notices).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignalCx {
    pub level: Option<u64>,
    pub day_ceiling_musd: Option<u64>,
    pub restarts_since_lkg: Option<u64>,
    pub provisional: bool,
    pub kernel_provisional: bool,
}

impl SignalCx {
    /// Level and day ceiling from `env.root` (the Root's status), restarts and provisional from
    /// the kernel's fields; kernel provisional from `evolution.state == "kernel_provisional"`.
    pub fn from_boot(boot: &Value) -> SignalCx {
        let state = get_s(boot, &["evolution", "state"])
            .or_else(|| get_s(boot, &["evolution", "evolution_state"]));
        SignalCx {
            level: get_u(boot, &["env", "root", "level"]),
            day_ceiling_musd: get_u(boot, &["env", "root", "ceilings", "day_musd"]),
            restarts_since_lkg: get_u(boot, &["restarts_since_lkg"]),
            provisional: get(boot, &["provisional"]).is_some(),
            kernel_provisional: state == Some("kernel_provisional"),
        }
    }
}

/// `/_init/stats` fields read (§4.4); None renders `?`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub today_musd: Option<u64>,
    pub cap_today_musd: Option<u64>,
    /// False means metering only; legacy proxies omit this field.
    pub money_enforced: Option<bool>,
    /// Gate/probe spend only, not all reasoning spent on free evolution.
    pub evolution_checks_today_musd: Option<u64>,
    pub cache_last_permille: Option<u64>,
    pub cache_today_permille: Option<u64>,
}

impl Stats {
    /// The §4.4 fields of a `/_init/stats` body; anything not a JSON integer is None.
    pub fn from_json(v: &Value) -> Stats {
        let money_enforced = if get_s(v, &["spend", "policy"]) == Some("observe") {
            Some(false)
        } else {
            get(v, &["spend", "enforced"]).and_then(Value::as_bool)
        };
        Stats {
            today_musd: get_u(v, &["spend", "today_musd"]),
            cap_today_musd: if money_enforced == Some(false) { None } else { get_u(v, &["spend", "cap_today_musd"]) },
            money_enforced,
            evolution_checks_today_musd: get_u(v, &["spend", "evolution_today_musd"]),
            cache_last_permille: get_u(v, &["cache", "last_permille"]),
            cache_today_permille: get_u(v, &["cache", "today_permille"]),
        }
    }
}

/// A Unix stream whose every read waits at most until one shared deadline.
struct Deadline {
    s: UnixStream,
    end: Instant,
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // Best effort: macOS refuses `SO_RCVTIMEO` (EINVAL) once the peer has closed, and then
        // the timeout set earlier (never longer than the whole budget) still bounds the read.
        let _ = self.s.set_read_timeout(Some(left));
        self.s.read(buf)
    }
}

/// Largest stats body read.
const STATS_MAX: u64 = 1 << 20;

fn try_fetch_stats(sock: &Path, timeout: Duration) -> Option<Value> {
    let end = Instant::now() + timeout;
    let mut s = UnixStream::connect(sock).ok()?;
    let t = Some(timeout.max(Duration::from_millis(1)));
    s.set_write_timeout(t).ok()?;
    s.set_read_timeout(t).ok()?;
    s.write_all(b"GET /_init/stats HTTP/1.1\r\nhost: init\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut r = BufReader::new(Deadline { s, end });
    let (status, headers) = read_head(&mut r).ok()?;
    if status != 200 {
        return None;
    }
    let mut body = Vec::new();
    match body_framing(&headers).ok()? {
        Framing::Chunked => {
            ChunkedReader::new(r)
                .take(STATS_MAX)
                .read_to_end(&mut body)
                .ok()?;
        }
        Framing::Length(n) => {
            if n > STATS_MAX {
                return None;
            }
            r.take(n).read_to_end(&mut body).ok()?;
            if body.len() as u64 != n {
                return None;
            }
        }
        Framing::UntilClose => {
            r.take(STATS_MAX).read_to_end(&mut body).ok()?;
        }
    }
    serde_json::from_slice(&body).ok()
}

/// `GET /_init/stats` over the proxy socket (HTTP/1.1, `Host: init`), `timeout` total; any failure
/// gives all-None.
pub fn fetch_stats(sock: &Path, timeout: Duration) -> Stats {
    try_fetch_stats(sock, timeout)
        .map(|v| Stats::from_json(&v))
        .unwrap_or_default()
}

fn pct(permille: Option<u64>) -> String {
    permille.map_or_else(|| "?".to_string(), |p| format!("{}%", p / 10))
}

/// `2h14m`, or `14m` under an hour.
fn uptime_text(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60) {
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h{m:02}m"),
    }
}

/// The `[init:signals] …` line (§3.8.2).
pub fn signal_text(
    ctx_tokens: u64,
    th: &Thresholds,
    cx: &SignalCx,
    stats: &Stats,
    uptime: Duration,
) -> String {
    let mut s = format!(
        "[init:signals] L{} · context {}% ({}k of {}k)",
        q(cx.level),
        ctx_tokens.saturating_mul(100) / th.w_eff.max(1),
        ctx_tokens / 1000,
        th.w_eff / 1000
    );
    let ceiling = q_dollars(cx.day_ceiling_musd);
    if stats.money_enforced == Some(false) {
        s.push_str(&format!(" · spend {} today (observed)", q_dollars(stats.today_musd)));
    } else { match (stats.today_musd, stats.cap_today_musd) {
        (None, None) => s.push_str(&format!(" · spend ?, ceiling {ceiling}")),
        (t, c) => s.push_str(&format!(
            " · spend {} of {} today, ceiling {ceiling}",
            q_dollars(t),
            q_dollars(c)
        )),
    }
    }
    match (stats.cache_last_permille, stats.cache_today_permille) {
        (None, None) => s.push_str(" · cache ?"),
        (l, t) => s.push_str(&format!(" · cache {} last, {} today", pct(l), pct(t))),
    }
    s.push_str(&format!(
        " · up {} · restarts since LKG {}",
        uptime_text(uptime),
        q(cx.restarts_since_lkg)
    ));
    if cx.provisional {
        s.push_str(" · provisional: G4 pending, no external actions");
    }
    if cx.kernel_provisional {
        s.push_str(" · kernel provisional: audit pending, no external actions");
    }
    // The hint appears at 85% of T_trigger, never before (a countdown on every line would feed
    // context anxiety).
    if th.t_trigger > 0 && ctx_tokens.saturating_mul(100) >= th.t_trigger.saturating_mul(85) {
        s.push_str(&format!(" · compaction at {}k", th.t_trigger / 1000));
    }
    s
}

/// The body's `SignalSource` (replaces WP11's `BasicSignals`).
pub struct Signals {
    /// `INIT_PROXY_SOCK`; None skips the stats fetch (all `?`).
    pub proxy_sock: Option<PathBuf>,
    pub cx: SignalCx,
    pub start: Instant,
    pub stats_timeout: Duration,
}

impl SignalSource for Signals {
    fn signal(&mut self, ctx_tokens: u64, ctx_permille: u64, th: &Thresholds) -> SignalEv {
        let stats = match &self.proxy_sock {
            Some(s) => fetch_stats(s, self.stats_timeout),
            None => Stats::default(),
        };
        let uptime = self.start.elapsed();
        let mut extra = Map::new();
        for (k, v) in [
            ("spend_today_musd", stats.today_musd),
            ("cap_today_musd", stats.cap_today_musd),
            ("evolution_checks_today_musd", stats.evolution_checks_today_musd),
            ("cache_last_permille", stats.cache_last_permille),
            ("cache_today_permille", stats.cache_today_permille),
            ("uptime_s", Some(uptime.as_secs())),
            ("level", self.cx.level),
        ] {
            if let Some(v) = v {
                extra.insert(k.to_string(), json!(v));
            }
        }
        SignalEv {
            ctx_tokens,
            ctx_permille,
            text: signal_text(ctx_tokens, th, &self.cx, &stats, uptime),
            extra,
        }
    }
}
