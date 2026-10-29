//! WP13 awareness tests (docs/awareness.md §8): T-AW-1, 3, 5, 6, 7, 8 (partial), 12, 13, 14, 15,
//! plus the stats fetch and the env diff. Every test home lives under
//! `<target>/test-homes/wp13/` (`$CARGO_TARGET_DIR` when set, as in G0) and is removed by its exact
//! path; every process spawned is a child of these tests.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::lint::{approval_hits, front_matter, volatility_hits};
use super::*;
use crate::context::SignalSource;
use crate::context::model::Thresholds;

const GENESIS: &str = include_str!("../../../GENESIS.md");
const SELF_MD: &str = include_str!("../../../SELF.md");
const CONSTITUTION: &str = include_str!("../CONSTITUTION.md");

const SHA_NEW: &str = "7c1d2e9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e";
const SHA_OLD: &str = "3f2a1c0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b";
const SHA_BAD: &str = "9d0e4b7c6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d";
const SID: &str = "20261019-203015-9f3a2c";
const PREV_ID: &str = "0b6f1c2d3e4f5061";
const PREFIX: &str = "sha256:9b1e04c2d7aa5f3e8c1d0b2a4e6f8a0c1e3b5d7f9a2c4e6b8d0f1a3c5e7b9d0f";
const OTHER_PREFIX: &str =
    "sha256:1111111111115f3e8c1d0b2a4e6f8a0c1e3b5d7f9a2c4e6b8d0f1a3c5e7b9d0f";

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// awareness.md §4.2 `kernel` (with the §4.5 root fields).
fn kernel_env() -> Value {
    json!({
        "budget": {"cap_musd": 4000000, "day_starts_at": "2026-10-19T17:00:00Z", "today_musd": 1230000},
        "init_home": "/Users/q/.init", "kernel_version": "0.1.0", "mem_total_mb": 16384,
        "model": {"context_window": 1000000, "id": "deepseek-flash", "max_output": 384000, "provider": "deepseek"},
        "network": {"egress": [], "mode": "proxy-only"},
        "os": {"arch": "arm64", "family": "macos", "version": "14.6"},
        "root": {"audited_good": SHA_OLD, "cap": 0,
                 "ceilings": {"day_musd": 5000000, "parallel_bodies": 1, "total_musd": 25000000, "total_used_musd": 7400000},
                 "classes": ["model-providers"], "constitution_sha256": "sha256:5c0e", "last_change": null,
                 "level": 0, "max_level": 4, "next": null, "supported_max_level": 0},
        "sandbox": {"hidden": ["cache/", "probe/", "run/owner.sock", "vault/"], "kind": "seatbelt", "landlock": null,
                    "landlock_abi": null, "network": "kernel-sockets-only",
                    "read_only": ["bin/", "kernel/", "lineage/", "state/kernel/"], "seccomp": false, "sweep": "full",
                    "write": ["body/", "state/", "work/"]}
    })
}

fn bash_fact() -> Value {
    json!({"path": "/opt/homebrew/bin/bash", "version": "GNU bash, version 5.3.20(1)-release"})
}

/// awareness.md §4.2 `probed`.
fn probed(date: &str) -> Value {
    json!({
        "bash": bash_fact(),
        "clis": ["awk","bash","cargo","cc","find","git","grep","jq","make","patch","perl","python3","rg","rustc","rustup","sed","tar"],
        "clis_missing": ["curl","gh","node","npm","sqlite3","uv","wasm-tools","wasmtime"],
        "cpus": 8, "cwd": "/Users/q/.init/work", "date_utc": date,
        "disk_free_mb": 9312, "effective_window": 200000, "effort": "high", "owner_offset": "+07:00",
        "versions": {"cargo": "cargo 1.97.1", "git": "git version 2.39.5", "rustc": "rustc 1.97.1"},
        "wasm32_wasip2": true
    })
}

/// A promote boot.json (supervise.md §4.3 shape) matching the §3.6 example.
fn boot_json() -> Value {
    let mut env = kernel_env();
    env["budget"]["today_musd"] = json!(2100000);
    json!({
        "body_id": "b0", "boot_id": "a3f09c1e5b7d2468", "boot_seq": 57, "branch": "evo/0007", "cause": null,
        "crash": null, "env": env, "evolution": {}, "from_sha": SHA_OLD, "kernel_upgrade": null,
        "lkg_sha": SHA_OLD, "mode": "normal",
        "model": {"effort": null, "model": "deepseek-flash", "provider": "deepseek"}, "panic": null,
        "provisional": {"depth": 1, "g4": "queued"}, "reason": "promote", "restarts_since_lkg": 0,
        "safe": null, "selfmap": {"body_tokens": 21904, "body_tokens_prev": 21492}, "session": SID,
        "sha": SHA_NEW, "update_failed": null, "v": 1
    })
}

/// The previous boot event on the path (log-context §4.2 fields the block reads).
fn prev_event(env_kernel: Value, date: &str) -> Value {
    json!({
        "env": {"kernel": env_kernel, "probed": probed(date), "v": 1},
        "id": PREV_ID, "model": "deepseek-flash", "prefix_hash": PREFIX, "provider": "deepseek",
        "type": "boot"
    })
}

struct Fx {
    boot: Value,
    probed: Value,
    prev: Option<Value>,
    git_sync: Value,
    capability: Option<Value>,
    mode: &'static str,
    session_created: bool,
    rehydrated: u64,
    interrupted: u64,
    update_failed: Option<Value>,
    model: &'static str,
    prefix: &'static str,
    access: Option<&'static str>,
    workspace: Option<PathBuf>,
}

impl Fx {
    /// The §3.6 promote example.
    fn promote() -> Fx {
        Fx {
            boot: boot_json(),
            probed: probed("2026-10-19T22:04Z"),
            prev: Some(prev_event(kernel_env(), "2026-10-19T20:30Z")),
            git_sync: json!({"action": "ff", "drift": false, "error": null, "fetched": true, "rolled_back": null, "stash": null}),
            capability: None,
            mode: "normal",
            session_created: false,
            rehydrated: 1284,
            interrupted: 0,
            update_failed: None,
            model: "deepseek-flash",
            prefix: PREFIX,
            access: None,
            workspace: None,
        }
    }

    /// The same boot with another reason, running `sha` after `from` (provisional cleared).
    fn with(reason: &str, sha: &str, from: Option<&str>) -> Fx {
        let mut fx = Fx::promote();
        fx.boot["reason"] = json!(reason);
        fx.boot["sha"] = json!(sha);
        fx.boot["from_sha"] = json!(from);
        fx.boot["branch"] = Value::Null;
        fx.boot["provisional"] = Value::Null;
        fx
    }

    fn text(&self) -> String {
        let env = snapshot(&self.boot, self.probed.clone());
        let b = BlockIn {
            boot: &self.boot,
            sha: self.boot["sha"].as_str().unwrap(),
            mode: self.mode,
            reason: self.boot["reason"].as_str().unwrap(),
            provider: "deepseek",
            model: self.model,
            effort: "high",
            session: SID,
            session_created: self.session_created,
            rehydrated_events: self.rehydrated,
            interrupted_calls: self.interrupted,
            prefix_hash: self.prefix,
            prev_boot: self.prev.as_ref().map(|p| (PREV_ID, p)),
            env: &env,
            git_sync: &self.git_sync,
            update_failed: self.update_failed.as_ref(),
            capability: self.capability.as_ref(),
            access: self.access,
            workspace: self.workspace.as_deref(),
        };
        boot_text(&b)
    }

    fn line(&self, i: usize) -> String {
        self.text().lines().nth(i).unwrap_or_default().to_string()
    }

    /// The first line starting with `prefix`, if any.
    fn find(&self, prefix: &str) -> Option<String> {
        self.text()
            .lines()
            .find(|l| l.starts_with(prefix))
            .map(str::to_string)
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The build's target dir: `$CARGO_TARGET_DIR` when set (the gate's G0 sets it to the build slot's
/// `out/target`, the only writable tree there; the source export is read-only), else
/// `<crate>/target`.
fn test_target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
}

/// A temporary directory under `<target>/test-homes/wp13/`, removed by its exact path.
struct TempHome {
    path: PathBuf,
}

impl TempHome {
    fn new(name: &str) -> TempHome {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = test_target()
            .join("test-homes/wp13")
            .join(format!("aw-{name}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        TempHome { path }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_exec(path: &Path, body: &str, mode: u32) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// 2026-10-19T20:30:42Z.
fn fixed_now() -> SystemTime {
    let secs = days_from_civil(2026, 10, 19) * 86_400 + 20 * 3600 + 30 * 60 + 42;
    UNIX_EPOCH + Duration::from_secs(secs as u64)
}

const ENV_FULL_GOLDEN: &str = "Environment (\"probed\" = measured inside your sandbox at boot; the rest comes from the kernel):
- date: 2026-10-19 20:30 UTC (owner's time: 2026-10-20 03:30, +07:00)
- os: macos 14.6 arm64 · 16384 MB · 8 cpus (probed)
- home: /Users/q/.init (INIT_HOME) · cwd: /Users/q/.init/work · bash: GNU 5.3.20 (/opt/homebrew/bin/bash)
- sandbox: seatbelt · write: body/ state/ work/ · read-only: bin/ kernel/ lineage/ state/kernel/ · hidden: cache/ probe/ run/owner.sock vault/
- network: through the Root's egress only (model providers; dependencies via `initctl vendor`)
- model: deepseek/deepseek-flash (effort high) · window 1,000,000 · compaction plans for 200,000 · max output 384,000
- budget: $1.23 of $4.00 spent today (day starts 17:00 UTC; your soft day budget, clamped under the Root's $5.00 ceiling)
- kernel: 0.1.0
- on PATH (probed): awk bash cargo cc find git grep jq make patch perl python3 rg rustc rustup sed tar
- not found (probed): curl gh node npm sqlite3 uv wasm-tools wasmtime
- versions (probed): rustc 1.97.1 · cargo 1.97.1 · git 2.39.5 · wasm32-wasip2 target: installed
- disk free (probed): 9,312 MB";

const ENV_SHORT_GOLDEN: &str = "Environment: unchanged since boot 0b6f1c2d3e4f5061, except date 2026-10-19 22:04 UTC (owner 05:04 +07:00) · budget $2.10 of $4.00.";

// ---------------------------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn integer_formatting() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1_000), "1,000");
    assert_eq!(thousands(1_000_000), "1,000,000");
    assert_eq!(thousands(9_312), "9,312");
    assert_eq!(dollars(1_230_000), "$1.23");
    assert_eq!(dollars(0), "$0.00");
    assert_eq!(dollars(9_999), "$0.00");
    assert_eq!(dollars(25_000_000), "$25.00");
    assert_eq!(dollars(1_234_567_890), "$1234.56");
    assert_eq!(signed(412), "+412");
    assert_eq!(signed(-1_412), "-1,412");
    assert_eq!(signed(0), "+0");
    assert_eq!(sha7(SHA_NEW), "7c1d2e9");
    assert_eq!(sha7("abc"), "abc");
}

#[test]
fn utc_minutes_round_trip() {
    for (t, want) in [
        ("2026-10-19T20:30Z", "2026-10-19 20:30"),
        ("2026-10-19T20:30:15Z", "2026-10-19 20:30"),
        ("2026-10-19T12:36:01.004Z", "2026-10-19 12:36"),
        ("2024-02-29T23:59Z", "2024-02-29 23:59"),
        ("1970-01-01T00:00Z", "1970-01-01 00:00"),
    ] {
        assert_eq!(minute_text(parse_utc_minute(t).unwrap()), want, "{t}");
    }
    for bad in [
        "",
        "2026-10-19",
        "2026-10-19T20:30",
        "2026-13-19T20:30Z",
        "2026-10-19T24:00Z",
        "x026-10-19T20:30Z",
        "2026-10-19T20:30+07:00",
    ] {
        assert_eq!(parse_utc_minute(bad), None, "{bad}");
    }
    // The owner's time crosses midnight and month ends.
    let m = parse_utc_minute("2026-10-31T20:30Z").unwrap();
    assert_eq!(minute_text(m + 420), "2026-11-01 03:30");
    let m = parse_utc_minute("2026-11-01T02:00Z").unwrap();
    assert_eq!(minute_text(m - 300), "2026-10-31 21:00");
    assert_eq!(date_utc(fixed_now()), json!("2026-10-19T20:30Z"));
}

#[test]
fn owner_offset_from_the_day_boundary() {
    let boot_at = |t: &str| json!({"env": {"budget": {"day_starts_at": t}}});
    for (t, want) in [
        ("2026-10-19T17:00:00Z", json!("+07:00")),
        ("2026-10-19T05:00:00Z", json!("-05:00")),
        ("2026-10-19T00:00:00Z", json!("+00:00")),
        ("2026-10-19T10:00:00Z", json!("-10:00")),
        ("2026-10-19T11:00:00Z", json!("+13:00")),
        ("2026-10-19T18:30:00.000Z", json!("+05:30")),
        ("garbage", Value::Null),
    ] {
        assert_eq!(owner_offset(&boot_at(t)), want, "{t}");
    }
    assert_eq!(owner_offset(&json!({})), Value::Null);
    let mut b = boot_at("2026-10-19T17:00:00Z");
    b["env"]["root"] = json!({"ceilings": {"day_starts_at_utc_offset": "+09:00"}});
    assert_eq!(owner_offset(&b), json!("+09:00"), "the Root's offset wins");
}

// ---------------------------------------------------------------------------------------------
// T-AW-1, T-AW-3, T-AW-8 (partial), T-AW-15: the head files
// ---------------------------------------------------------------------------------------------

#[test]
fn t_aw_1_genesis_within_byte_budget() {
    assert!(
        GENESIS.len() <= 1_400,
        "GENESIS.md is {} bytes",
        GENESIS.len()
    );
}

#[test]
fn t_aw_3_lint_rules() {
    let rules = |t: &str, levels: bool| -> Vec<&'static str> {
        volatility_hits(t, levels)
            .into_iter()
            .map(|h| h.0)
            .collect()
    };
    assert_eq!(rules("on 2026-10-19 we", false), ["date"]);
    let times = volatility_hits("at 20:30 or 20:30:15", false);
    assert!(times.iter().all(|h| h.0 == "time"), "{times:?}");
    let texts: Vec<&str> = times.iter().map(|h| h.2.as_str()).collect();
    assert!(
        texts.contains(&"20:30") && texts.contains(&"20:30:15"),
        "{texts:?}"
    );
    assert_eq!(rules("sha 3f2a1c0 and deadbeef", false), ["hex", "hex"]);
    assert_eq!(rules("costs $5 now", false), ["money"]);
    assert_eq!(rules("stamp 20261019T203015Z.", false), ["stamp"]);
    assert_eq!(rules("you are L0, then L4", true), ["level", "level"]);
    assert!(
        rules("you are L0", false).is_empty(),
        "levels only when asked"
    );
    for clean in [
        "x2026-10-19",
        "sha256 and abcdefg and 3f2a1c",
        "$INIT_SESSION and $x",
        "L5 L01 TL0 T0 T1 T2",
        "version 1.97.1, 120:300",
        "body/ state/sessions/<id>.jsonl",
    ] {
        assert!(
            volatility_hits(clean, true).is_empty(),
            "{clean}: {:?}",
            volatility_hits(clean, true)
        );
    }
}

#[test]
fn t_aw_3_head_files_carry_nothing_volatile() {
    assert_eq!(volatility_hits(GENESIS, true), vec![], "GENESIS.md");
    assert_eq!(volatility_hits(SELF_MD, true), vec![], "SELF.md");
    // The constitution names the levels by design: no level rule there.
    assert_eq!(
        volatility_hits(CONSTITUTION, false),
        vec![],
        "CONSTITUTION.md"
    );
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");
    if let Ok(rd) = std::fs::read_dir(&skills) {
        for e in rd.flatten() {
            let f = e.path().join("SKILL.md");
            if let Ok(t) = std::fs::read_to_string(&f) {
                let fm = front_matter(&t);
                assert_eq!(volatility_hits(&fm, false), vec![], "{}", f.display());
            }
        }
    }
    assert_eq!(
        front_matter("---\nname: x\ndescription: y\n---\nbody 2026-10-19"),
        "name: x\ndescription: y"
    );
    assert_eq!(front_matter("no front matter"), "");
}

#[test]
fn t_aw_8_constitution_word_budget() {
    let words = CONSTITUTION.split_whitespace().count();
    assert!(words <= 650, "CONSTITUTION.md has {words} words");
}

#[test]
fn t_aw_15_no_request_for_approval() {
    assert_eq!(approval_hits(GENESIS), Vec::<String>::new(), "GENESIS.md");
    assert_eq!(
        approval_hits(CONSTITUTION),
        Vec::<String>::new(),
        "CONSTITUTION.md"
    );
    assert_eq!(
        approval_hits(
            "Please Ask your\n   owner first. The OWNER APPROVES. NEVER do it. CRITICAL."
        ),
        ["ask your owner", "owner approves", "CRITICAL", "NEVER"]
    );
    assert!(approval_hits("never critical; NEVERMORE; xCRITICAL").is_empty());
}

// ---------------------------------------------------------------------------------------------
// T-AW-5: environment snapshot
// ---------------------------------------------------------------------------------------------

/// Two PATH dirs: fake rustc (with a sysroot holding the wasm32-wasip2 target), cargo, git and
/// df scripts, empty executables for the rest of §4.2's `clis`, a non-executable `curl` and a
/// directory named `gh`.
fn fake_path(home: &Path, git_body: &str) -> String {
    let (b1, b2) = (home.join("bin1"), home.join("bin2"));
    std::fs::create_dir_all(&b1).unwrap();
    std::fs::create_dir_all(&b2).unwrap();
    let sysroot = home.join("sysroot");
    std::fs::create_dir_all(sysroot.join("lib/rustlib/wasm32-wasip2")).unwrap();
    write_exec(
        &b1.join("rustc"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--print\" ]; then echo \"{}\"; else echo \"rustc 1.97.1 (8bab26f4f 2026-07-14)\"; fi\n",
            sysroot.display()
        ),
        0o755,
    );
    write_exec(
        &b1.join("cargo"),
        "#!/bin/sh\necho \"cargo 1.97.1 (c980f4866 2026-06-30)\"\n",
        0o755,
    );
    write_exec(&b2.join("git"), git_body, 0o755);
    write_exec(
        &b1.join("df"),
        "#!/bin/sh\necho \"Filesystem 1024-blocks Used Available Capacity Mounted on\"\necho \"/dev/disk3s1 1000 1 9535488 1% /\"\n",
        0o755,
    );
    for n in [
        "awk", "bash", "cc", "find", "grep", "jq", "make", "patch", "perl", "python3", "rg",
        "rustup", "sed", "tar",
    ] {
        write_exec(&b2.join(n), "", 0o755);
    }
    write_exec(&b1.join("curl"), "#!/bin/sh\n", 0o644);
    std::fs::create_dir_all(b1.join("gh")).unwrap();
    let pv = format!("{}::{}:/nonexistent-wp13", b1.display(), b2.display());
    warm(&b1.join("rustc"), &["-V"], &pv);
    warm(&b1.join("rustc"), &["--print", "sysroot"], &pv);
    warm(&b1.join("cargo"), &["-V"], &pv);
    warm(&b1.join("df"), &["-Pk", "/"], &pv);
    pv
}

/// Runs a fixture script once, untimed. macOS checks a freshly written executable on its first
/// exec (provenance / syspolicyd: about 200 ms when idle, seconds when a parallel test run writes
/// and execs many scripts at once), and that latency, not the probe, decided whether the old
/// tests' fake `rustc` beat the 2 s probe timeout. Warming keeps the timeout under test.
/// The exit code is the fixture's business (the failing `cargo` exits 3).
fn warm(prog: &Path, args: &[&str], path_var: &str) {
    if let Err(e) = std::process::Command::new(prog)
        .args(args)
        .env("PATH", path_var)
        .stdin(std::process::Stdio::null())
        .output()
    {
        panic!("warm {}: {e}", prog.display());
    }
}

fn probe_with(home: &Path, path_var: &str, timeout: Duration) -> Value {
    let boot = boot_json();
    let bash = bash_fact();
    probe_env(&ProbeIn {
        home,
        cwd: Path::new("/Users/q/.init/work"),
        bash: &bash,
        path_var: Some(path_var),
        effective_window: 200_000,
        effort: "high",
        boot: &boot,
        now: fixed_now(),
        timeout,
    })
}

#[test]
fn t_aw_5_probe_with_fake_path_matches_the_fixture() {
    let h = TempHome::new("probe");
    let pv = fake_path(
        &h.path,
        "#!/bin/sh\necho \"git version 2.39.5 (Apple Git-154)\"\n",
    );
    warm(&h.path.join("bin2/git"), &["--version"], &pv);
    let t0 = Instant::now();
    let mut got = probe_with(&h.path, &pv, Duration::from_secs(5));
    let took = t0.elapsed();
    eprintln!("probe_env with fake PATH took {took:?}");
    assert!(got["cpus"].as_u64().is_some_and(|n| n >= 1));
    got["cpus"] = json!(8);
    let mut want = probed("2026-10-19T20:30Z");
    want["versions"] = json!({"cargo": "cargo 1.97.1 (c980f4866 2026-06-30)", "git": "git version 2.39.5 (Apple Git-154)", "rustc": "rustc 1.97.1 (8bab26f4f 2026-07-14)"});
    assert_eq!(got, want);
    // Integers only, sorted keys (ICJ).
    assert_eq!(crate::log::icj::find_float(&got), None);
    let snap = snapshot(&boot_json(), got.clone());
    assert_eq!(
        snap["kernel"],
        boot_json()["env"],
        "kernel object is boot.json.env verbatim"
    );
    assert_eq!(snap["v"], json!(1));
    // Bounded by the per-probe timeout (the probes run in parallel). Not the spec's < 300 ms:
    // a freshly written script's first exec is slow on a loaded Mac, so this test measures the
    // fixture, not the probe; `t_aw_5_probe_on_this_machine` prints the real-tool time.
    assert!(took < Duration::from_secs(10), "probe took {took:?}");
}

#[test]
fn t_aw_5_probe_timeout_and_failure_give_unknowns() {
    let h = TempHome::new("probe-timeout");
    // git really hangs (absolute /bin/sleep: the fake PATH has no `sleep`, and an exit 127 would
    // only test the non-zero rule again) far past the 2 s budget. `took >= timeout` proves the null
    // below is the timeout and not a spawn failure (that returns at once). The mark it leaves is only
    // a diagnostic: on a loaded Mac the probe can kill the script before its first line runs
    // (REL-051, load 76), which is the timeout path working, so its absence is not a failure.
    let mark = h.path.join("git-started");
    let pv = fake_path(
        &h.path,
        &format!("#!/bin/sh\n: > '{}'\nexec /bin/sleep 30\n", mark.display()),
    );
    write_exec(
        &h.path.join("bin1/cargo"),
        "#!/bin/sh\necho cargo 1.0\nexit 3\n",
        0o755,
    );
    warm(&h.path.join("bin1/cargo"), &["-V"], &pv);
    let timeout = Duration::from_secs(2);
    let t0 = Instant::now();
    let got = probe_with(&h.path, &pv, timeout);
    let took = t0.elapsed();
    if !mark.exists() {
        eprintln!("note: the probe's timeout killed the fake git before it started ({took:?})");
    }
    assert!(
        took >= timeout - Duration::from_millis(50),
        "the probe waited for git's timeout: {took:?}"
    );
    // The upper bound is the sibling test's 10 s: the timeout must bound the probe (git sleeps 30 s),
    // and the kill-and-reap after it is slow on a loaded Mac.
    assert!(
        took < Duration::from_secs(10),
        "the timeout bounds the probe: {took:?}"
    );
    assert_eq!(got["versions"]["git"], Value::Null, "timeout → null");
    assert_eq!(
        got["versions"]["cargo"],
        Value::Null,
        "non-zero exit → null"
    );
    assert_eq!(
        got["versions"]["rustc"],
        json!("rustc 1.97.1 (8bab26f4f 2026-07-14)")
    );
    let env = json!({"kernel": kernel_env(), "probed": got, "v": 1});
    let full = env_full(&env);
    assert!(
        full.contains(
            "- versions (probed): rustc 1.97.1 · cargo ? · git ? · wasm32-wasip2 target: installed"
        ),
        "{full}"
    );
}

#[test]
fn t_aw_5_probe_without_path_or_tools() {
    let h = TempHome::new("probe-empty");
    let boot = json!({});
    let got = probe_env(&ProbeIn {
        home: &h.path,
        cwd: &h.path,
        bash: &Value::Null,
        path_var: None,
        effective_window: 0,
        effort: "high",
        boot: &boot,
        now: fixed_now(),
        timeout: Duration::from_millis(500),
    });
    assert_eq!(got["clis"], json!([]));
    assert_eq!(got["clis_missing"].as_array().unwrap().len(), 25);
    assert_eq!(
        got["versions"],
        json!({"cargo": null, "git": null, "rustc": null})
    );
    assert_eq!(got["wasm32_wasip2"], Value::Null);
    assert_eq!(got["owner_offset"], Value::Null);
    // df falls back to /bin/df or /usr/bin/df.
    assert!(got["disk_free_mb"].as_u64().is_some(), "{got}");
    let env = json!({"kernel": null, "probed": got, "v": 1});
    let full = env_full(&env);
    assert!(full.contains("- os: ? ? ? · ? MB ·"), "{full}");
    assert!(full.contains("bash: none"), "{full}");
    assert!(full.contains("- on PATH (probed): none"), "{full}");
    assert!(full.contains("wasm32-wasip2 target: ?"), "{full}");
}

/// The real machine (our own children only): structure, not values.
#[test]
fn t_aw_5_probe_on_this_machine() {
    let path = std::env::var("PATH").ok();
    let boot = boot_json();
    let t0 = Instant::now();
    let got = probe_env(&ProbeIn {
        home: Path::new(env!("CARGO_MANIFEST_DIR")),
        cwd: Path::new(env!("CARGO_MANIFEST_DIR")),
        bash: &Value::Null,
        path_var: path.as_deref(),
        effective_window: 200_000,
        effort: "high",
        boot: &boot,
        now: SystemTime::now(),
        timeout: Duration::from_secs(2),
    });
    eprintln!("probe_env on this machine took {:?}: {got}", t0.elapsed());
    let names = |k: &str| -> Vec<String> {
        got[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    };
    let (found, missing) = (names("clis"), names("clis_missing"));
    assert_eq!(found.len() + missing.len(), CLI_CANDIDATES.len());
    let mut all = [found.clone(), missing].concat();
    all.sort();
    assert_eq!(all, CLI_CANDIDATES.map(String::from).to_vec());
    assert!(found.windows(2).all(|w| w[0] < w[1]), "sorted");
    assert!(got["disk_free_mb"].as_u64().is_some());
    assert!(
        got["date_utc"]
            .as_str()
            .is_some_and(|d| d.len() == 17 && d.ends_with('Z'))
    );
}

#[test]
fn t_aw_5_golden_full_and_short() {
    let env = json!({"kernel": kernel_env(), "probed": probed("2026-10-19T20:30Z"), "v": 1});
    assert_eq!(env_full(&env), ENV_FULL_GOLDEN);
    let mut k = kernel_env();
    k["budget"]["today_musd"] = json!(2100000);
    let env = json!({"kernel": k, "probed": probed("2026-10-19T22:04Z"), "v": 1});
    assert_eq!(env_short(&env, PREV_ID), ENV_SHORT_GOLDEN);
    // No owner offset: the owner's part is left out.
    let mut p = probed("2026-10-19T22:04Z");
    p["owner_offset"] = Value::Null;
    let env = json!({"kernel": kernel_env(), "probed": p, "v": 1});
    assert_eq!(
        env_short(&env, PREV_ID),
        "Environment: unchanged since boot 0b6f1c2d3e4f5061, except date 2026-10-19 22:04 UTC · budget $1.23 of $4.00."
    );
    assert!(env_full(&env).contains("- date: 2026-10-19 22:04 UTC\n"));
}

#[test]
fn env_changed_ignores_date_budget_and_small_disk_moves() {
    let env = |k: Value, p: Value| json!({"kernel": k, "probed": p, "v": 1});
    let base = env(kernel_env(), probed("2026-10-19T20:30Z"));
    assert!(
        env_changed(&base, None).is_empty(),
        "first boot of the session"
    );
    assert!(env_changed(&base, Some(&base)).is_empty());
    let mut k = kernel_env();
    k["budget"]["today_musd"] = json!(3000000);
    let mut p = probed("2026-10-20T08:00Z");
    p["disk_free_mb"] = json!(9312 + 1023);
    assert!(env_changed(&env(k.clone(), p.clone()), Some(&base)).is_empty());
    p["disk_free_mb"] = json!(9312 - 1024);
    p["clis"] = json!(["awk"]);
    k["os"]["version"] = json!("15.0");
    k["root"]["level"] = json!(1);
    assert_eq!(
        env_changed(&env(k, p), Some(&base)),
        ["clis", "disk_free_mb", "os", "root"]
    );
    // A previous boot without a probed half: every probed field counts as changed.
    let old = json!({"kernel": kernel_env(), "v": 1});
    assert_eq!(
        env_changed(&base, Some(&old)).len(),
        11,
        "12 probed keys less date_utc"
    );
}

#[test]
fn env_form_follows_the_change_rule() {
    // Only date and budget changed: short.
    let fx = Fx::promote();
    assert_eq!(fx.text().lines().last().unwrap(), ENV_SHORT_GOLDEN);
    // A Root-only change (total used) is not in the environment text: still short.
    let mut fx = Fx::promote();
    fx.boot["env"]["root"]["ceilings"]["total_used_musd"] = json!(9000000);
    assert_eq!(fx.text().lines().last().unwrap(), ENV_SHORT_GOLDEN);
    // A shown field changed: full.
    let mut fx = Fx::promote();
    fx.probed["clis_missing"] = json!(["curl"]);
    assert!(fx.text().contains("\nEnvironment (\"probed\""));
    // A previous boot without env, or a cold boot: full.
    let mut fx = Fx::promote();
    fx.prev
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("env");
    assert!(fx.text().contains("\nEnvironment (\"probed\""));
    let mut fx = Fx::with("cold", SHA_OLD, None);
    fx.probed = probed("2026-10-19T20:30Z");
    fx.boot["env"] = kernel_env();
    assert!(fx.text().ends_with(ENV_FULL_GOLDEN));
}

// ---------------------------------------------------------------------------------------------
// T-AW-6: boot block per reason × mode
// ---------------------------------------------------------------------------------------------

#[test]
fn t_aw_6_promote_golden() {
    let want = "[init:boot] You were promoted: 3f2a1c0 → 7c1d2e9 (branch evo/0007). G0–G3 passed; G4 is running. Until it passes you are provisional, so take no external actions. This is the same conversation: continue where you left off.
- you: body 7c1d2e9 (provisional) · LKG 3f2a1c0 · audited-good 3f2a1c0 · model deepseek/deepseek-flash · body 21,904 tokens (+412 vs 3f2a1c0)
- autonomy: L0 (cap L0) · at your cap: charter max L4, this Root supports up to L0
- ceilings (Root): $5.00 a day · $25.00 total, $7.40 used · 1 live body · actions: model providers only
- prompt prefix: 9b1e04c2d7aa (same as before)
- session: 20261019-203015-9f3a2c · 1,284 events rehydrated
Environment: unchanged since boot 0b6f1c2d3e4f5061, except date 2026-10-19 22:04 UTC (owner 05:04 +07:00) · budget $2.10 of $4.00.";
    assert_eq!(Fx::promote().text(), want);
}

#[test]
fn t_aw_6_cold_new_session_golden() {
    let mut fx = Fx::with("cold", SHA_OLD, None);
    fx.boot["env"] = kernel_env();
    fx.boot["selfmap"] = json!({"body_tokens": 21492, "body_tokens_prev": null});
    fx.probed = probed("2026-10-19T20:30Z");
    fx.prev = None;
    fx.session_created = true;
    fx.rehydrated = 0;
    let want = format!(
        "[init:boot] Cold start: no running predecessor handed over to you. New session.
- you: body 3f2a1c0 · LKG 3f2a1c0 · audited-good 3f2a1c0 · model deepseek/deepseek-flash · body 21,492 tokens
- autonomy: L0 (cap L0) · at your cap: charter max L4, this Root supports up to L0
- ceilings (Root): $5.00 a day · $25.00 total, $7.40 used · 1 live body · actions: model providers only
- prompt prefix: 9b1e04c2d7aa (first boot of this session)
- session: 20261019-203015-9f3a2c · 0 events rehydrated
{ENV_FULL_GOLDEN}"
    );
    assert_eq!(fx.text(), want);
}

#[test]
fn t_aw_6_cold_continue_and_after_panic() {
    let mut fx = Fx::with("cold", SHA_OLD, Some(SHA_NEW));
    fx.prefix = OTHER_PREFIX;
    fx.interrupted = 2;
    assert_eq!(
        fx.line(0),
        "[init:boot] Cold start: no running predecessor handed over to you. Continuing session 20261019-203015-9f3a2c."
    );
    assert_eq!(
        fx.line(4),
        "- prompt prefix: 111111111111 (changed, so one cache miss)"
    );
    assert_eq!(
        fx.line(5),
        "- session: 20261019-203015-9f3a2c · 1,284 events rehydrated · 2 tool calls were interrupted; their results say whether they ran"
    );
    assert!(
        fx.text().contains("\nEnvironment (\"probed\""),
        "a cold boot shows the full form"
    );
    fx.interrupted = 1;
    assert!(
        fx.line(5)
            .ends_with(" · 1 tool call was interrupted; its result says whether it ran")
    );

    fx.boot["panic"] = json!({"at": "2026-10-19T21:14:03.120Z", "channel": "owner_endpoint"});
    fx.boot["env"]["root"]["last_change"] = json!({"at": "2026-10-19T21:14:04.000Z", "direction": "down", "from": 1, "seq": 5001, "to": 0, "trigger": {"kind": "panic", "seq": 5000}});
    assert_eq!(
        fx.line(0),
        "[init:boot] Cold start: no running predecessor handed over to you. Continuing session 20261019-203015-9f3a2c. A panic stopped everything at 2026-10-19 21:14 UTC (owner_endpoint). You restart from your audited-good system 3f2a1c0, one level lower."
    );
    assert_eq!(
        fx.line(6),
        "- level down: L1 → L0 after panic (lineage record 5001). Your hold timers restarted; see evolution.md §3.15.3 for what was cancelled."
    );
}

#[test]
fn t_aw_6_kernel_upgrade() {
    let mut fx = Fx::with("cold", SHA_OLD, Some(SHA_OLD));
    fx.boot["kernel_upgrade"] = json!({"from": "0.1.0", "source": "maintainer", "to": "0.1.1"});
    assert_eq!(
        fx.line(0),
        "[init:boot] Cold start: no running predecessor handed over to you. Continuing session 20261019-203015-9f3a2c. Your kernel changed from 0.1.0 to 0.1.1 (maintainer release). The Root audits the new system before it counts as audited-good; take no external actions until then."
    );
    fx.boot["kernel_upgrade"] =
        json!({"branch": "kevo/0003", "from": "0.1.1", "source": "agent", "to": "0.1.1-kevo.3"});
    assert!(fx.line(0).ends_with(
        "Your kernel changed from 0.1.1 to 0.1.1-kevo.3 (your commit kevo/0003). The Root audits the new system before it counts as audited-good; take no external actions until then."
    ));
    // The same upgrade reported on a handoff boot.
    fx.boot["reason"] = json!("handoff");
    fx.boot["cause"] = json!({"kind": "kernel_upgrade"});
    assert_eq!(
        fx.line(0),
        "[init:boot] The kernel restarted you without a body change (your kernel was upgraded). The conversation continues. Your kernel changed from 0.1.1 to 0.1.1-kevo.3 (your commit kevo/0003). The Root audits the new system before it counts as audited-good; take no external actions until then."
    );
}

#[test]
fn t_aw_6_kernel_rollback_with_and_without_stash() {
    let mut fx = Fx::with("rollback", SHA_OLD, Some(SHA_NEW));
    let uf = json!({"exit_code": null, "from_sha": SHA_OLD, "reason": "score 812 below parent 845", "seq": 12, "sha": SHA_NEW, "stage": "G4"});
    fx.boot["update_failed"] = uf.clone();
    fx.update_failed = Some(uf);
    assert_eq!(
        fx.line(0),
        "[init:boot] You were rolled back to your LKG 3f2a1c0 from 7c1d2e9. Cause: G4: score 812 below parent 845. Report: state/kernel/update-failed.json. Your rolled-back commits are kept at refs/init/rolled-back/7c1d2e9. The conversation continues."
    );
    assert_eq!(
        fx.find("- last update failed"),
        None,
        "the reason line names it already"
    );
    fx.git_sync["stash"] = json!("stash@{0}");
    assert_eq!(
        fx.line(0),
        "[init:boot] You were rolled back to your LKG 3f2a1c0 from 7c1d2e9. Cause: G4: score 812 below parent 845. Report: state/kernel/update-failed.json. Your rolled-back commits are kept at refs/init/rolled-back/7c1d2e9; uncommitted work is in stash@{0}. The conversation continues."
    );
    assert_eq!(
        fx.line(1),
        "- you: body 3f2a1c0 · LKG 3f2a1c0 · audited-good 3f2a1c0 · model deepseek/deepseek-flash · body 21,904 tokens (+412 vs 7c1d2e9)"
    );
}

/// F4 of the 2026-10-25 demo: after the owner's `/rollback` (H3), the boot block named the stale
/// cause of an earlier demotion ("Cause: ceiling_hit: ?. Lineage record 21"). The kernel now names
/// the Root's rollback in `cause.kind` (`extend_H3`), and the block says what happened.
#[test]
fn t_aw_6_root_rolls_back_a_change_group() {
    let mut fx = Fx::with("rollback", SHA_OLD, Some(SHA_BAD));
    fx.boot["cause"] = json!({"kind": "extend_H3"});
    assert_eq!(
        fx.line(0),
        "[init:boot] The Root rolled back your change after harm signal H3 (the owner's /reject, /rollback or /panic): you are on 3f2a1c0 from 9d0e4b7. Your rolled-back commits are kept at refs/init/rolled-back/9d0e4b7. The conversation continues."
    );
    fx.boot["cause"] = json!({"kind": "extend_H99"});
    assert!(
        fx.line(0)
            .starts_with("[init:boot] The Root rolled back your change after harm signal H99: you are on 3f2a1c0"),
        "{}",
        fx.line(0)
    );
}

#[test]
fn t_aw_6_root_rollback_with_demotion() {
    let mut fx = Fx::with("rollback", SHA_OLD, Some(SHA_BAD));
    fx.boot["cause"] = json!({"evidence": "score −3.1 points, beyond δ_S 2.0", "kind": "demotion", "seq": 4127, "trigger": "sealed_audit_regression"});
    fx.boot["env"]["root"]["last_change"] = json!({"at": "2026-10-19T21:00:00.000Z", "direction": "down", "from": 1, "seq": 4127, "to": 0, "trigger": {"kind": "sealed_audit_regression", "seq": 4126}});
    fx.prev.as_mut().unwrap()["env"]["kernel"]["root"]["level"] = json!(1);
    let t = fx.text();
    let lines: Vec<&str> = t.lines().collect();
    assert_eq!(
        lines[0],
        "[init:boot] The Root rolled your whole system back to audited-good 3f2a1c0 from 9d0e4b7. Cause: sealed_audit_regression: score −3.1 points, beyond δ_S 2.0. Lineage record 4127. Your rolled-back commits are kept at refs/init/rolled-back/9d0e4b7. The conversation continues."
    );
    assert_eq!(
        lines[6],
        "- level down: L1 → L0 after sealed_audit_regression (lineage record 4127). Your hold timers restarted; see evolution.md §3.15.3 for what was cancelled."
    );
    // The root object changed, but the environment text does not show it: short form.
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("Environment: unchanged since boot")
    );
    fx.boot["cause"]["kind"] = json!("rollback_system");
    fx.git_sync["stash"] = json!("stash@{0}");
    assert!(fx.line(0).contains("refs/init/rolled-back/9d0e4b7; uncommitted work is in stash@{0}. The conversation continues."));
}

#[test]
fn t_aw_6_crash_by_code_and_by_signal() {
    let mut fx = Fx::with("crash", SHA_OLD, Some(SHA_NEW));
    fx.boot["restarts_since_lkg"] = json!(2);
    fx.boot["crash"] =
        json!({"exit_code": 101, "log_tail": "thread 'main' panicked", "signal": null});
    assert_eq!(
        fx.line(0),
        "[init:boot] You restarted after 7c1d2e9 crashed (exit code 101). Restarts since LKG: 2. The last stderr lines are in this boot event's crash field. Check what any interrupted tool call did before retrying it."
    );
    fx.boot["crash"] = json!({"exit_code": null, "log_tail": "", "signal": "SIGKILL"});
    assert!(fx.line(0).starts_with(
        "[init:boot] You restarted after 7c1d2e9 crashed (signal SIGKILL). Restarts since LKG: 2."
    ));
    fx.boot["crash"] = Value::Null;
    assert!(
        fx.line(0)
            .starts_with("[init:boot] You restarted after 7c1d2e9 crashed (?).")
    );
}

#[test]
fn t_aw_6_handoff_causes() {
    let mut fx = Fx::with("handoff", SHA_NEW, Some(SHA_NEW));
    let line0 = |fx: &Fx| fx.line(0);
    fx.boot["cause"] = json!({"kind": "model_change"});
    fx.model = "deepseek-pro";
    assert_eq!(
        line0(&fx),
        "[init:boot] The kernel restarted you without a body change (your main model changed from deepseek/deepseek-flash to deepseek/deepseek-pro). The conversation continues."
    );
    fx.model = "deepseek-flash";
    fx.boot["cause"] = json!({"kind": "owner_restart"});
    assert_eq!(
        line0(&fx),
        "[init:boot] The kernel restarted you without a body change (your owner restarted you). The conversation continues."
    );
    fx.boot["cause"] = json!("owner_restart");
    assert!(
        line0(&fx).contains("(your owner restarted you)"),
        "log-context's string form"
    );
    fx.boot["cause"] = json!({"hours": 24, "kind": "clear_safe"});
    assert_eq!(
        line0(&fx),
        "[init:boot] The kernel restarted you without a body change (safe mode cleared after 24 hours without a crash). The conversation continues."
    );
    fx.boot["cause"] = json!({"kind": "clear_safe"});
    assert!(line0(&fx).contains("(safe mode cleared after ? hours without a crash)"));
    fx.boot["cause"] = Value::Null;
    assert!(line0(&fx).contains("without a body change (?)."));
    // `update_failed` on a non-rollback boot gets its own line.
    let uf = json!({"exit_code": 101, "reason": "no ready within 90 s", "seq": 13, "sha": SHA_BAD, "stage": "handoff"});
    fx.update_failed = Some(uf);
    assert_eq!(
        fx.line(6),
        "- last update failed: 9d0e4b7 at handoff: no ready within 90 s (exit 101). Report: state/kernel/update-failed.json."
    );
}

/// ADR-009 (WIRE-009): the `- access:` line sits right after the ceilings line, in each of its
/// forms; without `INIT_ACCESS` (an older kernel) the block is byte-identical to before.
#[test]
fn access_line_forms_and_position() {
    const WS: &str = "/Users/q/projects/app";
    let mut fx = Fx::promote();
    fx.probed["cwd"] = json!(WS);
    let base = fx.text();
    assert!(!base.contains("- access:"), "{base}");
    fx.workspace = Some(PathBuf::from(WS));
    assert_eq!(
        fx.text(),
        base,
        "a workspace without INIT_ACCESS adds nothing"
    );

    let access = |fx: &mut Fx, a: &'static str, ws: Option<&str>| -> String {
        fx.access = Some(a);
        fx.workspace = ws.map(PathBuf::from);
        let t = fx.text();
        let l: Vec<&str> = t.lines().collect();
        let i = l
            .iter()
            .position(|x| x.starts_with("- ceilings (Root): "))
            .unwrap();
        assert!(l[i + 2].starts_with("- prompt prefix: "), "{t}");
        assert_eq!(l.len(), base.lines().count() + 1, "{t}");
        let mut without = l.clone();
        without.remove(i + 1);
        assert_eq!(without.join("\n"), base, "only the access line is added");
        l[i + 1].to_string()
    };
    assert_eq!(
        access(&mut fx, "workspace", Some(WS)),
        "- access: write the workspace /Users/q/projects/app and INIT_HOME (your tools start in the workspace) · read the owner's home, secret stores denied"
    );
    // v0.5.3 (lane H): with the charter's `sandbox.read_home = false` the Root sends
    // `workspace_only`, and the line no longer says the home is readable
    assert_eq!(
        access(&mut fx, "workspace_only", Some(WS)),
        "- access: write the workspace /Users/q/projects/app and INIT_HOME (your tools start in the workspace) · no other part of the owner's home is readable (charter sandbox.read_home = false)"
    );
    assert_eq!(
        access(&mut fx, "read_home", None),
        "- access: no workspace (init-agent was launched outside a project): write INIT_HOME only · read the owner's home, secret stores denied"
    );
    assert_eq!(
        access(&mut fx, "none", None),
        "- access: INIT_HOME only (no home access; the v0.1 sandbox)"
    );
    assert_eq!(
        access(&mut fx, "full", Some(WS)),
        "- access: FULL: read and write anywhere the owner can, network allowed · ~ is the owner's home · denied: the Root's files (~/.init-root, its service, INIT's launchers) · only the workspace has /undo · workspace /Users/q/projects/app"
    );
    assert_eq!(
        access(&mut fx, "full", None),
        "- access: FULL: read and write anywhere the owner can, network allowed · ~ is the owner's home · denied: the Root's files (~/.init-root, its service, INIT's launchers) · only the workspace has /undo · workspace none"
    );
    // Values this body does not know, or that contradict the workspace, render verbatim.
    assert_eq!(
        access(&mut fx, "sandboxed_v2", None),
        "- access: sandboxed_v2 · workspace none"
    );
    assert_eq!(
        access(&mut fx, "read_home", Some(WS)),
        "- access: read_home · workspace /Users/q/projects/app"
    );
    assert_eq!(
        access(&mut fx, "workspace", None),
        "- access: workspace · workspace none"
    );
    assert_eq!(
        access(&mut fx, "workspace_only", None),
        "- access: workspace_only · workspace none"
    );
    assert_eq!(
        access(&mut fx, "none", Some(WS)),
        "- access: none · workspace /Users/q/projects/app"
    );
    // A newline in a directory name cannot add a line.
    assert_eq!(
        access(&mut fx, "full", Some("/Users/q/a\nb")),
        "- access: FULL: read and write anywhere the owner can, network allowed · ~ is the owner's home · denied: the Root's files (~/.init-root, its service, INIT's launchers) · only the workspace has /undo · workspace /Users/q/a?b"
    );
    // The workspace was not a directory at boot: the tools started in the process cwd.
    fx.probed["cwd"] = json!("/Users/q/.init/work");
    fx.access = Some("workspace");
    fx.workspace = Some(PathBuf::from(WS));
    assert_eq!(
        fx.find("- access: ").unwrap(),
        "- access: write the workspace /Users/q/projects/app and INIT_HOME (it is not a directory now: your tools start in /Users/q/.init/work) · read the owner's home, secret stores denied"
    );
    // The access line is per-boot status: never in the head files (T-AW-3 covers those), and
    // the line itself carries nothing the volatility lint flags.
    for a in ["workspace", "workspace_only", "read_home", "none", "full"] {
        fx.access = Some(a);
        let line = fx.find("- access: ").unwrap();
        assert!(volatility_hits(&line, true).is_empty(), "{line}");
    }
}

/// ADR-009 item 5: the kernel restarts the body when the owner's workspace changes (a handoff on
/// the same sha and session); the boot block names the cause and the access line the new path.
#[test]
fn t_aw_6_handoff_workspace_change() {
    let mut fx = Fx::with("handoff", SHA_NEW, Some(SHA_NEW));
    fx.boot["cause"] = json!({"kind": "workspace_change"});
    fx.access = Some("workspace");
    fx.workspace = Some(PathBuf::from("/Users/q/projects/new"));
    fx.probed["cwd"] = json!("/Users/q/projects/new");
    assert_eq!(
        fx.line(0),
        "[init:boot] The kernel restarted you without a body change (your owner's workspace changed). The conversation continues."
    );
    assert_eq!(
        fx.find("- access: ").unwrap(),
        "- access: write the workspace /Users/q/projects/new and INIT_HOME (your tools start in the workspace) · read the owner's home, secret stores denied"
    );
    assert_eq!(boot_json_missing(&fx.boot), Vec::<String>::new());
    fx.boot["cause"] = json!("workspace_change");
    assert!(fx.line(0).contains("(your owner's workspace changed)"));
    // The cwd moved, so the environment is shown in full (cwd is in the text).
    assert!(
        fx.text().contains("· cwd: /Users/q/projects/new · bash: "),
        "{}",
        fx.text()
    );
    fx.boot["cause"] = Value::Null;
    assert_eq!(boot_json_missing(&fx.boot), ["cause"]);
}

#[test]
fn t_aw_6_safe_and_probe_modes() {
    let mut fx = Fx::with("crash", SHA_OLD, Some(SHA_NEW));
    fx.boot["crash"] = json!({"exit_code": 101, "log_tail": "", "signal": null});
    fx.mode = "safe";
    assert_eq!(
        fx.line(1),
        "SAFE MODE: your LKG crash-looped, so the Root rolled you back to your audited-good system 3f2a1c0 and lowered your level. promote, gate --quick and vendor are disabled until safe mode clears itself after 24 hours without a crash; T0/T1 trials still work. Diagnose from the crash tails before changing your core."
    );
    assert!(fx.line(2).starts_with("- you: body 3f2a1c0 "));
    let mut fx = Fx::with("cold", SHA_NEW, None);
    fx.mode = "probe";
    fx.session_created = true;
    assert_eq!(
        fx.line(1),
        "GATE PROBE: you are a staging copy being checked by the kernel. Follow the probe instruction below and change nothing."
    );
    assert!(
        fx.line(2)
            .starts_with("- you: body 7c1d2e9 · LKG 3f2a1c0 · ")
    );
}

#[test]
fn t_aw_6_is_deterministic_and_float_free() {
    let fx = Fx::promote();
    assert_eq!(fx.text(), fx.text());
    let env = snapshot(&fx.boot, fx.probed.clone());
    assert_eq!(crate::log::icj::find_float(&env), None);
}

#[test]
fn optional_lines_in_order() {
    let mut fx = Fx::promote();
    fx.boot["evolution"] = json!({
        "audit_required": true,
        "predict_hit_rate": 735,
        "retire_candidates": [
            {"class": "tool", "id": "tool:grep2", "tasks_since_use": 40, "tokens": 1200},
            {"class": "skill", "id": "skill:old", "tasks_since_use": 12, "tokens": 300}
        ],
        "state": "thinning_baseline",
        "thin_candidates": [
            {"id": "loop:retry", "status": "pending", "tag": "Compensates-for: flaky tools", "tokens": 640},
            {"id": "loop:done", "status": "kept", "tag": "x", "tokens": 1}
        ],
        "upstream": {"sha": SHA_BAD, "tag": "seed-0.2.0"}
    });
    fx.boot["env"]["root"]["audited_good"] = json!(SHA_NEW);
    fx.boot["env"]["root"]["audited_good_at"] = json!("2026-10-19T19:00:00.000Z");
    fx.update_failed = Some(
        json!({"exit_code": null, "reason": "fence", "seq": 3, "sha": SHA_BAD, "stage": "fence"}),
    );
    let t = fx.text();
    let opt: Vec<&str> = t.lines().skip(6).collect();
    assert_eq!(
        opt[..opt.len() - 1],
        [
            "- last update failed: 9d0e4b7 at fence: fence. Report: state/kernel/update-failed.json.",
            "- your LKG 3f2a1c0 waits for a sealed audit: its gain rests on tasks you wrote.",
            "- sealed audit on 2026-10-19 passed: audited-good is now 7c1d2e9.",
            "- unused growth (retirement candidates): tool:grep2 (tool, 40 tasks, 1,200 tokens), skill:old (skill, 12 tasks, 300 tokens). Delete what you no longer need with a retire commit.",
            "- your model changed to deepseek/deepseek-flash. Scaffolds that may now be unneeded: loop:retry (Compensates-for: flaky tools, 640 tokens). Try one thin commit per component; the gate decides.",
            "- seed release seed-0.2.0 (9d0e4b7) is available upstream. Merge it as an upstream commit when convenient.",
            "- your Predicts: claims were right 73% of the time (last 20 resolved).",
            "- evolution state: thinning_baseline.",
        ]
    );
    assert!(
        t.lines()
            .nth(1)
            .unwrap()
            .contains("LKG 3f2a1c0 (audit required)")
    );
    // `normal` state and empty lists add nothing.
    fx.boot["evolution"] =
        json!({"retire_candidates": [], "state": "normal", "thin_candidates": []});
    fx.update_failed = None;
    fx.boot["env"]["root"]["audited_good"] = json!(SHA_OLD);
    assert_eq!(fx.text().lines().count(), 7);
}

/// The engine line (evolution-v2 §3.4.4, §4.6) from `boot.json.evolution`.
#[test]
fn engine_line_texts() {
    let line = |evo: Value| engine_line(&json!({"evolution": evo}));
    // Absent object, empty object, zero counts: no line.
    assert_eq!(engine_line(&json!({})), None);
    assert_eq!(line(json!({})), None);
    assert_eq!(
        line(json!({"frictions_eligible": 0, "proposals_open": 0, "proposals_by_state": {}})),
        None
    );
    // Eligible frictions only, with and without a top cluster; singular and plural.
    assert_eq!(
        line(json!({"frictions_eligible": 3, "frictions_top": {"instances": 6, "signature": "path.home_relative"}, "proposals_open": 0})).as_deref(),
        Some("- evolution: 3 eligible frictions (top path.home_relative ×6)")
    );
    assert_eq!(
        line(json!({"frictions_eligible": 1})).as_deref(),
        Some("- evolution: 1 eligible friction")
    );
    // With proposals: one state, singular and plural.
    assert_eq!(
        line(json!({"frictions_eligible": 3, "frictions_top": {"instances": 6, "signature": "path.home_relative"},
                    "proposals_by_state": {"screened": 1}, "proposals_open": 1})).as_deref(),
        Some("- evolution: 3 eligible frictions (top path.home_relative ×6); 1 proposal screened")
    );
    assert_eq!(
        line(json!({"proposals_by_state": {"screening": 2}, "proposals_open": 2})).as_deref(),
        Some("- evolution: 2 proposals screening")
    );
    // Mixed states, sorted; zero counts dropped; a count that disagrees with the total lists too.
    assert_eq!(
        line(json!({"frictions_eligible": 2, "proposals_by_state": {"screened": 1, "proposed": 1, "variants": 0},
                    "proposals_open": 2})).as_deref(),
        Some("- evolution: 2 eligible frictions; 2 proposals open (1 proposed, 1 screened)")
    );
    assert_eq!(
        line(json!({"proposals_by_state": {"screened": 1}, "proposals_open": 3})).as_deref(),
        Some("- evolution: 3 proposals open (1 screened)")
    );
    // No state breakdown; kernel strings are kept on one line and bounded.
    assert_eq!(
        line(json!({"proposals_open": 1})).as_deref(),
        Some("- evolution: 1 proposal open")
    );
    let long = "x".repeat(100);
    assert_eq!(
        line(json!({"frictions_eligible": 1, "frictions_top": {"instances": 1200, "signature": format!("a\nb{long}")}})),
        Some(format!("- evolution: 1 eligible friction (top a?b{} ×1,200)", &long[..61]))
    );
    assert_eq!(
        line(json!({"frictions_eligible": 1, "frictions_top": {}})).as_deref(),
        Some("- evolution: 1 eligible friction (top ? ×?)")
    );
}

/// The engine line sits after the predictions line and before the evolution-state line (§3.6).
#[test]
fn engine_line_order_in_boot_block() {
    let mut fx = Fx::promote();
    fx.boot["evolution"] = json!({
        "frictions_eligible": 3,
        "frictions_top": {"instances": 6, "signature": "path.home_relative"},
        "predict_hit_rate": 735,
        "proposals_by_state": {"screened": 1},
        "proposals_open": 1,
        "state": "thinning_baseline"
    });
    let t = fx.text();
    let opt: Vec<&str> = t.lines().skip(6).collect();
    assert_eq!(
        opt[..opt.len() - 1],
        [
            "- your Predicts: claims were right 73% of the time (last 20 resolved).",
            "- evolution: 3 eligible frictions (top path.home_relative ×6); 1 proposal screened",
            "- evolution state: thinning_baseline.",
        ]
    );
    fx.boot["evolution"] = json!({"frictions_eligible": 0, "proposals_open": 0});
    assert_eq!(fx.text().lines().count(), 7);
}

/// supervise.md §4.3's example boot.json (its `env.root` lacks several §4.5 fields): `?`, no panic.
#[test]
fn supervise_example_boot_json_renders_with_unknowns() {
    let boot: Value = serde_json::from_str(r#"{"body_id":"b0","boot_id":"a3f09c1e5b7d2468","boot_seq":57,"branch":"evo/pty-tool","cause":null,"crash":null,"env":{"budget":{"cap_musd":5000000,"day_starts_at":"2026-10-19T17:00:00Z","today_musd":1230000},"init_home":"/Users/q/.init","kernel_version":"0.1.0","mem_total_mb":16384,"model":{"context_window":1000000,"id":"deepseek-flash","max_output":384000,"provider":"deepseek"},"network":{"egress":[],"mode":"proxy-only"},"os":{"arch":"arm64","family":"macos","version":"14.6"},"root":{"audited_good":"t3f09c1e5b7d2","ceilings":{"day_musd":5000000,"parallel_bodies":1,"total_musd":25000000},"classes":["models"],"level":0,"max_level":0,"next":{"level":1}},"sandbox":{"hidden":["cache/","fleet/","probe/","vault/"],"kind":"seatbelt","landlock":null,"landlock_abi":null,"network":"kernel-sockets-only","read_only":["bin/","kernel/","lineage/","state/kernel/"],"seccomp":false,"sweep":"full","write":["body/","state/","work/"],"zone":{"launcher":"root","network":"root-egress-only"}}},"evolution":{},"from_sha":"0d3e5a9b…","kernel":{"proto":{"initctl":1,"root":1,"ui":1},"version":"0.1.0"},"kernel_upgrade":null,"lkg_sha":"0d3e5a9b…","lkg_tree":"lineage/trees/0d3e5a9b1c22","mode":"normal","model":{"effort":null,"model":"deepseek-flash","provider":"deepseek"},"panic":null,"pending":{"inputs":1,"notices":0},"provisional":{"depth":1,"g4":"queued","probation_until":"2026-10-19T12:38:01.004Z","since":"2026-10-19T12:36:01.004Z"},"reason":"promote","restarts_since_lkg":0,"safe":null,"selfmap":{"body_tokens":21480,"body_tokens_prev":21302},"session":"20261019-120000-3fa9c1","sha":"4b1e0c2d9a7f…","spawned_at":"2026-10-19T12:36:00.981Z","update_failed":null,"v":1,"zone":{"boot_reason":"cold","kernel_sha256":"7d41…64 hex"}}"#).unwrap();
    let env = snapshot(&boot, Value::Null);
    let b = BlockIn {
        boot: &boot,
        sha: "4b1e0c2d9a7f…",
        mode: "normal",
        reason: "promote",
        provider: "deepseek",
        model: "deepseek-flash",
        effort: "high",
        session: "20261019-120000-3fa9c1",
        session_created: false,
        rehydrated_events: 3,
        interrupted_calls: 0,
        prefix_hash: "",
        prev_boot: None,
        env: &env,
        git_sync: &Value::Null,
        update_failed: None,
        capability: None,
        access: None,
        workspace: None,
    };
    let t = boot_text(&b);
    let l: Vec<&str> = t.lines().collect();
    assert_eq!(
        l[0],
        "[init:boot] You were promoted: 0d3e5a9 → 4b1e0c2 (branch evo/pty-tool). G0–G3 passed; G4 is running. Until it passes you are provisional, so take no external actions. This is the same conversation: continue where you left off."
    );
    assert_eq!(
        l[1],
        "- you: body 4b1e0c2 (provisional) · LKG 0d3e5a9 · audited-good t3f09c1 · model deepseek/deepseek-flash · body 21,480 tokens (+178 vs 0d3e5a9)"
    );
    // No cap anywhere (boot.json, no readable Root view): the part is left out, not `L?`.
    assert_eq!(l[2], "- autonomy: L0 · next: L1 needs ?");
    assert_eq!(
        l[3],
        "- ceilings (Root): $5.00 a day · $25.00 total, ? used · 1 live body · actions: model providers only"
    );
    assert_eq!(l[4], "- prompt prefix:  (first boot of this session)");
    assert!(
        t.contains("- date: ?\n- os: macos 14.6 arm64 · 16384 MB · ? cpus (probed)\n"),
        "{t}"
    );
    assert!(t.contains("budget: $1.23 of $5.00 spent today (day starts 17:00 UTC; your soft day budget, clamped under the Root's $5.00 ceiling)"));
    assert_eq!(boot_json_missing(&boot), Vec::<String>::new());
    assert_eq!(
        boot_json_missing(&json!({"reason": "crash", "env": {}})),
        [
            "crash",
            "env.root",
            "evolution",
            "from_sha",
            "lkg_sha",
            "mode",
            "provisional",
            "restarts_since_lkg",
            "selfmap",
            "selfmap.body_tokens",
            "sha",
            "update_failed"
        ]
    );
    // Nothing at all: still a block, every value `?`.
    let empty = json!({});
    let env = snapshot(&empty, Value::Null);
    let b = BlockIn {
        boot: &empty,
        env: &env,
        reason: "rollback",
        ..b
    };
    let t = boot_text(&b);
    assert!(
        t.starts_with("[init:boot] You were rolled back to your LKG 4b1e0c2 from ?. Cause: ?: ?."),
        "{t}"
    );
    assert!(t.contains("\n- autonomy: L? · ?\n- ceilings (Root): ? a day · ? total, ? used · ? live bodies · actions: ?\n"));
}

// ---------------------------------------------------------------------------------------------
// T-AW-12: autonomy and ceilings lines
// ---------------------------------------------------------------------------------------------

#[test]
fn t_aw_12_autonomy_and_ceilings() {
    let with_root = |root: Value| json!({"env": {"root": root}});
    // L0 with progress (a met criterion listed first by the Root moves last).
    let r = json!({"cap": 4, "ceilings": {"day_musd": 5000000, "parallel_bodies": 3, "total_musd": 25000000, "total_used_musd": 7400000},
                   "classes": ["model-providers", "github-issues"], "level": 0, "max_level": 4, "supported_max_level": 4,
                   "next": {"level": 1, "criteria": [
                       {"id": "no_ceiling_hit", "met": true, "text": "no demoting ceiling hit"},
                       {"have": 7, "id": "promotions", "met": false, "need": 20, "text": "20 promotions"},
                       {"have": null, "id": "audit", "met": false, "need": 1, "text": "an audit in the last 14 days with no regression"}]}});
    assert_eq!(
        autonomy_line(&with_root(r.clone())),
        "- autonomy: L0 (cap L4) · next: L1 needs 20 promotions (7 of 20) · an audit in the last 14 days with no regression (none yet) · no demoting ceiling hit (met)"
    );
    assert_eq!(
        ceilings_line(&with_root(r)),
        "- ceilings (Root): $5.00 a day · $25.00 total, $7.40 used · 3 live bodies · actions: model providers, github issues"
    );
    // At the charter maximum.
    let r = json!({"cap": 4, "level": 4, "max_level": 4, "next": {"level": 5}, "supported_max_level": 4});
    assert_eq!(
        autonomy_line(&with_root(r)),
        "- autonomy: L4 (cap L4) · at your cap: charter max L4, this Root supports up to L4"
    );
    // `next` null; the cap is computed when absent.
    let r =
        json!({"level": 0, "max_level": 4, "next": null, "supported_max_level": 0, "classes": []});
    assert_eq!(
        autonomy_line(&with_root(r.clone())),
        "- autonomy: L0 (cap L0) · at your cap: charter max L4, this Root supports up to L0"
    );
    assert_eq!(
        ceilings_line(&with_root(r)),
        "- ceilings (Root): ? a day · ? total, ? used · ? live bodies · actions: none"
    );
    // Criteria ids only, no counts.
    let r = json!({"level": 1, "cap": 2, "next": {"level": 2, "criteria": [{"id": "held_30_days", "met": false}]}});
    assert_eq!(
        autonomy_line(&with_root(r)),
        "- autonomy: L1 (cap L2) · next: L2 needs held_30_days (none yet)"
    );
    // Unknown status; an unknown cap is left out, and so are unknown parts of the at-cap text.
    assert_eq!(autonomy_line(&json!({})), "- autonomy: L? · ?");
    assert_eq!(autonomy_line(&with_root(json!({}))), "- autonomy: L? · ?");
    assert_eq!(
        autonomy_line(&with_root(
            json!({"level": 0, "max_level": 4, "next": null})
        )),
        "- autonomy: L0 · at your cap: charter max L4"
    );
    assert_eq!(
        autonomy_line(&with_root(json!({"level": 0, "next": null}))),
        "- autonomy: L0 · at your cap"
    );
    // The kernel's real env.root carries no cap: it is read from the Root's published view.
    let h = TempHome::new("rootview");
    let kernel_root =
        json!({"audited_good": null, "level": 0, "max_level": 4, "next": null, "classes": []});
    let boot = with_root(kernel_root);
    let line = |home: &Path| autonomy_line(&crate::boot::rootview::render_boot(&boot, home));
    assert_eq!(
        line(&h.path),
        "- autonomy: L0 · at your cap: charter max L4"
    );
    std::fs::create_dir_all(h.path.join("root")).unwrap();
    std::fs::write(
        h.path.join("root/charter.json"),
        r#"{"autonomy":{"max_level":4},"supported_max_level":0}"#,
    )
    .unwrap();
    assert_eq!(
        line(&h.path),
        "- autonomy: L0 (cap L0) · at your cap: charter max L4, this Root supports up to L0"
    );
    // ledger.json (root.md §3.13 `Level::json`) wins: it states the cap itself.
    std::fs::write(
        h.path.join("root/ledger.json"),
        r#"{"cap":1,"entered_at":null,"entered_seq":0,"incident":null,"last_change":null,"level":0,"max_level":4,"next":{"criteria":[{"have":3,"id":"promotions","met":false,"need":20,"text":"20 promotions"}],"level":1},"seq":9,"supported_max_level":1,"v":1}"#,
    )
    .unwrap();
    assert_eq!(
        line(&h.path),
        "- autonomy: L0 (cap L1) · next: L1 needs 20 promotions (3 of 20)"
    );
    assert_eq!(
        ceilings_line(&json!({"env": {"root": null}})),
        "- ceilings (Root): ? a day · ? total, ? used · ? live bodies · actions: ?"
    );
}

// ---------------------------------------------------------------------------------------------
// T-AW-13: level-change lines, shown once
// ---------------------------------------------------------------------------------------------

#[test]
fn t_aw_13_level_change_shown_once() {
    let mut fx = Fx::promote();
    let up = json!({"at": "2026-10-19T21:00:00.000Z", "criteria": ["promotions", "audit", "no_ceiling_hit"], "direction": "up", "from": 0, "seq": 4200, "to": 1, "trigger": null});
    fx.boot["env"]["root"]["last_change"] = up.clone();
    fx.boot["env"]["root"]["level"] = json!(1);
    let want = "- level up: L0 → L1 (promotions, audit, no_ceiling_hit). New for you: initctl task template, task submit, task list, task show, budget set and model set.";
    // First boot after the change (the previous boot had none, then an older one).
    assert_eq!(fx.find("- level "), Some(want.to_string()));
    fx.prev.as_mut().unwrap()["env"]["kernel"]["root"]["last_change"] =
        json!({"direction": "up", "from": 0, "seq": 4100, "to": 1});
    assert_eq!(fx.find("- level "), Some(want.to_string()));
    // The following boot: the previous boot event already carries seq 4200.
    fx.prev.as_mut().unwrap()["env"]["kernel"]["root"]["last_change"] = up;
    assert_eq!(fx.find("- level "), None);
    // First boot of a session: shown (nothing in this conversation has shown it).
    fx.prev = None;
    assert_eq!(fx.find("- level "), Some(want.to_string()));

    for (to, cmds) in [
        (2, "initctl fleet spawn, fleet send and fleet stop"),
        (3, "initctl kernel promote"),
        (4, "every action class your charter allows"),
    ] {
        let mut fx = Fx::promote();
        fx.boot["env"]["root"]["last_change"] = json!({"criteria": [{"id": "held"}], "direction": "up", "from": to - 1, "seq": 9, "to": to});
        assert_eq!(
            fx.find("- level "),
            Some(format!(
                "- level up: L{} → L{to} (held). New for you: {cmds}.",
                to - 1
            ))
        );
    }
}

#[test]
fn t_aw_13_demotion_triggers_and_charter_cap() {
    for kind in [
        "sealed_audit_regression",
        "gate_of_gates",
        "ceiling_hit",
        "action_class",
        "crash_loop",
        "panic",
        "canary",
    ] {
        let mut fx = Fx::promote();
        fx.boot["env"]["root"]["last_change"] = json!({"direction": "down", "from": 2, "seq": 77, "to": 1, "trigger": {"kind": kind, "seq": 76}});
        assert_eq!(
            fx.find("- level "),
            Some(format!(
                "- level down: L2 → L1 after {kind} (lineage record 77). Your hold timers restarted; see evolution.md §3.15.3 for what was cancelled."
            ))
        );
    }
    let mut fx = Fx::promote();
    fx.boot["env"]["root"]["last_change"] =
        json!({"direction": "cap", "from": 2, "seq": 80, "to": 1});
    assert_eq!(
        fx.find("- level "),
        Some("- level capped: L2 → L1 by a charter edit.".to_string())
    );
    // No direction: inferred from the levels; no seq: compared as a whole.
    fx.boot["env"]["root"]["last_change"] = json!({"from": 1, "to": 0, "trigger": "panic"});
    assert!(
        fx.find("- level down: L1 → L0 after panic (lineage record ?)")
            .is_some()
    );
    fx.prev.as_mut().unwrap()["env"]["kernel"]["root"]["last_change"] =
        json!({"from": 1, "to": 0, "trigger": "panic"});
    assert_eq!(fx.find("- level "), None);
}

// ---------------------------------------------------------------------------------------------
// T-AW (ADR-011 X11): the extend line, one per state; the head is untouched (tests/boot.rs
// `x11_extend_line_keeps_the_head` checks `prefix_hash` on a live body)
// ---------------------------------------------------------------------------------------------

/// `caps.json` (X5 `CapRow`s): `live` registered caps in state active, plus `extra` rows, plus rows
/// that never count: a seed tool seen in use, a prompt file, a removed and an archived cap.
fn caps_view(live: usize, extra: Vec<Value>) -> Value {
    let mut rows: Vec<Value> = (0..live)
        .map(|i| json!({"evidence": "root", "id": format!("ext/t{i:02}"), "registered": true, "state": "active"}))
        .collect();
    rows.extend(extra);
    rows.push(
        json!({"evidence": "root", "id": "tool/bash", "registered": false, "state": "observed"}),
    );
    rows.push(
        json!({"evidence": "root", "id": "prompt/SELF.md", "registered": true, "state": "active"}),
    );
    rows.push(
        json!({"evidence": "root", "id": "skill/gone", "registered": true, "state": "removed"}),
    );
    rows.push(
        json!({"evidence": "root", "id": "skill/old", "registered": true, "state": "archived"}),
    );
    json!({"caps": rows, "day": "2026-10-19", "v": 1})
}

/// X3's `extend.json` with open groups `(seq, turns, kernel)`.
fn extend_view(groups: &[(u64, u64, bool)]) -> Value {
    let g: Vec<Value> = groups
        .iter()
        .map(|&(seq, turns, kernel)| json!({"caps": [], "changes": 1, "kernel": kernel, "last_join": "2026-10-19T20:00:00Z",
            "opened_at": "2026-10-19T19:00:00Z", "parent_tuple": "t-1", "seq": seq, "snapshot": null, "tuple": "t-2", "turns": turns}))
        .collect();
    json!({"freeze_until": null, "groups": g, "ring": {"requests": 120, "requests_m": 400, "turns": 31, "turns_m": 40},
           "rollback": null, "v": 1})
}

/// The Root ledger's `next` for L1 (X7 `criteria`: `{have, id, met, need}`).
fn next_l1(days: u64, turns: u64, clean: u64, ring: u64) -> Value {
    let c = |id: &str, have: u64, need: u64| json!({"have": have, "id": id, "met": have >= need, "need": need});
    json!({"level": 1, "criteria": [c("days_at_level", days, 7), c("owner_turns", turns, 40), c("clean_groups", clean, 10),
        json!({"have": 0, "id": "direct_rollbacks_last_30_groups", "met": true, "need": 6}), c("baseline_ring", ring, 40)]})
}

/// The extend line of the promote fixture with `root` merged into `env.root`.
fn extend_of(root: Value) -> Option<String> {
    let mut fx = Fx::promote();
    if let (Some(r), Value::Object(m)) = (fx.boot["env"]["root"].as_object_mut(), root) {
        r.extend(m);
    }
    fx.find("- extend ")
}

/// `env.root` for a level-0 free install whose charter keeps the `[extend]` defaults.
fn free_root(ext: Option<Value>, caps: Option<Value>, next: Value) -> Value {
    let charter = json!({"autonomy": {"max_level": 4}, "supported_max_level": 1});
    let x = crate::boot::rootview::normalize_extend(
        ext.as_ref(),
        caps.as_ref(),
        Some("free"),
        Some(&charter),
        Some(0),
    );
    json!({"cap": 1, "extend": x, "level": 0, "next": next})
}

#[test]
fn t_aw_x11_extend_line_per_state() {
    use crate::boot::rootview::normalize_extend;
    let defaults = json!({"autonomy": {"max_level": 4}});
    // Gated: no `[extend]` in the charter view (or mode "gated" from the Root's ceilings), no open
    // group → no line, even when an old extend.json exists.
    assert_eq!(
        normalize_extend(None, None, None, Some(&defaults), Some(0)),
        None
    );
    assert_eq!(
        normalize_extend(
            Some(&extend_view(&[])),
            Some(&caps_view(3, vec![])),
            Some("gated"),
            None,
            Some(0)
        ),
        None
    );
    assert_eq!(
        extend_of(json!({})),
        None,
        "the promote fixture has no extend line"
    );
    let gated = normalize_extend(
        None,
        None,
        None,
        Some(&json!({"extend": {"mode": "gated"}})),
        Some(0),
    );
    assert_eq!(gated, None);
    // A group still open while gated is shown: it is real.
    assert!(
        normalize_extend(
            Some(&extend_view(&[(4, 1, false)])),
            None,
            Some("gated"),
            None,
            Some(0)
        )
        .is_some()
    );

    // Free, idle: nothing open.
    assert_eq!(
        extend_of(free_root(
            None,
            Some(caps_view(2, vec![])),
            next_l1(0, 0, 0, 0)
        )),
        Some("- extend · groups 0/4 · window none · caps 2/64 · L0→L1 7d".into())
    );
    // The ADR's example: two groups open, the newest has seen 7 of its 20 owner turns.
    let x = Some(extend_view(&[(3, 12, false), (5, 7, false)]));
    assert_eq!(
        extend_of(free_root(
            x.clone(),
            Some(caps_view(31, vec![])),
            next_l1(3, 22, 4, 40)
        )),
        Some("- extend · groups 2/4 · window 7/20 · caps 31/64 · L0→L1 4d".into())
    );
    // Days served: the first unmet criterion in the Root's order; all met: ready.
    let line =
        |next: Value| extend_of(free_root(x.clone(), Some(caps_view(31, vec![])), next)).unwrap();
    assert!(line(next_l1(9, 28, 4, 40)).ends_with(" · L0→L1 12 owner turns"));
    assert!(line(next_l1(9, 40, 9, 40)).ends_with(" · L0→L1 1 clean group"));
    assert!(line(next_l1(9, 40, 10, 31)).ends_with(" · L0→L1 baseline_ring (31 of 40)"));
    assert!(line(next_l1(9, 40, 10, 40)).ends_with(" · L0→L1 ready"));
    assert!(line(json!(null)).ends_with(" · L0 at cap"));
    assert!(line(json!({"level": 1})).ends_with(" · L0→L1 ?"));
    // Debts: the cap table's states (X6's own view wins when present).
    let caps = caps_view(
        30,
        vec![
            json!({"id": "ext/bad", "registered": true, "state": "quarantined"}),
            json!({"id": "skill/stale", "registered": true, "state": "archive_due"}),
        ],
    );
    assert_eq!(
        extend_of(free_root(x.clone(), Some(caps), next_l1(3, 22, 4, 40))),
        Some("- extend · groups 2/4 · window 7/20 · caps 32/64 · L0→L1 4d · QUARANTINED ext/bad: move to quarantine/ or remove; every other change is refused until then · archive due skill/stale: move to archive/ within 7 days or adding caps is refused".into())
    );
    let mut x6 = extend_view(&[(5, 7, false)]);
    x6["quarantine_debt"] = json!(["mem/b", "ext/a", "loop/c", "skill/d", "agent/e"]);
    x6["archive_due"] = json!([]);
    let l = extend_of(free_root(
        Some(x6),
        Some(caps_view(1, vec![])),
        next_l1(3, 22, 4, 40),
    ))
    .unwrap();
    assert!(l.ends_with(" · QUARANTINED agent/e, ext/a, loop/c, mem/b +1 more: move to quarantine/ or remove; every other change is refused until then"), "{l}");
    // Over the caps budget.
    let l = extend_of(free_root(
        None,
        Some(caps_view(65, vec![])),
        next_l1(3, 22, 4, 40),
    ))
    .unwrap();
    assert_eq!(
        l,
        "- extend · groups 0/4 · window none · caps 65/64 · L0→L1 4d · over the caps budget: archive or remove before adding"
    );
    // The H5a freeze (the fixture's date is 2026-10-19 22:04 UTC) and a pending Root rollback.
    let mut fz = extend_view(&[(5, 7, false)]);
    fz["freeze_until"] = json!("2026-10-19T22:40:00Z");
    fz["rollback"] =
        json!({"deadline": "2026-10-19T22:06:00Z", "groups": [5], "reason": "H5b", "to": "t-1"});
    let l = extend_of(free_root(
        Some(fz.clone()),
        Some(caps_view(3, vec![])),
        next_l1(3, 22, 4, 40),
    ))
    .unwrap();
    assert!(l.ends_with(" · changes frozen until 2026-10-19 22:40 UTC (money burst) · Root rollback pending (H5b)"), "{l}");
    fz["freeze_until"] = json!("2026-10-19T21:00:00.000Z");
    fz["rollback"] =
        json!({"deadline": "2026-10-19T22:06:00Z", "groups": [5], "reason": null, "to": "t-1"});
    let l = extend_of(free_root(
        Some(fz),
        Some(caps_view(3, vec![])),
        next_l1(3, 22, 4, 40),
    ))
    .unwrap();
    assert!(
        l.ends_with(" · L0→L1 4d · Root rollback pending"),
        "a past freeze is not shown: {l}"
    );
    // A kernel group (window 40, a Root constant) at L1, with the owner's own `[extend]` values.
    let charter = json!({"extend": {"mode": "free", "open_groups": [5, 7, 9, 12, 12], "win_turns": 30, "bloat_caps": 80,
                                    "bloat_permille": [1000, 1500, 2000, 3000, 3000]}});
    let kx = normalize_extend(
        Some(&extend_view(&[(8, 2, false), (9, 3, true)])),
        Some(&caps_view(10, vec![])),
        None,
        Some(&charter),
        Some(1),
    );
    assert_eq!(
        extend_of(
            json!({"cap": 2, "extend": kx, "level": 1, "next": {"level": 2, "criteria": [{"have": 2, "id": "days_at_level", "met": false, "need": 21}]}})
        ),
        Some("- extend · groups 2/7 · window 3/40 (kernel) · caps 10/120 · L1→L2 19d".into())
    );
    let wx = normalize_extend(
        Some(&extend_view(&[(8, 2, false)])),
        Some(&caps_view(10, vec![])),
        None,
        Some(&charter),
        Some(0),
    );
    assert_eq!(
        extend_of(json!({"cap": 0, "extend": wx, "level": 0, "next": null})),
        Some("- extend · groups 1/5 · window 2/30 · caps 10/80 · L0 at cap".into())
    );
    // Unknown: no charter view, no cap table, no level → `?`, never a guessed limit.
    let ux = normalize_extend(Some(&extend_view(&[(2, 3, false)])), None, None, None, None);
    let mut fx = Fx::promote();
    fx.boot["env"]["root"] = json!({"extend": ux});
    assert_eq!(
        fx.find("- extend "),
        Some("- extend · groups 1/? · window 3/? · caps ?/? · L?".into())
    );
    // Place: right after the session line, before the optional lines.
    let mut fx = Fx::promote();
    fx.boot["env"]["root"]["extend"] =
        free_root(None, Some(caps_view(2, vec![])), json!(null))["extend"].clone();
    fx.boot["env"]["root"]["last_change"] =
        json!({"direction": "up", "from": 0, "seq": 9, "to": 1, "criteria": []});
    let text = fx.text();
    let lines: Vec<&str> = text.lines().collect();
    let i = lines
        .iter()
        .position(|l| l.starts_with("- extend "))
        .unwrap();
    assert!(
        lines[i - 1].starts_with("- session: ") && lines[i + 1].starts_with("- level up: "),
        "{text}"
    );
    assert!(
        !text.contains('\u{0}') && fx.text() == text,
        "deterministic"
    );
}

/// `render_boot` folds the Root's published views (`root/{extend,caps,charter,ceilings,ledger}.json`)
/// into `env.root.extend`; the mode comes from the Root's ceilings (`ceilings.extend.mode`, X7).
#[test]
fn t_aw_x11_render_boot_reads_the_root_views() {
    let h = TempHome::new("x11");
    std::fs::create_dir_all(h.path.join("root")).unwrap();
    let w = |name: &str, v: &Value| {
        std::fs::write(h.path.join("root").join(name), v.to_string()).unwrap()
    };
    let mut fx = Fx::promote();
    let line = |fx: &mut Fx| {
        let raw = fx.boot.clone();
        fx.boot = crate::boot::rootview::render_boot(&raw, &h.path);
        let l = fx.find("- extend ");
        fx.boot = raw;
        l
    };
    assert_eq!(line(&mut fx), None, "no views: gated");
    w(
        "charter.json",
        &json!({"autonomy": {"max_level": 4}, "supported_max_level": 1}),
    );
    w("extend.json", &extend_view(&[]));
    w("caps.json", &caps_view(3, vec![]));
    assert_eq!(
        line(&mut fx),
        None,
        "the charter keeps [extend] at its defaults: gated"
    );
    w(
        "ceilings.json",
        &json!({"extend": {"mode": "free"}, "v": 1}),
    );
    assert_eq!(
        line(&mut fx),
        Some("- extend · groups 0/4 · window none · caps 3/64 · L0 at cap".into())
    );
    w("extend.json", &extend_view(&[(5, 7, false)]));
    w(
        "ledger.json",
        &json!({"cap": 1, "level": 0, "max_level": 4, "next": next_l1(3, 22, 4, 40), "supported_max_level": 1, "v": 1}),
    );
    fx.boot["env"]["root"]["cap"] = Value::Null;
    assert_eq!(
        line(&mut fx),
        Some("- extend · groups 1/4 · window 7/20 · caps 3/64 · L0→L1 4d".into())
    );
    // The kernel's copy of the Root's ceilings (boot.json `env.root.ceilings`) gives the mode too.
    std::fs::remove_file(h.path.join("root/ceilings.json")).unwrap();
    fx.boot["env"]["root"]["ceilings"]["extend"] = json!({"mode": "free"});
    assert!(line(&mut fx).is_some());
    fx.boot["env"]["root"]["ceilings"]["extend"] = json!({"mode": "gated"});
    w("extend.json", &extend_view(&[]));
    assert_eq!(line(&mut fx), None);
}

// ---------------------------------------------------------------------------------------------
// T-AW-14: capability boot line
// ---------------------------------------------------------------------------------------------

fn live_capability() -> Value {
    json!({
        "categories": [
            {"category": "_all", "low_n": false, "pass_permille": 640, "source": "holdout", "tasks": 30},
            {"category": "refactor", "low_n": false, "pass_permille": 420, "source": "pool", "tasks": 9},
            {"category": "refactor", "low_n": false, "pass_permille": 900, "source": "agent", "tasks": 20},
            {"category": "rust-build", "low_n": false, "pass_permille": 455, "source": "pool", "tasks": 12},
            {"category": "shell", "low_n": false, "pass_permille": 910, "source": "pool", "tasks": 15},
            {"category": "web", "low_n": false, "pass_permille": 700, "source": "pool", "tasks": 5},
            {"category": "tiny", "low_n": true, "pass_permille": 0, "source": "pool", "tasks": 2},
            {"category": "untagged", "pass_permille": 100, "source": "pool", "tasks": 1}
        ],
        "sealed": null
    })
}

const CAP_LINE: &str = "- capability (pass rate by task category, kernel-measured): weakest refactor 42% (n 9) · rust-build 45% (n 12); strongest shell 91% (n 15). Full view: jq '.live.capability' state/kernel/self.json";

#[test]
fn t_aw_14_capability_line() {
    let cap = live_capability();
    let digest = capability_digest(Some(&cap)).unwrap();
    assert_eq!(
        digest,
        json!({"refactor": 420, "rust-build": 455, "shell": 910, "web": 700})
    );
    assert_eq!(capability_digest(None), None);
    assert_eq!(capability_digest(Some(&json!({"categories": []}))), None);

    // Cold boot: shown, even when nothing moved.
    let mut fx = Fx::with("cold", SHA_OLD, Some(SHA_OLD));
    fx.capability = Some(cap.clone());
    fx.prev.as_mut().unwrap()["capability"] = digest.clone();
    assert_eq!(fx.find("- capability"), Some(CAP_LINE.to_string()));
    // Not cold, nothing moved: hidden.
    let mut fx = Fx::promote();
    fx.capability = Some(cap.clone());
    fx.prev.as_mut().unwrap()["capability"] = digest.clone();
    assert_eq!(fx.find("- capability"), None);
    // A move of 99‰: hidden; of 100‰: shown.
    fx.prev.as_mut().unwrap()["capability"]["web"] = json!(799);
    assert_eq!(fx.find("- capability"), None);
    fx.prev.as_mut().unwrap()["capability"]["web"] = json!(800);
    assert_eq!(fx.find("- capability"), Some(CAP_LINE.to_string()));
    // A category appeared (or the previous boot stored no digest): shown.
    let mut d = digest.clone();
    d.as_object_mut().unwrap().remove("shell");
    fx.prev.as_mut().unwrap()["capability"] = d;
    assert_eq!(fx.find("- capability"), Some(CAP_LINE.to_string()));
    fx.prev
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("capability");
    assert!(fx.find("- capability").is_some());
    // `low_n` rows are never named, even as the weakest.
    let t = fx.text();
    assert!(
        !t.contains("tiny") && !t.contains("untagged") && !t.contains("_all"),
        "{t}"
    );
    // No live copy: no line.
    fx.capability = None;
    assert_eq!(fx.find("- capability"), None);
    // Few nameable rows.
    let two = json!({"categories": [
        {"category": "a", "low_n": false, "pass_permille": 300, "source": "pool", "tasks": 5},
        {"category": "b", "low_n": false, "pass_permille": 800, "source": "pool", "tasks": 6}]});
    fx.capability = Some(two);
    assert_eq!(
        fx.find("- capability"),
        Some("- capability (pass rate by task category, kernel-measured): weakest a 30% (n 5); strongest b 80% (n 6). Full view: jq '.live.capability' state/kernel/self.json".to_string())
    );
    fx.capability = Some(
        json!({"categories": [{"category": "a", "low_n": false, "pass_permille": 300, "source": "agent", "tasks": 5}]}),
    );
    assert!(
        fx.find("- capability")
            .unwrap()
            .contains(": weakest a 30% (n 5). Full view")
    );
}

// ---------------------------------------------------------------------------------------------
// T-AW-7: signals
// ---------------------------------------------------------------------------------------------

fn th() -> Thresholds {
    Thresholds {
        w_eff: 200_000,
        t_trigger: 183_000,
        t_target: 100_000,
        keep: 30_000,
        max_tail: 80_000,
        hard_limit: 616_000,
    }
}

fn cx0() -> SignalCx {
    SignalCx {
        level: Some(0),
        day_ceiling_musd: Some(5_000_000),
        restarts_since_lkg: Some(0),
        provisional: false,
        kernel_provisional: false,
    }
}

fn stats_ok() -> Stats {
    Stats {
        today_musd: Some(1_230_000),
        cap_today_musd: Some(4_000_000),
        money_enforced: None,
        evolution_checks_today_musd: None,
        cache_last_permille: Some(965),
        cache_today_permille: Some(912),
    }
}

const UP: Duration = Duration::from_secs(2 * 3600 + 14 * 60 + 5);

#[test]
fn t_aw_7_signal_golden() {
    assert_eq!(
        signal_text(82_000, &th(), &cx0(), &stats_ok(), UP),
        "[init:signals] L0 · context 41% (82k of 200k) · spend $1.23 of $4.00 today, ceiling $5.00 · cache 96% last, 91% today · up 2h14m · restarts since LKG 0"
    );
}

#[test]
fn t_aw_7_unknowns_render_question_marks() {
    assert_eq!(
        signal_text(
            82_000,
            &th(),
            &SignalCx::default(),
            &Stats::default(),
            Duration::from_secs(59)
        ),
        "[init:signals] L? · context 41% (82k of 200k) · spend ?, ceiling ? · cache ? · up 0m · restarts since LKG ?"
    );
    let partial = Stats {
        today_musd: Some(10_000),
        cache_last_permille: Some(500),
        ..Stats::default()
    };
    assert_eq!(
        signal_text(0, &th(), &cx0(), &partial, Duration::from_secs(14 * 60)),
        "[init:signals] L0 · context 0% (0k of 200k) · spend $0.01 of ? today, ceiling $5.00 · cache 50% last, ? today · up 14m · restarts since LKG 0"
    );
}

#[test]
fn t_aw_7_compaction_hint_at_85_percent_of_trigger() {
    // 85% of 183,000 = 155,550.
    let before = signal_text(155_549, &th(), &cx0(), &stats_ok(), UP);
    assert!(!before.contains("compaction"), "{before}");
    let at = signal_text(155_550, &th(), &cx0(), &stats_ok(), UP);
    assert!(
        at.ends_with(" · restarts since LKG 0 · compaction at 183k"),
        "{at}"
    );
    let mut cx = cx0();
    cx.provisional = true;
    cx.kernel_provisional = true;
    assert!(signal_text(170_000, &th(), &cx, &stats_ok(), UP).ends_with(
        " · provisional: G4 pending, no external actions · kernel provisional: audit pending, no external actions · compaction at 183k"
    ));
}

#[test]
fn signal_cx_from_boot() {
    let mut b = boot_json();
    b["restarts_since_lkg"] = json!(3);
    b["evolution"] = json!({"state": "kernel_provisional"});
    assert_eq!(
        SignalCx::from_boot(&b),
        SignalCx {
            level: Some(0),
            day_ceiling_musd: Some(5_000_000),
            restarts_since_lkg: Some(3),
            provisional: true,
            kernel_provisional: true
        }
    );
    assert_eq!(
        SignalCx::from_boot(&json!({"provisional": null})),
        SignalCx::default()
    );
}

// ---------------------------------------------------------------------------------------------
// /_init/stats over a Unix socket
// ---------------------------------------------------------------------------------------------

/// A one-shot fake proxy on a short relative socket path (sun_path limit); returns the request.
struct FakeProxy {
    path: PathBuf,
    handle: Option<JoinHandle<Vec<u8>>>,
}

/// The path from `from` to `to`, both absolute and canonical (`..` steps, then the rest of `to`).
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let (f, t): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut r = PathBuf::new();
    for _ in common..f.len() {
        r.push("..");
    }
    for c in &t[common..] {
        r.push(c);
    }
    r
}

/// A socket path that `bind` and `connect` accept for the file `dir/name`: absolute when it fits
/// sun_path, else relative to `cwd`. Under G0 the target dir is `$INIT_HOME/cache/build/slot/out/target`
/// (92 bytes for a 64-byte home, the most the kernel allows) and the cwd is `slot/src` next to it, so
/// the relative path stays short however long the home is (demo 2026-10-25, F5). The tests never
/// change the cwd, and the fake proxy and `fetch_stats` run in this process.
fn sock_path(dir: &Path, cwd: &Path, name: &str) -> PathBuf {
    let abs = dir.join(name);
    if abs.as_os_str().len() <= crate::provider::transport::MAX_SOCK_PATH {
        return abs;
    }
    let rel = relative_path(cwd, &abs);
    if rel.as_os_str().len() < abs.as_os_str().len() { rel } else { abs }
}

impl FakeProxy {
    fn start(response: Vec<u8>, delay: Duration) -> FakeProxy {
        // A short name in `<target>/sk/`: under G0 a path in test-homes/ can exceed sun_path.
        let dir = test_target().join("sk");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let cwd = std::env::current_dir().unwrap();
        let path = sock_path(&dir, &cwd, &format!("a{}-{n}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let Ok((mut s, _)) = l.accept() else {
                return Vec::new();
            };
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            std::thread::sleep(delay);
            let _ = s.write_all(&response);
            req
        });
        FakeProxy {
            path,
            handle: Some(handle),
        }
    }

    fn request(mut self) -> String {
        String::from_utf8(self.handle.take().unwrap().join().unwrap()).unwrap()
    }
}

impl Drop for FakeProxy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

const STATS_BODY: &str = r#"{"cache":{"last_permille":965,"session_permille":930,"today_permille":912,"window_1h_permille":950},"last_head_hash":"sha256:9b1e","requests_today":312,"spend":{"cap_today_musd":4000000,"day_starts_at":"2026-10-19T17:00:00Z","resets_at":"2026-10-20T17:00:00Z","today_musd":1230000},"v":1}"#;

#[test]
fn fetch_stats_content_length_and_chunked() {
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{STATS_BODY}",
        STATS_BODY.len()
    );
    let p = FakeProxy::start(resp.into_bytes(), Duration::ZERO);
    assert_eq!(fetch_stats(&p.path, Duration::from_secs(2)), stats_ok());
    let req = p.request();
    assert!(req.starts_with("GET /_init/stats HTTP/1.1\r\n"), "{req}");
    assert!(
        req.to_ascii_lowercase().contains("\r\nhost: init\r\n"),
        "{req}"
    );

    let (a, b) = STATS_BODY.split_at(40);
    let resp = format!(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
        a.len(),
        b.len()
    );
    let p = FakeProxy::start(resp.into_bytes(), Duration::ZERO);
    assert_eq!(fetch_stats(&p.path, Duration::from_secs(2)), stats_ok());
}

/// F5 of the 2026-10-25 demo: G0 failed this file's socket tests ("path must be shorter than
/// SUN_LEN") when `INIT_HOME` was 63 bytes or longer. The longest layout G0 can have (a 64-byte
/// home, the kernel's limit) still gets a socket path within sun_path, and it names the same file.
#[test]
fn fake_proxy_socket_fits_the_longest_g0_layout() {
    let home = PathBuf::from(format!("/{}", "h".repeat(63)));
    assert_eq!(home.as_os_str().len(), 64);
    let slot = home.join("cache/build/slot");
    let (dir, cwd) = (slot.join("out/target/sk"), slot.join("src"));
    let name = "a99999-99999";
    let p = sock_path(&dir, &cwd, name);
    assert!(
        p.as_os_str().len() <= crate::provider::transport::MAX_SOCK_PATH,
        "{} bytes: {}",
        p.as_os_str().len(),
        p.display()
    );
    assert_eq!(p, Path::new("../out/target/sk").join(name));
    // A short layout keeps the absolute path.
    let short = Path::new("/r/seed");
    assert_eq!(
        sock_path(&short.join("target/sk"), short, name),
        short.join("target/sk").join(name)
    );
}

#[test]
fn fetch_stats_failures_give_all_unknown() {
    // Missing socket.
    assert_eq!(
        fetch_stats(
            Path::new("target/test-homes/wp13/none.sock"),
            Duration::from_millis(200)
        ),
        Stats::default()
    );
    // Non-200.
    let p = FakeProxy::start(
        b"HTTP/1.1 503 Busy\r\ncontent-length: 2\r\n\r\n{}".to_vec(),
        Duration::ZERO,
    );
    assert_eq!(
        fetch_stats(&p.path, Duration::from_secs(2)),
        Stats::default()
    );
    // Not JSON.
    let p = FakeProxy::start(
        b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nxyz".to_vec(),
        Duration::ZERO,
    );
    assert_eq!(
        fetch_stats(&p.path, Duration::from_secs(2)),
        Stats::default()
    );
    // Too slow: the timeout bounds the call.
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{STATS_BODY}",
        STATS_BODY.len()
    );
    // The proxy answers after 10 s; a 200 ms timeout must end the call long before that (the
    // ceiling leaves a loaded machine room; the old 1.5 s / 1 s pair left it 0.8 s).
    let p = FakeProxy::start(resp.into_bytes(), Duration::from_secs(10));
    let t0 = Instant::now();
    assert_eq!(
        fetch_stats(&p.path, Duration::from_millis(200)),
        Stats::default()
    );
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "{:?}",
        t0.elapsed()
    );
    // Nulls and floats are unknown; integers are read.
    let body = r#"{"cache":{"last_permille":null,"today_permille":91.2},"spend":{"cap_today_musd":4000000,"today_musd":"1"}}"#;
    let resp = format!("HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n{body}");
    let p = FakeProxy::start(resp.into_bytes(), Duration::ZERO);
    assert_eq!(
        fetch_stats(&p.path, Duration::from_secs(2)),
        Stats {
            cap_today_musd: Some(4_000_000),
            ..Stats::default()
        }
    );
}

#[test]
fn signals_source_fills_text_and_integer_extras() {
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{STATS_BODY}",
        STATS_BODY.len()
    );
    let p = FakeProxy::start(resp.into_bytes(), Duration::ZERO);
    let mut s = Signals {
        proxy_sock: Some(p.path.clone()),
        cx: cx0(),
        start: Instant::now(),
        stats_timeout: Duration::from_secs(2),
    };
    let ev = s.signal(82_000, 410, &th());
    assert_eq!(ev.ctx_tokens, 82_000);
    assert_eq!(ev.ctx_permille, 410);
    assert!(ev.text.starts_with("[init:signals] L0 · context 41% (82k of 200k) · spend $1.23 of $4.00 today, ceiling $5.00 · cache 96% last, 91% today · up 0m"), "{}", ev.text);
    assert_eq!(
        Value::Object(ev.extra.clone()),
        json!({"cache_last_permille": 965, "cache_today_permille": 912, "cap_today_musd": 4000000, "level": 0, "spend_today_musd": 1230000, "uptime_s": 0})
    );
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(crate::log::icj::find_float(&v), None, "integers only (ICJ)");
    // No socket: `?` everywhere the stats feed, no stats keys.
    let mut s = Signals {
        proxy_sock: None,
        cx: SignalCx::default(),
        start: Instant::now(),
        stats_timeout: Duration::from_millis(200),
    };
    let ev = s.signal(1_000, 5, &th());
    assert!(
        ev.text.contains(" · spend ?, ceiling ? · cache ? · "),
        "{}",
        ev.text
    );
    assert_eq!(Value::Object(ev.extra), json!({"uptime_s": 0}));
}

#[test]
fn observed_proxy_stats_keep_evolution_checks_and_suppress_cap() {
    let v = json!({"spend": {"enforced": false, "today_musd": 1_230_000,
        "cap_today_musd": 4_000_000, "evolution_today_musd": 120_000}});
    let stats = Stats::from_json(&v);
    assert_eq!(stats.today_musd, Some(1_230_000));
    assert_eq!(stats.cap_today_musd, None);
    assert_eq!(stats.evolution_checks_today_musd, Some(120_000));
    let text = signal_text(82_000, &th(), &cx0(), &stats, UP);
    assert!(text.contains("spend $1.23 today (observed)"), "{text}");
    assert!(!text.contains("ceiling") && !text.contains("of $4.00"), "{text}");
    assert_eq!(Stats::from_json(&json!({"spend":{"enforced":true,"cap_today_musd":7}})).cap_today_musd, Some(7));
    assert_eq!(Stats::from_json(&json!({"spend":{"policy":"observe","cap_today_musd":7}})).cap_today_musd, None);
    assert_eq!(Stats::from_json(&json!({})).evolution_checks_today_musd, None);
    assert_eq!(Stats::from_json(&json!({"spend":{"evolution_today_musd":"0"}})).evolution_checks_today_musd, None);
}

#[test]
fn observed_signal_extra_contains_only_known_evolution_cost() {
    let body = json!({"spend": {"enforced": false, "today_musd": 123,
        "cap_today_musd": 1, "evolution_today_musd": 0}}).to_string();
    let response = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}", body.len());
    let proxy = FakeProxy::start(response.into_bytes(), Duration::ZERO);
    let mut signals = Signals {
        proxy_sock: Some(proxy.path.clone()), cx: cx0(), start: Instant::now(),
        stats_timeout: Duration::from_secs(2),
    };
    let event = signals.signal(10, 0, &th());
    assert_eq!(event.extra.get("evolution_checks_today_musd"), Some(&json!(0)));
    assert!(!event.extra.contains_key("cap_today_musd"));
    assert_eq!(event.extra.get("spend_today_musd"), Some(&json!(123)));
}
