// WP14 test fixture: one guest with many behaviors, selected by `{"op": …}`.
//
// Not an extension of the seed. The ext_host tests write each variant crate into a throwaway
// workspace under target/test-homes/wp14/ (a `Cargo.toml` here would be an unlisted crate for
// gate.md §3.3), prefixing this file with the variant's constants:
//   NAME: &str, SCHEMA: &str, TIMEOUT_MS: Option<u32>, DESCRIBE: u8, MARK: &str
// DESCRIBE: 0 pure, 1 nondeterministic, 2 calls process.run, 3 reads /work/x.
// The prefix also carries the crate's inner attributes.
use serde_json::{Value, json};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// No locals in linear memory: exhausts the native wasm stack (Trap::StackOverflow).
#[allow(unconditional_recursion)]
fn recurse(n: u64) -> u64 {
    std::hint::black_box(recurse(std::hint::black_box(n + 1))) + 1
}

/// 256 bytes of locals per frame on the shadow stack (linear memory): overflows that first.
#[allow(unconditional_recursion)]
fn recurse_big(n: u64) -> u64 {
    let buf = [n; 32];
    std::hint::black_box(&buf);
    recurse_big(std::hint::black_box(n + 1)) + buf[(n % 32) as usize]
}

/// Ops that need no host interface.
fn plain_op(op: &str, v: &Value, input: &str) -> Option<Result<String, (bool, String)>> {
    Some(match op {
        "echo" => Ok(input.to_string()),
        "mark" => Ok(MARK.to_string()),
        "loop" => loop {
            std::hint::black_box(0u8);
        },
        "sleep" => {
            std::thread::sleep(std::time::Duration::from_millis(v["ms"].as_u64().unwrap_or(1000)));
            Ok("slept".into())
        }
        "alloc" => {
            let n = (v["mb"].as_u64().unwrap_or(512) as usize) << 20;
            let mut x: Vec<u8> = Vec::new();
            x.resize(n, 1);
            Ok(format!("{}", std::hint::black_box(&x).len()))
        }
        "recurse" => Ok(format!("{}", recurse(std::hint::black_box(1)))),
        "recurse_big" => Ok(format!("{}", recurse_big(std::hint::black_box(1)))),
        "panic" => panic!("boom"),
        "exit" => std::process::exit(3),
        "fail" => Err((false, "nope".into())),
        "invalid" => Err((true, "bad input".into())),
        "print" => {
            let n = v["n"].as_u64().unwrap_or(10) as usize;
            println!("{}", "x".repeat(n));
            eprintln!("to-stderr");
            Ok("printed".into())
        }
        "bigout" => Ok("y".repeat(v["n"].as_u64().unwrap_or(0) as usize)),
        "read" => Ok(match std::fs::read_to_string(v["path"].as_str().unwrap_or("")) {
            Ok(s) => json!({"ok": s}),
            Err(e) => json!({"err": e.to_string()}),
        }
        .to_string()),
        "write" => Ok(match std::fs::write(
            v["path"].as_str().unwrap_or(""),
            v["data"].as_str().unwrap_or(""),
        ) {
            Ok(()) => json!({"ok": true}),
            Err(e) => json!({"err": e.to_string()}),
        }
        .to_string()),
        "connect" => Ok(match std::net::TcpStream::connect(v["addr"].as_str().unwrap_or("1.1.1.1:80")) {
            Ok(_) => json!({"ok": "connected"}),
            Err(e) => json!({"err": e.to_string()}),
        }
        .to_string()),
        _ => return None,
    })
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: "../../wit/init-ext/v1",
        world: "init:ext/tool",
    });

    use super::{DESCRIBE, MARK, NAME, SCHEMA, TIMEOUT_MS, VERSION, plain_op, strs};
    use init::ext::process::{self, ProcessError, RunRequest, SpawnRequest};
    use serde_json::{Value, json};

    struct Component;

    /// AU-6: the skill's process snippet, verbatim; it must compile against wit-bindgen 0.63.0.
    #[allow(dead_code)]
    fn git_status() -> Result<String, ToolError> {
        let out = process::run(&RunRequest {
            argv: vec!["git".into(), "status".into(), "--porcelain".into()],
            cwd: "/work".into(), env: vec![], stdin: vec![], timeout_ms: 10_000,
        }).map_err(|e| ToolError::Failed(format!("{e:?}")))?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// AU-6: the skill's session calls, verbatim.
    #[allow(dead_code)]
    fn repl(id: u32) {
        let _ = process::write(id, b"1+1\n");
        let _ = process::read(id, 2000, Some(">>> "));
        let _ = process::close(id);
    }

    fn perr(e: ProcessError) -> Value {
        match e {
            ProcessError::NotFound(m) => json!({"err": "not-found", "msg": m}),
            ProcessError::Denied(m) => json!({"err": "denied", "msg": m}),
            ProcessError::Io(m) => json!({"err": "io", "msg": m}),
        }
    }

    fn proc_op(op: &str, v: &Value) -> Option<Value> {
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        Some(match op {
            "run" | "slowmark" => {
                let r = process::run(&RunRequest {
                    argv: strs(&v["argv"]),
                    cwd: v["cwd"].as_str().unwrap_or("/work").into(),
                    env: vec![],
                    stdin: v["stdin"].as_str().unwrap_or("").as_bytes().to_vec(),
                    timeout_ms: v["timeout_ms"].as_u64().unwrap_or(10_000) as u32,
                });
                if op == "slowmark" {
                    return Some(Value::String(MARK.into()));
                }
                match r {
                    Ok(o) => json!({
                        "exit_code": o.exit_code,
                        "stdout": String::from_utf8_lossy(&o.stdout),
                        "stderr": String::from_utf8_lossy(&o.stderr),
                        "timed_out": o.timed_out,
                        "truncated": o.truncated,
                    }),
                    Err(e) => perr(e),
                }
            }
            "spawn" => match process::spawn(&SpawnRequest {
                argv: strs(&v["argv"]),
                cwd: v["cwd"].as_str().unwrap_or("/work").into(),
                env: vec![],
                pty: v["pty"].as_bool().unwrap_or(false),
            }) {
                Ok(id) => json!({"id": id}),
                Err(e) => perr(e),
            },
            "pwrite" => match process::write(id, v["data"].as_str().unwrap_or("").as_bytes()) {
                Ok(()) => json!({"ok": true}),
                Err(e) => perr(e),
            },
            "pread" => match process::read(
                id,
                v["wait_ms"].as_u64().unwrap_or(1000) as u32,
                v["until"].as_str(),
            ) {
                Ok(r) => json!({
                    "data": String::from_utf8_lossy(&r.data),
                    "exit_code": r.exit_code,
                    "exited": r.exited,
                    "truncated": r.truncated,
                }),
                Err(e) => perr(e),
            },
            "close" => match process::close(id) {
                Ok(()) => json!({"ok": true}),
                Err(e) => perr(e),
            },
            "sessions" => json!(process::sessions()),
            _ => return None,
        })
    }

    impl Guest for Component {
        fn describe() -> ToolInfo {
            let mut description = String::from("WP14 test fixture.");
            match DESCRIBE {
                1 => {
                    use std::hash::BuildHasher;
                    let s = std::collections::hash_map::RandomState::new();
                    description.push_str(&format!(" {}", s.hash_one(1u8)));
                }
                2 => {
                    let _ = process::run(&RunRequest {
                        argv: vec!["true".into()],
                        cwd: "/work".into(),
                        env: vec![],
                        stdin: vec![],
                        timeout_ms: 1000,
                    })
                    .unwrap();
                }
                3 => {
                    let _ = std::fs::read_to_string("/work/x").unwrap();
                }
                _ => {}
            }
            ToolInfo {
                name: NAME.to_string(),
                description,
                input_schema: SCHEMA.to_string(),
                version: VERSION.to_string(),
                timeout_ms: TIMEOUT_MS,
            }
        }

        fn call(input: String) -> Result<String, ToolError> {
            let v: Value =
                serde_json::from_str(&input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
            let op = v["op"].as_str().unwrap_or("").to_string();
            if let Some(r) = plain_op(&op, &v, &input) {
                return r.map_err(|(inv, m)| {
                    if inv {
                        ToolError::InvalidInput(m)
                    } else {
                        ToolError::Failed(m)
                    }
                });
            }
            match proc_op(&op, &v) {
                Some(Value::String(s)) => Ok(s),
                Some(x) => Ok(x.to_string()),
                None => Err(ToolError::InvalidInput(format!("unknown op {op:?}"))),
            }
        }
    }

    export!(Component);
}
