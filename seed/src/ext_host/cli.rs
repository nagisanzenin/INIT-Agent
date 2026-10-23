//! `init-body ext describe|call|inspect` (ext/host.md §4.5, a proposal for body/boot.md): run a
//! component with the exact host configuration, without touching the live registry.
//!
//! Wiring is boot's (WP13): `init-body ext <args…>` → [`main`]. Output is one JSON object with
//! sorted keys on stdout. Exit 0 if the operation ran (even if the tool returned an error), 2 for
//! usage errors, 1 for load errors.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use wasmtime::component::Component;

use super::dispatch::{chain, kind_name};
use super::registry::Origin;
use super::state::GuestRoots;
use super::{ExtHost, ExtPaths};

pub const USAGE: &str = "usage: init-body ext describe <wasm>\n       init-body ext call <wasm> --input '<json>' [--workspace <dir>] [--data <dir>]\n       init-body ext inspect <wasm>";

/// `args` are the words after `ext`.
pub fn main(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (code, out) = run(&args);
    match out {
        Some(v) => println!("{v}"),
        None => eprintln!("{USAGE}"),
    }
    ExitCode::from(code)
}

/// (exit code, stdout JSON or None for a usage error).
pub fn run(args: &[String]) -> (u8, Option<Value>) {
    let Some((cmd, rest)) = args.split_first() else {
        return (2, None);
    };
    let Some((wasm, opts)) = rest.split_first() else {
        return (2, None);
    };
    let wasm = PathBuf::from(wasm);
    match cmd.as_str() {
        "describe" if opts.is_empty() => describe(&wasm),
        "inspect" if opts.is_empty() => inspect(&wasm),
        "call" => {
            let mut input = None;
            let mut workspace = None;
            let mut data = None;
            let mut it = opts.iter();
            while let Some(o) = it.next() {
                let slot = match o.as_str() {
                    "--input" => &mut input,
                    "--workspace" => &mut workspace,
                    "--data" => &mut data,
                    _ => return (2, None),
                };
                match it.next() {
                    Some(v) => *slot = Some(v.clone()),
                    None => return (2, None),
                }
            }
            match input {
                Some(i) => call(
                    &wasm,
                    &i,
                    workspace.map(PathBuf::from),
                    data.map(PathBuf::from),
                ),
                None => (2, None),
            }
        }
        _ => (2, None),
    }
}

fn host() -> Result<Arc<ExtHost>, String> {
    let paths = ExtPaths::from_env().unwrap_or_else(|_| ExtPaths::from_home(Path::new("/"), None));
    ExtHost::build(paths)
}

fn err(e: impl Into<String>) -> (u8, Option<Value>) {
    (1, Some(json!({"error": e.into()})))
}

fn describe(wasm: &Path) -> (u8, Option<Value>) {
    let h = match host() {
        Ok(h) => h,
        Err(e) => return err(e),
    };
    match h.load_one(None, wasm, Origin::Trial, None) {
        Ok((v, _)) => (
            0,
            Some(json!({
                "api": format!("init:ext@{}", v.api),
                "compile_ms": v.compile_ms,
                "info": {
                    "description": v.info.description,
                    "input_schema": v.info.input_schema,
                    "name": v.info.name,
                    "timeout_ms": v.info.timeout_ms,
                    "version": v.info.version,
                },
            })),
        ),
        Err(e) => err(e),
    }
}

fn call(
    wasm: &Path,
    input: &str,
    workspace: Option<PathBuf>,
    data: Option<PathBuf>,
) -> (u8, Option<Value>) {
    let input: Value = match serde_json::from_str(input) {
        Ok(v @ Value::Object(_)) => v,
        _ => return (2, None),
    };
    let h = match host() {
        Ok(h) => h,
        Err(e) => return err(e),
    };
    let ver = match h.load_one(None, wasm, Origin::Trial, None) {
        Ok((v, _)) => v,
        Err(e) => return err(e),
    };
    let workspace = match workspace.map(Ok).unwrap_or_else(std::env::current_dir) {
        Ok(w) => w,
        Err(e) => return err(format!("workspace: {e}")),
    };
    let data = match data {
        Some(d) => d,
        None => match ExtPaths::from_env() {
            Ok(p) => p.data_dir(&ver.name),
            Err(_) => return (2, None), // no INIT_HOME and no --data
        },
    };
    if let Err(e) = std::fs::create_dir_all(&data) {
        return err(format!("{}: {e}", data.display()));
    }
    let abort = AtomicBool::new(false);
    let (o, log) = h.call_version(
        &ver,
        input.to_string(),
        GuestRoots {
            work: workspace,
            data,
        },
        &abort,
    );
    (
        0,
        Some(json!({
            "elapsed_ms": o.elapsed_ms,
            "is_error": o.is_error,
            "kind": kind_name(o.kind),
            "log": String::from_utf8_lossy(&log),
            "text": o.text,
        })),
    )
}

fn inspect(wasm: &Path) -> (u8, Option<Value>) {
    let h = match host() {
        Ok(h) => h,
        Err(e) => return err(e),
    };
    let bytes = match std::fs::read(wasm) {
        Ok(b) => b,
        Err(e) => return err(format!("{}: {e}", wasm.display())),
    };
    let c = match Component::from_binary(&h.engine, &bytes) {
        Ok(c) => c,
        Err(e) => return err(format!("compile failed: {}", chain(&e))),
    };
    let ty = c.component_type();
    let mut imports: Vec<String> = ty.imports(&h.engine).map(|(n, _)| n.to_string()).collect();
    let mut exports: Vec<String> = ty.exports(&h.engine).map(|(n, _)| n.to_string()).collect();
    imports.sort();
    exports.sort();
    (0, Some(json!({"exports": exports, "imports": imports})))
}
