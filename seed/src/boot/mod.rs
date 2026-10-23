//! Body boot (docs/body/boot.md v0.3): from kernel facts (env, `state/kernel/boot.json`) and the
//! durable log to a serving body that continues the same conversation.
//!
//! Execution order (§3 table; each step's failure code in brackets):
//! 1. argv dispatch, panic hook (`main.rs`) [64]
//! 2. env and process facts (`env::validate`, `env::process_facts`) [78]
//! 3. `boot.json` (`env::read_boot_json`) [78]
//! 4. model check (`env::check_model`) [78]
//! 5. `body/` git sync, then housekeeping (`gitsync`, `housekeep`) [warnings only]
//! 6. extension host (`ext::start`; queues the Boot load, not awaited) [warning]
//! 7. rehydration: lock, scan, repair, head, `boot` event (`rehydrate`) [1 on lock or log I/O]
//! 8. probe, probe mode only (`probe`) [70]
//! 9. `ready` (`ready`) [70]; mode `check` (ADR-011 A4) exits 0 here: no heartbeat, no serving, no model call
//! 10. heartbeat thread (`loop::status::spawn_heartbeat`)
//! 11. serve (`serve`): the stdio-jsonl/1 threads and `loop::Session::serve`

pub mod awareness;
pub mod env;
pub mod ext;
pub mod gitsync;
pub mod housekeep;
pub mod manifest;
pub mod probe;
pub mod ready;
pub mod rehydrate;
pub mod rootview;
pub mod serve;


use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::model::{Abort, ModelClient};
use crate::provider::client::Client;
use crate::provider::summarizer::{ProxySummarizer, SummaryCx};
use crate::provider::transport::Transport;
use crate::r#loop::status::{HeartbeatInfo, spawn_heartbeat};
use crate::r#loop::{FaultHook, LogContext, Phase, Status};
use crate::tools::{Mode, Signal, Tools, ToolsConfig, kill_group};

use env::{Vars, mode_str};

/// A boot failure: exit code, step, message (§4.4, §4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootError {
    pub code: u8,
    pub step: &'static str,
    pub message: String,
}

impl BootError {
    /// 78: config error (env, `boot.json`, unknown model, bad effort).
    pub fn config(step: &'static str, message: impl Into<String>) -> Self {
        BootError {
            code: 78,
            step,
            message: message.into(),
        }
    }
    /// 70: internal boot error (probe failed, `ready` write failed, head unreadable).
    pub fn internal(step: &'static str, message: impl Into<String>) -> Self {
        BootError {
            code: 70,
            step,
            message: message.into(),
        }
    }
    /// 1: the session log is locked by another writer, or a log write/fsync failed.
    pub fn log(step: &'static str, message: impl Into<String>) -> Self {
        BootError {
            code: 1,
            step,
            message: message.into(),
        }
    }
    pub fn exit_code(&self) -> u8 {
        self.code
    }
    /// `{"type":"boot_error","code":…,"step":…,"message":…}` (sorted keys).
    pub fn to_json_line(&self) -> String {
        json!({"code": self.code, "message": self.message, "step": self.step, "type": "boot_error"})
            .to_string()
    }
}

/// Heartbeat period (§3.12).
pub const HEARTBEAT: Duration = Duration::from_secs(5);

/// `INIT_FAULT` (testing.md §3.8.1 grammar), honoured only under `INIT_TEST=1`: `point[@n]:action`
/// with `action` = `abort` | `exit<code>` | `panic` (body-side addition: a main-thread panic, exit
/// 101) | `sleep<ms>`. Boot points: `boot.after_env`, `boot.after_boot_event`, `boot.before_ready`;
/// loop points: `loop.after_on_response`, `loop.batch.after_first_result`.
#[derive(Clone, Debug, Default)]
pub struct Faults {
    rules: Vec<(String, u64, String)>,
    hits: Arc<Mutex<Vec<u64>>>,
}

impl Faults {
    pub fn parse(spec: &str) -> Faults {
        let mut rules = Vec::new();
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let Some((pt, action)) = item.rsplit_once(':') else { continue };
            let (point, n) = match pt.split_once('@') {
                Some((p, n)) => (p, n.parse().unwrap_or(1)),
                None => (pt, 1),
            };
            rules.push((point.to_string(), n, action.to_string()));
        }
        let hits = Arc::new(Mutex::new(vec![0; rules.len()]));
        Faults { rules, hits }
    }

    pub fn from_vars(v: &Vars) -> Faults {
        match (v.test_mode(), v.get("INIT_FAULT")) {
            (true, Some(s)) => Faults::parse(s),
            _ => Faults::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Fires the point's action when its count is reached. Returns false (never returns for
    /// `abort`, `exit…` and `panic`).
    pub fn hit(&self, point: &str) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|p| p.into_inner());
        for (i, (p, n, action)) in self.rules.iter().enumerate() {
            if p != point {
                continue;
            }
            hits[i] += 1;
            if hits[i] != *n {
                continue;
            }
            match action.as_str() {
                "abort" => std::process::abort(),
                "panic" => panic!("INIT_FAULT {point}: panic"),
                a if a.starts_with("exit") => {
                    std::process::exit(a[4..].parse().unwrap_or(1));
                }
                a if a.starts_with("sleep") => {
                    std::thread::sleep(Duration::from_millis(a[5..].parse().unwrap_or(0)));
                }
                _ => {}
            }
        }
        false
    }

    /// The loop's hook (`loop::FaultHook`).
    pub fn hook(&self) -> Option<FaultHook> {
        if self.is_empty() {
            return None;
        }
        let f = self.clone();
        Some(Box::new(move |p: &str| f.hit(p)))
    }
}

/// Mode `check` makes no model call: a compaction it would need fails instead of reaching the proxy.
struct NoModel;

impl crate::context::compact::Summarizer for NoModel {
    fn summarize(
        &mut self,
        _: &crate::context::compact::ForkRequest,
    ) -> Result<crate::context::compact::SummaryReply, String> {
        Err("mode check makes no model call".into())
    }
}

fn kill_hook(pg: i32) {
    kill_group(pg, Signal::Kill);
}

/// Steps 2–11. Returns the process exit code from serving (0 or 75; 1 on a fatal log error).
pub fn run() -> Result<u8, BootError> {
    let vars = Vars::from_process();
    let faults = Faults::from_vars(&vars);
    // 2–4: env, process facts, boot.json, model check.
    let env = env::validate(&vars)?;
    let proc_facts = env::process_facts(&env)?;
    let boot = env::read_boot_json(&env)?;
    let info = env::check_model(&boot)?;
    let effort = boot.effort.unwrap_or(info.default_effort);
    faults.hit("boot.after_env");

    // 5: `body/` git sync, then housekeeping (non-fatal).
    let git_mode = match env.mode {
        Mode::Normal => gitsync::SyncMode::Full,
        Mode::Safe => gitsync::SyncMode::FetchOnly,
        Mode::Probe | Mode::Check => gitsync::SyncMode::Skip,
    };
    let git_sync = gitsync::sync(&env.home.join("body"), git_mode, &gitsync::GitEnv::production());
    let hk = housekeep::housekeep(&env.home, &env.session, SystemTime::now());
    let mut warnings = proc_facts.warnings.clone();
    warnings.extend(hk.warnings.iter().cloned());

    // 6: the extension host (not awaited). Mode `check` loads no extension: it only proves the state boots.
    let (ext, ext_fact, ext_warning) = if env.mode == Mode::Check {
        (None, json!({"status": "disabled"}), None)
    } else {
        ext::start(&env)
    };
    warnings.extend(ext_warning);

    // The model client, abort and tools: built once, shared by the probe and the loop.
    let transport = Transport::from_vars(|k| vars.get(k).map(str::to_string))
        .map_err(|e| BootError::config("env", format!("INIT_PROXY_SOCK: {e}")))?;
    let client = Arc::new(Client::new(info, transport));
    let abort = Arc::new(Abort::new(kill_hook));
    let summary_cx = Arc::new(Mutex::new(SummaryCx {
        session: env.session.clone(),
        task: None,
    }));
    let summarizer = ProxySummarizer::new(client.clone(), abort.clone(), summary_cx.clone());
    let tools = Tools::new(ToolsConfig {
        bash: proc_facts
            .bash
            .clone()
            .unwrap_or_else(|| PathBuf::from("/bin/bash")),
        ext: ext.clone(),
        provider: info.provider.to_string(),
        model: info.id.to_string(),
        init_home: Some(env.home.clone()),
        base_env: None,
        limits: Default::default(),
    });

    // 7: rehydration (lock, scan, repair, head, boot event, sync).
    let lock_wait = vars
        .test_ms("INIT_TEST_LOCK_WAIT_MS")
        .unwrap_or(crate::log::LOCK_WAIT);
    let rh = rehydrate::rehydrate(rehydrate::RehydrateIn {
        env: &env,
        proc_facts: &proc_facts,
        boot: &boot,
        info,
        effort,
        git_sync: &git_sync,
        spill_bytes: hk.spill_bytes,
        warnings,
        ext_fact,
        summarizer: if env.mode == Mode::Check { Box::new(NoModel) } else { Box::new(summarizer) },
        lock_wait,
        probe_timeout: Duration::from_secs(2),
    })?;
    let session = rh.ctx.log.sid().to_string();
    if let Ok(mut cx) = summary_cx.lock() {
        cx.session = session.clone();
    }
    if rh.created
        && let Some(x) = &ext
    {
        x.reset_trials(); // host.md §3.9 item 5: a new session starts with kept components only
    }
    faults.hit("boot.after_boot_event");

    let status = Arc::new(Status::default());
    let proxy_sock = env.proxy_sock.clone();
    let signals = awareness::Signals {
        proxy_sock: Some(proxy_sock),
        cx: awareness::SignalCx::from_boot(&rootview::render_boot(&boot.raw, &env.home)),
        start: status.start,
        stats_timeout: Duration::from_millis(200),
    };
    let mut ctx = LogContext::new(rh.ctx, Box::new(signals));

    // 8: the probe (probe mode only).
    let probe_v = if env.mode == Mode::Probe {
        let watchdog = vars
            .test_ms("INIT_TEST_PROBE_WATCHDOG_MS")
            .unwrap_or(probe::WATCHDOG);
        let rep = probe::run(
            &mut ctx,
            client.as_ref(),
            &abort,
            &probe::ProbeCx {
                tools: &tools,
                cwd: &proc_facts.cwd,
                home: &env.home,
                user_home: env.user_home.as_deref(),
                effort,
                watchdog,
            },
        )?;
        rep.to_value()
    } else {
        Value::Null
    };

    // 9: ready (the boot event is durable: append_boot synced it).
    faults.hit("boot.before_ready");
    let leaf = ctx.ctx.leaf();
    let ready_v = ready::ready_value(&ready::ReadyIn {
        boot_event_id: &rh.boot_event_id,
        boot_id: &env.boot_id,
        leaf: &leaf,
        mode: mode_str(env.mode),
        prefix_hash: &rh.prefix_hash,
        probe: probe_v,
        session: &session,
        sha: &env.sha,
    });
    ready::write_ready(&env.home, &ready_v)
        .map_err(|e| BootError::internal("ready", format!("state/run/ready: {e}")))?;
    if env.mode == Mode::Check {
        return Ok(0); // ADR-011 §2.1: booted, rehydrated, ready; the Root's A4 probe reads the exit code
    }

    // 10: the heartbeat, at once and then every 5 s (also while paused).
    status.set_leaf(leaf);
    status.phase.store(Phase::Idle as u8, Ordering::SeqCst);
    let hb_stop = Arc::new(AtomicBool::new(false));
    let _hb = spawn_heartbeat(
        HeartbeatInfo {
            run_dir: ready::run_dir(&env.home),
            boot_id: env.boot_id.clone(),
            session: session.clone(),
            period: HEARTBEAT,
        },
        status.clone(),
        abort.clone(),
        hb_stop.clone(),
    )
    .map_err(|e| BootError::internal("heartbeat", e.to_string()))?;

    // 11: serve.
    let head_root = rehydrate::head_root(&env, &boot);
    let home = env.home.clone();
    let head_source: crate::r#loop::HeadSource =
        Box::new(move || rehydrate::load_head(&head_root, &home));
    let task_cx = summary_cx.clone();
    let on_task: crate::r#loop::TaskHook = Box::new(move |sid: &str, task: Option<&str>| {
        if let Ok(mut c) = task_cx.lock() {
            c.session = sid.to_string();
            c.task = task.map(str::to_string);
        }
    });
    let model: Arc<dyn ModelClient> = client;
    let code = serve::serve(
        serve::ServeIn {
            ctx: Box::new(ctx),
            model,
            tools,
            ext,
            abort,
            status,
            mode: env.mode,
            effort,
            cwd: proc_facts.cwd.clone(),
            home: env.home.clone(),
            user_home: env.user_home.clone(),
            head_source: Some(head_source),
            fault: faults.hook(),
            on_task: Some(on_task),
        },
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
    )
    .map_err(|e| BootError::internal("serve", e.to_string()))?;
    hb_stop.store(true, Ordering::SeqCst);
    Ok(code)
}
