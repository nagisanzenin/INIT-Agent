//! Serve (body/boot.md §3.13): the stdio-jsonl/1 front-end threads and the hand-off to the loop.
//! This file is the only place boot touches WP11's loop API (`Session`, `SessionConfig`, the frame
//! threads), so a change there stays here.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use crate::model::{Abort, Effort, ModelClient};
use crate::r#loop::frames::{OUT_CAP, spawn_input, spawn_output};
use crate::r#loop::{
    ContextHooks, FaultHook, HeadSource, OutTx, Session, SessionConfig, Status, TaskHook,
};
use crate::tools::{ExtHostApi, Mode, Tools};

pub struct ServeIn {
    pub ctx: Box<dyn ContextHooks>,
    pub model: Arc<dyn ModelClient>,
    pub tools: Tools,
    pub ext: Option<Arc<dyn ExtHostApi>>,
    pub abort: Arc<Abort>,
    pub status: Arc<Status>,
    pub mode: Mode,
    pub effort: Effort,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub user_home: Option<PathBuf>,
    pub head_source: Option<HeadSource>,
    pub fault: Option<FaultHook>,
    pub on_task: Option<TaskHook>,
}

/// How long the output thread may take to flush after the loop returns (supervise's grace is 10 s).
pub const FLUSH_WAIT: Duration = Duration::from_secs(5);

/// Starts the output and input threads, runs `Session::serve` and flushes. Returns the exit code
/// (0 stop/EOF, 75 handoff, 1 fatal log error).
pub fn serve<R, W>(s: ServeIn, stdin: R, stdout: W) -> Result<u8, std::io::Error>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let (out, out_rx) = OutTx::channel(OUT_CAP);
    let out_thread = spawn_output(stdout, out_rx)?;
    let (ctl_tx, ctl_rx) = mpsc::channel();
    spawn_input(
        stdin,
        ctl_tx,
        s.abort.clone(),
        s.ext.clone(),
        s.status.clone(),
        out.clone(),
    )?;
    let status = s.status.clone();
    let session = Session::new(SessionConfig {
        ctx: s.ctx,
        model: s.model,
        tools: s.tools,
        abort: s.abort,
        status: s.status,
        ctl: ctl_rx,
        out,
        mode: s.mode,
        effort: s.effort,
        cwd: s.cwd,
        home: s.home,
        user_home: s.user_home,
        head_source: s.head_source,
        fault: s.fault,
        on_task: s.on_task,
    });
    let code = session.serve(); // consumes the session's `out` sender
    status.gate.detach(); // the input thread's sender: now the output thread can finish
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("flush".into())
        .spawn(move || {
            let _ = out_thread.join();
            let _ = tx.send(());
        });
    let _ = rx.recv_timeout(FLUSH_WAIT);
    Ok(code)
}
