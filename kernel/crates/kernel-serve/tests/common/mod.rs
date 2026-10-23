//! Test harness for kernel-serve: an in-test fake [`Backend`] (WP4 is not done; the fake follows
//! the trait's contract), per-test homes under `kernel/target/test-homes/wp7/`, the real
//! `initctl` binary from WP1 (built once with the same cargo and target dir), and a raw socket
//! client for the frame-level cases.
//!
//! Safety: the only processes spawned are `cargo build -p initctl` (once) and `initctl` itself;
//! a hung `initctl` is killed by its own PID. Homes are deleted by exact path.

#![allow(dead_code)]

use init_proto::errors::{BudgetData, ErrorCode, GateFailedData, RootReason};
use init_proto::files::{SandboxStatus, SandboxZone};
use init_proto::initctl::*;
use init_proto::{ErrorBody, ErrorDetail};
use kernel_serve::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Proc, Stdio};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

pub const LKG: &str = "0d3e5a9b1c22f4e6a8b0c2d4e6f8a0b2c4d6e8f0";

/// The `initctl` binary (WP1), built once per test run into this lane's target dir.
pub fn initctl_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Ok(p) = std::env::var("INITCTL_BIN") {
            // Absolute: `initctl` runs with another working directory.
            return std::fs::canonicalize(&p).unwrap_or_else(|_| PathBuf::from(p));
        }
        let exe = std::env::current_exe().unwrap();
        // <target>/debug/deps/<test> → <target>
        let target = exe.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = init_testkit::watchdog::status(Proc::new(cargo)
            .args(["build", "--offline", "--quiet", "-p", "initctl", "--target-dir"])
            .arg(&target)
            .current_dir(env!("CARGO_MANIFEST_DIR")))
            .expect("cargo build -p initctl");
        assert!(st.success(), "building initctl failed");
        let bin = target.join("debug/initctl");
        assert!(bin.exists(), "{} missing", bin.display());
        bin
    })
    .clone()
}

/// A deterministic 40-hex SHA for a ref.
pub fn sha_of(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    let a = format!("{h:016x}");
    format!("{a}{a}{}", &a[..8])
}

/// What the fake has been asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ev {
    /// A backend method call.
    Call(String),
    /// A request the fake "sent" to the Root.
    Root(String),
    /// A stdin frame the fake "sent" to a body.
    Body(String),
}

pub struct FakeState {
    pub view: KernelView,
    pub ledger: Option<LedgerSnapshot>,
    pub refresh_ok: bool,
    pub root_status: Value,
    pub stage_ms: u64,
    pub gate_pass: bool,
    pub gog_pass: bool,
    pub current: String,
    pub no_body: bool,
    /// Hold promote jobs before the milestone until cleared.
    pub hold: bool,
    pub members: Vec<String>,
    pub qualified: Vec<String>,
    /// IC-22: the test client's socket, peeked when `pause` is sent.
    pub probe: Option<UnixStream>,
    pub pause_saw_final: Option<bool>,
    pub proposals: u32,
}

pub struct Fake {
    me: Weak<Fake>,
    pub st: Mutex<FakeState>,
    pub log: Mutex<Vec<Ev>>,
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

impl Fake {
    pub fn new() -> Arc<Fake> {
        Arc::new_cyclic(|me| Fake {
            me: me.clone(),
            st: Mutex::new(FakeState {
                view: KernelView::default(),
                ledger: Some(ledger_snapshot(0, 0)),
                refresh_ok: true,
                root_status: json!({"audited_good": "t3f09c1e5b7d2", "ceilings": {"day_musd": 5000000, "parallel_bodies": 1, "total_musd": 25000000}, "classes": ["models", "crates"], "level": 0, "max_level": 4, "mode": "normal", "next": {"level": 1}}),
                stage_ms: 20,
                gate_pass: true,
                gog_pass: true,
                current: LKG.to_string(),
                no_body: false,
                hold: false,
                members: Vec::new(),
                qualified: Vec::new(),
                probe: None,
                pause_saw_final: None,
                proposals: 0,
            }),
            log: Mutex::new(Vec::new()),
        })
    }

    pub fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.st.lock().unwrap()
    }

    pub fn ev(&self, e: Ev) {
        self.log.lock().unwrap().push(e);
    }

    pub fn events(&self) -> Vec<Ev> {
        self.log.lock().unwrap().clone()
    }

    pub fn root_requests(&self) -> Vec<String> {
        self.events().into_iter().filter_map(|e| if let Ev::Root(r) = e { Some(r) } else { None }).collect()
    }

    pub fn calls(&self) -> Vec<String> {
        self.events().into_iter().filter_map(|e| if let Ev::Call(r) = e { Some(r) } else { None }).collect()
    }

    pub fn clear(&self) {
        self.log.lock().unwrap().clear();
    }

    pub fn set_level(&self, l: u8) {
        let mut s = self.st();
        s.view.level = Some(l);
        if let Some(led) = &mut s.ledger {
            led.ledger["level"] = json!(l);
        }
    }

    fn stage_ms(&self) -> u64 {
        self.st().stage_ms
    }

    /// Run gate stages with progress; `Err` = the failing stage's error.
    fn run_stages(&self, job: &JobCtl, stages: &[Stage], pct0: u8, pct1: u8, gate_pass: bool) -> Result<(), ErrorBody> {
        let n = stages.len() as u8;
        for (i, s) in stages.iter().enumerate() {
            let pct = (u32::from(pct0) + u32::from(pct1 - pct0) * i as u32 / u32::from(n.max(1))) as u8;
            if !job.progress(JobState::Gating, Some(s.clone()), pct, Some(u64::from(n - i as u8)), &format!("{s} running")) {
                return Err(ErrorBody::new(ErrorCode::Cancelled, "job was cancelled"));
            }
            sleep_ms(self.stage_ms());
            let fail = !gate_pass && *s == Stage::G1;
            job.stage_done(StageResult {
                stage: s.clone(),
                pass: !fail,
                wall_ms: self.stage_ms(),
                code: fail.then(|| "gate.g1.self_json_drift".into()),
                reason: fail.then(|| "self.json drift: tools[pty].schema".into()),
            });
            if fail {
                return Err(ErrorBody::new(ErrorCode::parse("gate.g1.self_json_drift"), "self.json drift: tools[pty].schema").with_data(&GateFailedData {
                    stage: "G1".into(),
                    reason: "self.json drift: tools[pty].schema".into(),
                    report: Some(format!("state/kernel/gate/{}/report.json", job.id())),
                }));
            }
        }
        Ok(())
    }

    /// `pause` to the body; IC-22 peeks the client's socket first.
    fn pause(&self, job: &JobCtl) {
        let mut s = self.st();
        if let Some(p) = &s.probe {
            let mut buf = vec![0u8; 256 * 1024];
            // SAFETY: a non-blocking peek into a valid buffer.
            let n = unsafe { libc::recv(p.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), libc::MSG_PEEK | libc::MSG_DONTWAIT) };
            let seen = n > 0 && String::from_utf8_lossy(&buf[..n as usize]).contains("\"ok\":true");
            s.pause_saw_final = Some(seen);
        }
        drop(s);
        self.ev(Ev::Body(format!("pause {}", job.id())));
    }
}

pub fn ledger_snapshot(level: u8, age_s: u64) -> LedgerSnapshot {
    LedgerSnapshot {
        ledger: json!({"cap": 4, "entered_at": "2026-10-20T07:55:00.000Z", "entered_seq": 2, "incident": null, "level": level, "max_level": 4, "next": {"criteria": [{"id": "min_promotions", "met": false}], "level": level + 1}, "seq": 311, "supported_max_level": 4, "v": 1}),
        ceilings: json!({"max_instances": 12, "money": {"day_musd": 5000000, "total_musd": 25000000}, "parallel_bodies": 1}),
        age_s,
    }
}

impl Backend for Fake {
    fn view(&self, _lane: &LaneCtx) -> KernelView {
        self.st().view.clone()
    }

    fn status(&self, lane: &LaneCtx) -> Result<StatusResult, ErrorBody> {
        self.ev(Ev::Call("status".into()));
        let s = self.st();
        let mut soft = BTreeMap::new();
        soft.insert("day_musd".to_string(), 5_000_000u64);
        soft.insert("gate_musd".to_string(), 3_000_000u64);
        Ok(StatusResult {
            body: StatusBody { id: lane.id.clone(), lane: lane.lane },
            budget: StatusBudget { day_cap_musd: 5_000_000, day_spent_musd: 1_420_000, gate_cap_musd: None, gate_spent_musd: None, soft },
            cache_hit_permille: Some(930),
            constitution_sha256: "sha256:backend-value".into(),
            counters: Counters { crashes_total: 3, restarts_since_lkg: 1, rollbacks: 1, swaps_failed: 0, swaps_ok: 9 },
            current: StatusCurrent { depth: s.view.provisional_depth, provisional: s.view.primary_provisional, sha: s.current.clone(), since: "2026-10-19T12:36:01.004Z".into() },
            disk: StatusDisk { free_mib: 42188 },
            evolution: json!({}),
            fleet: None,
            handoff: None,
            jobs: vec![],
            kernel: StatusKernel { proto: 1, uptime_s: 5234, version: "0.1.0".into() },
            lane: Lane::Live,
            link: StatusLink { boot_reason: "cold".into(), ceilings_age_s: s.view.ledger_age_s, connected: s.view.link_up, outbox_unanswered: 0 },
            lkg: StatusLkg { seq: 7, sha: LKG.into(), since: "2026-10-19T09:12:44.000Z".into() },
            mode: if s.view.safe_mode { Mode::Safe } else if s.view.primary_provisional { Mode::Provisional } else { Mode::Normal },
            root: s.root_status.clone(),
            safe: None,
            sandbox: Some(SandboxStatus {
                hidden: vec!["cache/".into(), "probe/".into(), "vault/".into()],
                kind: "seatbelt".into(),
                landlock: None,
                landlock_abi: None,
                network: "kernel-sockets-only".into(),
                read_only: vec!["bin/".into(), "kernel/".into()],
                seccomp: false,
                sweep: Sweep::Full,
                write: vec!["body/".into(), "state/".into(), "work/".into()],
                zone: serde_json::from_value::<SandboxZone>(json!({"launcher": "root", "network": "root-egress-only"})).unwrap(),
            }),
            slot: Slot::Running,
            sweep: Sweep::Full,
        })
    }

    fn ledger(&self) -> Option<LedgerSnapshot> {
        self.st().ledger.clone()
    }

    fn refresh_ledger(&self) -> Result<LedgerSnapshot, ErrorBody> {
        self.ev(Ev::Root("ledger.get".into()));
        self.ev(Ev::Root("ceilings.get".into()));
        let mut s = self.st();
        if !s.refresh_ok {
            return Err(ErrorBody::root(RootReason::Refused, "no answer from the Root"));
        }
        let level = s.view.level.unwrap_or(0);
        let snap = ledger_snapshot(level, 0);
        s.ledger = Some(snap.clone());
        s.view.ledger_age_s = Some(0);
        Ok(snap)
    }

    fn reload(&self, _lane: &LaneCtx, _args: &ReloadArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call("reload".into()));
        if self.st().no_body {
            return Err(ErrorBody::new(ErrorCode::NoBody, "no live body to deliver to"));
        }
        self.ev(Ev::Body(format!("reload {} request", job.id())));
        job.set_awaiting_boundary().unwrap();
        let ms = self.stage_ms();
        std::thread::spawn(move || {
            sleep_ms(ms * 3);
            job.succeed(&ReloadOutcome { components: vec![], error: None, ok: true, prefix_changed: true });
        });
        Ok(Begun::started())
    }

    fn gate(&self, _lane: &LaneCtx, args: &GateArgs, repo: &Path, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call(format!("gate {} {}", repo.display(), args.rev)));
        let sha = if args.rev.len() == 40 { args.rev.clone() } else { sha_of(&args.rev) };
        let me = self.self_arc();
        let pass = self.st().gate_pass;
        let sha2 = sha.clone();
        std::thread::spawn(move || {
            job.progress(JobState::Running, Some(Stage::Fetch), 2, None, "fetching");
            let r = me.run_stages(&job, &[Stage::Build, Stage::G0, Stage::G1, Stage::G2, Stage::G3], 10, 95, pass);
            let rec = job.record().unwrap();
            job.succeed(&GateResult {
                job: job.id().into(),
                pass: r.is_ok(),
                report: format!("state/kernel/gate/{}/report.json", job.id()),
                sha: sha2,
                stages: rec.stages,
                warnings: vec![],
            });
        });
        Ok(Begun::started().with("sha", sha))
    }

    fn promote(&self, lane: &LaneCtx, args: &PromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call(format!("promote {}", args.git_ref.clone().or(args.sha.clone()).unwrap_or_default())));
        let sha = match (&args.git_ref, &args.sha) {
            (Some(r), _) if r == "evo/missing" => return Err(ErrorBody::new(ErrorCode::NotFound, "ref evo/missing not found in body/")),
            (Some(r), _) if r == "evo/no-trailers" => {
                return Err(ErrorBody::new(ErrorCode::parse("trailer.missing:Predicts"), "2 problems")
                    .with_data(&json!({"sha": sha_of(r)}))
                    .with_details(vec![
                        ErrorDetail { code: "trailer.missing:Predicts".into(), detail: "type feat requires Predicts".into() },
                        ErrorDetail { code: "trailer.noncanonical_key".into(), detail: "'prefix-change' → 'Prefix-change'".into() },
                    ]));
            }
            (Some(r), _) => sha_of(r),
            (None, Some(s)) => {
                if !self.st().qualified.contains(s) {
                    return Err(ErrorBody::new(ErrorCode::NotFound, "SHA is neither qualified nor archived"));
                }
                s.clone()
            }
            _ => unreachable!(),
        };
        let me = self.self_arc();
        let member = lane.lane == Lane::Member;
        let sha2 = sha.clone();
        std::thread::spawn(move || {
            let pass = me.st().gate_pass;
            let mut stages = vec![Stage::Build, Stage::G0, Stage::G1, Stage::G2, Stage::G3];
            if member {
                stages.push(Stage::G4);
            }
            if let Err(e) = me.run_stages(&job, &stages, 5, 80, pass) {
                if e.code != ErrorCode::Cancelled {
                    job.fail(e);
                }
                return;
            }
            while me.st().hold {
                sleep_ms(5);
            }
            if job.awaiting_boundary().is_err() {
                return;
            }
            me.pause(&job);
            job.progress(JobState::HandingOff, Some(Stage::Handoff), 90, None, "handing off");
            sleep_ms(me.stage_ms());
            if member {
                me.st().qualified.push(sha2.clone());
                me.ev(Ev::Root(format!("qualified {sha2}")));
            } else {
                me.st().current = sha2.clone();
            }
            job.succeed_value(json!({"sha": sha2, "provisional": !member, "qualified": member}));
        });
        Ok(Begun::started().with("sha", sha))
    }

    fn rollback(&self, _lane: &LaneCtx, _args: &RollbackArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call("rollback".into()));
        let from = self.st().current.clone();
        if from == LKG {
            return Err(ErrorBody::new(ErrorCode::AlreadyLkg, "already running the LKG"));
        }
        job.set_awaiting_boundary().unwrap();
        let me = self.self_arc();
        std::thread::spawn(move || {
            job.wait_flushed();
            me.pause(&job);
            me.st().current = LKG.into();
            job.succeed_value(json!({"to": LKG}));
        });
        Ok(Begun::started().with("from", from).with("to", LKG))
    }

    fn vendor(&self, _lane: &LaneCtx, args: &VendorArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call(format!("vendor {}@{}", args.krate, args.version)));
        let me = self.self_arc();
        let a = args.clone();
        std::thread::spawn(move || {
            job.progress(JobState::Running, Some(Stage::Registry), 10, None, "filling the local registry");
            me.ev(Ev::Root(format!("egress crates GET index.crates.io/{}", a.krate)));
            sleep_ms(me.stage_ms());
            job.progress(JobState::Running, Some(Stage::Vendor), 60, None, "cargo vendor --offline");
            me.ev(Ev::Root(format!("egress crates GET static.crates.io/{}-{}.crate", a.krate, a.version)));
            sleep_ms(me.stage_ms());
            let id = job.id().to_string();
            job.succeed(&VendorResult {
                added: vec![VendoredCrate { checksum: "8a1b".into(), name: a.krate.clone(), version: a.version.clone() }],
                krate: a.krate,
                job: id.clone(),
                lock: format!("state/kernel/vendor/{id}/Cargo.lock"),
                note: "copy vendor/* and Cargo.lock into your worktree; the next candidate runs full G4".into(),
                vendor_dir: format!("state/kernel/vendor/{id}/vendor"),
                version: a.version,
            });
        });
        Ok(Begun::started())
    }

    fn task_submit(&self, _lane: &LaneCtx, _args: &TaskSubmitArgs, dir: &Path, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call(format!("task_submit {}", dir.display())));
        let fam: Value = std::fs::read(dir.join("family.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, "family.json missing"))?;
        let me = self.self_arc();
        let family = dir.file_name().unwrap().to_string_lossy().into_owned();
        std::thread::spawn(move || {
            job.progress(JobState::Running, Some(Stage::Admission), 20, None, "admission");
            sleep_ms(me.stage_ms());
            let ids: Vec<String> = fam["instances"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
            let det = fam["deterministic"].as_bool().unwrap_or(true);
            let mut checks = vec![TaskCheck { detail: format!("{} instances", ids.len()), ok: ids.len() >= 2, rule: "family_size".into() }];
            checks.push(TaskCheck { detail: "ran twice".into(), ok: det, rule: "determinism".into() });
            let admitted = checks.iter().all(|c| c.ok);
            let (seq, sealed, visible) = if admitted {
                job.progress(JobState::Running, Some(Stage::Register), 80, None, "registering");
                me.ev(Ev::Root("lineage.append task_admit".into()));
                let half = ids.len() / 2;
                (Some(1000), half as u32, ids[half..].to_vec())
            } else {
                (None, 0, vec![])
            };
            job.succeed(&TaskSubmitResult { admitted, checks, family, job: job.id().into(), seq, sealed, visible });
        });
        Ok(Begun::started())
    }

    fn fleet_spawn(&self, _lane: &LaneCtx, _args: &FleetSpawnArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call("fleet_spawn".into()));
        self.ev(Ev::Root("spawn body".into()));
        let mut s = self.st();
        let bid = format!("b{}", s.members.len() + 1);
        s.members.push(bid.clone());
        if let Some(b) = &mut s.view.bodies {
            b.used += 1;
        }
        drop(s);
        let b2 = bid.clone();
        std::thread::spawn(move || {
            job.progress(JobState::Running, Some(Stage::Member), 50, None, "spawning");
            job.succeed_value(json!({"bid": b2}));
        });
        Ok(Begun::started().with("bid", bid))
    }

    fn kernel_promote(&self, _lane: &LaneCtx, args: &KernelPromoteArgs, job: JobCtl) -> Result<Begun, ErrorBody> {
        self.ev(Ev::Call(format!("kernel_promote {}", args.git_ref)));
        let sha = sha_of(&args.git_ref);
        let me = self.self_arc();
        std::thread::spawn(move || {
            if me.run_stages(&job, &[Stage::K0, Stage::K1, Stage::K2, Stage::K3], 5, 60, true).is_err() {
                return;
            }
            job.progress(JobState::Running, Some(Stage::Store), 65, None, "store.put");
            for k in ["init-kernel", "initctl", "kernel-src"] {
                me.ev(Ev::Root(format!("store.put {k}")));
            }
            me.ev(Ev::Root("system.swap".into()));
            if job.awaiting_root(Some("s-000001")).is_err() {
                return;
            }
            job.progress(JobState::AwaitingRoot, Some(Stage::GateOfGates), 70, None, "gate-of-gates");
            sleep_ms(me.stage_ms() * 2);
            if me.st().gog_pass {
                if job.awaiting_swap().is_err() {
                    return;
                }
                job.progress(JobState::AwaitingSwap, Some(Stage::Prepare), 85, None, "prepare");
                while me.st().hold {
                    sleep_ms(5);
                }
                sleep_ms(me.stage_ms());
                job.succeed_value(json!({"swapped": true}));
            } else {
                job.fail(ErrorBody::new(ErrorCode::GateFailed, "the gate-of-gates failed").with_data(&GateFailedData {
                    stage: "gate_of_gates".into(),
                    reason: "mutation suite: 2 mutants survived".into(),
                    report: None,
                }));
            }
        });
        Ok(Begun::started().with("sha", sha))
    }

    fn cancel(&self, _lane: &LaneCtx, job: &str) {
        self.ev(Ev::Root(format!("kill instances of {job}")));
    }

    fn task_template(&self, _lane: &LaneCtx) -> Result<Value, ErrorBody> {
        Ok(json!({"family": "<name>", "instances": [{"v": 1}, {"v": 1}]}))
    }

    fn task_list(&self, _lane: &LaneCtx) -> Result<TaskListResult, ErrorBody> {
        Ok(TaskListResult(vec![]))
    }

    fn task_show(&self, _lane: &LaneCtx, args: &TaskShowArgs) -> Result<Value, ErrorBody> {
        Ok(json!({"family": args.family, "instances": []}))
    }

    fn budget_set(&self, _lane: &LaneCtx, args: &BudgetSetArgs) -> Result<BudgetSetResult, ErrorBody> {
        self.ev(Ev::Call("budget_set".into()));
        let sum: i64 = args.set.iter().filter(|(k, _)| k.ends_with("_musd") && *k != "day_musd").map(|(_, v)| if let BudgetValue::Int(n) = v { *n } else { 0 }).sum();
        if sum > 5_000_000 {
            return Err(ErrorBody::new(ErrorCode::Budget, "the shares exceed the soft day budget").with_data(&BudgetData { key: "day_musd".into(), limit: 5_000_000 }));
        }
        Ok(BudgetSetResult { budgets: serde_json::to_value(&args.set).unwrap() })
    }

    fn model_set(&self, _lane: &LaneCtx, args: &ModelSetArgs) -> Result<ModelSetResult, ErrorBody> {
        self.ev(Ev::Call("model_set".into()));
        if !["deepseek/deepseek-flash", "deepseek/deepseek-v4-pro"].contains(&args.model.as_str()) {
            return Err(ErrorBody::new(ErrorCode::Args, format!("{} is not allowed by the charter", args.model)));
        }
        Ok(ModelSetResult { model: args.model.clone(), thinning_job: None })
    }

    fn fleet_list(&self, _lane: &LaneCtx) -> Result<FleetListResult, ErrorBody> {
        Ok(FleetListResult { candidates: vec![], members: vec![], suggested_parent: Some("main".into()) })
    }

    fn fleet_send(&self, _lane: &LaneCtx, args: &FleetSendArgs) -> Result<FleetSendResult, ErrorBody> {
        if !self.st().members.contains(&args.bid) {
            return Err(ErrorBody::new(ErrorCode::NotFound, format!("no member {}", args.bid)));
        }
        Ok(FleetSendResult { queued: true })
    }

    fn fleet_stop(&self, _lane: &LaneCtx, args: &FleetStopArgs) -> Result<FleetStopResult, ErrorBody> {
        self.ev(Ev::Call("fleet_stop".into()));
        let s = self.st();
        let stopping = if args.bid == "all" { s.members.clone() } else { vec![args.bid.clone()] };
        Ok(FleetStopResult { stopping })
    }

    fn kernel_status(&self, _lane: &LaneCtx) -> Result<KernelStatusResult, ErrorBody> {
        Ok(KernelStatusResult {
            candidate: None,
            last_audit: None,
            running: KernelRunning { built_from: LKG.into(), source: "maintainer".into(), version: "0.1.0".into() },
        })
    }

    fn file_proposal(&self, _lane: &LaneCtx, title: &str, text: &str) -> Result<String, ErrorBody> {
        self.ev(Ev::Root(format!("proposal.file {title} ({} bytes)", text.len())));
        let mut s = self.st();
        s.proposals += 1;
        Ok(format!("rp-{:04}", s.proposals))
    }
}

impl Fake {
    fn self_arc(&self) -> Arc<Fake> {
        self.me.upgrade().expect("the fake is alive")
    }
}

/// A per-test home, fake backend and running server with the live lane bound.
pub struct Fx {
    pub home: PathBuf,
    pub fake: Arc<Fake>,
    pub srv: ServerHandle,
}

pub fn test_home(tag: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp7");
    std::fs::create_dir_all(&root).unwrap();
    let dir = root.canonicalize().unwrap().join(format!("{tag}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap(); // exact path: our own leftover
    }
    for d in ["body", "work", "kernel", "state/kernel", "run"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    dir
}

impl Fx {
    pub fn new(tag: &str) -> Fx {
        Fx::with(tag, |_| {}, Arc::new(SameUidPolicy))
    }

    pub fn with(tag: &str, tweak: impl FnOnce(&mut ServeConfig), policy: Arc<dyn PeerPolicy>) -> Fx {
        let home = test_home(tag);
        std::fs::write(home.join("kernel/CONSTITUTION.md"), "# CONSTITUTION v0.3 (test copy)\n").unwrap();
        Fx::start_in(home, Fake::new(), tweak, policy)
    }

    pub fn start_in(home: PathBuf, fake: Arc<Fake>, tweak: impl FnOnce(&mut ServeConfig), policy: Arc<dyn PeerPolicy>) -> Fx {
        let mut cfg = ServeConfig::new(&home, "0.1.0");
        cfg.jobs.progress_interval = Duration::from_millis(200);
        tweak(&mut cfg);
        let srv = kernel_serve::start(cfg, fake.clone(), policy).unwrap();
        srv.add_lane(LaneSpec::live(&home)).unwrap();
        Fx { home, fake, srv }
    }

    pub fn sock(&self) -> PathBuf {
        self.home.join("run/kernel.sock")
    }

    /// Run `initctl` against the live lane.
    pub fn ctl(&self, args: &[&str]) -> Out {
        run_initctl(&self.home, &self.sock(), args)
    }

    /// Run `initctl` against another socket.
    pub fn ctl_on(&self, sock: &Path, args: &[&str]) -> Out {
        run_initctl(&self.home, sock, args)
    }

    pub fn raw(&self) -> Raw {
        Raw::connect(&self.sock())
    }

    /// Wait until job `id` satisfies `f` (polls the store).
    pub fn until(&self, id: &str, what: &str, f: impl Fn(&JobRecord) -> bool) -> JobRecord {
        let t = Instant::now();
        loop {
            if let Some(r) = self.srv.jobs().record(id)
                && f(&r)
            {
                return r;
            }
            assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {id}: {what}");
            sleep_ms(5);
        }
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.srv.shutdown();
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.home); // exact path
        }
    }
}

/// An `initctl` run.
#[derive(Debug)]
pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {self:?}"))
    }

    pub fn err_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error in {self:?}")).to_string()
    }
}

pub fn spawn_initctl(home: &Path, sock: &Path, args: &[&str]) -> Child {
    init_testkit::watchdog::spawn(Proc::new(initctl_bin())
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("TMPDIR", home.join("work"))
        .env("INIT_KERNEL_SOCK", sock)
        .current_dir(home.join("work"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()))
        .unwrap()
}

/// Wait for a spawned `initctl` (killed by its PID after 30 s).
pub fn finish(mut child: Child) -> Out {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut out = String::new();
    let mut err = String::new();
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = so.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = se.read_to_string(&mut s);
        s
    });
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if Instant::now() > deadline {
            let _ = child.kill(); // our own child, by PID
            break child.wait().unwrap();
        }
        sleep_ms(2);
    };
    out.push_str(&t_out.join().unwrap());
    err.push_str(&t_err.join().unwrap());
    Out { code: status.code().unwrap_or(-1), stdout: out, stderr: err }
}

pub fn run_initctl(home: &Path, sock: &Path, args: &[&str]) -> Out {
    finish(spawn_initctl(home, sock, args))
}

/// A raw frame-level client.
pub struct Raw {
    pub s: UnixStream,
    r: BufReader<UnixStream>,
}

impl Raw {
    pub fn connect(sock: &Path) -> Raw {
        let s = UnixStream::connect(sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let r = BufReader::new(s.try_clone().unwrap());
        Raw { s, r }
    }

    pub fn send_line(&mut self, line: &[u8]) {
        let mut b = line.to_vec();
        b.push(b'\n');
        self.s.write_all(&b).unwrap();
    }

    /// Like `send_line`, ignoring a write error (the server may close mid-line).
    pub fn send_line_lossy(&mut self, line: &[u8]) {
        let mut b = line.to_vec();
        b.push(b'\n');
        let _ = self.s.write_all(&b);
    }

    /// The next frame, or None at EOF.
    pub fn frame(&mut self) -> Option<Value> {
        let mut l = String::new();
        match self.r.read_line(&mut l) {
            Ok(0) => None,
            Ok(_) => Some(serde_json::from_str(&l).unwrap_or_else(|e| panic!("bad frame {l:?}: {e}"))),
            Err(e) => panic!("read: {e}"),
        }
    }

    /// Send a request and return (progress frames, final frame).
    pub fn call(&mut self, id: &str, cmd: &str, args: Value) -> (Vec<Value>, Value) {
        let line = serde_json::to_vec(&json!({"v": 1, "id": id, "cmd": cmd, "args": args})).unwrap();
        self.send_line(&line);
        let mut progress = Vec::new();
        loop {
            let f = self.frame().expect("connection closed before the final response");
            if f.get("ok").is_some() {
                return (progress, f);
            }
            progress.push(f);
        }
    }

    /// True when the server closed the connection.
    pub fn closed(&mut self) -> bool {
        self.frame().is_none()
    }
}
