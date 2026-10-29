//! ROOT-MSG (v0.5.1; docs/user-sessions/v0.5.0-new.md and v0.5.0-returning.md): every command the Root prints runs
//! as printed, and its refusals say the rule and the fix.
//!
//! - Source audit (every build, no processes): no string the Root prints tells the owner to type a bare
//!   `init-root <command>`. `init-root` lives in ROOT_HOME/bin and is not on the owner's PATH (only `init-agent` is), so
//!   a bare one fails with "zsh: command not found: init-root". The same holds for the charter template and for the
//!   commands docs/guide.md tells the owner to type.
//! - CLI (every build): an unreachable Root's error names a start command that runs as printed.
//! - Live (`--features test-fixtures`, macOS): the panic message, a spawn refused because the Root's HOME is not the
//!   account's home (the kernel's `HOME_REAL`), the workspace refusal, the no-workspace notice and `/undo` without one.
//!
//! Live-system safety (CONVENTIONS §1): every process is this test's own Root daemon or one that Root spawned and holds
//! (the fake kernel); the test signals only its own children, by handle. `HOME` is a fake home under
//! `root/target/test-homes/rmsg/`; nothing outside it is read or written.

#[path = "common/shown.rs"]
mod shown;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_init-root");

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The subcommands of `init-root` (cli.rs USAGE): `init-root <one of these>` is a command someone could type.
const SUBCOMMANDS: [&str; 21] = ["install", "toolchain", "upgrade", "start", "attach", "panic", "resume", "stop", "providers", "models",
    "model", "keys", "egress", "status", "ledger", "charter", "audit", "sealed", "test", "doctor", "exec-clean"];

/// Strings that name the program without telling anyone to type it: (file, the exact text around the name).
const NAMES_THE_PROGRAM: [(&str, &str); 3] = [
    ("cli.rs", "init-root attach"),                                   // the client's name in the attach frame (a whole literal)
    ("install.rs", "written by `init-root install`"),                 // who wrote kernel/config.toml (a file header)
    ("providers.rs", "(init-root providers add rewrites this block"), // who rewrites a charter block (a comment in it)
];

/// Every non-test source file of the crate, (path relative to src/, text). `tests.rs` and `*_tests.rs` are test-only.
fn sources() -> Vec<(String, String)> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = crate_dir().join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    files.into_iter().filter(|p| p.file_name().is_some_and(|n| n != "tests.rs" && !n.to_string_lossy().ends_with("_tests.rs")))
        .map(|p| (p.strip_prefix(&src).unwrap().display().to_string(), fs::read_to_string(&p).unwrap())).collect()
}

/// The string literals of Rust source outside comments and `#[cfg(test)] mod … { … }` blocks, with each `\`-newline
/// continuation folded as the compiler folds it. A small byte lexer: line and block comments, char literals and
/// lifetimes, normal, byte and raw strings. Every literal starts and ends at an ASCII quote, so it is valid UTF-8.
fn literals(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let find = |from: usize, pat: &[u8]| (from..b.len()).find(|&k| b[k..].starts_with(pat));
    let text = |from: usize, to: usize| String::from_utf8_lossy(&b[from..to.min(b.len())]).into_owned();
    let (mut i, mut out) = (0, Vec::new());
    let mut skip_mod: Option<i32> = None; // brace depth inside a test module
    while i < b.len() {
        if skip_mod.is_none() && b[i..].starts_with(b"#[cfg(test)]") {
            let mut k = i + 12;
            while k < b.len() && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if b[k..].starts_with(b"mod ") || b[k..].starts_with(b"pub mod ") {
                let (open, semi) = (find(k, b"{"), find(k, b";"));
                if let Some(o) = open.filter(|&o| semi.is_none_or(|s| s > o)) {
                    i = o + 1;
                    skip_mod = Some(1);
                    continue;
                }
            }
        }
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => i = find(i, b"\n").unwrap_or(b.len()),
            b'/' if b.get(i + 1) == Some(&b'*') => i = find(i + 2, b"*/").map_or(b.len(), |n| n + 2),
            b'\'' => {
                // a char literal ('x', '\n', '\'', '\u{..}', '"', 'é') or a lifetime ('a)
                if b.get(i + 1) == Some(&b'\\') {
                    i = find(i + 3, b"'").map_or(b.len(), |n| n + 1);
                } else {
                    let len = match b.get(i + 1) { Some(&c) if c >= 0xf0 => 4, Some(&c) if c >= 0xe0 => 3, Some(&c) if c >= 0xc0 => 2, _ => 1 };
                    i = if b.get(i + 1 + len) == Some(&b'\'') { i + 2 + len } else { i + 1 };
                }
            }
            b'r' if matches!(b.get(i + 1), Some(b'"' | b'#')) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) => {
                let hashes = b[i + 1..].iter().take_while(|&&x| x == b'#').count();
                if b.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let start = i + 2 + hashes;
                let close = [b"\"".as_slice(), &b"#".repeat(hashes)].concat();
                let end = find(start, &close).unwrap_or(b.len());
                if skip_mod.is_none() {
                    out.push(text(start, end));
                }
                i = end + close.len();
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                if skip_mod.is_none() {
                    out.push(fold(&text(i + 1, j)));
                }
                i = j + 1;
            }
            b'{' => {
                if let Some(d) = skip_mod.as_mut() {
                    *d += 1;
                }
                i += 1;
            }
            b'}' => {
                if let Some(d) = skip_mod.as_mut() {
                    *d -= 1;
                    if *d == 0 {
                        skip_mod = None;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// A `\` at the end of a line inside a string drops the newline and the next line's leading whitespace.
fn fold(s: &str) -> String {
    let mut out = String::new();
    let mut parts = s.split("\\\n");
    out.push_str(parts.next().unwrap_or(""));
    for p in parts {
        out.push_str(p.trim_start());
    }
    out
}

/// The byte offsets of `init-root <subcommand>` in `s` that read as a command to type: not a usage synopsis
/// (`usage: init-root …`) and not a program-name prefix (`init-root upgrade: <error>`).
fn bare_commands(s: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (i, _) in s.match_indices("init-root ") {
        if i > 0 && (s.as_bytes()[i - 1].is_ascii_alphanumeric() || b"/-_.".contains(&s.as_bytes()[i - 1])) {
            continue; // part of a path (`…/bin/init-root start`) or another word
        }
        let rest = &s[i + "init-root ".len()..];
        let word: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if !SUBCOMMANDS.contains(&word.as_str()) || rest[word.len()..].starts_with(':') || s[..i].ends_with("usage: ") {
            continue;
        }
        hits.push((i, format!("init-root {word}")));
    }
    hits
}

fn allowed(file: &str, lit: &str, at: usize) -> bool {
    NAMES_THE_PROGRAM.iter().filter(|(f, _)| file.ends_with(f)).map(|(_, t)| *t)
        .any(|t| lit.match_indices(t).any(|(p, _)| (p..p + t.len()).contains(&at)))
}

/// Task 1: no string literal of the Root's non-test code tells the owner to type a bare `init-root <command>`.
#[test]
fn every_init_root_command_the_root_prints_runs_as_printed() {
    let mut bad = Vec::new();
    for (file, text) in sources() {
        for lit in literals(&text) {
            for (at, cmd) in bare_commands(&lit) {
                if !allowed(&file, &lit, at) {
                    bad.push(format!("{file}: `{cmd}` in {lit:?}"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "bare `init-root` commands (not on the owner's PATH; use Paths::cmd / util::root_cmd, or an \
        init-agent form such as /resume or /key):\n{}", bad.join("\n"));
}

/// The lexer itself: literals in code are found, comments and test modules are not, continuations fold.
#[test]
fn the_literal_scanner_sees_code_strings_only() {
    let src = "// \"init-root start\" in a comment\nfn a() { let _ = ('\"', 'x', \"one \\\n        two\", r#\"raw \"q\"\"#); }\n\
        /* \"block\" */\n#[cfg(test)]\nmod tests { fn b() { let _ = \"in a test\"; } }\nfn c<'a>(_: &'a str) { let _ = \"after\"; }\n";
    assert_eq!(literals(src), ["one two", "raw \"q\"", "after"]);
    assert_eq!(bare_commands("run init-root resume to restart").len(), 1);
    assert!(bare_commands("usage: init-root upgrade --release DIR").is_empty());
    assert!(bare_commands("init-root upgrade: the Root is running").is_empty());
    assert!(bare_commands("~/.init-root/bin/init-root resume").is_empty());
    assert!(bare_commands("init-root installed.").is_empty());
}

/// The charter template is what the owner edits: its "after editing" line must run as printed.
#[test]
fn the_charter_template_names_runnable_commands() {
    let t = fs::read_to_string(crate_dir().join("assets/charter-template.toml")).unwrap();
    let bad: Vec<_> = t.lines().filter(|l| !bare_commands(l).is_empty()).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

/// docs/guide.md: every `init-root …` the owner is told to type (in backticks or a code block) runs as printed.
#[test]
fn the_guide_names_runnable_init_root_commands() {
    let g = fs::read_to_string(crate_dir().join("../../../docs/guide.md")).unwrap();
    let mut bad = Vec::new();
    let mut code = false;
    for l in g.lines() {
        if l.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        // in a code block, or inside a `code span` (an odd number of backticks before it on the line)
        let typed = |s: &str| bare_commands(s).into_iter().any(|(i, _)| code || s[..i].matches('`').count() % 2 == 1);
        if typed(l) {
            bad.push(l.to_string());
        }
    }
    assert!(bad.is_empty(), "docs/guide.md tells the owner to type a bare init-root command:\n{}", bad.join("\n"));
}

/// A word for a POSIX shell, quoted when it needs it (the rule util::sh_word follows).
fn sh(s: &str) -> String {
    if s.bytes().all(|c| c.is_ascii_alphanumeric() || b"/._+-,:@%=".contains(&c)) { s.to_string() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

/// How the Root prints this install's `init-root` for a ROOT_HOME that is not `~/.init-root` (util::root_cmd).
fn exe(rh: &Path) -> String {
    format!("INIT_ROOT_HOME={} {}", sh(&rh.display().to_string()), sh(&rh.join("bin/init-root").display().to_string()))
}

/// A fresh fake HOME `root/target/test-homes/rmsg/<name>` (short: ROOT_HOME must stay ≤ 80 bytes in test builds).
fn test_home(name: &str) -> PathBuf {
    let base = fs::canonicalize(crate_dir().join("../../target")).unwrap().join("test-homes/rmsg");
    let dir = base.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, created by an earlier run of this test
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Task 1, the returning user's "zsh: command not found: init-root": an unreachable Root says how to start it, with
/// this install's own binary, runnable as printed (no service file here, so: in a terminal of its own).
#[test]
fn an_unreachable_root_names_a_start_command_that_runs() {
    let dir = test_home("u");
    let rh = dir.join("rh");
    let o = Command::new(BIN).env_clear().env("HOME", &dir).env("INIT_ROOT_HOME", &rh).env("PATH", "/usr/bin:/bin").arg("stop").output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(69), "{err}");
    let start = format!("{} start --foreground", exe(&rh));
    assert!(err.contains("the Root is not running") && err.contains(&start) && err.contains("run this in a terminal of its own"), "{err}");
    assert!(!err.contains("`init-root start`"), "{err}");
    fs::remove_dir_all(&dir).unwrap(); // exact path, created above
}

#[cfg(all(feature = "test-fixtures", target_os = "macos"))]
mod live {
    use super::{BIN, exe, test_home};
    use serde_json::{Value, json};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::{Child, Command, Output, Stdio};
    use std::time::{Duration, Instant};

    const FK: &str = env!("CARGO_BIN_EXE_init-root-fakekernel");
    const ROOT_CFG: &str = "[root]\ntest_mode = true\nhb_interval_s = 1\nhb_stale_s = 3\nkernel_hello_timeout_s = 20\nbackoff_initial_ms = 50\n\
        backoff_max_ms = 200\nstop_grace_s = 5\n";

    struct Home {
        dir: PathBuf,
        rh: PathBuf,
        ih: PathBuf,
    }

    /// Removed by its exact path when the test passes (after the daemon, dropped first); kept when it fails.
    impl Drop for Home {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.dir);
            }
        }
    }

    fn home(name: &str) -> Home {
        let dir = test_home(name);
        for d in ["rel", "tc/bin", "proj", "other"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        fs::copy(FK, dir.join("rel/init-kernel")).unwrap();
        Home { rh: dir.join("rh"), ih: dir.join("ih"), dir }
    }

    fn wait(secs: u64, what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(secs), "not within {secs} s: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    struct Daemon(Child, PathBuf);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["stop"]).output();
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(8) {
                let o = Command::new(BIN).env_clear().env("HOME", "/nonexistent").env("INIT_ROOT_HOME", &self.1).args(["status", "--json"]).output();
                let v: Value = o.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or_default();
                if v["instances"].as_array().is_none_or(Vec::is_empty) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.0.kill(); // our own child, by its handle
            let _ = self.0.wait();
        }
    }

    impl Home {
        fn cmd(&self) -> Command {
            let mut c = Command::new(BIN);
            c.env_clear().env("HOME", &self.dir).env("INIT_ROOT_HOME", &self.rh).env("INIT_HOME", &self.ih).env("PATH", "/usr/bin:/bin");
            c
        }
        fn run(&self, args: &[&str]) -> Output {
            self.cmd().args(args).stdin(Stdio::null()).output().unwrap()
        }
        /// This install's `init-root` as the Root prints it (a non-default ROOT_HOME).
        fn exe(&self) -> String {
            exe(&self.rh)
        }
        fn install(&self, kernel: bool) {
            fs::create_dir_all(&self.rh).unwrap();
            let charter = format!("schema_version = 1\n[purpose]\ntext = \"ROOT-MSG test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
                disk_min_free_gib = 0\n{ROOT_CFG}\n");
            fs::write(self.rh.join("charter.toml"), charter).unwrap();
            let (rel, tc) = (self.dir.join("rel"), self.dir.join("tc"));
            let o = if kernel {
                self.run(&["install", "--no-service", "--release", rel.to_str().unwrap(), "--toolchain", tc.to_str().unwrap()])
            } else {
                self.run(&["install", "--no-service"])
            };
            assert_eq!(o.status.code(), Some(0), "install: {}", String::from_utf8_lossy(&o.stderr));
        }
        /// The daemon; the fake kernel (when installed) reports `home_real` as the account's home (`HOME_REAL`).
        fn daemon(&self, fk: &str, home_real: &std::path::Path) -> Daemon {
            let info: Value = serde_json::from_str(&fs::read_to_string(self.rh.join("install.json")).unwrap()).unwrap();
            let s = |k: &str| info[k].as_str().unwrap_or("").to_string();
            let c = self.cmd().arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).env("INIT_TEST_FK", fk)
                .env("INIT_TEST_TOOLCHAIN", s("toolchain")).env("INIT_TEST_DEVDIR", s("devdir")).env("INIT_TEST_HOME_REAL", home_real)
                .env("INIT_ROOT_TEST_NOW", "2030-01-10T00:00:00Z").spawn().unwrap();
            let d = Daemon(c, self.rh.clone());
            wait(10, "owner.sock", || UnixStream::connect(self.rh.join("run/owner.sock")).is_ok());
            d
        }
        fn req(&self, cmd: &str, args: Value) -> Value {
            let mut s = UnixStream::connect(self.rh.join("run/owner.sock")).unwrap();
            writeln!(s, "{}", json!({"args": args, "cmd": cmd, "id": "t-1", "v": 1})).unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        fn fk(&self, ev: &str) -> Vec<Value> {
            fs::read_to_string(self.ih.join("state/kernel/fk.jsonl")).unwrap_or_default().lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["ev"] == ev).collect()
        }
    }

    /// Task 1: the panic message names a resume command that runs (the TUI's `/resume`, or this install's binary).
    #[test]
    fn the_panic_message_names_a_resume_command_that_runs() {
        let h = home("p");
        h.install(false);
        let _d = h.daemon("", &h.dir);
        // task 4: the charter's money values are references, not caps, and `status` says so
        let st: Value = serde_json::from_slice(&h.run(&["status", "--json"]).stdout).unwrap();
        assert_eq!((st["ceilings"]["enforced"].as_bool(), st["ceilings"]["policy"].as_str(), st["ceilings"]["day_musd"].as_u64()),
            (Some(false), Some("observe"), Some(5_000_000)), "{st}");
        let o = h.run(&["panic"]);
        let out = String::from_utf8_lossy(&o.stdout);
        assert_eq!(o.status.code(), Some(0), "{out}");
        let want = format!("To restart: type /resume in init-agent, or run {} resume.", h.exe());
        assert!(out.contains("PANIC at") && out.contains(&want), "{out}");
        assert!(!out.contains(" init-root resume to restart"), "{out}");
        let alerts = fs::read_to_string(h.rh.join("alerts.jsonl")).unwrap();
        assert!(alerts.contains(&want), "{alerts}");
        // R-UNDO (v0.5.2): a line typed during the panic is queued, and the notice says so and names /resume (not "the
        // system is restarting"), which runs as typed on the line path
        let mut a = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
        writeln!(a, "{}", json!({"client": "rmsg-test", "replay": false, "t": "attach"})).unwrap();
        a.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut rd = BufReader::new(a.try_clone().unwrap());
        let mut next = |prefix: &str| loop {
            let mut l = String::new();
            assert!(rd.read_line(&mut l).unwrap() > 0, "the attach connection closed before {prefix:?}");
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["t"] == "notice" && v["text"].as_str().is_some_and(|t| t.starts_with(prefix)) {
                break v["text"].as_str().unwrap().to_string();
            }
        };
        writeln!(a, "{}", json!({"t": "line", "text": "hello"})).unwrap();
        assert_eq!(next("[root] INIT is stopped"), "[root] INIT is stopped (panic); your message is queued (1) and goes to the agent after /resume");
        writeln!(a, "{}", json!({"t": "line", "text": "/resume"})).unwrap();
        next("[root] resumed after panic");
        // and the CLI form the panic message prints
        assert_eq!(h.run(&["panic"]).status.code(), Some(0));
        assert_eq!(h.run(&["resume"]).status.code(), Some(0));
    }

    /// Task 2, the returning user's "✕ root.args: root.args V3: HOME_REAL: outside INIT_HOME, TOOLCHAIN, DEVDIR and
    /// extra_read": a Root started from a shell whose HOME is not the account's home still refuses the agent (same code),
    /// and says what is wrong and how to fix it.
    #[test]
    fn a_root_started_with_another_home_says_what_is_wrong_and_the_fix() {
        let h = home("h");
        h.install(true);
        let _d = h.daemon("body", &h.dir.join("other")); // the kernel's passwd home is not the Root's HOME
        wait(20, "the refused body spawn", || !h.fk("spawn").is_empty());
        let e = &h.fk("spawn")[0];
        assert_eq!((e["ok"].as_bool(), e["error"]["code"].as_str()), (Some(false), Some("root.args")), "still refused: {e}");
        let m = e["error"]["message"].as_str().unwrap();
        let start = format!("{} start --foreground", h.exe());
        for want in ["INIT cannot start the agent", "HOME that is not your account's home folder", "Stop this Root (Ctrl-C",
            "from a new terminal window, where HOME is your home", &start] {
            assert!(m.contains(want), "{want:?} in {m}");
        }
        assert!(m.ends_with(&start), "the fix comes last, so a cut alert keeps it: {m}");
        assert!(!m.contains("V3") && !m.contains("extra_read") && !m.contains(&h.dir.join("other").display().to_string()), "{m}");
        assert!(m.len() - start.len() < 400, "the kernel relays the last 500 bytes of an alert: {m}");
    }

    /// Task 3: a refused start folder says why and the rule; `/workspace` and `/undo` without a workspace say where the
    /// agent works, that `/undo` has nothing, and how to get a workspace. The rule itself is unchanged.
    #[test]
    fn the_workspace_refusal_states_the_rule_and_the_fix() {
        let h = home("w");
        h.install(false);
        let _d = h.daemon("", &h.dir);
        // an attached client sees the Root's notices
        let mut a = UnixStream::connect(h.rh.join("run/owner.sock")).unwrap();
        writeln!(a, "{}", json!({"client": "rmsg-test", "replay": false, "t": "attach"})).unwrap();
        a.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut rd = BufReader::new(a.try_clone().unwrap());
        let r = h.req("workspace.set", json!({"cwd": "/usr/bin"}));
        assert_eq!((r["result"]["refused"]["code"].as_str(), r["result"]["refused"]["message"].as_str()),
            (Some("outside_home"), Some("/usr/bin is outside your home folder")), "{r}");
        let rule = format!("must be a folder inside your home ({}), not your home itself", h.dir.display());
        let notice = loop {
            let mut l = String::new();
            assert!(rd.read_line(&mut l).unwrap() > 0, "the attach connection closed");
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["t"] == "notice" && v["code"] == "access" {
                break v["text"].as_str().unwrap_or("").to_string();
            }
        };
        // v0.5.3 (REL-052 §7.5 finding 4): the launcher by its path (install.json bin_dir, default ~/.local/bin): a fresh
        // Mac has no ~/.local/bin on PATH, so a bare `init-agent` is "command not found"
        let run_there = format!("run {} there", crate::shown::agent(&h.dir.join(".local/bin"), ""));
        for want in ["[root] no workspace: /usr/bin is outside your home folder.", rule.as_str(), "the agent works in ~/ih/work",
            "/undo has nothing to restore", run_there.as_str()] {
            assert!(notice.contains(want), "{want:?} in {notice}");
        }
        let u = h.req("undo", json!({}));
        assert_eq!((u["error"]["code"].as_str(), u["error"]["data"]["reason"].as_str()), (Some("root.mode"), Some("no_workspace")), "{u}");
        let m = u["error"]["message"].as_str().unwrap();
        assert!(m.starts_with("/undo has nothing to restore: this session has no workspace (/usr/bin is outside your home folder).")
            && m.contains(&rule) && m.contains("cd into a project folder inside your home") && m.contains(&run_there), "{m}");
        // `/full off` says it holds for this session only (a plain relaunch was back to FULL in the returning session)
        h.req("full.set", json!({"on": false}));
        let off = loop {
            let mut l = String::new();
            assert!(rd.read_line(&mut l).unwrap() > 0, "the attach connection closed");
            let v: Value = serde_json::from_str(&l).unwrap_or_default();
            if v["t"] == "notice" && v["text"].as_str().is_some_and(|t| t.starts_with("[root] full access off")) {
                break v["text"].as_str().unwrap_or("").to_string();
            }
        };
        assert!(off.contains("for this session") && off.contains("A new init-agent launch starts with the charter's default"), "{off}");
        assert!(off.contains(&format!("launch with {}", crate::shown::agent(&h.dir.join(".local/bin"), "--workspace"))), "{off}");
        // the rule is unchanged: a folder inside the home is a workspace
        let ok = h.req("workspace.set", json!({"cwd": h.dir.join("proj")}));
        assert_eq!(ok["result"]["refused"], Value::Null, "{ok}");
        assert!(ok["result"]["access"]["workspace"].as_str().is_some_and(|w| w.ends_with("/proj")), "{ok}");
    }
}
