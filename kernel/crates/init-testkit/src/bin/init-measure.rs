//! `init-measure`: the Phase 0 measurements (testing.md §3.11), $0, mock only (WP16).
//!
//! ```text
//! init-measure env [--note TEXT]
//! init-measure m1 --label L --dir DIR --target-dir DIR [--touch FILE] [--n 5] [--noop 3] [--du]
//!                 [--out measurements/m1.json] [--note TEXT] -- <cargo args…>
//! init-measure m2 (as m1) [--stage-fixture fixtures/ext/semver-sort] [--wasm PATH]
//! init-measure gen-steps [--steps 1000] [--output-bytes 2048] [--read-every 100] [--seed 7]
//! init-measure m4 [--handoffs 5] [--body BIN] [--mock BIN] [--keep] [--out measurements/m4.json]
//! init-measure m4-a3 [--runs 1] [--keep] [--out measurements/m4.json]
//! init-measure m5 [--steps 1000] [--output-bytes 2048] [--read-every 100] [--seed 7] [--sample-ms 200]
//!                 [--metric footprint|rss] [--baseline epoch|STEP] [--trials 1]
//!                 [--body BIN] [--mock BIN] [--limit-s 3600] [--keep] [--out measurements/m5.json]
//! init-measure m5-reanalyze --in FILE [--metric footprint|rss] [--baseline epoch|STEP]
//! ```
//!
//! M-5 defaults (provisional owner decision 2026-10-20): `--metric footprint` on macOS (`rss`
//! elsewhere) and `--baseline epoch` (the first `mask`/`compaction` in the session log). The old
//! rule is `--metric rss --baseline 100`; every run also reports all four combinations.
//!
//! Results go to `measurements/<m>.json` with sorted keys: `{machine, toolchain, load, runs:[{kind,
//! n, median, min, max, unit}], notes, …}`; repeated invocations merge by run `kind`. Paths are
//! relative to the repo root (`kernel/crates/init-testkit/../../..`). Builds use the existing
//! target dirs with `CARGO_INCREMENTAL=0`; homes live under `target/test-homes/wp16/` and are
//! removed by exact path; every process is this tool's own child. A disk guard stops below 5.3 GiB.
//!
//! Exit codes: 0 done (and passed, for m4/m5), 1 measured but over the threshold, 2 error,
//! 64 usage. M-3 (real-model component trials) is not implemented here: it needs the key and money.

#[path = "init-measure/builds.rs"]
mod builds;
#[path = "init-measure/common.rs"]
mod common;
#[path = "init-measure/m4.rs"]
mod m4;
#[path = "init-measure/m5.rs"]
mod m5;
#[path = "init-measure/rig.rs"]
mod rig;

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: init-measure env|m1|m2|gen-steps|m4|m4-a3|m5|m5-reanalyze [options] (see the module docs)";

struct Args {
    opts: Vec<String>,
    rest: Vec<String>,
}

impl Args {
    fn new(v: &[String]) -> Args {
        match v.iter().position(|a| a == "--") {
            Some(i) => Args {
                opts: v[..i].to_vec(),
                rest: v[i + 1..].to_vec(),
            },
            None => Args {
                opts: v.to_vec(),
                rest: Vec::new(),
            },
        }
    }
    fn get(&self, name: &str) -> Option<String> {
        self.opts
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.opts.get(i + 1).cloned())
    }
    fn flag(&self, name: &str) -> bool {
        self.opts.iter().any(|a| a == name)
    }
    fn num(&self, name: &str, default: u64) -> Result<u64, String> {
        match self.get(name) {
            Some(s) => s
                .parse()
                .map_err(|_| format!("{name} needs a number, not {s:?}")),
            None => Ok(default),
        }
    }
    fn path(&self, name: &str) -> Option<PathBuf> {
        self.get(name).map(|p| abs(&p))
    }
}

/// A path relative to the repo root unless absolute.
fn abs(p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        common::repo().join(p)
    }
}

fn default_body() -> PathBuf {
    common::scratch_base().join("bin/init-body")
}

fn default_mock() -> PathBuf {
    common::scratch_base().join("bin/init-mockllm")
}

fn m5_metric(a: &Args) -> Result<m5::Metric, String> {
    a.get("--metric")
        .map(|m| m5::Metric::parse(&m))
        .unwrap_or(Ok(m5::Metric::default_for_os()))
}

fn m5_baseline(a: &Args) -> Result<m5::Baseline, String> {
    a.get("--baseline")
        .map(|b| m5::Baseline::parse(&b))
        .unwrap_or(Ok(m5::Baseline::FirstEpoch))
}

fn notes(a: &Args) -> Value {
    json!(a.get("--note").into_iter().collect::<Vec<_>>())
}

fn write(
    out: &Path,
    a: &Args,
    runs: Vec<Value>,
    mut extra: Map<String, Value>,
) -> Result<(), String> {
    let mut doc_notes: Vec<Value> = std::fs::read(out)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["notes"].as_array().cloned())
        .unwrap_or_default();
    for n in notes(a).as_array().into_iter().flatten() {
        if !doc_notes.contains(n) {
            doc_notes.push(n.clone());
        }
    }
    extra.insert("notes".into(), Value::Array(doc_notes));
    common::write_result(
        out,
        common::environment(a.get("--note").as_deref()),
        runs,
        extra,
    )
    .map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

fn builds(a: &Args, m: &str) -> Result<ExitCode, String> {
    let dir = match a.path("--stage-fixture") {
        Some(f) => builds::stage_fixture(&f)?,
        None => a
            .path("--dir")
            .ok_or("--dir (or --stage-fixture) is required")?,
    };
    let touch = match a.get("--touch") {
        Some(t) if a.flag("--stage-fixture") => Some(dir.join(t)),
        Some(t) => Some(abs(&t)),
        None => None,
    };
    let spec = builds::Spec {
        label: a.get("--label").ok_or("--label is required")?,
        dir: dir.clone(),
        target: a.path("--target-dir").ok_or("--target-dir is required")?,
        cargo: if a.rest.is_empty() {
            vec!["build".into(), "--offline".into()]
        } else {
            a.rest.clone()
        },
        touch,
        n: a.num("--n", 5)? as usize,
        noop: a.num("--noop", 3)? as usize,
        wasm: a.path("--wasm"),
        du: a.flag("--du"),
    };
    let (runs, extra) = builds::run(&spec)?;
    let out = a
        .path("--out")
        .unwrap_or_else(|| common::repo().join(format!("measurements/{m}.json")));
    for r in &runs {
        eprintln!(
            "{}: median {} s (n {}, max {}, recompiled {}, lock waits {})",
            r["kind"], r["median"], r["n"], r["max"], r["recompiled"], r["lock_waits"]
        );
    }
    write(&out, a, runs, extra)?;
    Ok(ExitCode::SUCCESS)
}

fn real_main(argv: &[String]) -> Result<ExitCode, String> {
    let Some(cmd) = argv.first() else {
        eprintln!("{USAGE}");
        return Ok(ExitCode::from(64));
    };
    let a = Args::new(&argv[1..]);
    match cmd.as_str() {
        "env" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&Value::Object(common::environment(
                    a.get("--note").as_deref()
                )))
                .unwrap()
            );
            Ok(ExitCode::SUCCESS)
        }
        "m1" | "m2" => builds(&a, cmd),
        "gen-steps" => {
            print!(
                "{}",
                m5::gen_steps(
                    a.num("--steps", 1000)?,
                    a.num("--output-bytes", 2048)?,
                    a.num("--read-every", 100)?,
                    a.num("--seed", 7)?,
                    200
                )
            );
            Ok(ExitCode::SUCCESS)
        }
        "m4" => {
            let o = m4::Opts {
                handoffs: a.num("--handoffs", 5)? as usize,
                body: a.path("--body").unwrap_or_else(default_body),
                mock: a.path("--mock").unwrap_or_else(default_mock),
                keep: a.flag("--keep"),
            };
            let (runs, extra, pass) = m4::run(&o)?;
            for r in &runs {
                eprintln!(
                    "{}: median {} ms (min {}, max {}, n {})",
                    r["kind"], r["median"], r["min"], r["max"], r["n"]
                );
            }
            eprintln!("m4 body: pass {pass}");
            let out = a
                .path("--out")
                .unwrap_or_else(|| common::repo().join("measurements/m4.json"));
            let mut extra2 = Map::new();
            extra2.insert("body_level".into(), Value::Object(extra));
            write(&out, &a, runs, extra2)?;
            Ok(if pass {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        "m4-a3" => {
            let (runs, extra) = m4::run_a3(a.num("--runs", 1)? as usize, a.flag("--keep"))?;
            for r in &runs {
                eprintln!(
                    "{}: median {} ms (min {}, max {}, n {})",
                    r["kind"], r["median"], r["min"], r["max"], r["n"]
                );
            }
            let out = a
                .path("--out")
                .unwrap_or_else(|| common::repo().join("measurements/m4.json"));
            let mut extra2 = Map::new();
            extra2.insert("kernel_level".into(), Value::Object(extra));
            write(&out, &a, runs, extra2)?;
            Ok(ExitCode::SUCCESS)
        }
        "m5" => {
            let o = m5::Opts {
                steps: a.num("--steps", 1000)?,
                output_bytes: a.num("--output-bytes", 2048)?,
                read_every: a.num("--read-every", 100)?,
                seed: a.num("--seed", 7)?,
                sample_ms: a.num("--sample-ms", 200)?,
                body: a.path("--body").unwrap_or_else(default_body),
                mock: a.path("--mock").unwrap_or_else(default_mock),
                keep: a.flag("--keep"),
                limit_s: a.num("--limit-s", 3600)?,
                metric: m5_metric(&a)?,
                baseline: m5_baseline(&a)?,
            };
            let trials = a.num("--trials", 1)?.max(1);
            let (runs, extra, pass) = if trials == 1 {
                let (runs, extra, pass) = m5::run(&o)?;
                eprintln!("m5: {}", extra["result"]);
                eprintln!("m5 compare: {}", extra["compare"]);
                (runs, extra, pass)
            } else {
                // N sequential sessions, each in a fresh home; the result passes only if all do.
                let (mut runs, mut list, mut passed) = (Vec::new(), Vec::new(), 0u64);
                for i in 1..=trials {
                    let (rs, mut extra, pass) = m5::run(&o)?;
                    eprintln!("m5 trial {i}/{trials}: {}", extra["result"]);
                    eprintln!("m5 trial {i}/{trials} compare: {}", extra["compare"]);
                    for mut r in rs {
                        r["kind"] = json!(format!("{}#{i}", r["kind"].as_str().unwrap_or("m5")));
                        runs.push(r);
                    }
                    extra.insert("trial".into(), json!(i));
                    list.push(Value::Object(extra));
                    passed += pass as u64;
                }
                let mut extra = Map::new();
                extra.insert("trials".into(), Value::Array(list));
                extra.insert(
                    "trials_summary".into(),
                    json!({"n": trials, "passed": passed, "metric": o.metric.as_str(),
                           "baseline": o.baseline.label()}),
                );
                (runs, extra, passed == trials)
            };
            eprintln!("m5: pass {pass}");
            let out = a
                .path("--out")
                .unwrap_or_else(|| common::repo().join("measurements/m5.json"));
            write(&out, &a, runs, extra)?;
            Ok(if pass {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        "m5-reanalyze" => {
            let file = a.path("--in").ok_or("--in FILE is required")?;
            let doc: Value = serde_json::from_slice(
                &std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?,
            )
            .map_err(|e| format!("{}: {e}", file.display()))?;
            let r = m5::reanalyze(&doc, m5_metric(&a)?, m5_baseline(&a)?)?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            Ok(if r["pass"] == true {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        "-h" | "--help" => {
            eprintln!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("init-measure: unknown command {other:?}\n{USAGE}");
            Ok(ExitCode::from(64))
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match real_main(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("init-measure: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_split_options_and_cargo_rest() {
        let v: Vec<String> = [
            "--label",
            "seed",
            "--n",
            "3",
            "--du",
            "--",
            "build",
            "-p",
            "init-body",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let a = Args::new(&v);
        assert_eq!(a.get("--label").as_deref(), Some("seed"));
        assert_eq!(a.num("--n", 5).unwrap(), 3);
        assert_eq!(a.num("--noop", 3).unwrap(), 3);
        assert!(a.flag("--du"));
        assert_eq!(a.rest, ["build", "-p", "init-body"]);
        assert!(
            Args::new(&["--n".into(), "x".into()])
                .num("--n", 1)
                .is_err()
        );
    }

    #[test]
    fn unknown_command_is_usage() {
        assert_eq!(
            real_main(&["frobnicate".into()]).unwrap(),
            ExitCode::from(64)
        );
        assert_eq!(real_main(&[]).unwrap(), ExitCode::from(64));
    }
}
