//! `ext` dispatch, body side (tools.md §3.6). ext/host.md (WP14) owns everything below the call;
//! until it lands, `StubHost` stands in. The host is reached only through `ExtHostApi`, which
//! mirrors host.md §4.1, so WP14 implements the trait for its `ExtHost`.

use std::any::Any;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{Map, Value};

use super::args::{check_fields, req, req_str, type_error};
use super::{Mode, Ran, ToolCx, ToolError, text_result};

/// ext/host.md §4.1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadKind {
    Boot,
    Request,
    SessionStart,
    Kept(Vec<String>),
}

/// ext/host.md §3.10: the call context (`abort` = the loop's `Abort::flag()`).
pub struct CallCtx<'a> {
    pub call_id: &'a str,
    pub workspace: &'a Path,
    pub abort: &'a AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    Ok,
    InvalidInput,
    Failed,
    Timeout,
    Aborted,
    Oom,
    StackOverflow,
    Panic,
    Trap,
    Exit,
    Busy,
    Unknown,
    Host,
}

impl OutcomeKind {
    /// tools.md §3.6 code mapping.
    pub fn code(self) -> &'static str {
        match self {
            OutcomeKind::Ok => "ok",
            OutcomeKind::InvalidInput => "invalid_input",
            OutcomeKind::Unknown => "ext_unknown",
            OutcomeKind::Timeout => "timeout",
            OutcomeKind::Aborted => "aborted",
            OutcomeKind::Failed => "ext_failed",
            OutcomeKind::Busy => "ext_busy",
            OutcomeKind::Host => "ext_host",
            OutcomeKind::Oom
            | OutcomeKind::StackOverflow
            | OutcomeKind::Panic
            | OutcomeKind::Trap
            | OutcomeKind::Exit => "ext_trap",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExtOutcome {
    pub text: String,
    pub is_error: bool,
    pub kind: OutcomeKind,
    pub version: Option<String>,
    pub elapsed_ms: u64,
    pub truncated: bool,
}

/// ext/host.md §4.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadReport {
    pub ok: bool,
    pub generation: u64,
    pub components: Vec<String>,
    pub text: String,
}

/// A registry snapshot, opaque to the loop and the tools (host.md's `Arc<Registry>`, type-erased so
/// this seam does not depend on Wasmtime types).
pub type Snapshot = Arc<dyn Any + Send + Sync>;

/// host.md §4.1 as a trait. WP14's `ExtHost` implements it.
pub trait ExtHostApi: Send + Sync {
    fn begin_reload(&self, kind: ReloadKind);
    fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport>;
    fn reload_pending(&self) -> bool;
    fn reset_trials(&self);
    fn snapshot(&self) -> Snapshot;
    fn call(&self, reg: &Snapshot, name: &str, input: &Value, cx: &CallCtx) -> ExtOutcome;
    fn shutdown(&self);
}

/// Stand-in until WP14: no extensions are ever loaded; every name is unknown.
pub struct StubHost;

impl ExtHostApi for StubHost {
    fn begin_reload(&self, _kind: ReloadKind) {}
    fn commit_reload(&self, _max_wait: Duration) -> Option<ReloadReport> {
        None
    }
    fn reload_pending(&self) -> bool {
        false
    }
    fn reset_trials(&self) {}
    fn snapshot(&self) -> Snapshot {
        Arc::new(())
    }
    fn call(&self, _reg: &Snapshot, name: &str, _input: &Value, _cx: &CallCtx) -> ExtOutcome {
        ExtOutcome {
            text: format!(
                "no extension named \"{name}\". Loaded extensions: none (the extension host is not built into this body yet)."
            ),
            is_error: true,
            kind: OutcomeKind::Unknown,
            version: None,
            elapsed_ms: 0,
            truncated: false,
        }
    }
    fn shutdown(&self) {}
}

/// Input objects larger than this (serialized) are refused before the host (ext/wit.md §3.7).
pub const INPUT_MAX: usize = 1 << 20;

#[derive(Clone, Debug)]
pub struct ExtArgs {
    pub name: String,
    pub input: Value,
}

pub fn parse(m: &Map<String, Value>) -> Result<ExtArgs, ToolError> {
    check_fields(m, &["input", "name"], "")?;
    let name = req_str(m, "name", "")?;
    if name.is_empty() {
        return Err(ToolError::invalid("name: must not be empty"));
    }
    let input = req(m, "input", "")?;
    if !input.is_object() {
        return Err(type_error("input", "an object", input));
    }
    let size = serde_json::to_vec(input)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if size > INPUT_MAX {
        return Err(ToolError::invalid(format!(
            "input: {} bytes once serialized; extensions take at most 1 MiB",
            super::commas(size as u64)
        )));
    }
    Ok(ExtArgs {
        name,
        input: input.clone(),
    })
}

pub fn run(host: Option<&dyn ExtHostApi>, a: &ExtArgs, cx: &ToolCx) -> Result<Ran, ToolError> {
    let (Some(host), false) = (host, cx.mode == Mode::Safe) else {
        return Err(ToolError::new(
            "ext_disabled",
            "extensions are disabled in safe mode",
        ));
    };
    let reg = match cx.reg {
        Some(r) => r.clone(),
        None => host.snapshot(),
    };
    let o = host.call(
        &reg,
        &a.name,
        &a.input,
        &CallCtx {
            call_id: cx.call_id,
            workspace: cx.paths.cwd,
            abort: cx.abort.flag(),
        },
    );
    if !o.is_error && o.kind == OutcomeKind::Ok {
        return Ok(Ran {
            result: text_result(cx.call_id, o.text),
            summary: "ok".into(),
        });
    }
    let code = if o.kind == OutcomeKind::Ok {
        "ext_failed"
    } else {
        o.kind.code()
    };
    let e = ToolError::new(code, o.text);
    Err(if o.kind == OutcomeKind::Unknown {
        e.hint("schemas are in $INIT_HOME/state/ext/catalog.json")
    } else {
        e
    })
}
