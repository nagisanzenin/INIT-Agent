//! Candidate trees (Phase 0): the body as a directory the Root hands over, judged without a build slot.
//!
//! The one form judged today is the **script body**: an executable `init-body` at the tree root whose
//! shebang is `/usr/bin/perl`, with its tool manifest (`init-tools-v1` shape: a JSON list of
//! `{name, description, parameters}`) after a `__DATA__` line, and `init-body --selftest` as its test
//! entry point. This is the stand-in body of root/sealed-eval.md's Phase 0 M (WP24). For it:
//!
//! - **G0** (gate.md §3.3, analogues of the cargo steps, same failure codes): the safety pragmas
//!   `use strict;` and `use warnings;` in the header (the `#![forbid(unsafe_code)]` check,
//!   `gate.g0.forbid_missing`), no top-level `no strict`/`no warnings` (the lint-cap policy,
//!   `gate.g0.build_config`), `perl -c` (`gate.g0.check`) and `--selftest` run by the gate itself and
//!   judged by exit code, never by its output (`gate.g0.test`).
//! - **G1** (gate.md §3.4): the manifest must parse and have the tool shape (`gate.g1.manifest_error`).
//!
//! A cargo tree (`Cargo.toml` at the root) needs the build slot and selfmap, which `judge` does not have
//! yet; it is reported as `error: unsupported_tree`, never judged by these rules.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::record::cap;

/// The perl interpreter a script body must name.
pub const PERL: &str = "/usr/bin/perl";
/// Largest `init-body` judged (16 MiB).
pub const BODY_MAX: u64 = 16 << 20;

/// A stage failure: the gate code (gate.md §4.1) and ≤ 4 KiB of detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageFail {
    pub code: &'static str,
    pub detail: String,
}

impl StageFail {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        StageFail {
            code,
            detail: cap(&detail.into(), 4096),
        }
    }
    /// The record's short reason: the last component of the code (`forbid_missing`).
    pub fn reason(&self) -> &'static str {
        self.code.rsplit('.').next().unwrap_or(self.code)
    }
}

/// The tree's form, or why it cannot be judged here.
pub fn check_form(dir: &Path) -> Result<(), String> {
    if dir.join("Cargo.toml").is_file() {
        return Err("a cargo tree needs the build slot and selfmap (judge takes script-body trees in Phase 0)".into());
    }
    let body = dir.join("init-body");
    let mut head = [0u8; 128];
    let n = fs::File::open(&body)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| format!("init-body: {e}"))?;
    let first = String::from_utf8_lossy(&head[..n]);
    let first = first.lines().next().unwrap_or("");
    let interp = first
        .strip_prefix("#!")
        .map(|s| s.split_whitespace().next().unwrap_or(""));
    if interp != Some(PERL) {
        return Err(format!("init-body is not a {PERL} script body"));
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut v: Vec<_> = fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        let md = fs::symlink_metadata(&p)?;
        if md.is_dir() {
            walk(root, &p, out)?;
        } else {
            out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
        }
    }
    Ok(())
}

/// sha256 over the sorted `<path>\0<kind>\0<sha256>\n` lines of every entry (kind `x` executable file,
/// `f` file, `l` symlink with its target hashed). Two trees with the same files have the same digest.
pub fn digest(dir: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    let mut h = Sha256::new();
    for rel in &files {
        let p = dir.join(rel);
        let md = fs::symlink_metadata(&p)?;
        let (kind, data) = if md.file_type().is_symlink() {
            (
                "l",
                fs::read_link(&p)?
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes(),
            )
        } else {
            let k = if md.permissions().mode() & 0o111 != 0 {
                "x"
            } else {
                "f"
            };
            (k, fs::read(&p)?)
        };
        let fh: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        h.update(format!("{}\0{kind}\0{fh}\n", rel.to_string_lossy()).as_bytes());
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `body_tokens` of a tree (gate.md §3.9 size tie-break): ⌈bytes / 4⌉ over its regular files.
pub fn body_tokens(dir: &Path) -> u64 {
    let mut files = Vec::new();
    let _ = walk(dir, dir, &mut files);
    let bytes: u64 = files
        .iter()
        .filter_map(|r| fs::symlink_metadata(dir.join(r)).ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();
    bytes.div_ceil(4)
}

/// A finished child: exit code (None if killed), whether it timed out, stdout (≤ 1 MiB).
pub struct Ran {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Kills a child this process spawned in its own process group (`process_group(0)`): the group by its
/// id (equal to the child's pid), then the child by its handle, then reaps it.
pub fn kill_own_group(c: &mut Child) {
    let pid = c.id();
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = c.kill();
    let _ = c.wait();
}

/// Runs `cmd` (stdin null, stdout and stderr captured) in its own process group with a timeout.
pub fn run_timeout(mut cmd: Command, timeout: Duration) -> io::Result<Ran> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut c = cmd.spawn()?;
    let mut so = c.stdout.take();
    let mut se = c.stderr.take();
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = so.as_mut() {
            let _ = s.take(1 << 20).read_to_end(&mut b);
            let _ = io::copy(s, &mut io::sink());
        }
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = se.as_mut() {
            let _ = s.take(64 << 10).read_to_end(&mut b);
            let _ = io::copy(s, &mut io::sink());
        }
        b
    });
    let t0 = Instant::now();
    let (code, timed_out) = loop {
        if let Some(st) = c.try_wait()? {
            break (st.code(), false);
        }
        if t0.elapsed() >= timeout {
            kill_own_group(&mut c);
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Ran {
        code,
        timed_out,
        stdout: ho.join().unwrap_or_default(),
        stderr: he.join().unwrap_or_default(),
    })
}

fn scratch_cmd(prog: &Path, scratch: &Path) -> Command {
    let mut c = Command::new(prog);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("HOME", scratch.join("home"))
        .env("TMPDIR", format!("{}/", scratch.join("tmp").display()))
        .current_dir(scratch);
    c
}

/// The script header: the lines before the first `sub` definition or the `__DATA__` marker.
fn header(text: &str) -> Vec<&str> {
    text.lines()
        .take_while(|l| !l.starts_with("sub ") && *l != "__DATA__" && *l != "__END__")
        .collect()
}

/// G0 on a script-body tree. `scratch` is an empty directory the caller deletes. Returns the stage
/// record and the failure, if any.
pub fn g0(tree: &Path, scratch: &Path, step_timeout: Duration) -> (Value, Option<StageFail>) {
    let body = tree.join("init-body");
    let mut steps = Vec::new();
    let done = |steps: Vec<Value>, f: Option<StageFail>| {
        let mut v = json!({"pass": f.is_none(), "form": "script", "steps": steps});
        if let Some(f) = &f {
            v["code"] = json!(f.code);
            v["reason"] = json!(f.reason());
            v["detail"] = json!(f.detail);
        }
        (v, f)
    };
    // shape: an executable regular file of bounded size
    let md = match fs::metadata(&body) {
        Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 && m.len() <= BODY_MAX => m,
        Ok(_) => {
            return done(
                steps,
                Some(StageFail::new(
                    "gate.g0.metadata",
                    "init-body must be an executable file of at most 16 MiB",
                )),
            );
        }
        Err(e) => {
            return done(
                steps,
                Some(StageFail::new(
                    "gate.g0.metadata",
                    format!("init-body: {e}"),
                )),
            );
        }
    };
    let text = fs::read_to_string(&body).unwrap_or_default();
    steps.push(json!({"step": "shape", "bytes": md.len()}));
    // policy: the safety pragmas in the header, no lint caps at the top level
    let head = header(&text);
    let has = |p: &str| head.iter().any(|l| l.trim() == p);
    let missing: Vec<&str> = ["use strict;", "use warnings;"]
        .into_iter()
        .filter(|p| !has(p))
        .collect();
    steps.push(json!({"step": "pragmas", "missing": missing}));
    if !missing.is_empty() {
        return done(
            steps,
            Some(StageFail::new(
                "gate.g0.forbid_missing",
                format!("init-body: the header lacks {}", missing.join(" and ")),
            )),
        );
    }
    let capped: Vec<&str> = text
        .lines()
        .filter(|l| {
            let t = l.split('#').next().unwrap_or("").trim();
            t.starts_with("no strict") || t.starts_with("no warnings")
        })
        .collect();
    if let Some(l) = capped.first() {
        return done(
            steps,
            Some(StageFail::new(
                "gate.g0.build_config",
                format!("init-body: lint cap `{}`", l.trim()),
            )),
        );
    }
    for d in ["home", "tmp"] {
        let _ = fs::create_dir_all(scratch.join(d));
    }
    // check: perl -c
    let mut c = scratch_cmd(Path::new(PERL), scratch);
    c.arg("-c").arg(&body);
    match run_timeout(c, step_timeout) {
        Ok(r) => {
            steps.push(json!({"step": "check", "exit": r.code, "timed_out": r.timed_out}));
            if r.code != Some(0) {
                let code = if r.timed_out {
                    "gate.g0.timeout"
                } else {
                    "gate.g0.check"
                };
                return done(
                    steps,
                    Some(StageFail::new(
                        code,
                        String::from_utf8_lossy(&r.stderr).into_owned(),
                    )),
                );
            }
        }
        Err(e) => {
            return done(
                steps,
                Some(StageFail::new("gate.g0.check", format!("perl -c: {e}"))),
            );
        }
    }
    // test: the body's own self-test, run by the gate, judged by its exit code only
    let mut c = scratch_cmd(&body, scratch);
    c.arg("--selftest");
    match run_timeout(c, step_timeout) {
        Ok(r) => {
            steps.push(json!({"step": "test", "exit": r.code, "timed_out": r.timed_out}));
            if r.code != Some(0) {
                let code = if r.timed_out {
                    "gate.g0.timeout"
                } else {
                    "gate.g0.test"
                };
                let why = format!("init-body --selftest exited {:?}", r.code);
                return done(steps, Some(StageFail::new(code, why)));
            }
        }
        Err(e) => {
            return done(
                steps,
                Some(StageFail::new("gate.g0.test", format!("--selftest: {e}"))),
            );
        }
    }
    done(steps, None)
}

fn tool_name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// G1 on a script-body tree: the tool manifest after `__DATA__`.
pub fn g1(tree: &Path) -> (Value, Option<StageFail>) {
    let text = fs::read_to_string(tree.join("init-body")).unwrap_or_default();
    let fail = |why: String| {
        let f = StageFail::new("gate.g1.manifest_error", why);
        (
            json!({"pass": false, "code": f.code, "reason": f.reason(), "detail": f.detail}),
            Some(f),
        )
    };
    let Some((_, m)) = text.split_once("\n__DATA__\n") else {
        return fail("init-body: no __DATA__ manifest".into());
    };
    let tools: Value = match serde_json::from_str(m) {
        Ok(v) => v,
        Err(e) => return fail(format!("manifest: {e}")),
    };
    let Some(list) = tools.as_array().filter(|a| !a.is_empty()) else {
        return fail("manifest: expected a non-empty list of tools".into());
    };
    let mut names = BTreeSet::new();
    for (i, t) in list.iter().enumerate() {
        let name = t["name"].as_str().unwrap_or("");
        if !tool_name_ok(name) || !t["description"].is_string() || !t["parameters"].is_object() {
            return fail(format!(
                "manifest: tool {i} needs a name, a description and an object `parameters`"
            ));
        }
        if !names.insert(name) {
            return fail(format!("manifest: tool `{name}` is listed twice"));
        }
    }
    let canon = crate::judge::sorted_json(&tools);
    let sha: String = Sha256::digest(canon.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (
        json!({"pass": true, "tools": names.len(), "tools_sha256": sha}),
        None,
    )
}
