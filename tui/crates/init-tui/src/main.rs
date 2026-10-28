//! `init-tui`, the owner's terminal client for INIT Agent (docs/tui.md). A client of
//! `ROOT_HOME/run/owner.sock` and nothing more: no model calls, no keys, no files written, no other
//! socket (T-1…T-5, §3.1).
#![forbid(unsafe_code)]

mod access;
mod app;
mod conn;
mod input;
mod keys;
mod line;
mod model;
mod notice;
mod proto;
mod render;
mod setup;
mod term;
mod time;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "usage: init-tui [--root-home DIR] [--plain | --raw] [--no-replay] [--verbose] [--wait] [--wait-idle]
                [--timeout S] [--full | --workspace] [--theme dark|light] [--latency-report] [--version] [--help]

  Access is full by default (ADR-012): the agent may read and write anywhere you can and use the
  network, except INIT's Root (~/.init-root, its service, INIT's launchers). The current directory
  is the workspace: /undo restores the snapshot the Root took of it first; nothing else has /undo.
  Launched in ~ (or outside your home), there is no workspace: the agent then works in its own
  folder (INIT_HOME/work, ~/.init/work by default) and /undo has nothing to restore.

  --root-home DIR    the Root's home (else INIT_ROOT_HOME, else ~/.init-root)
  --plain            line mode even on a TTY
  --raw              line mode printing the frames as JSON lines
  --no-replay        the first attach asks for no replay
  --verbose          thinking text and turn-end lines (TTY mode: Ctrl-T toggles)
  --wait             line mode: wait for an unreachable Root instead of exiting 69
  --wait-idle        line mode: after stdin EOF stay attached until the agent is idle
  --timeout S        line mode: overall bound in seconds (default 600 with --wait-idle)
  --full             full access for this session (the default; the Root asks you to confirm only
                     when the charter sets sandbox.mode = \"workspace\"); /full off ends it
  --workspace        confine this session: write only the workspace and ~/.init, read your home
                     except secrets, no network but the model provider (/full on lifts it)
  --theme dark|light colours for a dark or a light background (default: the terminal's answer to an
                     OSC 11 query, else COLORFGBG, else dark)
  --latency-report   at exit, print latency histograms to stderr
";

#[derive(Debug, Default, PartialEq)]
struct Args {
    root_home: Option<PathBuf>,
    plain: bool,
    raw: bool,
    no_replay: bool,
    verbose: bool,
    wait: bool,
    wait_idle: bool,
    timeout: Option<f64>,
    latency: bool,
    full: bool,
    workspace: bool,
    /// TUI-BRAND: `--theme dark|light` overrides the background detection (§3.9a).
    theme: Option<render::palette::Bg>,
    version: bool,
    help: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(s) = it.next() {
        let (flag, inline) = match s.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (s.as_str(), None),
        };
        let mut val = |name: &str| inline.clone().or_else(|| it.next().cloned()).ok_or_else(|| format!("{name} needs a value"));
        match flag {
            "--root-home" => a.root_home = Some(PathBuf::from(val("--root-home")?)),
            "--plain" => a.plain = true,
            "--raw" => a.raw = true,
            "--no-replay" => a.no_replay = true,
            "--verbose" => a.verbose = true,
            "--wait" => a.wait = true,
            "--wait-idle" => a.wait_idle = true,
            "--timeout" => {
                let v = val("--timeout")?;
                let t: f64 = v.parse().map_err(|_| format!("--timeout: not a number: {v}"))?;
                if !(t > 0.0 && t.is_finite()) {
                    return Err("--timeout must be positive".into());
                }
                a.timeout = Some(t);
            }
            "--latency-report" => a.latency = true,
            "--full" => a.full = true,
            "--workspace" => a.workspace = true,
            "--theme" => {
                a.theme = Some(match val("--theme")?.as_str() {
                    "dark" => render::palette::Bg::Dark,
                    "light" => render::palette::Bg::Light,
                    other => return Err(format!("--theme: dark or light, not {other}")),
                })
            }
            "--version" | "-V" => a.version = true,
            "--help" | "-h" => a.help = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if a.plain && a.raw {
        return Err("--plain and --raw exclude each other".into());
    }
    if a.full && a.workspace {
        return Err("--full and --workspace exclude each other".into());
    }
    Ok(a)
}

/// `--root-home`, else `INIT_ROOT_HOME`, else `~/.init-root` (the rule `init-root` uses).
fn root_home(a: &Args) -> Option<PathBuf> {
    if let Some(p) = &a.root_home {
        return Some(p.clone());
    }
    if let Some(p) = std::env::var_os("INIT_ROOT_HOME").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".init-root"))
}

fn version() -> String {
    let mut s = format!("init-tui (INIT Agent TUI) {}", proto::VERSION);
    if let Some(sha) = option_env!("INIT_TUI_GIT_SHA") {
        s.push_str(&format!(", built from tui/ at {}", &sha[..sha.len().min(12)]));
    }
    s
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("init-tui: {e}\n{USAGE}");
            std::process::exit(64);
        }
    };
    if a.help {
        print!("{USAGE}");
        return;
    }
    if a.version {
        println!("{}", version());
        return;
    }
    let Some(rh) = root_home(&a) else {
        eprintln!("init-tui: no --root-home, INIT_ROOT_HOME or HOME");
        std::process::exit(64);
    };
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let line_mode = a.plain || a.raw || dumb || !tty;
    // TUI-BRAND (§3.9a): the palette from the environment; `--theme` decides the background, else TTY
    // mode asks the terminal (OSC 11) when it enters raw mode
    if let Some(bg) = a.theme {
        term::set_palette(render::palette::Palette { bg, ..term::detect_palette(false) });
    } else if line_mode {
        term::set_palette(term::detect_palette(false));
    }
    // ADR-009: the cwd as the OS gives it; the Root canonicalizes and validates it.
    let cwd = std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned());
    let code = if line_mode {
        let timeout = a.timeout.map(Duration::from_secs_f64).or(a.wait_idle.then(|| Duration::from_secs(600)));
        line::run(line::LineOpts {
            root_home: rh,
            raw: a.raw,
            verbose: a.verbose,
            wait: a.wait,
            wait_idle: a.wait_idle,
            timeout,
            replay: !a.no_replay,
            latency: a.latency,
            style: std::io::stdout().is_terminal() && !no_color && !a.raw,
            prompt: std::io::stdin().is_terminal() && !a.raw,
            cwd,
            full: a.full,
            workspace: a.workspace,
        })
    } else {
        app::run(app::Opts { root_home: rh, replay: !a.no_replay, verbose: a.verbose, latency: a.latency, no_color, cwd, full: a.full, workspace: a.workspace })
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Result<Args, String> {
        parse(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn args() {
        let a = p(&["--root-home", "/x", "--plain", "--wait-idle", "--timeout=5"]).unwrap();
        assert_eq!((a.root_home.unwrap(), a.plain, a.wait_idle, a.timeout), (PathBuf::from("/x"), true, true, Some(5.0)));
        assert!(p(&["--which"]).is_err(), "--which is the launcher's");
        assert!(p(&["--plain", "--raw"]).is_err());
        assert!(p(&["--full", "--workspace"]).is_err());
        assert!(p(&["--workspace"]).unwrap().workspace);
        assert!(p(&["--timeout", "0"]).is_err());
        assert!(p(&["--root-home"]).is_err());
        assert!(p(&["--full"]).unwrap().full);
        assert_eq!(p(&["--theme", "light"]).unwrap().theme, Some(crate::render::palette::Bg::Light));
        assert!(p(&["--theme", "blue"]).is_err());
        assert!(!p(&[]).unwrap().full);
    }
}
