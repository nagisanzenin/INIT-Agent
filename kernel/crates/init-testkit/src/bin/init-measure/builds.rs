//! M-1 (body and workspace build times) and M-2 (T1 component rebuild), testing.md §3.11.
//!
//! One invocation measures one subject: a warm-up build (recorded, not counted), `noop` no-op
//! rebuilds, then `n` rebuilds each after `touch <file>` (an mtime-only change, so no source owned
//! by another lane is edited). Every command runs under `/usr/bin/time -l` with
//! `CARGO_INCREMENTAL=0` and the given `CARGO_TARGET_DIR` (the orchestrator's disk rule: reuse the
//! existing target dirs). A run whose cargo output shows a lock wait is flagged, because another
//! lane building in the same target dir would inflate it.

use crate::common;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct Spec {
    pub label: String,
    pub dir: PathBuf,
    pub target: PathBuf,
    pub cargo: Vec<String>,
    pub touch: Option<PathBuf>,
    pub n: usize,
    pub noop: usize,
    /// M-2: the built `.wasm`, for its size.
    pub wasm: Option<PathBuf>,
    /// Record `du -sk` of the target dir (slow on a multi-GB dir).
    pub du: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timed {
    pub wall_s: f64,
    pub user_s: f64,
    pub sys_s: f64,
    pub max_rss_kib: u64,
    pub recompiled: bool,
    pub lock_wait: bool,
    pub ok: bool,
}

/// Parses macOS `/usr/bin/time -l` output (`<real> real <user> user <sys> sys` and
/// `<bytes>  maximum resident set size`).
pub fn parse_time_l(stderr: &str) -> (Option<f64>, Option<f64>, Option<f64>, Option<u64>) {
    let (mut real, mut user, mut sys, mut rss) = (None, None, None, None);
    for line in stderr.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() >= 6 && w[1] == "real" && w[3] == "user" && w[5] == "sys" {
            real = w[0].parse().ok();
            user = w[2].parse().ok();
            sys = w[4].parse().ok();
        }
        if line.contains("maximum resident set size") {
            rss = w
                .first()
                .and_then(|x| x.parse::<u64>().ok())
                .map(|b| b / 1024);
        }
    }
    (real, user, sys, rss)
}

fn timed(spec: &Spec) -> Result<(Timed, String), String> {
    let o = Command::new("/usr/bin/time")
        .arg("-l")
        .arg("cargo")
        .args(&spec.cargo)
        .current_dir(&spec.dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &spec.target)
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn /usr/bin/time cargo: {e}"))?;
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let (real, user, sys, rss) = parse_time_l(&err);
    let t = Timed {
        wall_s: real.ok_or_else(|| format!("no `real` line in /usr/bin/time output:\n{err}"))?,
        user_s: user.unwrap_or(0.0),
        sys_s: sys.unwrap_or(0.0),
        max_rss_kib: rss.unwrap_or(0),
        recompiled: err.contains("Compiling "),
        lock_wait: err.contains("Blocking waiting for file lock"),
        ok: o.status.success(),
    };
    Ok((t, err))
}

fn touch(p: &Path) -> Result<(), String> {
    let st = Command::new("/usr/bin/touch")
        .arg(p)
        .status()
        .map_err(|e| format!("touch: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("touch {} failed", p.display()))
    }
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn group(kind: &str, ts: &[Timed]) -> Value {
    let walls: Vec<f64> = ts.iter().map(|t| t.wall_s).collect();
    let mut s = common::summary(kind, "s", &walls);
    let rss: Vec<f64> = ts.iter().map(|t| t.max_rss_kib as f64 / 1024.0).collect();
    s["max_rss_mib_median"] = json!(common::median(&rss).map(common::r3));
    s["user_s_median"] =
        json!(common::median(&ts.iter().map(|t| t.user_s).collect::<Vec<_>>()).map(common::r3));
    s["recompiled"] = json!(ts.iter().filter(|t| t.recompiled).count());
    s["lock_waits"] = json!(ts.iter().filter(|t| t.lock_wait).count());
    s
}

/// Runs one subject; returns the `runs[]` entries and extra facts.
pub fn run(spec: &Spec) -> Result<(Vec<Value>, Map<String, Value>), String> {
    common::disk_guard()?;
    let (warm, err) = timed(spec)?;
    if !warm.ok {
        return Err(format!(
            "{}: warm-up build failed:\n{}",
            spec.label,
            tail(&err, 30)
        ));
    }
    eprintln!(
        "{}: warm-up {:.2} s (recompiled: {})",
        spec.label, warm.wall_s, warm.recompiled
    );
    let mut noops = Vec::new();
    for i in 0..spec.noop {
        let (t, err) = timed(spec)?;
        if !t.ok {
            return Err(format!(
                "{}: no-op build {i} failed:\n{}",
                spec.label,
                tail(&err, 30)
            ));
        }
        eprintln!("{}: no-op {} {:.2} s", spec.label, i + 1, t.wall_s);
        noops.push(t);
    }
    let mut touched = Vec::new();
    if let Some(f) = &spec.touch {
        for i in 0..spec.n {
            common::disk_guard()?;
            touch(f)?;
            let (t, err) = timed(spec)?;
            if !t.ok {
                return Err(format!(
                    "{}: touch build {i} failed:\n{}",
                    spec.label,
                    tail(&err, 30)
                ));
            }
            eprintln!(
                "{}: touch {} {:.2} s (recompiled: {}, lock wait: {})",
                spec.label,
                i + 1,
                t.wall_s,
                t.recompiled,
                t.lock_wait
            );
            touched.push(t);
        }
    }
    let mut runs = vec![group(
        &format!("{}.warmup", spec.label),
        std::slice::from_ref(&warm),
    )];
    if !noops.is_empty() {
        runs.push(group(&format!("{}.noop", spec.label), &noops));
    }
    if !touched.is_empty() {
        let mut g = group(&format!("{}.touch", spec.label), &touched);
        g["touched"] = json!(spec.touch.as_ref().map(|p| rel(p)));
        runs.push(g);
    }
    let mut extra = Map::new();
    let mut facts = json!({
        "cargo": spec.cargo.join(" "),
        "dir": rel(&spec.dir),
        "target_dir": rel(&spec.target),
        "env": "CARGO_INCREMENTAL=0",
    });
    if let Some(w) = &spec.wasm {
        facts["wasm"] = json!(rel(w));
        facts["wasm_bytes"] = json!(std::fs::metadata(w).ok().map(|m| m.len()));
    }
    if spec.du {
        facts["target_dir_kib"] = json!(common::du_kib(&spec.target));
    }
    extra.insert(format!("subject.{}", spec.label), facts);
    Ok((runs, extra))
}

/// A path relative to the repo when it is inside it.
pub fn rel(p: &Path) -> String {
    let r = common::repo();
    p.strip_prefix(&r)
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string_lossy().into_owned())
}

/// M-2 step 2: stage `fixtures/ext/<name>/` as `body/ext/<name>/` of a throwaway body tree under
/// the WP16 scratch dir, with the seed's extension workspace manifest and lock, the seed's WIT at
/// `body/wit/` (the crate's `../../wit/init-ext/v1`), and a cargo config naming the repo's
/// `vendor/` by its absolute path, so the shared `seed/target` keeps its compiled dependencies.
pub fn stage_fixture(fixture: &Path) -> Result<PathBuf, String> {
    let r = common::repo();
    let name = fixture
        .file_name()
        .ok_or("fixture has no name")?
        .to_string_lossy()
        .into_owned();
    let body = common::scratch_base().join("m2/body");
    common::remove_exact(&body);
    let ext = body.join("ext");
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(ext.join(&name).join("src")).map_err(io)?;
    for f in ["Cargo.toml", "Cargo.lock"] {
        std::fs::copy(r.join("seed/ext").join(f), ext.join(f)).map_err(io)?;
    }
    std::fs::copy(
        fixture.join("Cargo.toml"),
        ext.join(&name).join("Cargo.toml"),
    )
    .map_err(io)?;
    std::fs::copy(
        fixture.join("src/lib.rs"),
        ext.join(&name).join("src/lib.rs"),
    )
    .map_err(io)?;
    copy_dir(&r.join("seed/wit"), &body.join("wit")).map_err(io)?;
    std::fs::create_dir_all(body.join(".cargo")).map_err(io)?;
    std::fs::write(
        body.join(".cargo/config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = {:?}\n[net]\noffline = true\n",
            r.join("vendor").to_string_lossy()
        ),
    )
    .map_err(io)?;
    Ok(ext)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_time_l() {
        let s = "   Compiling x v0.1.0\n        0.38 real         0.30 user         0.07 sys\n            52314112  maximum resident set size\n                   0  average shared memory size\n";
        let (r, u, sy, rss) = parse_time_l(s);
        assert_eq!((r, u, sy), (Some(0.38), Some(0.30), Some(0.07)));
        assert_eq!(rss, Some(51088));
    }

    #[test]
    fn missing_lines_give_none() {
        assert_eq!(parse_time_l("nothing here"), (None, None, None, None));
    }
}
