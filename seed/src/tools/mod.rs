//! The seven tools (body/tools.md): `read`, `write`, `edit` run in-process, `bash` spawns one process
//! group per call, `ext` routes to the Wasm extension host (ext/host.md), `delegate` runs one
//! sub-task in a fresh context (loop/delegate.rs; this file only parses and frames it), and
//! `ask_owner` asks the owner a question with options and ends the turn (tools/ask.rs).
//!
//! The common pipeline (§3.1): typed argument parsing that rejects unknown fields, lexical path
//! resolution with no filesystem access and no policy (the OS sandbox enforces), and output
//! finalization (the 51,200-byte inline cap with a spill file).

pub mod args;
pub mod ask;
pub mod bash;
pub mod delegate;
pub mod edit;
pub mod ext;
pub mod memcap;
pub mod read;
pub mod write;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub mod testutil;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::model::{Abort, ContentBlock, ToolCall, ToolMeta, ToolResult, ToolSpec};

#[allow(unused_imports)]
pub use bash::{
    CaptureSpec, Captured, GroupOutcome, RunSpec, Signal, find_bash, kill_group, run_group,
};
#[allow(unused_imports)]
pub use ext::{
    CallCtx, ExtHostApi, ExtOutcome, OutcomeKind, ReloadKind, ReloadReport, Snapshot, StubHost,
};

/// tools.md §4.1, minified with sorted keys (6,582 bytes). The single source of the tool bytes in
/// every request, the head and `--manifest`. Changing it is a `Prefix-change: yes` commit.
pub const REGISTRY_JSON: &str = include_str!("registry.json");

/// Tool names, sorted byte-wise (the registry order).
pub const NAMES: [&str; 7] = ["ask_owner", "bash", "delegate", "edit", "ext", "read", "write"];

/// Body mode (boot.md §3.14). Safe mode disables `ext`. `Check` (ADR-011 §2.1, the Root's A4 rollback probe):
/// boot, rehydrate `INIT_SESSION`, write `ready`, exit 0; no model call, no kernel socket, no serving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Probe,
    Safe,
    Check,
}

/// tools.md §4.5 limits (defaults). Bash capture sizes are constants in `bash.rs`.
#[derive(Clone, Debug)]
pub struct Limits {
    pub read_lines: usize,
    pub read_bytes: usize,
    pub read_line_bytes: usize,
    pub read_scan: u64,
    pub image_bytes: u64,
    pub write_bytes: usize,
    pub edit_file_bytes: u64,
    pub edit_count: usize,
    pub diff_lines: usize,
    pub bash_default_timeout_s: u64,
    pub result_inline: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read_lines: 2_000,
            read_bytes: 51_200,
            read_line_bytes: 2_000,
            read_scan: 256 << 20,
            image_bytes: 3_750_000,
            write_bytes: 4 << 20,
            edit_file_bytes: 16 << 20,
            edit_count: 64,
            diff_lines: 120,
            bash_default_timeout_s: 120,
            result_inline: 51_200,
        }
    }
}

/// The registry as typed specs, in registry order.
pub fn specs() -> Vec<ToolSpec> {
    let v: Vec<Value> = serde_json::from_str(REGISTRY_JSON).expect("registry.json parses");
    v.into_iter()
        .map(|t| ToolSpec {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            description: t["description"].as_str().unwrap_or_default().to_string(),
            input_schema: t["input_schema"].clone(),
        })
        .collect()
}

/// The registry objects for the head (`context::head::build_head`'s `tools`).
pub fn spec_values() -> Vec<Value> {
    serde_json::from_str(REGISTRY_JSON).expect("registry.json parses")
}

// ---------------------------------------------------------------------------------------------
// Errors (§4.6)
// ---------------------------------------------------------------------------------------------

/// `error: <code>: <message>` plus optional `hint:` lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
    pub hints: Vec<String>,
}

impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        ToolError {
            code,
            message: message.into(),
            hints: Vec::new(),
        }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hints.push(h.into());
        self
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        ToolError::new("invalid_input", message)
    }
    pub fn text(&self) -> String {
        let mut s = format!("error: {}: {}", self.code, self.message);
        for h in &self.hints {
            s.push_str("\nhint: ");
            s.push_str(h);
        }
        s
    }
    pub fn into_result(self, call_id: &str) -> ToolResult {
        error_result(call_id, &self.text())
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

pub fn error_result(call_id: &str, text: &str) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        is_error: true,
        meta: ToolMeta::default(),
    }
}

pub fn text_result(call_id: &str, text: String) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: vec![ContentBlock::Text { text }],
        is_error: false,
        meta: ToolMeta::default(),
    }
}

pub const PERMISSION_HINT: &str =
    "writable roots are body/, state/ and work/ (kernel/ is read-only, vault/ is hidden)";

/// §4.6 `io::ErrorKind` mapping. `shown` is the path as the model wrote it.
pub fn io_error(shown: &str, e: &io::Error) -> ToolError {
    use io::ErrorKind as K;
    match e.kind() {
        K::NotFound => ToolError::new("not_found", format!("{shown} does not exist")),
        K::PermissionDenied | K::ReadOnlyFilesystem => {
            ToolError::new("permission_denied", format!("{shown}: permission denied"))
                .hint(PERMISSION_HINT)
        }
        K::IsADirectory => is_directory(shown),
        K::NotADirectory => ToolError::invalid(format!("a parent of {shown} is a file")),
        K::StorageFull => {
            ToolError::new("io_error", format!("{shown}: {}", os_message(e))).hint("disk full")
        }
        _ => ToolError::new("io_error", format!("{shown}: {}", os_message(e))),
    }
}

pub fn is_directory(shown: &str) -> ToolError {
    ToolError::new("is_directory", format!("{shown} is a directory"))
        .hint(format!("use bash: ls -la {shown}"))
}

/// The OS message as `std` prints it (`No such file or directory (os error 2)`).
pub fn os_message(e: &io::Error) -> String {
    e.to_string()
}

/// `1234567` → `1,234,567` (bytes in messages, §4.2).
pub fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Paths (§3.1)
// ---------------------------------------------------------------------------------------------

/// What path resolution needs: the body cwd, `$HOME` and `$INIT_HOME`.
#[derive(Clone, Copy, Debug)]
pub struct PathCx<'a> {
    pub cwd: &'a Path,
    pub home: Option<&'a Path>,
    pub init_home: Option<&'a Path>,
}

/// `resolve(p, cx)`: `~`/`$INIT_HOME` expansion, join to cwd, lexical normalization. No I/O.
pub fn resolve(p: &str, cx: &PathCx) -> Result<PathBuf, ToolError> {
    if p.is_empty() {
        return Err(ToolError::invalid("path: must not be empty"));
    }
    let expanded: PathBuf = if p == "~" {
        match cx.home {
            Some(h) => h.to_path_buf(),
            None => PathBuf::from(p),
        }
    } else if let (Some(rest), Some(h)) = (p.strip_prefix("~/"), cx.home) {
        h.join(rest)
    } else if let (Some(rest), Some(h)) = (p.strip_prefix("$INIT_HOME/"), cx.init_home) {
        h.join(rest)
    } else if let (true, Some(h)) = (p == "$INIT_HOME", cx.init_home) {
        h.to_path_buf()
    } else {
        PathBuf::from(p)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        cx.cwd.join(expanded)
    };
    Ok(normalize(&joined))
}

/// Drops `.`, resolves `..` lexically, collapses repeated separators.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(x) => out.push(x.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                let at_root = out.as_os_str() == "/";
                if !at_root && !out.pop() {
                    out.push("..");
                } else if !at_root && out.as_os_str().is_empty() && p.is_absolute() {
                    out.push("/");
                }
            }
            Component::Normal(x) => out.push(x),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(if p.is_absolute() { "/" } else { "." });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The registry object
// ---------------------------------------------------------------------------------------------

/// Construction inputs (boot.md §3.2 resolves `bash`; supervise sets the env).
pub struct ToolsConfig {
    pub bash: PathBuf,
    pub ext: Option<Arc<dyn ExtHostApi>>,
    pub provider: String,
    pub model: String,
    pub init_home: Option<PathBuf>,
    /// The environment bash children start from. `None`: inherit this process's environment
    /// (production). `Some`: start from exactly this map (tests; `std::env::set_var` is unsafe).
    pub base_env: Option<BTreeMap<String, String>>,
    pub limits: Limits,
}

pub struct Tools {
    specs: Vec<ToolSpec>,
    pub bash: PathBuf,
    ext: Option<Arc<dyn ExtHostApi>>,
    /// Per-call additions (§3.5.5) except `INIT_CALL_ID` and `INIT_SPILL_DIR`.
    env: BTreeMap<String, String>,
    base_env: Option<BTreeMap<String, String>>,
    pub limits: Limits,
    /// MEM-RUNAWAY (§3.5.8): the Root's memory cap and the events file its watchdog writes.
    pub mem: memcap::Config,
    /// `delegate` sub-task bounds (`INIT_DELEGATE_*`, tools/delegate.rs).
    pub delegate: delegate::Bounds,
}

/// Per-call context (§3.1). `reg` is the batch's ext registry snapshot (None in safe mode).
pub struct ToolCx<'a> {
    pub paths: PathCx<'a>,
    pub spill_dir: &'a Path,
    pub call_id: &'a str,
    pub abort: &'a Abort,
    pub images: bool,
    pub model_id: &'a str,
    pub mode: Mode,
    pub reg: Option<&'a Snapshot>,
}

/// A validated call (§3.1). `Done` carries the pre-filled error of a call that never executes.
#[derive(Clone, Debug)]
pub enum Prepared {
    Ready {
        idx: usize,
        call_id: String,
        name: String,
        job: Job,
    },
    Done {
        idx: usize,
        call_id: String,
        name: String,
        result: ToolResult,
    },
}

impl Prepared {
    pub fn call_id(&self) -> &str {
        match self {
            Prepared::Ready { call_id, .. } | Prepared::Done { call_id, .. } => call_id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Prepared::Ready { name, .. } | Prepared::Done { name, .. } => name,
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, Prepared::Ready { .. })
    }
}

#[derive(Clone, Debug)]
pub enum Job {
    Read(read::ReadArgs),
    Write(write::WriteArgs),
    Edit(edit::EditArgs),
    Bash(bash::BashArgs),
    Ext(ext::ExtArgs),
    /// Parsed here, executed by the loop (it owns the model call): `Prepared::Ready` with this job
    /// never reaches `Tools::run`.
    Delegate(delegate::Args),
    /// `ask_owner`: the loop sends its `ask` frame and ends the turn (tools/ask.rs).
    Ask(ask::Args),
}

/// A finished call: the result plus the one-line `tool_end` text (loop.md §4.1).
#[derive(Clone, Debug)]
pub struct Ran {
    pub result: ToolResult,
    pub summary: String,
}

impl Tools {
    pub fn new(cfg: ToolsConfig) -> Tools {
        bash::set_bash_path(&cfg.bash);
        let lookup = |k: &str| -> Option<String> {
            match &cfg.base_env {
                Some(m) => m.get(k).cloned(),
                None => std::env::var(k).ok(),
            }
        };
        let mut env = BTreeMap::new();
        env.insert("INIT_PROVIDER".to_string(), cfg.provider.clone());
        env.insert("INIT_MODEL".to_string(), cfg.model.clone());
        for (k, v) in [
            ("PAGER", "cat"),
            ("MANPAGER", "cat"),
            ("GIT_PAGER", "cat"),
            ("GIT_EDITOR", "true"),
            ("CLICOLOR", "0"),
            ("CARGO_TERM_COLOR", "never"),
        ] {
            env.insert(k.to_string(), v.to_string());
        }
        for (k, v) in [
            ("TERM", "dumb"),
            ("NO_COLOR", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ] {
            if lookup(k).is_none() {
                env.insert(k.to_string(), v.to_string());
            }
        }
        if lookup("CARGO_TARGET_DIR").is_none()
            && let Some(h) = &cfg.init_home
        {
            env.insert(
                "CARGO_TARGET_DIR".to_string(),
                h.join("work").join("target").to_string_lossy().into_owned(),
            );
        }
        let mut limits = cfg.limits;
        // tools.md §4.5: INIT_TEST_TOOL_TIMEOUT_S overrides the default only under INIT_TEST=1.
        if lookup("INIT_TEST").as_deref() == Some("1")
            && let Some(t) = lookup("INIT_TEST_TOOL_TIMEOUT_S").and_then(|s| s.parse().ok())
        {
            limits.bash_default_timeout_s = t;
        }
        let delegate = delegate::Bounds::from_lookup(lookup);
        let mem = memcap::Config::from_lookup(lookup);
        Tools {
            specs: specs(),
            bash: cfg.bash,
            ext: cfg.ext,
            env,
            base_env: cfg.base_env,
            limits,
            mem,
            delegate,
        }
    }

    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    pub fn ext(&self) -> Option<&Arc<dyn ExtHostApi>> {
        self.ext.as_ref()
    }

    /// Validation (loop.md §3.6): unknown tool, unparsable arguments, typed parsing.
    pub fn prepare(&self, idx: usize, c: &ToolCall, cx: &PathCx) -> Prepared {
        let done = |result: ToolResult| Prepared::Done {
            idx,
            call_id: c.id.clone(),
            name: c.name.clone(),
            result,
        };
        if !NAMES.contains(&c.name.as_str()) {
            return done(error_result(
                &c.id,
                &format!(
                    "error: unknown_tool: no tool named \"{}\". Available tools: {}.",
                    c.name,
                    NAMES.join(", ")
                ),
            ));
        }
        if let Some(e) = c.args_error() {
            return done(error_result(
                &c.id,
                &format!(
                    "error: invalid_json: the arguments for \"{}\" are not valid JSON ({e}). Re-issue the call with valid JSON.",
                    c.name
                ),
            ));
        }
        let job = match c.name.as_str() {
            "read" => read::parse(&c.input, cx, &self.limits).map(Job::Read),
            "write" => write::parse(&c.input, cx).map(Job::Write),
            "edit" => edit::parse(&c.input, cx, &self.limits).map(Job::Edit),
            "bash" => bash::parse(&c.input, &self.limits).map(Job::Bash),
            "delegate" => delegate::parse(&c.input).map(Job::Delegate),
            "ask_owner" => ask::parse(&c.input).map(Job::Ask),
            _ => ext::parse(&c.input).map(Job::Ext),
        };
        match job {
            Ok(job) => Prepared::Ready {
                idx,
                call_id: c.id.clone(),
                name: c.name.clone(),
                job,
            },
            Err(e) => done(e.into_result(&c.id)),
        }
    }

    /// The write/edit target, for the loop's unit grouping (loop.md §3.7).
    pub fn path_key(&self, p: &Prepared) -> Option<PathBuf> {
        match p {
            Prepared::Ready {
                job: Job::Write(w), ..
            } => Some(w.abs.clone()),
            Prepared::Ready {
                job: Job::Edit(e), ..
            } => Some(e.abs.clone()),
            _ => None,
        }
    }

    /// `display{kind:"tool_start"}` text (loop.md §4.1).
    pub fn start_text(&self, p: &Prepared) -> String {
        match p {
            Prepared::Ready { job, .. } => match job {
                Job::Bash(b) => truncate_chars(&b.command, 200),
                Job::Read(r) => format!("read {}", r.path),
                Job::Write(w) => format!("write {}", w.path),
                Job::Edit(e) => format!(
                    "edit {} ({} edit{})",
                    e.path,
                    e.edits.len(),
                    if e.edits.len() == 1 { "" } else { "s" }
                ),
                Job::Ext(x) => format!("ext {}", x.name),
                Job::Ask(a) => ask::start_text(a),
                // The row already shows the name `delegate`: the text is the task.
                Job::Delegate(a) => match a.mode {
                    delegate::Access::Read => delegate::summary_line(&a.task),
                    delegate::Access::Write => {
                        format!("(write) {}", delegate::summary_line(&a.task))
                    }
                },
            },
            Prepared::Done { name, .. } => name.clone(),
        }
    }

    /// Runs one ready call. Never panics on purpose; the loop wraps it in `catch_unwind`.
    pub fn run(&self, p: &Prepared, cx: &ToolCx) -> Ran {
        let (job, call_id) = match p {
            Prepared::Ready { job, call_id, .. } => (job, call_id.as_str()),
            Prepared::Done { result, .. } => {
                return Ran {
                    summary: summary_of_error(result),
                    result: result.clone(),
                };
            }
        };
        let ran = match job {
            Job::Read(a) => read::run(a, cx, &self.limits),
            Job::Write(a) => write::run(a, cx, &self.limits),
            Job::Edit(a) => edit::run(a, cx, &self.limits),
            Job::Bash(a) => return bash::run(self, a, cx),
            Job::Ext(a) => ext::run(self.ext.as_deref(), a, cx),
            Job::Ask(_) => Ok(ask::run(call_id)),
            // The loop runs this one (it owns the model call); reaching here is a bug.
            Job::Delegate(_) => Err(ToolError::new(
                "not_available",
                "delegate is run by the loop, not by the tool layer",
            )),
        };
        let mut ran = match ran {
            Ok(r) => r,
            Err(e) => {
                let result = e.into_result(call_id);
                Ran {
                    summary: summary_of_error(&result),
                    result,
                }
            }
        };
        ran.result.call_id = call_id.to_string();
        // read is bounded by its own limits (its trailer may pass 51,200 by a few bytes).
        if !matches!(job, Job::Read(_)) {
            finalize(&mut ran.result, cx, self.limits.result_inline);
        }
        ran
    }

    /// A bash `Command` with the §3.5.5 environment for one call.
    pub(crate) fn bash_command(&self, command: &str, cx: &ToolCx) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.bash);
        cmd.arg("-c").arg(command).current_dir(cx.paths.cwd);
        if let Some(base) = &self.base_env {
            cmd.env_clear().envs(base);
        }
        cmd.env_remove("BASH_ENV")
            .envs(&self.env)
            .env("INIT_CALL_ID", cx.call_id)
            .env("INIT_SPILL_DIR", cx.spill_dir);
        cmd
    }
}

pub fn truncate_chars(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(n).collect();
    if line.chars().count() > n || s.lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

/// Lines and bytes of a failed call's `detail` (TUI-OBSERVE, tui.md §3.6).
pub const DETAIL_LINES: usize = 10;
const DETAIL_LINE_CHARS: usize = 200;
const DETAIL_MAX: usize = 2_000;

/// TUI-OBSERVE: the owner's view of why a call failed: the last [`DETAIL_LINES`] lines of its
/// first text block (each cut to 200 characters, at most 2,000 bytes in all), without the bash
/// trailer (`[exit_code=… wall_time=…]`, `[timeout …]`, `[aborted …]`). None for a call that
/// succeeded or printed nothing. The model's result is unchanged; this goes only to `display`.
pub fn fail_detail(r: &ToolResult) -> Option<String> {
    if !r.is_error {
        return None;
    }
    let text = r.content.iter().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })?;
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let trailer = |l: &str| {
        l.ends_with(']') && ["[exit_code=", "[timeout after ", "[aborted after "].iter().any(|p| l.starts_with(p))
    };
    if lines.last().is_some_and(|l| trailer(l)) {
        lines.pop();
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0;
    for l in lines.iter().rev().take(DETAIL_LINES) {
        let mut s: String = l.chars().take(DETAIL_LINE_CHARS).collect();
        if l.chars().count() > DETAIL_LINE_CHARS {
            s.push('…');
        }
        if bytes + s.len() + 1 > DETAIL_MAX {
            break;
        }
        bytes += s.len() + 1;
        out.push(s);
    }
    if out.is_empty() {
        return None;
    }
    out.reverse();
    Some(out.join("\n"))
}

/// `error: <code>` from an error result's first line (loop.md §4.1 `tool_end` text).
pub fn summary_of_error(r: &ToolResult) -> String {
    let first = r
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.lines().next().unwrap_or("").to_string()),
            _ => None,
        })
        .unwrap_or_default();
    match first.strip_prefix("error: ") {
        Some(rest) => format!("error: {}", rest.split(':').next().unwrap_or(rest)),
        None => "error".to_string(),
    }
}

/// Output finalization (§3.1): a text block over the inline cap is cut to head + tail and the
/// whole text goes to a spill file.
pub fn finalize(r: &mut ToolResult, cx: &ToolCx, cap: usize) {
    for b in r.content.iter_mut() {
        let ContentBlock::Text { text } = b else {
            continue;
        };
        if text.len() <= cap {
            continue;
        }
        let spilled = bash::spill_whole(cx.spill_dir, cx.call_id, text.as_bytes());
        let (cut, path) = head_tail(text, 4_000, cap - 4_000, spilled.as_deref());
        *text = cut;
        if let Some(p) = path {
            r.meta.spill_path = Some(p);
        }
    }
}

/// Head (cut back to a newline) + marker + tail (cut forward past a newline), on char boundaries.
fn head_tail(
    text: &str,
    head: usize,
    tail: usize,
    spill: Option<&Path>,
) -> (String, Option<String>) {
    let bytes = text.as_bytes();
    let total = bytes.len();
    let h_end = match bytes[..head.min(total)].iter().rposition(|&c| c == b'\n') {
        Some(i) => i,
        None => {
            let mut e = head.min(total);
            while !text.is_char_boundary(e) {
                e -= 1;
            }
            e
        }
    };
    let t0 = total.saturating_sub(tail).max(h_end);
    let t_start = match bytes[t0..].iter().take(1_000).position(|&c| c == b'\n') {
        Some(i) => t0 + i + 1,
        None => {
            let mut s = t0;
            while !text.is_char_boundary(s) {
                s += 1;
            }
            s
        }
    };
    let omitted = t_start - h_end;
    let omitted_lines = bytes[h_end..t_start]
        .iter()
        .filter(|&&c| c == b'\n')
        .count();
    let (where_, path) = match spill {
        Some(p) => (format!("in {}", p.display()), Some(p.display().to_string())),
        None => ("not saved".to_string(), None),
    };
    let marker = format!(
        "\n[... {omitted_lines} lines ({} bytes) omitted; full output ({} bytes) {where_} ...]\n",
        commas(omitted as u64),
        commas(total as u64)
    );
    let mut out = String::with_capacity(h_end + marker.len() + total - t_start);
    out.push_str(&text[..h_end]);
    out.push_str(&marker);
    out.push_str(&text[t_start..]);
    (out, path)
}
