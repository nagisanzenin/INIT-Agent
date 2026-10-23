//! `init-mockllm`: the deterministic $0 model server (testing.md §3.4; script format: body/provider.md §3.12).
//!
//! ```text
//! init-mockllm --mode upstream --listen 127.0.0.1:0 --port-file $RUN/mock.port \
//!              --script scripts/a3.mock.jsonl [--tasks-dir scripts/a3/tasks/] --record-dir $RUN/mock
//!              [--record-bodies all|none] [--strict deepseek] [--simulate-cache [--cache-unit-tokens 64]]
//!              [--no-probe-responder] [--no-g3-responder]
//! init-mockllm --mode socket --socket $RUN/proxy.sock --script … --record-dir …
//! ```
//!
//! `--socket` also works in upstream mode (a `unix:` upstream). The server runs until it is killed.
//! Exit codes: 64 usage, 66 script unreadable or invalid, 74 bind/IO failure.

use init_testkit::mockllm::http::{Listen, MockServer};
use init_testkit::mockllm::{Mock, MockConfig, Mode, Script};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: init-mockllm [--mode upstream|socket] [--listen HOST:PORT] [--port-file PATH] \
[--socket PATH] [--script PATH] [--tasks-dir DIR] [--record-dir DIR] [--record-bodies all|none] \
[--strict deepseek] [--simulate-cache] [--cache-unit-tokens N] [--no-probe-responder] [--no-g3-responder]";

#[derive(Debug, Default)]
struct Args {
    mode: Option<Mode>,
    listen: Option<String>,
    port_file: Option<PathBuf>,
    socket: Option<PathBuf>,
    script: Option<PathBuf>,
    tasks_dir: Option<PathBuf>,
    record_dir: Option<PathBuf>,
    record_bodies: bool,
    strict: bool,
    simulate_cache: bool,
    cache_unit: Option<u64>,
    no_probe: bool,
    no_g3: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        record_bodies: true,
        ..Args::default()
    };
    let mut it = argv.iter();
    while let Some(flag) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--mode" => {
                a.mode = Some(match val()?.as_str() {
                    "upstream" => Mode::Upstream,
                    "socket" => Mode::Socket,
                    m => return Err(format!("unknown mode {m:?}")),
                })
            }
            "--listen" => a.listen = Some(val()?),
            "--port-file" => a.port_file = Some(val()?.into()),
            "--socket" => a.socket = Some(val()?.into()),
            "--script" => a.script = Some(val()?.into()),
            "--tasks-dir" => a.tasks_dir = Some(val()?.into()),
            "--record-dir" => a.record_dir = Some(val()?.into()),
            "--record-bodies" => {
                a.record_bodies = match val()?.as_str() {
                    "all" => true,
                    "none" => false,
                    v => return Err(format!("--record-bodies takes all or none, not {v:?}")),
                }
            }
            "--strict" => match val()?.as_str() {
                "deepseek" => a.strict = true,
                v => return Err(format!("--strict supports only deepseek, not {v:?}")),
            },
            "--simulate-cache" => a.simulate_cache = true,
            "--cache-unit-tokens" => {
                let n: u64 = val()?
                    .parse()
                    .map_err(|_| "--cache-unit-tokens needs a positive integer")?;
                if n == 0 {
                    return Err("--cache-unit-tokens needs a positive integer".into());
                }
                a.cache_unit = Some(n);
            }
            "--no-probe-responder" => a.no_probe = true,
            "--no-g3-responder" => a.no_g3 = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(a)
}

/// The default tasks dir for `scripts/<name>.mock.jsonl` is `scripts/<name>/tasks/` (testing.md §3.3).
fn default_tasks_dir(script: &Path) -> Option<PathBuf> {
    let name = script.file_name()?.to_str()?.strip_suffix(".mock.jsonl")?;
    let d = script.parent()?.join(name).join("tasks");
    d.is_dir().then_some(d)
}

fn write_port_file(path: &Path, port: u16) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, format!("{port}\n"))?;
    std::fs::rename(&tmp, path)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("init-mockllm: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(if e.is_empty() { 0 } else { 64 });
        }
    };
    let mode = args.mode.unwrap_or(Mode::Upstream);
    let script = match &args.script {
        Some(p) => match Script::load(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("init-mockllm: script: {e}");
                return ExitCode::from(66);
            }
        },
        None => Script::empty(),
    };
    let mut cfg = MockConfig::new(mode, script);
    cfg.tasks_dir = args
        .tasks_dir
        .clone()
        .or_else(|| args.script.as_deref().and_then(default_tasks_dir));
    cfg.record_dir = args.record_dir.clone();
    cfg.record_bodies = args.record_bodies;
    cfg.strict_deepseek = args.strict;
    cfg.simulate_cache = args.simulate_cache;
    if let Some(u) = args.cache_unit {
        cfg.cache_unit_tokens = u;
    }
    cfg.probe_responder = !args.no_probe;
    cfg.g3_responder = !args.no_g3;

    let listen = match (&args.socket, &args.listen, mode) {
        (Some(_), Some(_), _) => {
            eprintln!("init-mockllm: give --socket or --listen, not both\n{USAGE}");
            return ExitCode::from(64);
        }
        (Some(p), None, _) => {
            if p.as_os_str().len() > 103 {
                eprintln!(
                    "init-mockllm: socket path longer than 103 bytes: {}",
                    p.display()
                );
                return ExitCode::from(64);
            }
            Listen::Unix(p.clone())
        }
        (None, Some(l), _) => Listen::Tcp(l.clone()),
        (None, None, Mode::Upstream) => Listen::Tcp("127.0.0.1:0".into()),
        (None, None, Mode::Socket) => {
            eprintln!("init-mockllm: socket mode needs --socket\n{USAGE}");
            return ExitCode::from(64);
        }
    };
    let mock = match Mock::new(cfg) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("init-mockllm: record dir: {e}");
            return ExitCode::from(74);
        }
    };
    let server = match MockServer::start(mock, listen) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("init-mockllm: bind: {e}");
            return ExitCode::from(74);
        }
    };
    if let (Some(pf), Some(port)) = (&args.port_file, server.port())
        && let Err(e) = write_port_file(pf, port)
    {
        eprintln!("init-mockllm: port file {}: {e}", pf.display());
        return ExitCode::from(74);
    }
    match (server.port(), server.socket_path()) {
        (Some(p), _) => eprintln!("init-mockllm: listening on 127.0.0.1:{p}"),
        (_, Some(s)) => eprintln!("init-mockllm: listening on {}", s.display()),
        _ => {}
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Args, String> {
        parse_args(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn parses_the_documented_flags() {
        let a = args("--mode socket --socket /tmp/x.sock --script s.mock.jsonl --record-dir r --record-bodies none \
                      --strict deepseek --simulate-cache --cache-unit-tokens 32 --no-probe-responder --no-g3-responder")
        .unwrap();
        assert_eq!(a.mode, Some(Mode::Socket));
        assert!(a.strict && a.simulate_cache && a.no_probe && a.no_g3 && !a.record_bodies);
        assert_eq!(a.cache_unit, Some(32));
    }

    #[test]
    fn rejects_bad_flags() {
        assert!(args("--strict anthropic").is_err());
        assert!(args("--mode both").is_err());
        assert!(args("--cache-unit-tokens 0").is_err());
        assert!(args("--listen").is_err());
        assert!(args("--frobnicate").is_err());
    }
}
