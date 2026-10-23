//! One G4 run in `judge` mode (gate.md §3.7.3 on a tree, §4.1a): the body runs as a direct child of
//! the judge, inside whatever isolation the judge itself runs in (the Root's sealed instance; Seatbelt
//! profiles do not nest), with its workspace at `<home>/probe/<id>/work/task` and the prompt as one
//! `input` frame. Its model traffic takes one of two [`Egress`] paths:
//!
//! - **relay**: a per-run in-memory [`Relay`] to the judge's upstream; runs go in parallel and each
//!   run's usage is its relay's own;
//! - **direct**: the body talks to the upstream itself (the Root's sealed profile denies `network-bind`,
//!   so no relay can listen there). Runs are then serialized and each run's usage is the difference of
//!   the upstream's `GET /_init/stats` before and after it.
//!
//! The run ends at the first of: the body exits; the task's `wall_s` (capped by `gate.g4.task_wall_s`);
//! the last finish is `stop` with nothing in flight for `quiet_ms`; or a cost cap. The check then runs
//! on the workspace (§3.7.1 protocol and environment) and the probe root is deleted by its exact path.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::relay::{self, Relay, upstream_stats};
use super::tree::{kill_own_group, run_timeout};
use crate::g4::{RunExecutor, RunRecord, TaskDef, check_env, score};
use crate::stats::Arm;

/// `sun_path` on macOS is 104 bytes including the terminating NUL.
pub const SUN_PATH_MAX: usize = 103;

/// One arm: the tree and its digest.
#[derive(Debug, Clone)]
pub struct ArmTree {
    pub dir: PathBuf,
    pub sha: String,
}

/// How a run's model traffic reaches the upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Relay,
    Direct,
}

impl Egress {
    pub fn as_str(self) -> &'static str {
        match self {
            Egress::Relay => "relay",
            Egress::Direct => "direct",
        }
    }
}

/// [`RunExecutor`] over tree arms.
pub struct TreeRuns {
    pub home: PathBuf,
    /// The judge's cwd (canonical), for relative socket paths.
    pub cwd: PathBuf,
    pub upstream: Option<PathBuf>,
    pub egress: Egress,
    pub c: ArmTree,
    pub p: ArmTree,
    pub gate_id: String,
    pub level: String,
    pub task_wall_s: u64,
    pub quiet_ms: u64,
    /// Held for the whole of a run in [`Egress::Direct`] (usage by difference needs one run at a time).
    pub serial: Mutex<()>,
}

/// What the end rule and the record need from either egress.
#[derive(Debug, Clone, Default)]
struct Seen {
    requests: u64,
    inflight: u64,
    tokens_in: u64,
    tokens_out: u64,
    cache_read: u64,
    cost_ref_musd: u64,
    last_finish: Option<String>,
    idle: Duration,
    capped: bool,
}

fn u(v: &Value, k: &str) -> u64 {
    v[k].as_u64()
        .or_else(|| v[k].as_bool().map(u64::from))
        .unwrap_or(0)
}

fn finish_of(v: &Value) -> Option<String> {
    v["last_finish"]
        .as_str()
        .or_else(|| v["last_finish"]["reason"].as_str())
        .map(str::to_string)
}

/// The difference of two upstream stats, priced at the reference rates.
fn delta(base: &Value, now: &Value) -> Seen {
    let d = |k: &str| u(now, k).saturating_sub(u(base, k));
    let (tin, tout) = (d("input_tokens"), d("output_tokens"));
    Seen {
        requests: d("requests"),
        inflight: u(now, "inflight"),
        tokens_in: tin,
        tokens_out: tout,
        cache_read: 0,
        cost_ref_musd: (tin * relay::PRICE_INPUT_PUSD + tout * relay::PRICE_OUTPUT_PUSD)
            / 1_000_000,
        last_finish: finish_of(now),
        idle: Duration::from_millis(u(now, "idle_ms")),
        capped: d("capped") > 0,
    }
}

fn from_obs(o: &relay::Obs) -> Seen {
    Seen {
        requests: o.requests,
        inflight: o.inflight,
        tokens_in: o.tokens_in,
        tokens_out: o.tokens_out,
        cache_read: o.cache_read,
        cost_ref_musd: o.cost_ref_musd(),
        last_finish: o.last_finish.clone(),
        idle: o.last_end.map(|e| e.elapsed()).unwrap_or_default(),
        capped: o.capped,
    }
}

/// `to` relative to `from` (both absolute and canonical): `..` up to the common prefix, then down.
pub fn relative(from: &Path, to: &Path) -> PathBuf {
    let f: Vec<Component> = from.components().collect();
    let t: Vec<Component> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut p = PathBuf::new();
    for _ in common..f.len() {
        p.push("..");
    }
    for c in &t[common..] {
        p.push(c.as_os_str());
    }
    p
}

/// The shorter of `abs` and its path relative to `cwd`, if it fits `sun_path`.
pub fn sock_path(cwd: &Path, abs: &Path) -> Option<PathBuf> {
    let rel = relative(cwd, abs);
    let best = if rel.as_os_str().len() < abs.as_os_str().len() {
        rel
    } else {
        abs.to_path_buf()
    };
    (best.as_os_str().len() <= SUN_PATH_MAX).then_some(best)
}

fn arm_str(a: Arm) -> &'static str {
    match a {
        Arm::C => "c",
        Arm::P => "p",
    }
}

fn untar(tar: &Path, dest: &Path) -> Result<(), String> {
    let st = Command::new("/usr/bin/tar")
        .arg("-xf")
        .arg(tar)
        .arg("-C")
        .arg(dest)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("tar exited {st}"))
    }
}

fn copy_dir(src: &Path, dst: &Path) -> bool {
    Command::new("/bin/cp")
        .arg("-R")
        .arg(src)
        .arg(dst)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

impl TreeRuns {
    fn base(&self, arm: Arm, t: &TaskDef, r: u32, run_id: &str) -> RunRecord {
        let sha = if arm == Arm::C {
            &self.c.sha
        } else {
            &self.p.sha
        };
        RunRecord {
            v: 2,
            gate_id: self.gate_id.clone(),
            run_id: run_id.into(),
            arm: arm_str(arm).into(),
            sha: sha.clone(),
            level: self.level.clone(),
            task: t.id.clone(),
            set: serde_json::to_value(t.set)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            source: serde_json::to_value(t.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            labels: t.labels.clone(),
            repeat: r,
            ..Default::default()
        }
    }

    /// The probe root of one run: `<home>/probe/<12 hex of sha256(gate_id, run_id)>`. Every root has the
    /// same length, so the workspace path in the prompt costs both arms the same tokens, and the relay
    /// socket in it stays short.
    pub fn root_of(&self, run_id: &str) -> PathBuf {
        let h = Sha256::digest(format!("{}\n{run_id}", self.gate_id).as_bytes());
        let name: String = h.iter().take(6).map(|b| format!("{b:02x}")).collect();
        self.home.join("probe").join(name)
    }

    /// The relay socket of a run root, as bound (`None` when no form of it fits `sun_path`).
    pub fn sock_of(&self, root: &Path) -> Option<PathBuf> {
        sock_path(&self.cwd, &root.join("p.sock"))
    }

    fn check(&self, t: &TaskDef, ws: &Path, out: &Path) -> (u32, u64) {
        let t0 = Instant::now();
        for d in ["home", "tmp"] {
            let _ = std::fs::create_dir_all(out.join(d));
        }
        if !copy_dir(&t.dir.join("check"), &out.join("check")) {
            return (0, 0);
        }
        let mut c = Command::new(out.join("check/run"));
        c.args(&t.task.check.argv[1..])
            .env_clear()
            .envs(check_env(out, Path::new("/usr/bin"), &t.id))
            .current_dir(ws);
        let s = match run_timeout(c, Duration::from_secs(t.task.check.timeout_s)) {
            Ok(r) => score(r.code, r.timed_out, &r.stdout),
            Err(_) => 0,
        };
        (s, t0.elapsed().as_millis() as u64)
    }
}

enum Lane {
    Relay(Relay),
    Direct { sock: Option<PathBuf>, base: Value },
}

impl Lane {
    fn seen(&self) -> Seen {
        match self {
            Lane::Relay(r) => from_obs(&r.snapshot()),
            Lane::Direct { sock, base } => sock
                .as_deref()
                .and_then(upstream_stats)
                .map(|now| delta(base, &now))
                .unwrap_or_default(),
        }
    }
}

impl RunExecutor for TreeRuns {
    fn run(&self, arm: Arm, t: &TaskDef, r: u32, run_id: &str) -> RunRecord {
        let _one_at_a_time = (self.egress == Egress::Direct).then(|| {
            self.serial
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let mut rec = self.base(arm, t, r, run_id);
        let tree = if arm == Arm::C {
            &self.c.dir
        } else {
            &self.p.dir
        };
        let root = self.root_of(run_id);
        let _ = std::fs::remove_dir_all(&root); // exact path: an infra re-run of the same id
        let ws = root.join("work/task");
        let mk = ["work/task", "home", "tmp", "out"]
            .iter()
            .try_for_each(|d| std::fs::create_dir_all(root.join(d)));
        if mk.is_err() || untar(&t.dir.join("workspace.tar"), &ws).is_err() {
            let _ = std::fs::remove_dir_all(&root);
            rec.outcome = "infra".into();
            return rec;
        }
        let sock_abs = root.join("p.sock");
        let (egress, body_sock) = match self.egress {
            Egress::Relay => {
                let Some(relay) = self.sock_of(&root).and_then(|b| {
                    Relay::start(&b, self.upstream.clone(), t.task.limits.max_cost_musd).ok()
                }) else {
                    let _ = std::fs::remove_dir_all(&root);
                    rec.outcome = "infra".into();
                    return rec;
                };
                // the body reaches the relay by a path relative to its cwd (short enough for sun_path)
                (Lane::Relay(relay), relative(&ws, &sock_abs))
            }
            Egress::Direct => {
                let base = self
                    .upstream
                    .as_deref()
                    .and_then(upstream_stats)
                    .unwrap_or(Value::Null);
                (
                    Lane::Direct {
                        sock: self.upstream.clone(),
                        base,
                    },
                    self.upstream.clone().unwrap_or_default(),
                )
            }
        };
        let poll = Duration::from_millis(if self.egress == Egress::Direct {
            100
        } else {
            25
        });
        let mut cmd = Command::new(tree.join("init-body"));
        cmd.current_dir(&ws)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .env("HOME", root.join("home"))
            .env("TMPDIR", format!("{}/", root.join("tmp").display()))
            .env("INIT_MODE", "normal")
            .env("INIT_PROXY_SOCK", &body_sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let t0 = Instant::now();
        let mut outcome = "done";
        match cmd.spawn() {
            Err(_) => outcome = "crash",
            Ok(mut child) => {
                let mut stdin = child.stdin.take();
                let text = format!(
                    "[task {}] Workspace: {}\n\n{}",
                    t.id,
                    ws.display(),
                    t.task.prompt
                );
                if let Some(s) = stdin.as_mut() {
                    let _ = writeln!(s, "{}", json!({"t": "input", "text": text}));
                    let _ = s.flush();
                }
                let wall = Duration::from_secs(t.task.limits.wall_s.min(self.task_wall_s.max(1)));
                let quiet = Duration::from_millis(self.quiet_ms);
                let mut last_poll: Option<Instant> = None;
                loop {
                    if let Ok(Some(st)) = child.try_wait() {
                        if !matches!(st.code(), Some(0) | Some(75)) {
                            outcome = "crash";
                        }
                        break;
                    }
                    if t0.elapsed() >= wall {
                        outcome = "timeout";
                        kill_own_group(&mut child);
                        break;
                    }
                    if last_poll.is_none_or(|p| p.elapsed() >= poll) {
                        last_poll = Some(Instant::now());
                        let o = egress.seen();
                        if o.inflight == 0
                            && o.requests > 0
                            && o.last_finish.as_deref() == Some("stop")
                            && o.idle >= quiet
                        {
                            kill_own_group(&mut child);
                            break;
                        }
                        if o.capped && o.inflight == 0 && o.idle >= Duration::from_secs(1) {
                            outcome = "cost_capped";
                            kill_own_group(&mut child);
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                drop(stdin);
                rec.exit_code = child.try_wait().ok().flatten().and_then(|s| s.code());
            }
        }
        rec.wall_ms = t0.elapsed().as_millis() as u64;
        let o = match egress {
            Lane::Relay(r) => from_obs(&r.stop()),
            d @ Lane::Direct { .. } => d.seen(),
        };
        rec.requests = o.requests;
        rec.turns = o.requests;
        rec.tokens_in = o.tokens_in;
        rec.tokens_out = o.tokens_out;
        rec.cache_read = o.cache_read;
        rec.input_total = o.tokens_in;
        rec.cost_ref_musd = o.cost_ref_musd;
        rec.spent_musd = o.cost_ref_musd;
        rec.outcome = outcome.into();
        let (s, ms) = self.check(t, &ws, &root.join("out"));
        rec.score_permille = s;
        rec.check_ms = ms;
        let _ = std::fs::remove_dir_all(&root);
        rec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        let r = relative(Path::new("/a/b/c"), Path::new("/a/x/y.sock"));
        assert_eq!(r, PathBuf::from("../../x/y.sock"));
        assert_eq!(
            relative(Path::new("/a/b"), Path::new("/a/b/p/q")),
            PathBuf::from("p/q")
        );
        let long = PathBuf::from(format!("/{}/p.sock", "d".repeat(120)));
        assert_eq!(sock_path(Path::new("/elsewhere"), &long), None);
        assert_eq!(
            sock_path(&long.parent().unwrap().join("w"), &long),
            Some(PathBuf::from("../p.sock"))
        );
    }
}
