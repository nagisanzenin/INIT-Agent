//! The `init-root` CLI (root/root.md §4.3). Exit codes: 0 ok; 1 error; 2 invalid charter, not a TTY
//! or refused; 64 usage; 69 Root unreachable; 76 protocol; 77 resume refused.
//! WP21: `exec-clean`, the supervisor and signal threads in `start`, Ctrl-C → `interrupt` in `attach`,
//! tuples and the kernel in `status`, `install --toolchain`. The panic fallback stays latch-only: the CLI
//! never signals a pid it did not spawn (CONVENTIONS §1).

use crate::charter;
use crate::install::{self, Paths};
use crate::link::{self, Fail, Root};
use crate::util::{self, icj, now_ms, rfc3339};
use crate::{ledger, lineage, panic, sys};
use serde_json::{Value, json};
use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const USAGE: &str = "usage: init-root [--root-home DIR] <command>
  install [--home DIR] [--release DIR] [--seed-source REPO|BUNDLE] [--seed-tag TAG] [--charter FILE] [--git PATH]
          [--toolchain DIR|none] [--bin-dir DIR] [--with-testkit] [--no-service]
          [--prebuilt-body FILE --prebuilt-sha256 HEX [--prebuilt-commit SHA]]
  toolchain [--set [DIR]]   (show, or record the Rust toolchain self-evolution T1/T2 builds with; Root stopped)
  upgrade --release DIR [--bin-dir DIR] | --probe [--timeout S] | --commit | --rollback | --status [--json]   (Root stopped)
  start [--foreground] · attach [--no-replay] [--raw] · panic · resume · stop
  providers list · providers add <preset> [--base-url URL] [--model ID] [--name ALIAS] [--price IN,CACHED,OUT] [--json]
  models [--provider P] [--json] · model use <provider>/<model>
  keys set [--action] <id> · keys check · egress stats · status [--json] [--proposals] · ledger [--json] [--verify]
  charter check [<path>] [--apply [--yes]] · audit verify [--full] [--view] · audit run [--reason R]
  audit verify --file RECORDS --pub KEY [--head HASH]   (a published lineage file, offline; no install needed)
  sealed import <dir> · sealed list · sealed gog --gate EXE [--source maintainer|agent] [-- gate args…]
  sealed draw [--pack s1] [--pack-dir DIR] · sealed show [--g4 GATE_ID|--partition|--templates]
  sealed calibrate [--repeats 6] [--usd X] [--real] · sealed probe [--per-family 2] [--usd X] [--real] (= calibrate --probe N)
  sealed milestone [--reason R] [--repeats R] [--usd X] [--baseline T] [--current T] [--force] [--real]
  sealed slice --gate-id ID --body SHA256 --incumbent T [--tier standard] [--n 32] [--repeats 3] [--budget-musd N] [--wall-s S]
  test ledger --spec FILE · test clock --advance S | --set T
  doctor --sweep-dry-run   (the D1 sweep's selection over the live process table; sends no signal, ADR-008)";
const T: Duration = Duration::from_secs(10);

fn flag(a: &mut Vec<String>, name: &str) -> bool {
    let n = a.len();
    a.retain(|x| x != name);
    a.len() != n
}

fn opt(a: &mut Vec<String>, name: &str) -> Option<String> {
    let i = a.iter().position(|x| x == name)?;
    (i + 1 < a.len()).then(|| a.drain(i..=i + 1).nth(1))?
}

pub fn run(mut a: Vec<String>) -> u8 {
    if a.first().is_some_and(|x| x == "exec-clean") {
        return exec_clean(&a[1..]);
    }
    let rh = opt(&mut a, "--root-home");
    let home = opt(&mut a, "--home");
    let p = match Paths::resolve(rh, home) {
        Ok(p) => p,
        Err(e) => return fail(1, &e.to_string()),
    };
    let words: Vec<&str> = a.iter().map(String::as_str).filter(|w| !w.starts_with("--")).take(2).collect();
    match words.as_slice() {
        [] if a == ["--version"] => {
            println!("init-root {}", install::VERSION);
            0
        }
        ["install", ..] => {
            let tc = opt(&mut a, "--toolchain");
            let o = install::Opts { release: opt(&mut a, "--release").map(Into::into), charter: opt(&mut a, "--charter").map(Into::into),
                git: opt(&mut a, "--git").unwrap_or_else(|| "/usr/bin/git".into()), with_testkit: flag(&mut a, "--with-testkit"),
                toolchain: tc.clone().filter(|t| t != "none").map(Into::into), seed_source: opt(&mut a, "--seed-source").map(Into::into),
                seed_tag: opt(&mut a, "--seed-tag"), no_service: flag(&mut a, "--no-service"), bin_dir: opt(&mut a, "--bin-dir").map(Into::into) };
            let (body, sha, commit) = (opt(&mut a, "--prebuilt-body"), opt(&mut a, "--prebuilt-sha256"), opt(&mut a, "--prebuilt-commit"));
            let prebuilt = match (body, sha) {
                (Some(b), Some(s)) => Some(install::Prebuilt { body: b.into(), sha256: s, commit }),
                (None, None) if commit.is_none() => None,
                _ => return fail(64, "--prebuilt-body and --prebuilt-sha256 go together (--prebuilt-commit needs both)"),
            };
            let bo = install::BundleOpts { no_toolchain: tc.as_deref() == Some("none"), prebuilt };
            install::install_with(&p, &o, &bo).map_or_else(|(c, m)| fail(c, &m), |()| 0)
        }
        ["upgrade", ..] => crate::upgrade::cli(&p, a), // lane UPGRADE: the owner's stop-then-replace upgrade (upgrade.rs)
        ["toolchain", ..] => {
            let set = flag(&mut a, "--set");
            let dir = a.iter().skip(1).find(|x| !x.starts_with("--")).map(Into::into);
            install::toolchain_cmd(&p, set, dir).map_or_else(|(c, m)| fail(c, &m), |()| 0)
        }
        ["start", ..] => start(&p, flag(&mut a, "--foreground")),
        ["attach", ..] => attach(&p, !flag(&mut a, "--no-replay"), flag(&mut a, "--raw")),
        ["panic", ..] => panic_cmd(&p),
        ["resume", ..] | ["stop", ..] => match link::request(&p, words[0], json!({}), T) {
            Ok(r) => out(&r, 0),
            Err(Fail::Refused(_, m)) => fail(if words[0] == "resume" { 77 } else { 1 }, &m),
            Err(f) => unreachable_or(f),
        },
        ["keys", "set"] => keys_set(&p, flag(&mut a, "--action"), a.get(2).cloned()),
        // UX-KEYMODEL: providers, the catalog and the model selection (providers.rs)
        ["providers", ..] | ["models", ..] | ["model", ..] => crate::providers::cli(&p, a),
        ["keys", "check"] => keys_check(&p),
        ["egress", "stats"] => match link::request(&p, "egress.stats", json!({}), T) {
            Ok(v) => out(&v, 0),
            Err(f) => unreachable_or(f),
        },
        ["audit", "verify"] if a.iter().any(|x| x == "--file" || x == "--pub") => {
            let (file, key, head) = (opt(&mut a, "--file"), opt(&mut a, "--pub"), opt(&mut a, "--head"));
            crate::lineage::verify::file_cli(file, key, head, flag(&mut a, "--view"))
        }
        ["audit", "verify"] => crate::lineage::verify::audit_verify(&p, flag(&mut a, "--full"), flag(&mut a, "--view")),
        ["audit", "run"] | ["sealed", "gog" | "draw" | "calibrate" | "probe" | "milestone" | "slice"] => {
            let w = if words[0] == "audit" { "audit" } else { words[1] }.to_string();
            sealed_cmd(&p, &w, a)
        }
        ["sealed", "show"] => {
            let args = json!({"g4": opt(&mut a, "--g4"), "templates": flag(&mut a, "--templates")});
            crate::sealed::show(&p.rh, &args).map_or_else(|m| fail(1, &m), |v| out(&v, 0))
        }
        ["sealed", "import"] => match a.get(2) {
            Some(d) => crate::sealed::import(&p, d.as_ref()).map_or_else(|m| fail(1, &m), |v| out(&v, 0)),
            None => fail(64, USAGE),
        },
        ["sealed", "list"] => out(&crate::sealed::list(&p.rh), 0),
        ["doctor", ..] if a.iter().any(|x| x == "--sweep-dry-run") => crate::doctor::sweep_dry_run(&p).map_or_else(|m| fail(1, &m), |v| out(&v, 0)),
        ["status", ..] => status(&p, flag(&mut a, "--json"), flag(&mut a, "--proposals"), flag(&mut a, "--selftest")),
        // ADR-011 (X7): every level change against §7's terms, re-folded from the chain; exit 1 on a problem
        ["ledger", ..] if a.iter().any(|x| x == "--verify") => {
            let bad = crate::ledger::verify(&crate::lineage::records(&p.rh).unwrap_or_default());
            out(&json!({"ok": bad.is_empty(), "problems": bad}), u8::from(!bad.is_empty()))
        }
        ["ledger", ..] => {
            let v = link::request(&p, "ledger", json!({}), T).unwrap_or_else(|_| offline(&p)["ledger"].clone());
            out(&v, 0)
        }
        ["charter", "check"] => {
            let (apply, yes) = (flag(&mut a, "--apply"), flag(&mut a, "--yes"));
            charter_check(&p, a.get(2).map(Into::into), apply, yes)
        }
        #[cfg(feature = "test-fixtures")]
        ["test", "ledger"] => match opt(&mut a, "--spec") {
            Some(s) => crate::testfix::test_ledger(&p.rh, s.as_ref()).map_or_else(|(c, m)| fail(c, &m), |m| { println!("{m}"); 0 }),
            None => fail(64, USAGE),
        },
        #[cfg(feature = "test-fixtures")]
        ["test", "clock"] => {
            let args = match (opt(&mut a, "--advance"), opt(&mut a, "--set")) {
                (Some(s), _) => json!({"advance_ms": s.parse::<i64>().unwrap_or(-1).saturating_mul(1000)}),
                (None, Some(t)) => json!({"set": t}),
                _ => return fail(64, USAGE),
            };
            match link::request(&p, "test.clock", args, T) {
                Ok(r) => out(&r, 0),
                Err(Fail::Refused(_, m)) => fail(2, &m),
                Err(f) => unreachable_or(f),
            }
        }
        ["test", ..] => fail(2, "init-root test …: only in a test build (cargo feature test-fixtures)"),
        _ => fail(64, USAGE),
    }
}

/// `audit run`, `sealed gog` (root/sealed-eval.md §4.4) and M7's `sealed draw|calibrate|probe|milestone|slice`: through
/// the running Root, or in this process under the Root lock when none runs. They may take long, so the CLI waits up
/// to six hours.
fn sealed_cmd(p: &Paths, word: &str, mut a: Vec<String>) -> u8 {
    let rest: Vec<String> = a.iter().position(|x| x == "--").map(|i| a.split_off(i)[1..].to_vec()).unwrap_or_default();
    let num = |a: &mut Vec<String>, k: &str| opt(a, k).and_then(|x| x.parse::<u64>().ok());
    let real = flag(&mut a, "--real");
    let usd = opt(&mut a, "--usd");
    let probe = num(&mut a, "--probe");
    let milestone = word == "audit" && opt(&mut a, "--set").as_deref() == Some("reserve");
    let (cmd, args) = match word {
        "audit" if milestone => ("sealed.milestone", json!({"reason": opt(&mut a, "--reason").unwrap_or_else(|| "owner".into()), "real": real, "usd": usd})),
        "audit" => ("audit.run", json!({"reason": opt(&mut a, "--reason").unwrap_or_else(|| "manual".into())})),
        "gog" => {
            let Some(gate) = opt(&mut a, "--gate").and_then(|g| fs::canonicalize(g).ok()) else { return fail(64, USAGE) };
            ("sealed.gog", json!({"args": rest, "gate": gate, "source": opt(&mut a, "--source").unwrap_or_else(|| "maintainer".into())}))
        }
        "draw" => {
            let dir = match opt(&mut a, "--pack-dir").map(fs::canonicalize) {
                Some(Err(e)) => return fail(1, &format!("--pack-dir: {e}")),
                d => d.and_then(Result::ok),
            };
            ("sealed.draw", json!({"pack": opt(&mut a, "--pack"), "pack_dir": dir}))
        }
        "calibrate" | "probe" if word == "probe" || probe.is_some() => {
            let n = probe.or_else(|| num(&mut a, "--per-family"));
            ("sealed.probe", json!({"per_family": n, "real": real, "usd": usd}))
        }
        "calibrate" => ("sealed.calibrate", json!({"repeats": num(&mut a, "--repeats"), "splits": num(&mut a, "--splits"), "real": real, "usd": usd})),
        "milestone" => ("sealed.milestone", json!({"reason": opt(&mut a, "--reason").unwrap_or_else(|| "owner".into()), "repeats": num(&mut a, "--repeats"),
            "baseline": opt(&mut a, "--baseline"), "current": opt(&mut a, "--current"), "force": flag(&mut a, "--force"), "real": real, "usd": usd})),
        _ => {
            let tier = opt(&mut a, "--tier").unwrap_or_else(|| "standard".into());
            ("sealed.slice", json!({"gate_id": opt(&mut a, "--gate-id"), "candidate": {"body": opt(&mut a, "--body"), "ext": {"set": null, "components": []}},
                "incumbent": opt(&mut a, "--incumbent"), "tier": {"name": tier, "n": num(&mut a, "--n").unwrap_or(32), "repeats": num(&mut a, "--repeats").unwrap_or(3)},
                "budget_musd": num(&mut a, "--budget-musd").unwrap_or(3_000_000), "wall_s": num(&mut a, "--wall-s").unwrap_or(5400)}))
        }
    };
    let r = match link::request(p, cmd, args.clone(), Duration::from_secs(6 * 3600)) {
        Err(Fail::Unreachable(_)) => match take_lock(p) {
            Ok(Some(_lock)) => Root::boot(p.clone()).map_err(|e| ("root.internal", e.to_string())).and_then(|r| crate::sealed::request(&r, cmd, &args)),
            _ => return fail(1, "owner.sock is unreachable but a Root holds run/root.lock"),
        },
        Err(Fail::Refused(_, m)) => return fail(1, &m),
        Err(f) => return unreachable_or(f),
        Ok(v) => Ok(v),
    };
    r.map_or_else(|(_, m)| fail(1, &m), |v| out(&v, 0))
}

/// `exec-clean [--keep N]… -- argv…` (§3.5.2): runs before anything else, then `execv`s the launcher.
fn exec_clean(a: &[String]) -> u8 {
    let (mut keep, mut i) = (Vec::new(), 0);
    while i + 1 < a.len() && a[i] == "--keep" {
        keep.push(a[i + 1].parse().unwrap_or(-1));
        i += 2;
    }
    if a.get(i).map(String::as_str) != Some("--") || i + 1 >= a.len() {
        return fail(64, "usage: init-root exec-clean [--keep N]… -- argv…");
    }
    let e = sys::exec_clean(&keep, &a[i + 1..]);
    fail(127, &format!("exec-clean: {}: {e}", a[i + 1]))
}

fn fail(code: u8, msg: &str) -> u8 {
    eprintln!("init-root: {msg}");
    code
}

fn out(v: &Value, code: u8) -> u8 {
    println!("{}", icj(v).unwrap_or_default());
    code
}

fn unreachable_or(f: Fail) -> u8 {
    match f {
        Fail::Unreachable(e) => fail(69, &format!("the Root is unreachable ({e}); is `init-root start` running?")),
        Fail::Protocol(m) => fail(76, &format!("protocol error: {m}")),
        Fail::Refused(c, m) => fail(1, &format!("{c}: {m}")),
    }
}

/// Holds `run/root.lock` for the caller's lifetime; None if a Root already holds it.
pub(crate) fn take_lock(p: &Paths) -> io::Result<Option<File>> {
    let f = File::options().create(true).truncate(false).write(true).open(p.r("run/root.lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// `start`: the daemon, in this process (launchd and systemd run it). `--foreground` also logs to stderr.
fn start(p: &Paths, fg: bool) -> u8 {
    let lock = match take_lock(p) {
        Ok(Some(l)) => l,
        Ok(None) => {
            let r = link::request(p, "start", json!({}), T);
            println!("already running{}", r.map(|v| format!(": {}", icj(&v).unwrap_or_default())).unwrap_or_default());
            return 0;
        }
        Err(e) => return fail(1, &format!("{}: {e} (not installed?)", p.r("run/root.lock").display())),
    };
    if let Err(e) = util::write_atomic(&p.r("run/root.pid"), format!("{}\n", std::process::id()).as_bytes(), 0o600) {
        return fail(1, &format!("run/root.pid: {e}"));
    }
    match sys::raise_nofile(4096) { // R-SEC: launchd's soft 256 is below the egress, sealed lane and zone fd needs
        Ok((b, a, h)) => util::log(&p.rh, "info", &format!("RLIMIT_NOFILE soft {b} -> {a} (hard {h})")),
        Err(e) => util::log(&p.rh, "warn", &format!("RLIMIT_NOFILE could not be raised: {e}")),
    }
    // LINUX: claim the delegated cgroup subtree (if any) before anything is spawned (cgroup.rs)
    #[cfg(target_os = "linux")]
    crate::cgroup::init(&p.rh);
    let root = match Root::boot(p.clone()) {
        Ok(r) => r,
        Err(e) => return fail(1, &format!("cold start: {e}")),
    };
    if !matches!(link::lock(&root.st).slot, crate::sup::Slot::Halted { .. })
        && let Err(e) = crate::egress::start(&root)
    {
        root.alert("egress_down", &format!("[root] the egress could not start ({e}); the zone has no network"));
    }
    if fg {
        eprintln!("init-root: {}", icj(&root.status()).unwrap_or_default());
    }
    crate::sup::start(&root);
    crate::memwatch::spawn_zone(root.clone()); // MEM-RUNAWAY: every held instance, every second
    crate::lineage::mirror::spawn(&root, crate::sup::integrity_halt);
    panic::signals(root.clone());
    let w = root.clone();
    std::thread::spawn(move || panic::watch(w));
    let r = link::serve(root);
    drop(lock);
    r.map_or_else(|e| fail(1, &format!("owner.sock: {e}")), |()| 0)
}

/// The owner terminal (§3.15): lines from stdin, frames rendered to stdout; Ctrl-C sends `interrupt`
/// (the owner leaves with Ctrl-D).
fn attach(p: &Paths, replay: bool, raw: bool) -> u8 {
    let s = match UnixStream::connect(p.r("run/owner.sock")) {
        Ok(s) => s,
        Err(e) => return unreachable_or(Fail::Unreachable(e)),
    };
    let Ok(mut w) = s.try_clone() else { return 76 };
    let hello = icj(&json!({"client": "init-root attach", "replay": replay, "t": "attach"})).unwrap_or_default();
    if writeln!(w, "{hello}").is_err() {
        return 76;
    }
    if let (Ok(mut sig), Ok(mut w2)) = (sys::signal_pipe(&[libc::SIGINT]), w.try_clone()) {
        std::thread::spawn(move || {
            let mut b = [0u8; 1];
            while io::Read::read_exact(&mut sig, &mut b).is_ok() && writeln!(w2, "{{\"t\":\"interrupt\"}}").is_ok() {}
        });
    }
    // STEER: a multi-line paste is one owner message. On a terminal, bracketed paste marks it; it is
    // switched off again at stdin's EOF and when attach returns.
    let _paste = Paste::on(sys::isatty(0) && sys::isatty(1));
    std::thread::spawn(move || {
        let mut asm = Assembler::default();
        let send = |w: &mut UnixStream, m: String| writeln!(w, "{}", icj(&json!({"t": "line", "text": m})).unwrap_or_default());
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if asm.push(&line).into_iter().any(|m| send(&mut w, m).is_err()) {
                return;
            }
        }
        if let Some(m) = asm.finish() {
            let _ = send(&mut w, m);
        }
        Paste::off();
        let _ = writeln!(w, "{{\"t\":\"detach\"}}");
    });
    for line in BufReader::new(s).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { return fail(76, "malformed frame from the Root") };
        let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
        match v["t"].as_str().unwrap_or("") {
            _ if raw => println!("{line}"),
            "display" => print!("{}", if v["text"].is_string() { text("text") } else { format!("{line}\n") }),
            "echo" => println!("» {}", text("text")),
            "notice" => println!("{}", text("text")),
            _ => {} // state, replay_end
        }
        let _ = io::stdout().flush();
    }
    0
}

/// Bracketed paste on the owner's terminal (`ESC[?2004h`), switched off once (`ESC[?2004l`) by `off` or on drop.
struct Paste;
static PASTE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Paste {
    fn on(enable: bool) -> Paste {
        if enable && !PASTE_ON.swap(true, std::sync::atomic::Ordering::SeqCst) {
            print!("\x1b[?2004h");
            let _ = io::stdout().flush();
        }
        Paste
    }
    fn off() {
        if PASTE_ON.swap(false, std::sync::atomic::Ordering::SeqCst) {
            print!("\x1b[?2004l");
            let _ = io::stdout().flush();
        }
    }
}

impl Drop for Paste {
    fn drop(&mut self) {
        Paste::off();
    }
}

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// STEER: stdin lines → owner messages for `attach`. Everything between the bracketed-paste markers (which may
/// start and end mid-line and span lines) joins into one message with `\n` separators, markers stripped; a line
/// ending in a single `\` continues onto the next (the `\` becomes `\n`, as the TUI's `\` + Enter; `\\` stays
/// literal); otherwise one line is one message. A trailing `\r` is dropped and a `\r` inside a paste is a
/// newline. Messages empty after that (blank or whitespace only) are not sent.
#[derive(Default)]
struct Assembler {
    buf: String,
    paste: bool,
}

impl Assembler {
    /// One stdin line (without its `\n`): the messages it completes.
    fn push(&mut self, line: &str) -> Vec<String> {
        let mut rest = line.strip_suffix('\r').unwrap_or(line);
        loop {
            if self.paste {
                let Some(i) = rest.find(PASTE_END) else {
                    self.buf.push_str(&rest.replace("\r\n", "\n").replace('\r', "\n"));
                    self.buf.push('\n'); // the line ends inside the paste
                    return Vec::new();
                };
                self.buf.push_str(&rest[..i].replace("\r\n", "\n").replace('\r', "\n"));
                (self.paste, rest) = (false, &rest[i + PASTE_END.len()..]);
            } else if let Some(i) = rest.find(PASTE_START) {
                self.buf.push_str(&rest[..i]);
                (self.paste, rest) = (true, &rest[i + PASTE_START.len()..]);
            } else if let Some(head) = rest.strip_suffix('\\').filter(|h| !h.ends_with('\\')) {
                self.buf.push_str(head);
                self.buf.push('\n');
                return Vec::new();
            } else {
                self.buf.push_str(rest);
                return self.take().into_iter().collect();
            }
        }
    }

    /// EOF: an unterminated paste or continuation is sent as it stands.
    fn finish(&mut self) -> Option<String> {
        self.paste = false;
        self.take()
    }

    fn take(&mut self) -> Option<String> {
        let m = std::mem::take(&mut self.buf);
        let m = m.trim_end_matches('\n');
        (!m.trim().is_empty()).then(|| m.to_string())
    }
}

/// `panic` (§3.9.1, §3.9.6): over owner.sock, confirmed within 2 s; otherwise the latch, by hand.
fn panic_cmd(p: &Paths) -> u8 {
    match link::request(p, "panic", json!({}), Duration::from_secs(2)) {
        Ok(r) => {
            println!("{}", r["message"].as_str().map_or_else(|| icj(&r).unwrap_or_default(), String::from));
            0
        }
        Err(f) => {
            if let Err(e) = panic::write_latch(&p.rh, now_ms(), "cli_fallback") {
                return fail(1, &format!("the Root did not confirm ({f:?}) and the latch could not be written: {e}"));
            }
            println!("PANIC latched at {} (the Root did not confirm within 2 s): it enters Panicked within 1 s, or at its next start.",
                panic::latch(&p.rh).display());
            0
        }
    }
}

/// `keys set` (egress.md §3.2): the secret only from the TTY with echo off, stored 0600, never echoed.
fn keys_set(p: &Paths, action: bool, id: Option<String>) -> u8 {
    let Some(id) = id.filter(|i| (1..=32).contains(&i.len()) && !i.starts_with('-')
        && i.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-'))) else {
        return fail(64, "keys set [--action] <id>: id must match ^[a-z0-9][a-z0-9-]{0,31}$");
    };
    let dir = p.r(if action { "keys/actions" } else { "keys/providers" });
    if !dir.is_dir() {
        return fail(1, &format!("{} is missing; run init-root install first", dir.display()));
    }
    if !sys::isatty(0) {
        return fail(2, "keys set reads the secret from a TTY only (never from argv, the environment or a pipe)");
    }
    let mut line = String::new();
    let echo = sys::echo_off(0); // before the prompt, so nothing typed after it is echoed
    eprint!("secret for {id} (input hidden): ");
    let read = echo.and_then(|_g| io::stdin().read_line(&mut line));
    eprintln!();
    let mut b = std::mem::take(&mut line).into_bytes();
    while b.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
        b.pop();
    }
    let ok = read.is_ok() && (1..=512).contains(&b.len()) && std::str::from_utf8(&b).is_ok_and(|s| !s.chars().any(char::is_control));
    let res = if ok { util::write_atomic(&dir.join(&id), &b, 0o600) } else { Err(util::bad("must be 1–512 bytes of UTF-8 without control characters")) };
    sys::zero(&mut b);
    if let Err(e) = res {
        return fail(1, &format!("secret not stored: {e}"));
    }
    let _ = link::request(p, "keys.reload", json!({}), T);
    println!("stored {}/{id} (mode 0600)", if action { "keys/actions" } else { "keys/providers" });
    0
}

/// `keys check`: present / insecure / missing for each key file and each charter provider; never a secret.
fn keys_check(p: &Paths) -> u8 {
    let mut ids: std::collections::BTreeSet<String> = ["providers/deepseek".to_string()].into();
    if let Ok(l) = charter::load(&p.r("state/charter.active.toml"), &p.ctx()) {
        for c in l.ch.actions.values().filter_map(|c| c.get("providers")?.as_table()) {
            ids.extend(c.keys().map(|k| format!("providers/{k}")));
        }
    }
    for d in ["providers", "actions"] {
        let names = fs::read_dir(p.r(&format!("keys/{d}"))).into_iter().flatten().flatten();
        ids.extend(names.map(|e| format!("{d}/{}", e.file_name().to_string_lossy())));
    }
    for id in ids {
        let state = match fs::symlink_metadata(p.r(&format!("keys/{id}"))) {
            Err(_) => "missing",
            Ok(_) if util::read_secret(&p.r(&format!("keys/{id}"))).is_ok_and(|mut b| { let n = b.len(); sys::zero(&mut b); (1..=512).contains(&n) }) => "present",
            Ok(_) => "insecure",
        };
        println!("{id}: {state}");
    }
    0
}

/// Status from the files when no Root answers (`slot: "down"`).
fn offline(p: &Paths) -> Value {
    let l = charter::load(&p.r("state/charter.active.toml"), &p.ctx()).ok();
    let level = ledger::Level::fold(&lineage::records(&p.rh).unwrap_or_default());
    let max = l.as_ref().map_or(0, |l| l.ch.autonomy.max_level);
    let latch: Option<Value> = fs::read_to_string(panic::latch(&p.rh)).ok().map(|s| serde_json::from_str(&s).unwrap_or(json!({})));
    json!({"v": 1, "slot": "down", "panic": latch, "level": level.level, "cap": ledger::cap(max), "max_level": max,
        "charter_sha256": l.as_ref().map(|l| &l.digest), "ledger": level.json(max), "root": null})
}

fn status(p: &Paths, json_out: bool, proposals: bool, selftest: bool) -> u8 {
    if selftest {
        return fail(1, "status --selftest is not built yet (the RR-1/RR-2 probes run in init-sbx-selftest, WP6a)");
    }
    let mut v = match link::request(p, "status", json!({}), T) {
        Ok(v) => v,
        Err(Fail::Unreachable(_)) => offline(p),
        Err(f) => return unreachable_or(f),
    };
    if v.is_object() && v["evolution"].is_null() {
        v["evolution"] = install::evolution(p);
    }
    if json_out {
        println!("{}", icj(&v).unwrap_or_default());
    } else {
        let s = |k: &str| v[k].as_str().map_or_else(|| v[k].to_string(), String::from);
        println!("slot {} · mode {} · level L{} (cap L{}, charter max L{})", s("slot"), s("mode"), s("level"), s("cap"), s("max_level"));
        println!("panic {} · charter {}{}", if v["panic"].is_null() { "none".into() } else { icj(&v["panic"]).unwrap_or_default() },
            s("charter_sha256"), if v["charter_pending"] == true { " (charter.toml has unapplied edits)" } else { "" });
        let k = &v["kernel"];
        println!("tuples {} · kernel {} · now {}", icj(&v["tuples"]).unwrap_or_default(),
            if k.is_null() { "not running".into() } else { format!("pid {} ({})", k["pid"], s("slot")) },
            v["root"]["now"].as_str().map_or_else(|| rfc3339(now_ms()), String::from));
        if let Some(line) = access_line(&v["access"], &p.r("charter.toml")) {
            println!("{line}");
        }
        let ev = &v["evolution"]; // from install.json (install::evolution)
        let tiers = ev["tiers"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("+")).unwrap_or_default();
        match ev["hint"].as_str() {
            Some(h) => println!("self-evolution {tiers} · {h}"),
            None => println!("self-evolution {tiers} · toolchain {}", ev["toolchain"].as_str().unwrap_or("")),
        }
    }
    if proposals {
        let mut ps: Vec<_> = fs::read_dir(p.r("state/proposals")).into_iter().flatten().flatten().map(|e| e.path()).collect();
        ps.sort();
        for f in ps.iter().filter(|f| f.extension().is_some_and(|x| x == "md")) {
            let title = fs::read_to_string(f).ok().and_then(|s| s.lines().next().map(|l| l.trim_start_matches("# ").to_string())).unwrap_or_default();
            println!("proposal {} · {title} · {}", f.file_stem().unwrap_or_default().to_string_lossy(), f.display());
        }
    }
    0
}

/// ADR-012: the human `status` line for the access mode (`status.access`, an AccessView), with how to change it. None
/// when the Root is down (no view).
fn access_line(a: &Value, charter: &std::path::Path) -> Option<String> {
    let mode = a["mode"].as_str()?;
    let ws = a["workspace"].as_str().unwrap_or("none");
    let ch = charter.display();
    Some(match (a["full"] == true, mode, a["allow_full"] == true) {
        (true, "full", _) => format!("access: FULL (default) · workspace {ws} · to confine it: init-agent --workspace, /full off, or sandbox.mode = \"workspace\" in {ch}"),
        (true, _, _) => format!("access: FULL (this session) · workspace {ws} · /full off or a new init-agent launch ends it"),
        (false, _, false) => format!("access: WORKSPACE · workspace {ws} · full access is off in the charter (sandbox.allow_full = false in {ch})"),
        (false, "full", _) => format!("access: WORKSPACE (this session) · workspace {ws} · /full on, or init-agent without --workspace, gives full access"),
        (false, _, _) => format!("access: WORKSPACE (default) · workspace {ws} · full access: /full on, or sandbox.mode = \"full\" in {ch}"),
    })
}

#[cfg(test)]
mod access_line_tests {
    use super::*;
    #[test]
    fn status_shows_the_access_mode_and_how_to_change_it() {
        let ch = std::path::Path::new("/h/.init-root/charter.toml");
        let l = |v: Value| access_line(&v, ch).unwrap();
        assert_eq!(l(json!({"full": true, "mode": "full", "allow_full": true, "workspace": "/h/p"})),
            "access: FULL (default) · workspace /h/p · to confine it: init-agent --workspace, /full off, or sandbox.mode = \"workspace\" in /h/.init-root/charter.toml");
        assert!(l(json!({"full": true, "mode": "workspace", "allow_full": true})).starts_with("access: FULL (this session) · workspace none"));
        assert!(l(json!({"full": false, "mode": "full", "allow_full": true})).starts_with("access: WORKSPACE (this session)"));
        assert!(l(json!({"full": false, "mode": "workspace", "allow_full": true})).starts_with("access: WORKSPACE (default)"));
        assert!(l(json!({"full": false, "mode": "full", "allow_full": false})).contains("sandbox.allow_full = false"));
        assert_eq!(access_line(&Value::Null, ch), None, "the Root is down: no line");
    }
}

/// `charter check [<path>] [--apply [--yes]]` (§3.12.4).
fn charter_check(p: &Paths, path: Option<std::path::PathBuf>, apply: bool, yes: bool) -> u8 {
    let own = p.r("charter.toml");
    let path = path.unwrap_or_else(|| own.clone());
    let l = match charter::load(&path, &p.ctx()) {
        Ok(l) => l,
        Err(fs) => {
            fs.iter().for_each(|f| eprintln!("{f}"));
            return fail(2, &format!("{} is invalid ({} errors)", path.display(), fs.iter().filter(|f| !f.warn).count()));
        }
    };
    l.warnings.iter().for_each(|w| eprintln!("{w}"));
    let short = |s: &str| if s.chars().count() > 96 { format!("{}… ({} bytes)", s.chars().take(80).collect::<String>(), s.len()) } else { s.to_string() };
    let mut flat = std::collections::BTreeMap::new();
    charter::flatten(&charter::effective(&l.ch), "", &mut flat);
    flat.iter().for_each(|(k, v)| println!("{k} = {}", short(v)));
    let active = charter::load(&p.r("state/charter.active.toml"), &p.ctx()).ok();
    let d = charter::diff(active.as_ref().map(|a| charter::effective(&a.ch)).as_ref(), &charter::effective(&l.ch));
    println!("charter_sha256 = {}\n{} change(s) against the active charter:", l.digest, d.len());
    for (k, old, new) in &d {
        println!("  {k}: {} → {}", short(old.as_deref().unwrap_or("(unset)")), short(new.as_deref().unwrap_or("(unset)")));
    }
    if !apply {
        return 0;
    }
    if path != own {
        return fail(2, "--apply applies ROOT_HOME/charter.toml only (the Root re-reads its own file)");
    }
    if d.is_empty() {
        println!("nothing to apply");
        return 0;
    }
    if !yes {
        if !sys::isatty(0) {
            return fail(2, "not a TTY: pass --yes to apply");
        }
        eprint!("apply? [y/N] ");
        let mut ans = String::new();
        if io::stdin().read_line(&mut ans).is_err() || !matches!(ans.trim(), "y" | "Y" | "yes") {
            return fail(1, "not applied");
        }
    }
    match apply_own_charter(p) {
        Ok(v) => out(&v, 0),
        Err((code, m)) => fail(code, &m),
    }
}

/// `charter.apply` of ROOT_HOME/charter.toml: through the running Root, else in this process under the Root's own
/// lock with the same code (also `providers add`, UX-KEYMODEL).
pub(crate) fn apply_own_charter(p: &Paths) -> Result<Value, (u8, String)> {
    match link::request(p, "charter.apply", json!({}), T) {
        Err(Fail::Unreachable(_)) => match take_lock(p) {
            Ok(Some(_lock)) => Root::boot(p.clone()).map_err(|e| e.to_string()).and_then(|r| r.apply_charter().map_err(|(_, m)| m)).map_err(|m| (2, m)),
            _ => Err((1, "owner.sock is unreachable but a Root holds run/root.lock".into())),
        },
        Err(Fail::Refused(_, m)) => Err((2, m)),
        Err(Fail::Protocol(m)) => Err((76, format!("protocol error: {m}"))),
        Ok(v) => Ok(v),
    }
}

#[cfg(test)]
mod tests {
    use super::Assembler;

    fn run(lines: &[&str]) -> Vec<String> {
        let mut a = Assembler::default();
        let mut out: Vec<String> = lines.iter().flat_map(|l| a.push(l)).collect();
        out.extend(a.finish());
        out
    }

    #[test]
    fn plain_lines_are_one_message_each_and_blank_lines_are_not_sent() {
        assert_eq!(run(&["hello", "", "   ", "world"]), ["hello", "world"]);
        assert_eq!(run(&["/panic"]), ["/panic"]);
    }

    #[test]
    fn a_paste_across_three_lines_is_one_message() {
        assert_eq!(run(&["\x1b[200~first", "second", "third\x1b[201~", "after"]), ["first\nsecond\nthird", "after"]);
        // the closing marker alone on the next line (the paste ended with a newline)
        assert_eq!(run(&["\x1b[200~a", "b", "\x1b[201~"]), ["a\nb"]);
        // a backslash inside a paste is literal
        assert_eq!(run(&["\x1b[200~x \\", "y\x1b[201~"]), ["x \\\ny"]);
    }

    #[test]
    fn a_paste_may_start_and_end_mid_line() {
        assert_eq!(run(&["look: \x1b[200~one", "two\x1b[201~ thanks"]), ["look: one\ntwo thanks"]);
        assert_eq!(run(&["a\x1b[200~b\x1b[201~c\x1b[200~d\x1b[201~e"]), ["abcde"]);
        assert_eq!(run(&["\x1b[200~\x1b[201~"]), Vec::<String>::new(), "an empty paste sends nothing");
    }

    #[test]
    fn a_trailing_backslash_continues_the_message() {
        assert_eq!(run(&["one \\", "two\\", "three", "four"]), ["one \ntwo\nthree", "four"]);
        assert_eq!(run(&["literal \\\\", "next"]), ["literal \\\\", "next"]);
        assert_eq!(run(&["pasted \x1b[200~p\\\x1b[201~", "next"]), ["pasted p\\", "next"], "the `\\` came from the paste");
        assert_eq!(run(&["dangling \\"]), ["dangling "], "EOF ends a continuation");
    }

    #[test]
    fn eof_inside_a_paste_sends_it_as_one_message() {
        assert_eq!(run(&["\x1b[200~a", "b"]), ["a\nb"]);
        assert_eq!(run(&["\x1b[200~"]), Vec::<String>::new());
    }

    #[test]
    fn crlf_is_tolerated() {
        assert_eq!(run(&["hello\r", "\x1b[200~a\r", "b\r\nc\rd\x1b[201~\r", "x \\\r", "y\r"]), ["hello", "a\nb\nc\nd", "x \ny"]);
    }
}
