//! `initctl`: the agent's client for the kernel socket (kernel/initctl.md §4.5).
//!
//! A thin client: it serializes one request, sends it over a Unix socket, prints the answer and
//! exits with a sysexits-style code. std + `init-proto` only: no async runtime, no network crates.

mod cli;
mod client;
mod gitsnap;

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let code = match cli::parse(&argv, &cwd) {
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            0
        }
        Ok(cli::Parsed::Version) => {
            println!("initctl {} (protocol v{})", env!("CARGO_PKG_VERSION"), init_proto::PROTO);
            0
        }
        Ok(cli::Parsed::Run(opts, cmd)) => {
            let env = |k: &str| std::env::var(k).ok();
            let stdout = std::io::stdout();
            let stderr = std::io::stderr();
            client::run(&opts, &cmd, &env, &mut stdout.lock(), &mut stderr.lock())
        }
        Err(e) => {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "initctl: {}", e.message);
            if e.code == init_proto::errors::exit::USAGE {
                let _ = writeln!(err, "run `initctl --help` for usage");
            }
            e.code
        }
    };
    ExitCode::from(code)
}
