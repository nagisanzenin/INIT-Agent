#![forbid(unsafe_code)]
//! `init-body`: the seed body (body/boot.md). `main` is argv dispatch, the panic hook and the
//! boot sequence (`boot::run`); WP10-WP14 own the loop, provider, context, log, tools and ext host.
use std::process::ExitCode;

// WP13: the boot sequence (body/boot.md) and the awareness texts (awareness.md).
#[allow(dead_code)]
mod boot;
// WP12: the event log, context assembly and the in-house SHA-256. WP11/WP13 wire them into
// the loop and boot; until then only tests reach them, hence the dead_code allowances.
#[allow(dead_code)]
mod context;
#[allow(dead_code)]
mod log;
#[allow(dead_code)]
mod sha256;
// Normalized model I/O types shared by loop, tools, context and provider (body/provider.md §3.2).
#[allow(dead_code)]
mod model;
// WP11: the agent loop (body/loop.md) and the tools (body/tools.md).
#[allow(dead_code)]
mod r#loop;
#[allow(dead_code)]
mod tools;
// WP14: the extension host (ext/host.md): Wasmtime engine, registry, hot swap, `ext` dispatch.
#[allow(dead_code)]
mod ext_host;
// WP10: the provider (HTTP/1.1 + SSE client to the proxy socket, adapters, retries, model table).
#[allow(dead_code)]
mod provider;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        None => {}
        // body/boot.md §4.3: no env, no fs, no sockets.
        Some("--manifest") if args.len() == 1 => return boot::manifest::print(),
        Some("--version") if args.len() == 1 => {
            println!("init-body {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        // ext/host.md §4.5: describe | call | inspect.
        Some("ext") => return boot::ext::cli(&args[1..]),
        _ => {
            eprintln!("usage: init-body [--manifest | --version | ext <describe|call|inspect> …]");
            return ExitCode::from(64); // EX_USAGE
        }
    }
    // One JSON line on stderr, then unwind as usual (a main-thread panic exits 101).
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let th = std::thread::current().name().unwrap_or("?").to_string();
        eprintln!(
            "{}",
            serde_json::json!({"type": "panic", "message": msg, "location": loc, "thread": th})
        );
    }));
    match boot::run() {
        Ok(code) => ExitCode::from(code), // from Session::serve(): 0, 75, or 1 on a log failure
        Err(e) => {
            if e.code == 1 {
                eprintln!("{}", e.message); // log-context §3.4.1's plain line
            }
            eprintln!("{}", e.to_json_line());
            ExitCode::from(e.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    /// The vendored Wasmtime builds with ext/host.md §3.2's features and an engine starts.
    #[test]
    fn wasmtime_engine_starts() {
        let engine = wasmtime::Engine::default();
        let _linker: wasmtime::component::Linker<()> = wasmtime::component::Linker::new(&engine);
    }

    /// serde_json without `preserve_order`: object keys come out sorted (ICJ).
    #[test]
    fn json_keys_sorted() {
        let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":2}"#).unwrap();
        assert_eq!(v.to_string(), r#"{"a":2,"b":1}"#);
    }
}
