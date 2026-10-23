//! `init-kernel`: the thin kernel binary. argv dispatch over the lane library crates
//! (supervise, proxy, gate, selfmap, serve, sandbox). Std argument parsing, no clap.
//! `supervise` also runs the initctl server (WP7b, [`serve`]).
use std::process::ExitCode;

mod evo;
mod proxy;
mod serve;
mod upgrade;

/// The lane crates linked into this binary, in dispatch order.
const LANES: [&str; 7] = [
    init_proto::CRATE,
    kernel_supervise::CRATE,
    kernel_proxy::CRATE,
    kernel_gate::CRATE,
    kernel_selfmap::CRATE,
    kernel_serve::CRATE,
    kernel_sandbox::CRATE,
];

/// Leading `-c <key>=<value>` pairs (configuration overrides, read by `gate` only), then the rest.
fn split_overrides(args: &[String]) -> Result<(Vec<String>, &[String]), ()> {
    let mut o = Vec::new();
    let mut i = 0;
    while args.get(i).map(String::as_str) == Some("-c") {
        o.push(args.get(i + 1).ok_or(())?.clone());
        i += 2;
    }
    Ok((o, &args[i..]))
}

fn main() -> ExitCode {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let Ok((overrides, args)) = split_overrides(&all) else {
        eprintln!("init-kernel: -c needs <key>=<value>");
        return ExitCode::from(64);
    };
    if !overrides.is_empty() && args.first().map(String::as_str) != Some("gate") {
        eprintln!("init-kernel: -c applies to `gate` only");
        return ExitCode::from(64);
    }
    match args.first().map(String::as_str) {
        // kernel/supervise.md §4.8: the kernel daemon, started only by the Root (fds 3 and 4).
        // WP7b: plus the initctl server (run/kernel.sock, probe lanes) once the supervisor runs.
        Some("supervise") => {
            let proxy = std::sync::Arc::new(proxy::KernelProxy::default());
            let slot = serve::Slot::default();
            let (p2, s2) = (proxy.clone(), slot.clone());
            let code = kernel_supervise::supervisor::main_under_root_started(args[1..].to_vec(), proxy, move |h| serve::spawn(h, p2, s2));
            serve::shutdown(&slot);
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        // §4.8: install mode, spawned by `init-root install` (fds 3 and 4, reason `install`).
        Some("install") => {
            let code = kernel_supervise::install::main_install(args[1..].to_vec());
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        // kernel/selfmap.md §4: `init-kernel selfmap …` (G1 runs it in the build sandbox; WP9).
        Some("selfmap") => {
            let rest: Vec<std::ffi::OsString> = std::env::args_os().skip(2).collect();
            let exe = std::env::current_exe().map(|p| p.into_os_string()).unwrap_or_else(|_| "init-kernel".into());
            let code = kernel_selfmap::cli::run(&rest, &[exe, "selfmap".into()]);
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        // kernel/gate.md §4.1a: `init-kernel gate judge …`, run by the Root's gate-of-gates in its sealed
        // environment (no live proxy, no traces; WP8c).
        Some("gate") if args.get(1).map(String::as_str) == Some("judge") => {
            let code = kernel_gate::judge::cli(&args[2..], &overrides);
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        Some("--version") if args.len() == 1 => {
            println!("init-kernel {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: init-kernel supervise --under-root | install --from-bundle <bundle> [--seed-tag <tag>] | selfmap … | [-c k=v]… gate judge … | --version   (linked: {})", LANES.join(", "));
            ExitCode::from(64) // EX_USAGE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn leading_overrides_are_split_off() {
        let a: Vec<String> = ["-c", "gate.g4.min_pool=4", "-c", "x=1", "gate", "judge"].iter().map(|s| s.to_string()).collect();
        let (o, rest) = super::split_overrides(&a).unwrap();
        assert_eq!((o.len(), rest.len(), rest[0].as_str()), (2, 2, "gate"));
        assert!(super::split_overrides(&["-c".to_string()]).is_err());
    }

    #[test]
    fn links_every_lane_crate() {
        assert_eq!(super::LANES.len(), 7);
        assert!(super::LANES.iter().all(|c| !c.is_empty()));
    }
}
