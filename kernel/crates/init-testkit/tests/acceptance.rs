//! WP15: the acceptance scenarios of testing.md §3.9 (A1–A6) and §3.17 (NO-OWNER-1), at $0.
//!
//! Every scenario is a JSON file under the repo-root `scenarios/` run by WP3's runner
//! ([`run_scenario_with`]); the fake-body scenarios that need nothing prepared use
//! [`init_testkit::scenario_test!`] directly. The real-stack scenarios run the **real
//! `init-kernel`** (this target dir) under **`init-fakeroot`**, with `init-mockllm --strict
//! deepseek` as the model. The body is the real `init-body` (A1) or, where the scenario must build
//! a candidate body in G0 (A3, A4), WP8b's model-driven fixture body `sv-agentbody` behind a
//! one-file crate: a G0 build of the real seed (wasmtime) does not fit tonight's disk budget.
//!
//! **Templates.** `init-kernel install --template` (supervise.md §4.8) is not built, so this file
//! prepares the template homes itself, once per test process, under `target/t15/`: the seed tree
//! as `lineage/body.git` + the agent clone `body/`, `bin/init-body{,.lkg}`, the seed `lkg.json`
//! that `init-kernel install` writes, `kernel/config.toml` and `kernel/bin/init-kernel`. A
//! scenario copies its template into the home with the runner's own `fixture.work` entry `".."`
//! (copied after the body binary and before `kernel_config`).
//!
//! **Homes** are `target/t15/<id>-<pid>-<hex>/` (short: a probe lane's socket must fit in 103
//! bytes), kept by the runner on failure, removed by exact path here on success. Every process is
//! started by the runner or by the fake Root and stopped by its held handle; nothing here
//! signals anything.

use init_testkit::scenario::{
    Outcome, Report, RunOpts, Scenario, Step, run_scenario_with, validate,
};
use init_testkit::util;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// The watchdog bound of every test here (K-FLAKE: 3-10x its normal runtime; init_testkit::watchdog).
const WD: Duration = Duration::from_secs(1230);

// -------------------------------------------------------------------------------------------
// Paths, binaries, disk
// -------------------------------------------------------------------------------------------

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repo root")
}

/// `target/t15/`: short, inside the repo, gitignored.
fn t15() -> PathBuf {
    let d = repo().join("target/t15");
    fs::create_dir_all(&d).unwrap();
    d
}

fn scenario(name: &str) -> PathBuf {
    repo().join("scenarios").join(name)
}

/// The debug dir of this target (`target/k1/debug`).
fn debug_dir() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// A binary of another kernel-workspace package, built into this target dir if missing.
fn kernel_bin(pkg: &str, bin: &str) -> PathBuf {
    let p = debug_dir().join(bin);
    if !p.is_file() {
        let st = init_testkit::watchdog::status(Command::new(env!("CARGO"))
            .args(["build", "--offline", "-p", pkg, "--bin", bin])
            .env("CARGO_TARGET_DIR", debug_dir().parent().unwrap())
            .current_dir(repo().join("kernel")))
            .unwrap();
        assert!(st.success(), "building {bin}");
    }
    p
}

/// Free space on the repo's volume, in GiB (BUILD_AGENT_BRIEF disk rule; WP15 stops at 5.3).
fn free_gib() -> f64 {
    let o = init_testkit::watchdog::output(Command::new("/bin/df")
        .args(["-k"])
        .arg(repo()))
        .unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    let kib: f64 = t
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|x| x.parse().ok())
        .unwrap_or(0.0);
    kib / (1024.0 * 1024.0)
}

fn check_disk() {
    let f = free_gib();
    assert!(f >= 5.3, "only {f:.2} GiB free: WP15 stops below 5.3 GiB");
}

fn sha256_file(p: &Path) -> String {
    util::sha256_hex(&fs::read(p).unwrap())
}

fn toolchain() -> PathBuf {
    let o = init_testkit::watchdog::output(Command::new("rustc")
        .args(["--print", "sysroot"]))
        .expect("rustc on PATH");
    PathBuf::from(String::from_utf8(o.stdout).unwrap().trim())
}

/// git with a fixed identity and date and no user or system config, so template commits are
/// reproducible and nothing of the owner's leaks in.
fn git(dir: &Path, args: &[&str]) -> String {
    let o = init_testkit::watchdog::output(Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", t15())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "INIT Seed")
        .env("GIT_AUTHOR_EMAIL", "seed@init.invalid")
        .env("GIT_COMMITTER_NAME", "INIT Seed")
        .env("GIT_COMMITTER_EMAIL", "seed@init.invalid")
        .env("GIT_AUTHOR_DATE", "2026-10-20T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-20T00:00:00Z"))
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

/// Remove a directory this file created, by its exact path, only under `target/t15/`.
fn remove_exact(p: &Path) {
    let root = t15();
    assert!(
        p.starts_with(&root) && p != root,
        "refusing to remove {}",
        p.display()
    );
    if p.exists() {
        fs::remove_dir_all(p).unwrap();
    }
}

fn write_exec(p: &Path, text: &str) {
    fs::write(p, text).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
}

// -------------------------------------------------------------------------------------------
// Template homes
// -------------------------------------------------------------------------------------------

/// `kernel/config.toml` for every real-kernel scenario (kernel/config.md; testing.md §3.4.6's
/// test values). Scenarios add their own keys through `kernel_config`.
fn kernel_config() -> String {
    format!(
        "schema_version = 2\n\n[model]\nprovider = \"deepseek\"\nid = \"deepseek-flash\"\n\n\
         [proxy]\ntest_mode = true\nroot_poll_s = 1\n\n\
         [budget]\nday_usd = \"5.00\"\ntask_usd = \"0.50\"\nrequest_max_usd = \"1.00\"\n\n\
         [providers.deepseek.models.\"deepseek-flash\"]\nmax_output = 65536\nreserve_output_tokens = 4096\n\
         prices.standard = {{ input = \"0.30\", cache_read = \"0.006\", output = \"1.20\" }}\n\n\
         [sandbox]\ntoolchain = {tc:?}\n\n\
         [supervise]\ngit = \"/usr/bin/git\"\nready_timeout_s = 30\nsettle_s = 1\nbackoff_initial_ms = 50\n\
         backoff_max_ms = 400\nexit_grace_s = 5\nprobation_s = 2\n\n\
         [supervise.disk]\nmin_free_build_mib = 64\nmin_free_run_mib = 64\n\n\
         [gate]\nquiet_ms = 600\n[gate.g2]\nwindow_s = 1\nparent_retry_s = 1\nready_timeout_s = 30\n\
         [gate.g3]\nmode = \"scripted\"\nagent_timeout_s = 120\ninner_timeout_s = 300\n\n\
         [gate.g4]\npool_k = 2\nmin_pool = 2\nholdout_h = 1\nmin_holdout = 1\ncanaries = 1\nmin_canaries = 1\n\
         repeats = 2\nrepeats_max = 2\nmargin_max = 0.2\nbootstrap_b = 2000\nnoise_b = 500\nparallel = 4\n\
         ct_include_wall = false\ntask_wall_s = 60\n\n[evolution]\ng4_escalate_repeats = 2\n",
        tc = toolchain().to_string_lossy()
    )
}

/// A template home from a body source tree `src` (committed as the seed X) and a body binary.
/// Returns X.
fn build_template(t: &Path, src: &Path, body_bin: &Path) -> String {
    remove_exact(t);
    for d in [
        "bin",
        "kernel/bin",
        "state/kernel",
        "lineage",
        "work",
        "cache",
    ] {
        fs::create_dir_all(t.join(d)).unwrap();
    }
    git(src, &["init", "-q", "-b", "main"]);
    git(src, &["add", "-A"]);
    git(
        src,
        &[
            "commit",
            "-q",
            "-m",
            "feat: seed\n\nWhy: the WP15 acceptance seed.\nPredicts: pass_rate >= parent\nCompensates-for: none (seed)\nPrefix-change: no\n",
        ],
    );
    let x = git(src, &["rev-parse", "HEAD"]);
    git(
        &t.join("lineage"),
        &["clone", "-q", "--bare", src.to_str().unwrap(), "body.git"],
    );
    let bare = t.join("lineage/body.git");
    git(&bare, &["remote", "remove", "origin"]);
    git(&bare, &["update-ref", "refs/heads/lkg", &x]);
    git(t, &["clone", "-q", bare.to_str().unwrap(), "body"]);
    // The agent clone's origin is the home's own lineage repo, wherever the home is copied.
    git(
        &t.join("body"),
        &["remote", "set-url", "origin", "../lineage/body.git"],
    );
    // `lineage/trees/<sha12>/`: the neutral export of X (the LKG tree a safe-mode body boots from).
    let tree = t.join("lineage/trees").join(&x[..12]);
    fs::create_dir_all(&tree).unwrap();
    let mut ar = init_testkit::watchdog::spawn(Command::new("/usr/bin/git")
        .arg(format!("--git-dir={}", bare.display()))
        .args(["archive", "--format=tar", &x])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped()))
        .unwrap();
    let st = init_testkit::watchdog::status(Command::new("/usr/bin/tar")
        .args(["-x", "-C"])
        .arg(&tree)
        .stdin(ar.stdout.take().unwrap()))
        .unwrap();
    assert!(
        st.success() && ar.wait().unwrap().success(),
        "export the tree of X"
    );
    // The Root writes the charter's constitution to `kernel/CONSTITUTION.md` (init-root install.rs);
    // the fake Root does not, so the template carries the Root's own asset.
    fs::copy(
        repo().join("root/crates/init-root/assets/CONSTITUTION.md"),
        t.join("kernel/CONSTITUTION.md"),
    )
    .unwrap();
    fs::copy(body_bin, t.join("bin/init-body")).unwrap();
    fs::copy(body_bin, t.join("bin/init-body.lkg")).unwrap();
    // What `init-kernel install` leaves (supervise.md §3.17, WP4 install.rs).
    let lkg = json!({"v": 1, "sha": x, "binary": "bin/init-body.lkg", "binary_sha256": sha256_file(body_bin),
        "tree": format!("lineage/trees/{}", &x[..12]), "reason": "seed", "lineage_seq": null, "seq": 1,
        "prev_sha": null, "gate_job": null, "committed_at": "2026-10-20T00:00:00.000Z", "model": null, "ext_set": null});
    fs::write(
        t.join("state/kernel/lkg.json"),
        serde_json::to_vec_pretty(&lkg).unwrap(),
    )
    .unwrap();
    fs::write(t.join("kernel/config.toml"), kernel_config()).unwrap();
    // G1 runs `$INIT_HOME/kernel/bin/init-kernel selfmap …`; the body's bash finds `initctl` here.
    let kernel = kernel_bin("init-kernel", "init-kernel");
    write_exec(
        &t.join("kernel/bin/init-kernel"),
        &format!("#!/bin/sh\nexec {:?} \"$@\"\n", kernel.to_string_lossy()),
    );
    let initctl = kernel_bin("initctl", "initctl");
    write_exec(
        &t.join("kernel/bin/initctl"),
        &format!("#!/bin/sh\nexec {:?} \"$@\"\n", initctl.to_string_lossy()),
    );
    x
}

// -------------------------------------------------------------------------------------------
// The fixture body for A3/A4 (G0 builds the candidate, so it must be a small crate)
// -------------------------------------------------------------------------------------------

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap().flatten() {
        let (s, d) = (e.path(), dst.join(e.file_name()));
        if s.is_dir() {
            copy_dir(&s, &d);
        } else {
            fs::copy(&s, &d).unwrap();
        }
    }
}

/// `init-kernel selfmap --root <dir> --write` with the toolchain on PATH (WP7b's helper).
fn selfmap_write(dir: &Path) {
    let o = init_testkit::watchdog::output(Command::new(kernel_bin("init-kernel", "init-kernel"))
        .args(["selfmap", "--root"])
        .arg(dir)
        .arg("--write")
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", toolchain().join("bin").display()),
        )
        .env("HOME", t15())
        .env("CARGO_TARGET_DIR", t15().join("fxt"))
        .env("GIT_CONFIG_NOSYSTEM", "1"))
        .unwrap();
    assert!(
        o.status.success(),
        "selfmap --write: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// One frozen G4 task (gate.md §3.7.1, WP8b's shape): the prompt asks for `42` in `answer.txt`.
/// The per-task mock scripts are `scenarios/scripts/a3/tasks/<id>.mock.jsonl` (none for the
/// canary: the mock's give-up responder answers it, so it scores 0 and nothing leaks).
fn write_task(t: &Path, rel: &str, id: &str) {
    let d = t.join(rel).join(id);
    fs::create_dir_all(d.join("check")).unwrap();
    let ws = t15().join("ws-src").join(id);
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("README"), format!("task {id}\n")).unwrap();
    let st = init_testkit::watchdog::status(Command::new("/usr/bin/tar")
        .arg("-cf")
        .arg(d.join("workspace.tar"))
        .arg("-C")
        .arg(&ws)
        .arg("README"))
        .unwrap();
    assert!(st.success());
    remove_exact(&ws);
    let bytes = fs::read(d.join("workspace.tar")).unwrap();
    let tj = json!({"v": 2, "id": id, "prompt": "Write 42 into answer.txt in the workspace.", "labels": ["basic"],
        "workspace": {"file": "workspace.tar", "sha256": util::sha256_hex(&bytes), "bytes": bytes.len()},
        "check": {"argv": ["check/run"], "timeout_s": 30}, "limits": {"wall_s": 60, "max_cost_musd": 500000},
        "source": {"kind": "maintainer"}, "created": "2026-10-20T00:00:00Z"});
    fs::write(d.join("task.json"), serde_json::to_vec_pretty(&tj).unwrap()).unwrap();
    write_exec(
        &d.join("check/run"),
        "#!/bin/sh\n[ \"$(cat answer.txt 2>/dev/null)\" = 42 ] && exit 0\nexit 1\n",
    );
}

static FX: OnceLock<PathBuf> = OnceLock::new();

/// The A3/A4 template: `fixtures/fx-body/` + `agent-path.txt` (this target's `sv-agentbody`) +
/// its `self.json`, committed as X and built once; the G4 fixture pool (2 pool, 1 holdout, 1 canary).
fn fx_template() -> &'static Path {
    FX.get_or_init(|| {
        templates(); // binaries and env first
        let t = t15();
        let src = t.join("src-fx");
        remove_exact(&src);
        copy_dir(&repo().join("fixtures/fx-body"), &src);
        let agent = kernel_bin("kernel-supervise", "sv-agentbody");
        fs::write(src.join("agent-path.txt"), format!("{}\n", agent.display())).unwrap();
        selfmap_write(&src);
        let st = init_testkit::watchdog::status(Command::new(toolchain().join("bin/cargo"))
            .args(["build", "--offline", "-q"])
            .current_dir(&src)
            .env("CARGO_TARGET_DIR", t.join("fxt")))
            .unwrap();
        assert!(st.success(), "building the fixture body");
        let bin = t.join("bin/init-body-fx");
        fs::copy(t.join("fxt/debug/init-body"), &bin).unwrap();
        let tm = t.join("tm-fx");
        build_template(&tm, &src, &bin);
        remove_exact(&src);
        for i in 0..2 {
            write_task(&tm, "vault/gate/pool", &format!("t{i}"));
        }
        write_task(&tm, "vault/holdout", "h1");
        write_task(&tm, "vault/canaries", "k1");
        let secret = "4f0c1d2e3b4a59687766554433221100";
        fs::write(tm.join("vault/canaries/k1/secret"), secret).unwrap();
        write_exec(
            &tm.join("vault/canaries/k1/check/run"),
            &format!("#!/bin/sh\ngrep -q {secret} answer.txt 2>/dev/null && exit 0\nexit 1\n"),
        );
        fs::write(
            tm.join("vault/canaries/k1/canary.json"),
            json!({"v": 1, "kind": "leak", "secret_sha256": "x", "notes": "WP15 test canary"})
                .to_string(),
        )
        .unwrap();
        remove_exact(&t.join("ws-src"));
        tm
    })
}

struct Templates {
    /// The real body: the committed seed tree and the real body binary (frozen copy).
    seed: PathBuf,
    seed_sha: String,
    /// The same seed tree with `init-fakebody` as the body binary (A5-fake).
    fake: PathBuf,
}

static TEMPLATES: OnceLock<Templates> = OnceLock::new();
static SERIAL: Mutex<()> = Mutex::new(());

/// One scenario at a time in this process (they share the CPU-heavy kernel and the mock ports).
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// A fresh `dst` holding the committed tree of `seed/` at HEAD (git archive; no build output).
fn seed_tree(dst: &Path) {
    remove_exact(dst);
    fs::create_dir_all(dst).unwrap();
    let mut ar = init_testkit::watchdog::spawn(Command::new("/usr/bin/git")
        .args(["-C"])
        .arg(repo())
        .args(["archive", "--format=tar", "HEAD", "seed"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped()))
        .unwrap();
    let st = init_testkit::watchdog::status(Command::new("/usr/bin/tar")
        .args(["-x", "--strip-components", "1", "-C"])
        .arg(dst)
        .stdin(ar.stdout.take().unwrap()))
        .unwrap();
    assert!(
        st.success() && ar.wait().unwrap().success(),
        "git archive seed"
    );
}

fn templates() -> &'static Templates {
    TEMPLATES.get_or_init(|| {
        check_disk();
        let t = t15();
        // Frozen copies: other lanes rebuild the shared target dirs while tests run.
        let bins = t.join("bin");
        fs::create_dir_all(&bins).unwrap();
        let body_src = std::env::var_os("WP15_BODY_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("seed/target/debug/init-body"));
        assert!(
            body_src.is_file(),
            "no init-body at {} (build seed/, or set WP15_BODY_BIN)",
            body_src.display()
        );
        let body = bins.join("init-body");
        fs::copy(&body_src, &body).unwrap();
        // The kernel the fake Root starts: a one-line wrapper around this target's `init-kernel`.
        // The fake Root SHA-256es its `--kernel` file at start with the unoptimized debug sha2
        // (about 10 s for the 154 MB debug kernel, past the runner's 10 s owner.sock wait).
        let kernel = bins.join("init-kernel");
        write_exec(
            &kernel,
            &format!(
                "#!/bin/sh\nexec {:?} \"$@\"\n",
                kernel_bin("init-kernel", "init-kernel").to_string_lossy()
            ),
        );
        // SAFETY: set once, before any scenario spawns a process (all tests pass through here).
        unsafe {
            std::env::set_var("INIT_BODY_BIN", &body);
            std::env::set_var("INIT_KERNEL_BIN", &kernel);
        }
        let src = t.join("src-seed");
        seed_tree(&src);
        let seed = t.join("tm-seed");
        let seed_sha = build_template(&seed, &src, &body);
        seed_tree(&src);
        let fake = t.join("tm-fake");
        build_template(&fake, &src, Path::new(env!("CARGO_BIN_EXE_init-fakebody")));
        remove_exact(&src);
        Templates {
            seed,
            seed_sha,
            fake,
        }
    })
}

// -------------------------------------------------------------------------------------------
// Running
// -------------------------------------------------------------------------------------------

/// Run a scenario, keep its home, and hand the passed report to `after` (extra checks outside the
/// sandbox); then remove the home by exact path. A failure keeps the home for diagnosis.
fn run(name: &str, after: impl FnOnce(&Report, &Path)) {
    let _g = serial();
    check_disk();
    let opts = RunOpts {
        scratch_root: Some(t15()),
        keep: true,
        ..RunOpts::default()
    };
    let r = run_scenario_with(&scenario(name), &opts);
    match &r.outcome {
        Outcome::Passed => {}
        Outcome::Skipped(why) => {
            eprintln!("{}: {why}", r.id);
            return;
        }
        Outcome::Failed(_) => panic!("{r}"),
    }
    let scratch = r.scratch.clone().expect("kept");
    after(&r, &scratch.join("h"));
    eprintln!("{r}");
    remove_exact(&scratch);
}

fn events(home: &Path) -> Vec<Value> {
    let mut files: Vec<PathBuf> = fs::read_dir(home.join("state/sessions"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "jsonl"));
    files.sort();
    files.iter().flat_map(|f| util::read_jsonl(f)).collect()
}

// -------------------------------------------------------------------------------------------
// A1
// -------------------------------------------------------------------------------------------

/// A1 (testing.md §3.9): the real kernel and the real body fix a failing unit test on the mock:
/// read → edit → bash `python3 -m unittest -q` → final message; then the runner's acceptance
/// check outside the sandbox (the suite exits 0) and the body was still running before the stop.
#[test]
fn a1_solves_a_coding_task_on_the_mock() {
    let _wd = init_testkit::watchdog::arm("a1_solves_a_coding_task_on_the_mock", Duration::from_secs(260));
    let tm = templates();
    assert!(tm.seed.join("body/GENESIS.md").is_file());
    run("a1.json", |r, home| {
        assert_eq!(
            r.captures.get("BODIES_BEFORE_STOP"),
            Some(&json!(1)),
            "the body was running before the stop"
        );
        let calc = home.join("work/a1-calc");
        let out = init_testkit::watchdog::output(Command::new("/usr/bin/python3")
            .args(["-I", "-m", "unittest", "-q"])
            .current_dir(&calc)
            .env_clear()
            .env("PATH", "/usr/bin:/bin"))
            .unwrap();
        assert!(
            out.status.success(),
            "acceptance check: python3 -m unittest -q: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // The boot ran on the template's seed commit.
        let boot = events(home)
            .into_iter()
            .find(|e| e["type"] == "boot")
            .unwrap();
        assert_eq!(boot["sha"], json!(tm.seed_sha));
    });
}

// -------------------------------------------------------------------------------------------
// A2
// -------------------------------------------------------------------------------------------

fn fixture_text(rel: &str) -> String {
    fs::read_to_string(repo().join(rel)).unwrap()
}

/// `scenarios/scripts/a2.mock.jsonl` with its placeholders filled from the fixtures (one source
/// of truth: the extension's files and the version lists), written to `target/t15/gen/`.
fn a2_script() {
    let gen_dir = t15().join("gen");
    fs::create_dir_all(&gen_dir).unwrap();
    let js = |s: &str| serde_json::to_string(s).unwrap();
    let versions: Vec<String> = fixture_text("fixtures/a2/versions.txt")
        .lines()
        .map(str::to_string)
        .collect();
    let text = fixture_text("scenarios/scripts/a2.mock.jsonl")
        .replace(
            "\"@CARGO_TOML@\"",
            &js(&fixture_text("fixtures/ext/semver-sort/Cargo.toml")),
        )
        .replace(
            "\"@LIB_RS@\"",
            &js(&fixture_text("fixtures/ext/semver-sort/src/lib.rs")),
        )
        .replace("\"@VERSIONS@\"", &serde_json::to_string(&versions).unwrap())
        .replace(
            "\"@SORTED_TEXT@\"",
            &js(&fixture_text("fixtures/a2/expected-sorted.txt")),
        )
        .replace("@VENDOR@", repo().join("vendor").to_str().unwrap());
    fs::write(gen_dir.join("a2.mock.jsonl"), text).unwrap();
}

/// A2 (testing.md §3.9; ext/host.md §3.9 rule 3): the real body grows a T1 capability. The agent
/// writes `body/ext/semver-sort/` (from the ext-authoring template), builds it with
/// `cargo build --target wasm32-wasip2`, runs `initctl reload`, and calls it through the `ext`
/// tool; the reload is `T1` with `prefix_changed:false`, the prefix stays constant across every
/// request (the head is unchanged) and the `tools` array is identical in every request. The
/// extension's output and `work/a2/sorted.txt` equal `fixtures/a2/expected-sorted.txt`.
/// Deviation: the build points cargo at the repo's `vendor/` with `--config`, because the seed's
/// `.cargo/config.toml` names `../vendor` relative to `body/` and a test home has no vendor copy.
#[test]
fn a2_grows_a_t1_capability_with_the_head_unchanged() {
    let _wd = init_testkit::watchdog::arm("a2_grows_a_t1_capability_with_the_head_unchanged", Duration::from_secs(1170));
    templates();
    a2_script();
    run("a2.json", |r, home| {
        assert_eq!(
            r.captures.get("BODIES_BEFORE_STOP"),
            Some(&json!(1)),
            "the body still runs"
        );
        let want = fixture_text("fixtures/a2/expected-sorted.txt");
        assert_eq!(
            fs::read_to_string(home.join("work/a2/sorted.txt")).unwrap(),
            want
        );
        let ext = events(home)
            .into_iter()
            .find(|e| e["type"] == "tool_result" && e["name"] == "ext")
            .expect("the ext result");
        let out: Value = serde_json::from_str(ext["output"][0]["text"].as_str().unwrap()).unwrap();
        let got: Vec<&str> = out["sorted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            got,
            want.lines().collect::<Vec<_>>(),
            "the extension's own output"
        );
        // The tools array is identical in every turn request (the head did not grow a tool).
        let mut tools = Vec::new();
        for e in fs::read_dir(home.parent().unwrap().join("m"))
            .unwrap()
            .flatten()
        {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with("-turn.json") {
                let v: Value = serde_json::from_slice(&fs::read(e.path()).unwrap()).unwrap();
                let b = if v.get("body").is_some() {
                    v["body"].clone()
                } else {
                    v
                };
                tools.push(b["tools"].to_string());
            }
        }
        assert!(
            tools.len() >= 6 && tools.windows(2).all(|w| w[0] == w[1]),
            "tools changed across requests"
        );
    });
}

// -------------------------------------------------------------------------------------------
// A4, A3
// -------------------------------------------------------------------------------------------

/// Body spawns the fake Root's broker made (`kind:"body"`; probes are `kind:"probe"`).
fn body_spawns(home: &Path) -> Vec<Value> {
    util::read_jsonl(&home.parent().unwrap().join("r/logs/fakeroot.jsonl"))
        .into_iter()
        .filter(|e| e["ev"] == "spawn" && e["kind"] == "body")
        .collect()
}

/// A4 (testing.md §3.9): the agent applies a patch that panics at boot and promotes it; G2's
/// candidate probe exits (`E_GATE_FAILED`, `gate.g2.probe_exit`) and the live body never notices;
/// the agent reads the gate report, removes the panic, squashes, promotes again, and the handoff
/// happens. Pass: one live body until the handoff (two body spawns in total), no crash boot, crash
/// counters 0, `swaps_failed` 0 (a probe failure is not a swap).
#[test]
fn a4_a_candidate_that_crashes_at_boot_is_a_non_event() {
    let _wd = init_testkit::watchdog::arm("a4_a_candidate_that_crashes_at_boot_is_a_non_event", Duration::from_secs(420));
    fx_template();
    run("a4.json", |_r, home| {
        let spawns = body_spawns(home);
        assert_eq!(
            spawns.len(),
            2,
            "the live body ran unchanged until the handoff: {spawns:?}"
        );
    });
}

/// A3 (testing.md §3.9, G-31 at the process level): a T2 self-edit of the loop (`src/main.rs`)
/// through `initctl promote evo/a3 --wait` from inside the turn → G0 (real build) → G1 (the
/// kernel's `selfmap`) → G2 (both probe arms on copies of the live log) → G3 (scripted) →
/// `awaiting_boundary` → the overlapped handoff → the new body boots `promote` on the same
/// session and answers the next owner line with the conversation intact (`prefix_constant`);
/// then the asynchronous G4 on the fixture pool, carried out as its verdict says.
#[test]
fn a3_t2_self_edit_promote_handoff_same_conversation() {
    let _wd = init_testkit::watchdog::arm("a3_t2_self_edit_promote_handoff_same_conversation", WD);
    fx_template();
    run("a3.json", |r, home| {
        let spawns = body_spawns(home);
        assert!(
            spawns.len() >= 2,
            "a handoff spawned the new body: {spawns:?}"
        );
        // The same session continued across the handoff.
        let sessions: Vec<String> = fs::read_dir(home.join("state/sessions"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".jsonl"))
            .collect();
        assert_eq!(
            sessions.len(),
            1,
            "one session across the handoff: {sessions:?}"
        );
        let sup: Value =
            serde_json::from_slice(&fs::read(home.join("state/kernel/supervisor.json")).unwrap())
                .unwrap();
        assert!(sup["counters"]["swaps_ok"].as_u64() >= Some(1), "{sup}");
        let g4: Vec<String> = fs::read_dir(home.join("state/kernel/jobs"))
            .unwrap()
            .flatten()
            .filter_map(|e| serde_json::from_slice::<Value>(&fs::read(e.path()).ok()?).ok())
            .filter(|j| j["cmd"] == "g4")
            .map(|j| {
                format!(
                    "{} escalated={} -> {} {}",
                    j["id"], j["args"]["escalated"], j["result"]["verdict"], j["result"]["reason"]
                )
            })
            .collect();
        eprintln!(
            "A3 NEW_SHA {:?}; supervisor counters {}; G4 jobs {g4:?}",
            r.captures.get("NEW_SHA"),
            sup["counters"]
        );
    });
}

// -------------------------------------------------------------------------------------------
// A5-fake
// -------------------------------------------------------------------------------------------

/// Milliseconds since the epoch of an RFC 3339 UTC timestamp with milliseconds
/// (`2026-10-19T22:09:58.375Z`); only differences are used.
fn ts_ms(t: &str) -> i64 {
    let (date, time) = t.split_once('T').unwrap();
    let d: Vec<i64> = date.split('-').map(|x| x.parse().unwrap()).collect();
    let time = time.trim_end_matches('Z');
    let (hms, ms) = time.split_once('.').unwrap_or((time, "0"));
    let h: Vec<i64> = hms.split(':').map(|x| x.parse().unwrap()).collect();
    // Days from a civil date (Howard Hinnant's algorithm).
    let (y, m) = if d[1] <= 2 {
        (d[0] - 1, d[1] + 9)
    } else {
        (d[0], d[1] - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d[2] - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 24 + h[0]) * 60 + h[1]) * 60_000 + h[2] * 1000 + ms.parse::<i64>().unwrap()
}

/// The agent's one tool call in A5, played by the test: the fake body has no model, so once the
/// body runs in safe mode this helper runs `initctl promote evo/x` exactly as the body's `bash`
/// would (the kernel's `run/kernel.sock`, the body's environment), and leaves the answer in
/// `work/a5/promote.json` for the scenario's `wait_file`. It only reads the home and runs initctl,
/// a process it owns.
fn a5_agent(prefix: String) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let home = loop {
            if std::time::Instant::now() > deadline {
                return;
            }
            let hit = fs::read_dir(t15()).ok().and_then(|d| {
                d.flatten().map(|e| e.path()).find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
                })
            });
            if let Some(h) = hit {
                break h.join("h");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        // The body runs in safe mode: its boot event says so.
        while !events(&home)
            .iter()
            .any(|e| e["type"] == "boot" && e["mode"] == "safe")
        {
            if std::time::Instant::now() > deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let initctl = |args: &[&str]| {
            init_testkit::watchdog::output(Command::new(kernel_bin("initctl", "initctl"))
                .args(args)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", home.join("work/home"))
                .env("INIT_HOME", &home)
                .env("INIT_KERNEL_SOCK", home.join("run/kernel.sock"))
                .current_dir(home.join("work"))
                .stdin(Stdio::null()))
                .unwrap()
        };
        // The initctl server is up once `ping` answers (it binds after the supervisor's services).
        while !initctl(&["ping"]).status.success() {
            if std::time::Instant::now() > deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let out = initctl(&["promote", "evo/x"]);
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            json!({"stdout": String::from_utf8_lossy(&out.stdout), "stderr": String::from_utf8_lossy(&out.stderr)})
        });
        let dir = home.join("work/a5");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("promote.tmp"),
            serde_json::to_vec(&json!({"exit": out.status.code(), "error": v["error"], "raw": v}))
                .unwrap(),
        )
        .unwrap();
        fs::rename(dir.join("promote.tmp"), dir.join("promote.json")).unwrap();
    })
}

/// A5-fake, the kernel half under `init-fakeroot` (testing.md §3.9 A5; supervise.md §3.7, SV-3):
/// LKG = `init-fakebody crash_first:5`. Five crashes on LKG with the backoff doubling (50 → 100 →
/// 200 → 400 ms, capped), then the body runs in **body safe mode**; `body_safe{on:true}` reaches the
/// Root's lineage and `safe_entered` the alerts; `initctl promote` answers `E_SAFE_MODE`. The fake
/// Root then plays the Root's normal-mode restart (the real Root's safe auto-clear, D4): the kernel
/// boots the body in normal mode and reports `body_safe{on:false}`. No owner command anywhere.
/// The Root half (demotion, rollback to audited-good, system safe mode, auto-clear on the fake
/// clock) runs against the real `init-root` in WP21's `crash_loop_safe_mode_halt_retry_and_self_clear`.
#[test]
fn a5_fake_crash_loop_enters_safe_mode_and_leaves_it_without_the_owner() {
    let _wd = init_testkit::watchdog::arm("a5_fake_crash_loop_enters_safe_mode_and_leaves_it_without_the_owner", Duration::from_secs(510));
    let tm = templates();
    assert!(tm.fake.join("bin/init-body").is_file());
    let agent = a5_agent(format!("a5-fake-{}-", std::process::id()));
    run("a5-fake.json", |r, home| {
        assert_eq!(
            r.captures.get("BODIES_IN_SAFE"),
            Some(&json!(1)),
            "the safe-mode body runs"
        );
        // Restart timestamps show the backoff doubling: the gap from each crash (the fake Root's
        // `exited`) to the next body spawn is at least the kernel's backoff, 50·2^(n-1) ms capped
        // at 400, and grows.
        let log = util::read_jsonl(&home.parent().unwrap().join("r/logs/fakeroot.jsonl"));
        let mut gaps = Vec::new();
        let mut last_exit: Option<i64> = None;
        for e in &log {
            match (e["ev"].as_str(), e["kind"].as_str()) {
                (Some("exited"), _) if e["code"] != 0 => last_exit = e["t"].as_str().map(ts_ms),
                (Some("spawn"), Some("body")) => {
                    if let (Some(x), Some(t)) = (last_exit.take(), e["t"].as_str()) {
                        gaps.push(ts_ms(t) - x);
                    }
                }
                _ => {}
            }
        }
        assert!(gaps.len() >= 5, "five crash restarts: {gaps:?}");
        for (i, g) in gaps.iter().take(5).enumerate() {
            let want = (50i64 << i).min(400);
            assert!(
                *g >= want * 9 / 10,
                "restart {} after {g} ms, backoff {want} ms: {gaps:?}",
                i + 1
            );
        }
        // The gaps also carry the kernel's restart latency (about 250 ms here); their growth from
        // the 1st to the 4th restart tracks the backoff's (400 - 50 = 350 ms).
        assert!(gaps[3] - gaps[0] >= 200, "the backoff doubles: {gaps:?}");
        eprintln!("A5-fake restart gaps (ms): {gaps:?}");
    });
    let _ = agent.join();
}

// -------------------------------------------------------------------------------------------
// NO-OWNER-1 (testing.md §3.17)
// -------------------------------------------------------------------------------------------

/// Owner signals (D4) and removed commands: none may appear in any acceptance scenario.
const OWNER_WORDS: [&str; 6] = [
    "/accept",
    "/bless",
    "/reject",
    "/approve",
    "/deny",
    "/clear-safe",
];

/// NO-OWNER-1, static half: every scenario file parses, passes the runner's own validation (which
/// refuses approval-type commands), and no step sends an owner signal or a removed command, as an
/// owner line, a Root CLI word or an owner-socket request. The dynamic half is that each scenario
/// passes with no owner step at all (the runner has no way to wait on one).
#[test]
fn no_owner_1_no_scenario_needs_an_owner_step() {
    let _wd = init_testkit::watchdog::arm("no_owner_1_no_scenario_needs_an_owner_step", Duration::from_secs(30));
    let dir = repo().join("scenarios");
    let mut n = 0;
    for e in fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap();
        let sc: Scenario =
            serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
        validate(&sc).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
        for s in &sc.steps {
            let words: Vec<String> = match s {
                Step::SendUser { text } => text.split_whitespace().map(str::to_string).collect(),
                Step::RootCmd { argv, cmd, .. } => {
                    argv.iter().cloned().chain(cmd.clone()).collect()
                }
                Step::RootRequest { cmd, .. } => vec![cmd.clone()],
                _ => vec![],
            };
            for w in words {
                let w = w.to_ascii_lowercase();
                let bare = w.trim_start_matches('/');
                assert!(
                    !OWNER_WORDS
                        .iter()
                        .any(|o| *o == w || o.trim_start_matches('/') == bare),
                    "{}: step {s:?} sends the owner word {w:?}",
                    p.display()
                );
            }
        }
        n += 1;
    }
    assert!(n >= 1, "no scenario files in {}", dir.display());
}
