//! Shared pieces: the environment record (testing.md §3.11 common rules), statistics, result
//! files (`measurements/<m>.json`, sorted keys), the disk guard and small process helpers.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A measurement's outcome: the `runs[]` entries, extra result fields, and whether it passed.
pub type Outcome = (Vec<Value>, Map<String, Value>, bool);

/// The repository root (`kernel/crates/init-testkit/../../..`).
pub fn repo() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    p.canonicalize().unwrap_or(p)
}

/// Every home and staged copy WP16 creates lives under `<repo>/target/test-homes/wp16/`.
pub fn scratch_base() -> PathBuf {
    repo().join("target/test-homes/wp16")
}

/// Trimmed stdout of a command, or "" when it cannot run.
pub fn out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Free KiB on the file system holding `p` (`df -k`).
pub fn free_kib(p: &Path) -> Option<u64> {
    let t = out("/bin/df", &["-k", &p.to_string_lossy()]);
    t.lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|x| x.parse().ok())
}

/// The orchestrator's floor for WP16: stop below 5.3 GiB free.
pub const DISK_FLOOR_KIB: u64 = 5_557_453;

pub fn disk_guard() -> Result<u64, String> {
    match free_kib(&repo()) {
        Some(k) if k < DISK_FLOOR_KIB => Err(format!(
            "disk guard: only {:.2} GiB free (floor 5.3 GiB); stopping",
            k as f64 / 1_048_576.0
        )),
        Some(k) => Ok(k),
        None => Err("disk guard: df failed".into()),
    }
}

/// `du -sk` of a path, in KiB.
pub fn du_kib(p: &Path) -> Option<u64> {
    out("/usr/bin/du", &["-sk", &p.to_string_lossy()])
        .split_whitespace()
        .next()
        .and_then(|x| x.parse().ok())
}

/// `{machine, toolchain, load}` as testing.md §3.11 lists them, with an operator note on other
/// activity (other agents building at the same time).
pub fn environment(note: Option<&str>) -> Map<String, Value> {
    let mut m = Map::new();
    let mem: Option<u64> = out("/usr/sbin/sysctl", &["-n", "hw.memsize"]).parse().ok();
    let ncpu: Option<u64> = out("/usr/sbin/sysctl", &["-n", "hw.ncpu"]).parse().ok();
    m.insert(
        "machine".into(),
        json!({
            "cpu": out("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]),
            "mem_bytes": mem,
            "ncpu": ncpu,
            "os": out("/usr/bin/sw_vers", &[]).replace('\t', " ").lines().map(str::trim).collect::<Vec<_>>().join("; "),
        }),
    );
    m.insert(
        "toolchain".into(),
        json!({
            "cargo": out("cargo", &["-V"]),
            "rustc": out("rustc", &["-vV"]).lines().map(str::trim).collect::<Vec<_>>().join("; "),
        }),
    );
    let power = out("/usr/bin/pmset", &["-g", "ps"]);
    m.insert(
        "load".into(),
        json!({
            "free_kib": free_kib(&repo()),
            "note": note.unwrap_or(""),
            "power": power.lines().next().unwrap_or("").trim(),
            "uptime": out("/usr/bin/uptime", &[]),
        }),
    );
    m.insert("taken_at".into(), json!(init_testkit::util::ts()));
    m
}

pub fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn min(xs: &[f64]) -> Option<f64> {
    xs.iter().copied().reduce(f64::min)
}

fn max(xs: &[f64]) -> Option<f64> {
    xs.iter().copied().reduce(f64::max)
}

/// Rounds to 3 decimals (results are reported in s or ms; more digits are noise).
pub fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// One `runs[]` entry: `{kind, n, median, min, max, unit, values}`.
pub fn summary(kind: &str, unit: &str, xs: &[f64]) -> Value {
    json!({
        "kind": kind,
        "n": xs.len(),
        "median": median(xs).map(r3),
        "min": min(xs).map(r3),
        "max": max(xs).map(r3),
        "unit": unit,
        "values": xs.iter().map(|x| r3(*x)).collect::<Vec<_>>(),
    })
}

/// Least-squares slope of y over x; `None` with fewer than 2 distinct x.
pub fn ols_slope(pts: &[(f64, f64)]) -> Option<f64> {
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return None;
    }
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum();
    if sxx == 0.0 {
        return None;
    }
    let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    Some(sxy / sxx)
}

/// The result file: the environment, `runs`, `notes` and any extra fields, merged into an existing
/// file so that several invocations (one per workspace for M-1) share `measurements/<m>.json`.
/// An existing run of the same `kind` is replaced.
pub fn write_result(
    path: &Path,
    env: Map<String, Value>,
    runs: Vec<Value>,
    extra: Map<String, Value>,
) -> std::io::Result<Value> {
    let mut doc: Map<String, Value> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut all: Vec<Value> = doc
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for r in runs {
        all.retain(|o| o["kind"] != r["kind"]);
        all.push(r);
    }
    for (k, v) in env {
        doc.insert(k, v);
    }
    for (k, v) in extra {
        doc.insert(k, v);
    }
    doc.insert("runs".into(), Value::Array(all));
    let v = Value::Object(doc);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut s = serde_json::to_string_pretty(&v)?;
    s.push('\n');
    std::fs::write(path, s)?;
    Ok(v)
}

/// Removes a directory this tool created, by its exact path, only under [`scratch_base`].
pub fn remove_exact(p: &Path) {
    let base = scratch_base();
    if p.starts_with(&base) && p != base && p.exists() {
        let _ = std::fs::remove_dir_all(p);
    }
}

/// RSS of a process in KiB (`ps -o rss= -p <pid>`), as testing.md M-5 step 3 samples it.
pub fn rss_kib(pid: u32) -> Option<u64> {
    out("/bin/ps", &["-o", "rss=", "-p", &pid.to_string()])
        .trim()
        .parse()
        .ok()
}

/// Physical footprint of a process in KiB: macOS `phys_footprint`, the number Activity Monitor,
/// jetsam and `footprint -p` report (testing.md M-5 step 3 on macOS). Unlike `ps` RSS it does not
/// count pages that libmalloc has freed and marked reusable (FIX8). Read with
/// `proc_pid_rusage(RUSAGE_INFO_V2)`, one syscall, no child process. `None` off macOS, or when
/// the process is gone.
#[cfg(target_os = "macos")]
pub fn footprint_kib(pid: u32) -> Option<u64> {
    let mut ri = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: for flavor RUSAGE_INFO_V2 the kernel writes one `rusage_info_v2` into the buffer;
    // the C signature takes `rusage_info_t *` (a `void **`) for what is really that struct pointer.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            ri.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    if rc != 0 {
        return None;
    }
    // SAFETY: rc == 0, so the struct was filled (and it was zeroed before).
    let ri = unsafe { ri.assume_init() };
    Some(ri.ri_phys_footprint / 1024)
}

#[cfg(not(target_os = "macos"))]
pub fn footprint_kib(_pid: u32) -> Option<u64> {
    None
}

/// The same number from the `footprint` tool (`footprint -p <pid> -f bytes`, about 12 ms per
/// call), parsed from its `Footprint: <n> B` header line. Used as a cross-check of
/// [`footprint_kib`] at the start and the end of a run.
pub fn footprint_tool_kib(pid: u32) -> Option<u64> {
    parse_footprint_bytes(&out(
        "/usr/bin/footprint",
        &["-p", &pid.to_string(), "-f", "bytes"],
    ))
    .map(|b| b / 1024)
}

pub fn parse_footprint_bytes(text: &str) -> Option<u64> {
    let rest = text.lines().find_map(|l| l.split_once("Footprint:"))?.1;
    let mut words = rest.split_whitespace();
    let n = words.next()?;
    if words.next()? != "B" {
        return None;
    }
    n.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_odd_even_and_empty() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn ols_slope_of_a_line_and_degenerate_inputs() {
        let pts: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, 3.0 * i as f64 + 7.0)).collect();
        assert!((ols_slope(&pts).unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(ols_slope(&[(1.0, 2.0)]), None);
        assert_eq!(ols_slope(&[(1.0, 2.0), (1.0, 5.0)]), None);
    }

    #[test]
    fn summary_shape() {
        let s = summary("x", "s", &[0.3, 0.1, 0.2]);
        assert_eq!(s["n"], 3);
        assert_eq!(s["median"], 0.2);
        assert_eq!(s["min"], 0.1);
        assert_eq!(s["max"], 0.3);
        assert_eq!(s["unit"], "s");
    }

    #[test]
    fn footprint_header_parses() {
        let t =
            "======\nzsh [94320]: 64-bit    Footprint: 1426176 B (16384 bytes per page)\n=====\n";
        assert_eq!(parse_footprint_bytes(t), Some(1_426_176));
        assert_eq!(
            parse_footprint_bytes("x [1]: 64-bit    Footprint: 12 MB (16384 bytes per page)"),
            None,
            "only -f bytes output is accepted"
        );
        assert_eq!(parse_footprint_bytes(""), None);
    }

    /// The syscall sampler agrees with the `footprint` tool on this (own) process.
    #[cfg(target_os = "macos")]
    #[test]
    fn footprint_syscall_matches_the_tool() {
        let me = std::process::id();
        let a = footprint_kib(me).expect("proc_pid_rusage on our own pid");
        let b = footprint_tool_kib(me).expect("footprint -p on our own pid");
        let c = footprint_kib(me).unwrap();
        assert!(a > 0 && b > 0);
        let (lo, hi) = (a.min(c), a.max(c));
        // The test binary's footprint moves while other tests run: allow 25% around the two reads.
        assert!(
            b * 4 >= lo * 3 && b * 3 <= hi * 4,
            "syscall {a}/{c} KiB vs footprint tool {b} KiB"
        );
        assert_eq!(footprint_kib(u32::MAX >> 1), None, "no such process");
    }

    #[test]
    fn write_result_merges_runs_by_kind_with_sorted_keys() {
        let d = scratch_base().join(format!("unit-{}", std::process::id()));
        let p = d.join("m.json");
        let _ = std::fs::remove_dir_all(&d);
        write_result(
            &p,
            Map::new(),
            vec![summary("a", "s", &[1.0]), summary("b", "s", &[2.0])],
            Map::new(),
        )
        .unwrap();
        let mut extra = Map::new();
        extra.insert("notes".into(), json!(["n"]));
        let v = write_result(&p, Map::new(), vec![summary("a", "s", &[5.0])], extra).unwrap();
        let kinds: Vec<&str> = v["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["b", "a"]);
        assert_eq!(v["runs"][1]["median"], 5.0);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.find("\"notes\"").unwrap() < text.find("\"runs\"").unwrap(),
            "sorted keys"
        );
        remove_exact(&d);
        assert!(!d.exists());
    }
}
