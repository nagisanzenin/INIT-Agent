//! `Context`: the turn-boundary API the loop calls (log-context.md §4.3, §3.13), epochs (P3), masking
//! (§3.11) and compaction (§3.12) epochs, notices, commands, pause/resume and rendering.
//!
//! Input types here are log-level mirrors of body/loop.md's `BoundaryInput` and body/provider.md's
//! `AssistantMessage`; WP11 maps its types onto them (see build/status/WP12.md).

use std::collections::VecDeque;
use std::io;

use serde_json::{Map, Value, json};

use super::compact::{
    self, Artifacts, ForkRequest, OwnerMsg, Summarizer, Summary, SummaryReply, choose_cut,
    owner_msg_of, parse_summary, render_owner_msgs, render_summary, render_summary_text, repair,
    summary_block, summary_prompt, tail_tokens, validate,
};
use super::hash;
use super::head::Head;
use super::mask;
use super::model::{
    Api, Calib, MANUAL_COMPACT_MIN, SUMMARY_MAX_OUTPUT, SUMMARY_SOFT_CAP, THRASH_MAX, THRASH_WINDOW,
    Target, Thresholds,
};
use super::project::{
    Loaded, Msg, View, build_view, masked_tokens, project, project_for_estimate, push_loaded,
};
use super::render::{Renderer, assemble, head_fields, native_request, system_message};
use crate::log::clock::{id_hex, parse_id};
use crate::log::event::{
    Block, BootEv, BranchEv, CommandEv, DisplayEv, ErrorEv, EventBody, GateEv, Kind, LabelEv,
    MaskEv, MessageEv, NoticeEv, ReloadEv, RequestMeta, SignalEv, SteerEv, ThinningDoneEv,
    ToolCallEv, ToolResultEv, UpdateFailedEv, UsageEv,
};
use crate::log::icj;
use crate::log::session::OpenReport;
use crate::log::{LOCK_WAIT, SessionLog};

// ---------------------------------------------------------------------------------------------
// Inputs and outputs
// ---------------------------------------------------------------------------------------------

/// A tool call about to execute (its `tool_call` marker).
#[derive(Clone, Debug, PartialEq)]
pub struct CallStart {
    pub call_id: String,
    pub name: String,
    pub idempotency_key: Option<String>,
}

/// A complete model response (body/provider.md `AssistantMessage`, as logged).
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantIn {
    pub content: Vec<Block>,
    pub provider: String,
    pub api: String,
    pub model: String,
    pub response_id: Option<String>,
    /// `end_turn` | `tool_use` | `max_tokens` | `aborted` | `refusal` | `error`
    pub stop_reason: String,
    pub stop_details: Option<Value>,
    pub usage: UsageEv,
    pub wall_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultIn {
    pub call_id: String,
    pub name: String,
    /// Text and image blocks (images already stored as blobs).
    pub output: Vec<Block>,
    pub is_error: bool,
    pub spill_path: Option<String>,
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub wall_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OwnerIn {
    pub input_id: String,
    pub text: String,
    pub channel: Option<String>,
}

/// A kernel `notice{id, kind, data}` frame with its rendered text (awareness.md renders it).
#[derive(Clone, Debug, PartialEq)]
pub struct NoticeIn {
    pub id: String,
    pub kind: String,
    pub data: Value,
    pub text: String,
}

/// One reload tier's outcome. `head` is the rebuilt head for a T0 reload (`None` keeps the head).
#[derive(Clone, Debug, PartialEq)]
pub struct ReloadIn {
    pub tier: String,
    pub ok: bool,
    pub components: Vec<String>,
    pub head: Option<Head>,
    pub error: Option<String>,
    pub text: String,
}

/// §3.13 steps 1–4 input (body/loop.md `BoundaryInput`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryIn {
    pub results: Vec<ResultIn>,
    pub harness_text: Option<String>,
    pub owner: Vec<OwnerIn>,
    pub steers: Vec<OwnerIn>,
    pub commands: Vec<OwnerIn>,
    pub reload: Vec<ReloadIn>,
    pub notices: Vec<NoticeIn>,
    pub errors: Vec<ErrorEv>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryOut {
    /// Input or notice ids that were already in the log: acknowledged without appending.
    pub duplicates: Vec<String>,
    /// Command outcomes, by input id.
    pub outcomes: Vec<(String, String)>,
    /// `/new` created this session.
    pub new_session: Option<String>,
    /// `prefix_changed` per reload tier.
    pub reload_prefix_changed: Vec<bool>,
}

/// The next request, rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestOut {
    pub body: Vec<u8>,
    pub head_hash: String,
    pub est_tokens: u64,
    /// `x-init-task`: the input id of the latest owner message on the path.
    pub task_id: Option<String>,
    pub epoch: String,
    pub messages: Vec<Msg>,
    /// `mask` or `compaction` when this boundary started an epoch.
    pub rebuilt: Option<&'static str>,
}

#[derive(Debug)]
pub enum ContextError {
    Io(io::Error),
    /// The context cannot be brought under the window: the turn is aborted (owner told).
    Overflow(String),
    Render(String),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextError::Io(e) => write!(f, "context: {e}"),
            ContextError::Overflow(m) => write!(f, "context overflow: {m}"),
            ContextError::Render(m) => write!(f, "context render: {m}"),
        }
    }
}

impl std::error::Error for ContextError {}

impl From<io::Error> for ContextError {
    fn from(e: io::Error) -> Self {
        ContextError::Io(e)
    }
}

impl From<icj::IcjError> for ContextError {
    fn from(e: icj::IcjError) -> Self {
        ContextError::Render(e.to_string())
    }
}

/// Renders the self-signal for step 6 (content: awareness.md §3.8; WP13 implements it).
pub trait SignalSource {
    fn signal(&mut self, ctx_tokens: u64, ctx_permille: u64, th: &Thresholds) -> SignalEv;
}

/// Default `update_failed` text for a branch-back when the caller supplies none.
pub fn update_failed_text(stage: &str, reason: &str, sha: &str) -> String {
    format!(
        "[init:update-failed] The update to {} failed at {stage}: {reason}. You are still running the previous body. Report: state/kernel/update-failed.json.",
        &sha[..sha.len().min(7)]
    )
}

// ---------------------------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------------------------

pub struct Context {
    pub log: SessionLog,
    pub head: Head,
    head_hash: String,
    pub target: Target,
    pub th: Thresholds,
    pub calib: Calib,
    pub view: View,
    summarizer: Box<dyn Summarizer>,
    /// Body sha (for `/new` headers).
    pub sha: String,
    // Estimation (§3.14).
    last_usage: Option<(u64, u64)>,
    last_req: Option<(u64, u64)>,
    from_scratch: bool,
    // Rebuild policy.
    rebuild_hist: VecDeque<bool>,
    pending_compact: Option<Option<String>>,
    emergency: bool,
    signal_due: bool,
    // Rendering.
    fields: Map<String, Value>,
    system_bytes: Vec<u8>,
    cache: Vec<Vec<u8>>,
    last_meta: RequestMeta,
    last_owner_input: Option<String>,
    paused: Option<(u64, Option<u64>)>,
}

impl Context {
    /// §3.6 steps 7, 8, 10, 11: epoch, active window, calibration and head hash.
    pub fn new(
        log: SessionLog,
        head: Head,
        target: Target,
        summarizer: Box<dyn Summarizer>,
        report: &OpenReport,
        sha: &str,
    ) -> Result<Context, ContextError> {
        let calib = Calib {
            k_ppm: report.k_ppm.unwrap_or(crate::log::K0_PPM),
        };
        let th = Thresholds::for_target(&target);
        let view = build_view(
            &log,
            &calib,
            &target.profile,
            &target.provider,
            &target.model,
        )?;
        let mut c = Context {
            log,
            head,
            head_hash: String::new(),
            target,
            th,
            calib,
            view,
            summarizer,
            sha: sha.to_string(),
            last_usage: None,
            last_req: None,
            from_scratch: true,
            rebuild_hist: VecDeque::new(),
            pending_compact: None,
            emergency: false,
            signal_due: false,
            fields: Map::new(),
            system_bytes: Vec::new(),
            cache: Vec::new(),
            last_meta: RequestMeta::default(),
            last_owner_input: None,
            paused: None,
        };
        c.set_head_bytes()?;
        c.last_owner_input = c.scan_last_owner_input()?;
        Ok(c)
    }

    fn set_head_bytes(&mut self) -> Result<(), ContextError> {
        self.fields = if self.target.api == Api::ChatCompletions {
            head_fields(&self.head, &self.target)?
        } else {
            Map::new() // native adapter owns the entire Messages/Responses request
        };
        self.system_bytes = icj::to_vec(&system_message(&self.head))?;
        self.head_hash = self.compute_head_hash()?;
        self.cache.clear();
        Ok(())
    }

    /// The head hash of the head this body sends (from a request body with only the head in it).
    fn compute_head_hash(&self) -> Result<String, ContextError> {
        let body = if self.target.api == Api::ChatCompletions {
            assemble(&self.fields, self.target.max_tokens, &[&self.system_bytes])?
        } else {
            native_request(&self.head, &self.target, &self.log.home, self.log.sid(), &[], self.target.max_tokens)?
        };
        let v: Value =
            serde_json::from_slice(&body).map_err(|e| ContextError::Render(e.to_string()))?;
        Ok(hash::prefix_hash(
            &self.target.provider,
            self.target.api,
            &v,
        )?)
    }

    pub fn head_hash(&self) -> &str {
        &self.head_hash
    }

    pub fn leaf(&self) -> String {
        self.log.leaf().map(id_hex).unwrap_or_default()
    }

    fn scan_last_owner_input(&self) -> io::Result<Option<String>> {
        let path = self.log.path();
        for &p in path.iter().rev() {
            if self.log.index.ents()[p].kind != Kind::Message {
                continue;
            }
            if let EventBody::Message(m) = self.log.load(p)?.body
                && m.is_owner()
            {
                return Ok(m.input_id);
            }
        }
        Ok(None)
    }

    // -- appending ------------------------------------------------------------------------------

    /// Appends at the leaf and adds the event to the window.
    fn append(&mut self, body: &EventBody) -> io::Result<u64> {
        let id = self.log.append(body)?;
        self.push_appended(id)?;
        Ok(id)
    }

    fn push_appended(&mut self, id: u64) -> io::Result<()> {
        let pos = self.log.index.pos(id).expect("just appended");
        let ev = self.log.load(pos)?;
        if let EventBody::Message(m) = &ev.body
            && m.is_owner()
        {
            self.last_owner_input = m.input_id.clone();
        }
        let ppos = self.view.path_len;
        push_loaded(
            &mut self.view,
            ev,
            ppos,
            &self.calib,
            &self.target.profile,
            &self.target.provider,
            &self.target.model,
        );
        Ok(())
    }

    /// The path changed (epoch, branch, resume, new session): rebuild the view and the caches.
    fn rebuild_view(&mut self) -> io::Result<()> {
        // Free the old view and byte cache first, so an epoch never holds two windows at once (the
        // peak sets the allocator's high-water mark, M-5). A failed rebuild is a fatal log error.
        self.view = View::default();
        self.cache = Vec::new();
        self.view = build_view(
            &self.log,
            &self.calib,
            &self.target.profile,
            &self.target.provider,
            &self.target.model,
        )?;
        self.from_scratch = true;
        Ok(())
    }

    /// §3.6 steps 12–13: the boot event (sync D3), then corrupt-line and other warnings.
    pub fn append_boot(&mut self, mut boot: BootEv, rep: &OpenReport) -> io::Result<u64> {
        boot.prefix_hash = self.head_hash.clone();
        boot.log_repair = rep.repair.clone();
        boot.extra
            .entry("interrupted_calls")
            .or_insert(rep.interrupted_calls().into());
        boot.extra
            .entry("rehydrated_events")
            .or_insert(rep.rehydrated_events.into());
        let id = self.append(&EventBody::Boot(Box::new(boot)))?;
        self.log.sync()?;
        let before = self.log.index.len();
        self.log.append_repair_reports(rep)?;
        for w in &self.head.warnings.clone() {
            self.log.append(&EventBody::Display(DisplayEv {
                level: "warn".into(),
                text: w.clone(),
            }))?;
        }
        self.log.sync()?;
        let new: Vec<u64> = self.log.index.ents()[before..]
            .iter()
            .map(|e| e.id)
            .collect();
        for i in new {
            self.push_appended(i)?;
        }
        Ok(id)
    }

    /// The model response is complete: append the message and its `tool_call` markers, then sync (D1).
    pub fn on_response(&mut self, a: AssistantIn, started: &[CallStart]) -> io::Result<u64> {
        let prompt = a.usage.prompt_total();
        if let Some((bytes, images)) = self.last_req
            && prompt > 0
        {
            self.calib.observe(prompt, images, bytes);
        }
        let mut meta = self.last_meta.clone();
        meta.k_ppm = Some(self.calib.k_ppm);
        meta.wall_ms = a.wall_ms;
        meta.ttft_ms = a.ttft_ms;
        let body = EventBody::from(MessageEv {
            role: "assistant".into(),
            content: a.content,
            provider: Some(a.provider),
            api: Some(a.api),
            model: Some(a.model),
            response_id: Some(a.response_id.map(Value::from).unwrap_or(Value::Null)),
            stop_reason: Some(a.stop_reason),
            stop_details: Some(a.stop_details.unwrap_or(Value::Null)),
            usage: Some(Box::new(a.usage.clone())),
            request: Some(Box::new(meta)),
            ..Default::default()
        });
        let id = self.append(&body)?;
        for s in started {
            self.append(&EventBody::ToolCall(ToolCallEv {
                call_id: s.call_id.clone(),
                name: s.name.clone(),
                message_id: id_hex(id),
                idempotency_key: s.idempotency_key.clone(),
            }))?;
        }
        self.log.sync()?; // D1: before any tool runs
        if prompt > 0 {
            self.last_usage = Some((prompt, a.usage.output_tokens));
            self.from_scratch = false;
        }
        Ok(id)
    }

    /// §3.13 steps 1–4 with input/notice dedup, then sync.
    pub fn boundary_append(&mut self, input: BoundaryIn) -> io::Result<BoundaryOut> {
        let mut out = BoundaryOut::default();
        // 1. Tool results, in call order.
        if !input.results.is_empty() {
            self.signal_due = true;
        }
        for r in input.results {
            let tokens_est = r
                .output
                .iter()
                .map(|b| match b {
                    Block::Text { text } => self.calib.tokens(text.len() as u64),
                    Block::Image { tokens_est, .. } => *tokens_est,
                    _ => 0,
                })
                .sum();
            self.append(&EventBody::ToolResult(ToolResultEv {
                call_id: r.call_id,
                name: r.name,
                output: r.output,
                is_error: r.is_error,
                spill_path: r.spill_path,
                exit_code: r.exit_code,
                signal: r.signal,
                wall_ms: r.wall_ms,
                tokens_est,
                synthetic: None,
            }))?;
        }
        // Harness text (the cut-off note), then owner messages and steers.
        if let Some(t) = input.harness_text {
            self.append(&EventBody::from(MessageEv {
                role: "user".into(),
                content: vec![Block::text(t)],
                source: Some("harness".into()),
                ..Default::default()
            }))?;
        }
        for o in input.owner {
            if self.log.index.input_ids.contains(&o.input_id) {
                out.duplicates.push(o.input_id);
                continue;
            }
            self.append(&EventBody::from(MessageEv {
                role: "user".into(),
                content: vec![Block::text(o.text)],
                source: Some("owner".into()),
                input_id: Some(o.input_id),
                channel: o.channel,
                ..Default::default()
            }))?;
        }
        for s in input.steers {
            if self.log.index.input_ids.contains(&s.input_id) {
                out.duplicates.push(s.input_id);
                continue;
            }
            self.append(&EventBody::Steer(SteerEv {
                content: vec![Block::text(s.text)],
                input_id: s.input_id,
                channel: s.channel,
            }))?;
        }
        for c in input.commands {
            if self.log.index.input_ids.contains(&c.input_id) {
                out.duplicates.push(c.input_id);
                continue;
            }
            self.command(c, &mut out)?;
        }
        // 3. Reload (one event per tier).
        for r in input.reload {
            let before = self.head_hash.clone();
            if r.ok
                && let Some(h) = r.head
            {
                self.head = h;
                self.set_head_bytes()
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
            let after = self.head_hash.clone();
            let changed = before != after;
            if changed {
                self.from_scratch = true;
            }
            out.reload_prefix_changed.push(changed);
            let text = if r.text.starts_with("[init:reload]") {
                r.text
            } else {
                format!("[init:reload] {}", r.text)
            };
            self.append(&EventBody::Reload(ReloadEv {
                tier: r.tier,
                ok: r.ok,
                components: r.components,
                prefix_changed: changed,
                head_hash_before: before,
                head_hash_after: after,
                error: r.error,
                text,
            }))?;
        }
        // 4. Kernel notices.
        for n in input.notices {
            if self.log.index.notice_ids.contains(&n.id) {
                out.duplicates.push(n.id);
                continue;
            }
            if let Some(body) = self.notice_body(n, &mut out) {
                self.append(&body)?;
            }
        }
        for e in input.errors {
            self.append(&EventBody::Error(e))?;
        }
        self.log.sync()?;
        Ok(out)
    }

    fn notice_body(&self, n: NoticeIn, out: &mut BoundaryOut) -> Option<EventBody> {
        let mut data = n.data.as_object().cloned().unwrap_or_default();
        data.insert("notice_id".into(), n.id.clone().into());
        data.insert("text".into(), n.text.clone().into());
        let parsed = match n.kind.as_str() {
            "gate" => {
                data.insert("async".into(), true.into());
                serde_json::from_value::<GateEv>(Value::Object(data.clone()))
                    .ok()
                    .map(EventBody::Gate)
            }
            "update_failed" => {
                if let Some(seq) = data.get("seq").and_then(Value::as_u64)
                    && self.log.index.update_failed_seqs.contains(&seq)
                {
                    out.duplicates.push(n.id.clone());
                    return None;
                }
                Some(EventBody::UpdateFailed(update_failed_from(
                    &Value::Object(data.clone()),
                    Some(n.text.clone()),
                )))
            }
            "thinning_done" => {
                serde_json::from_value::<ThinningDoneEv>(Value::Object(data.clone()))
                    .ok()
                    .map(EventBody::ThinningDone)
            }
            _ => None,
        };
        Some(parsed.unwrap_or_else(|| {
            EventBody::Notice(NoticeEv {
                source: "kernel".into(),
                code: n.kind.clone(),
                projected: !n.text.is_empty(),
                text: n.text.clone(),
                notice_id: Some(n.id.clone()),
                data: Some(n.data.clone()),
            })
        }))
    }

    /// Owner slash commands (§4.5). The `command` event is appended first, carrying the input id.
    fn command(&mut self, c: OwnerIn, out: &mut BoundaryOut) -> io::Result<()> {
        let text = c.text.trim().to_string();
        let (cmd, rest) = text
            .split_once(char::is_whitespace)
            .map(|(a, b)| (a, b.trim()))
            .unwrap_or((text.as_str(), ""));
        let log_cmd = |s: &mut Self, outcome: &str| {
            s.append(&EventBody::Command(CommandEv {
                input_id: c.input_id.clone(),
                text: c.text.clone(),
                outcome: Some(outcome.into()),
            }))
        };
        let outcome = match cmd {
            "/compact" => {
                self.pending_compact = Some(Some(rest.to_string()).filter(|s| !s.is_empty()));
                log_cmd(self, "compaction at the next boundary")?;
                "compaction at the next boundary".to_string()
            }
            "/new" => {
                log_cmd(self, "new session")?;
                self.log.sync()?;
                let sid = self.new_session()?;
                out.new_session = Some(sid.clone());
                format!("new session {sid}")
            }
            "/branch" => {
                let (target, note) = rest
                    .split_once(char::is_whitespace)
                    .map(|(a, b)| (a, b.trim()))
                    .unwrap_or((rest, ""));
                match self.check_branch_target(target)? {
                    Ok(tid) => {
                        log_cmd(self, &format!("branched to {target}"))?;
                        let note = Some(note.to_string()).filter(|s| !s.is_empty());
                        self.branch(tid, "owner", note)?;
                        format!("branched to {target}")
                    }
                    Err(why) => {
                        log_cmd(self, &format!("refused: {why}"))?;
                        if why.contains("tool round") {
                            self.append(&EventBody::Error(ErrorEv {
                                where_: "context".into(),
                                code: "branch_in_tool_round".into(),
                                message: format!("branch to {target} refused: {why}"),
                                data: None,
                            }))?;
                        }
                        format!("refused: {why}")
                    }
                }
            }
            "/label" => {
                let (target, label) = rest
                    .split_once(char::is_whitespace)
                    .map(|(a, b)| (a, b.trim()))
                    .unwrap_or((rest, ""));
                match parse_id(target).filter(|t| self.log.index.contains(*t)) {
                    Some(t) => {
                        log_cmd(self, &format!("labeled {target}"))?;
                        let label = Some(label.to_string()).filter(|s| !s.is_empty());
                        self.append(&EventBody::Label(LabelEv {
                            target_id: id_hex(t),
                            label,
                        }))?;
                        format!("labeled {target}")
                    }
                    None => {
                        log_cmd(self, "refused: unknown event id")?;
                        "refused: unknown event id".into()
                    }
                }
            }
            _ => {
                log_cmd(self, "unknown command")?;
                "unknown command".into()
            }
        };
        out.outcomes.push((c.input_id, outcome));
        Ok(())
    }

    /// `/new`: a new session file and pointer; the old session is untouched.
    pub fn new_session(&mut self) -> io::Result<String> {
        let sha = self.sha.clone();
        let sid = self
            .log
            .new_session(&sha)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.last_owner_input = None;
        self.last_usage = None;
        self.signal_due = false; // the batch it would describe is in the old session
        self.rebuild_view()?;
        Ok(sid)
    }

    /// Validates a branch target: it exists and does not lie inside an open tool round (§3.5).
    pub fn check_branch_target(&self, target: &str) -> io::Result<Result<u64, String>> {
        let Some(tid) = parse_id(target) else {
            return Ok(Err("not an event id".into()));
        };
        if !self.log.index.contains(tid) {
            return Ok(Err("unknown event id".into()));
        }
        // Walk the target's path back to the nearest assistant message; every one of its calls must
        // have a result between it and the target.
        let path = self.log.index.path(Some(tid));
        let mut results = std::collections::HashSet::new();
        for &p in path.iter().rev() {
            let k = self.log.index.ents()[p].kind;
            if k != Kind::Message && k != Kind::ToolResult {
                continue;
            }
            match self.log.load(p)?.body {
                EventBody::ToolResult(r) => {
                    results.insert(r.call_id);
                }
                EventBody::Message(m) if m.is_assistant() => {
                    let open = m.tool_uses().any(|(c, _, _)| !results.contains(c));
                    return Ok(if open {
                        Err("the target lies inside an open tool round".into())
                    } else {
                        Ok(tid)
                    });
                }
                _ => {}
            }
        }
        Ok(Ok(tid))
    }

    /// Appends a `branch` event whose parent is `target`; the leaf moves there (§3.5). Sync D3.
    pub fn branch(
        &mut self,
        target: u64,
        reason: &str,
        summary: Option<String>,
    ) -> io::Result<u64> {
        let from_leaf = self.leaf();
        let text = summary.as_ref().map(|s| format!("[init:branch] {s}"));
        let id = self.log.append_with_parent(
            &EventBody::Branch(BranchEv {
                from_leaf,
                reason: reason.into(),
                summary,
                text,
            }),
            target,
        )?;
        self.log.sync()?;
        self.signal_due = false; // the batch it would describe is now off the path
        self.rebuild_view()?;
        self.last_owner_input = self.scan_last_owner_input()?;
        Ok(id)
    }

    // -- the request ----------------------------------------------------------------------------

    fn head_tokens(&self) -> u64 {
        let tools: u64 = self
            .head
            .tools
            .iter()
            .map(|t| icj::to_vec(t).map(|b| b.len() as u64).unwrap_or(0))
            .sum();
        self.calib
            .tokens(self.head.core.len() as u64 + self.head.self_.len() as u64 + tools)
    }

    /// §3.14 running estimate (exact where the provider told us), or from scratch after an epoch.
    pub fn estimate(&self, msgs: &[Msg]) -> u64 {
        match (self.from_scratch, self.last_usage) {
            (false, Some((p, o))) => {
                let tail_start = msgs
                    .iter()
                    .rposition(Msg::is_assistant)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let (b, i) =
                    super::project::content_bytes(&msgs[tail_start..], &self.target.profile);
                p + o + self.calib.tokens(b) + i
            }
            _ => self.head_tokens() + msgs.iter().map(Msg::tokens).sum::<u64>(),
        }
    }

    pub fn project(&self) -> Vec<Msg> {
        project(&self.view, &self.target.profile, &self.calib)
    }

    /// The current estimate (§3.14) without copying the content the estimate does not read.
    fn estimate_now(&self) -> u64 {
        self.estimate(&project_for_estimate(
            &self.view,
            &self.target.profile,
            &self.calib,
        ))
    }

    fn window_tokens(&self) -> Vec<u64> {
        self.view
            .window
            .iter()
            .map(|l| masked_tokens(&self.view, l, &self.calib))
            .collect()
    }

    fn renderer(&self) -> Renderer<'_> {
        Renderer {
            target: &self.target,
            home: &self.log.home,
        }
    }

    /// Renders a request for `msgs`, using the per-epoch byte cache for closed messages.
    fn render(
        &mut self,
        msgs: &[Msg],
        max_tokens: u32,
        use_cache: bool,
    ) -> Result<Vec<u8>, ContextError> {
        let max_tokens = crate::provider::models::request_tokens(&self.target.provider, &self.target.model, max_tokens);
        if self.target.api != Api::ChatCompletions {
            return Ok(native_request(
                &self.head, &self.target, &self.log.home, self.log.sid(), msgs, max_tokens,
            )?);
        }
        let closed = msgs
            .iter()
            .rposition(Msg::is_assistant)
            .map(|i| i + 1)
            .unwrap_or(0);
        let r = Renderer {
            target: &self.target,
            home: &self.log.home,
        };
        let mut fresh: Vec<Vec<u8>> = Vec::new();
        if use_cache {
            if self.cache.len() > closed {
                self.cache.truncate(closed);
            }
            for m in &msgs[self.cache.len()..closed] {
                self.cache.push(r.msg_bytes(m)?);
            }
            for m in &msgs[closed..] {
                fresh.push(r.msg_bytes(m)?);
            }
        } else {
            for m in msgs {
                fresh.push(r.msg_bytes(m)?);
            }
        }
        let mut pieces: Vec<&[u8]> = vec![&self.system_bytes];
        if use_cache {
            pieces.extend(self.cache.iter().map(Vec::as_slice));
        }
        pieces.extend(fresh.iter().map(Vec::as_slice));
        Ok(assemble(&self.fields, max_tokens, &pieces)?)
    }

    /// Full serialization without the cache (T-CTX-4 compares the two).
    pub fn render_uncached(
        &mut self,
        msgs: &[Msg],
        max_tokens: u32,
    ) -> Result<Vec<u8>, ContextError> {
        self.render(msgs, max_tokens, false)
    }

    /// §3.13 steps 5–8: rebuild check, signal, sync (D2), render.
    pub fn next_request(
        &mut self,
        signal: &mut dyn SignalSource,
        max_tokens: u32,
    ) -> Result<RequestOut, ContextError> {
        let rebuilt = self.rebuild_check()?;
        self.rebuild_hist.push_back(rebuilt.is_some());
        while self.rebuild_hist.len() > THRASH_WINDOW - 1 {
            self.rebuild_hist.pop_front();
        }
        if self.signal_due {
            let est = self.estimate_now();
            let permille = (est * 1000 / self.th.w_eff.max(1)).min(1000);
            let mut s = signal.signal(est, permille, &self.th);
            s.ctx_tokens = est;
            s.ctx_permille = permille;
            self.append(&EventBody::Signal(s))?;
            self.signal_due = false;
        }
        self.log.sync()?; // D2: the request's context is on disk before it is paid for
        let msgs = self.project();
        let est = self.estimate(&msgs);
        let body = self.render(&msgs, max_tokens, true)?;
        let (b, i) = super::project::content_bytes(&msgs, &self.target.profile);
        let head_bytes = self.head.core.len() as u64
            + self.head.self_.len() as u64
            + self
                .head
                .tools
                .iter()
                .map(|t| icj::to_vec(t).map(|b| b.len() as u64).unwrap_or(0))
                .sum::<u64>();
        self.last_req = Some((b + head_bytes, i));
        let epoch = id_hex(self.view.epoch);
        self.last_meta = RequestMeta {
            head_hash: Some(self.head_hash.clone()),
            est_tokens: Some(est),
            k_ppm: Some(self.calib.k_ppm),
            epoch: if self.view.epoch == 0 {
                None
            } else {
                Some(epoch.clone())
            },
            task_id: Some(
                self.last_owner_input
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            wall_ms: None,
            ttft_ms: None,
        };
        Ok(RequestOut {
            body,
            head_hash: self.head_hash.clone(),
            est_tokens: est,
            task_id: self.last_owner_input.clone(),
            epoch,
            messages: msgs,
            rebuilt,
        })
    }

    pub fn request_emergency_compaction(&mut self) {
        self.emergency = true;
    }

    /// Queues a manual compaction (also reached through `/compact`).
    pub fn request_compaction(&mut self, focus: Option<String>) {
        self.pending_compact = Some(focus);
    }

    fn thrashing(&self) -> bool {
        self.rebuild_hist.iter().filter(|x| **x).count() + 1 >= THRASH_MAX
    }

    fn display(&mut self, level: &str, text: &str) -> io::Result<u64> {
        self.append(&EventBody::Display(DisplayEv {
            level: level.into(),
            text: text.into(),
        }))
    }

    fn error(&mut self, code: &str, message: &str, data: Option<Value>) -> io::Result<u64> {
        self.append(&EventBody::Error(ErrorEv {
            where_: "context".into(),
            code: code.into(),
            message: message.into(),
            data,
        }))
    }

    /// §3.11 decision rule (boundary step 5).
    fn rebuild_check(&mut self) -> Result<Option<&'static str>, ContextError> {
        let est = self.estimate_now();
        let manual = self.pending_compact.take();
        let emergency = std::mem::take(&mut self.emergency);
        if let Some(focus) = manual.clone() {
            if est < MANUAL_COMPACT_MIN && !emergency {
                self.display("info", "nothing to compact")?;
                return Ok(None);
            }
            let _ = focus;
        }
        let forced = manual.is_some() || emergency;
        if !forced && est < self.th.t_trigger {
            return Ok(None);
        }
        if !forced && self.thrashing() {
            self.error(
                "rebuild_thrash",
                "auto-rebuild suspended for this turn: the third rebuild within 6 requests",
                Some(json!({"est_tokens": est})),
            )?;
            self.display(
                "warn",
                "Context refills faster than it can be compacted (a large output keeps re-entering). Read it in smaller pieces or start a new session with /new.",
            )?;
            if est >= self.th.hard_limit {
                return Err(self.overflow(est)?);
            }
            return Ok(None);
        }
        let tokens = self.window_tokens();
        let protect = choose_cut(&self.view.window, &tokens, self.th.keep, u64::MAX).unwrap_or(0);
        let cands = mask::candidates(&self.view, protect, &self.calib);
        let saved: u64 = cands.iter().map(|c| c.saved).sum();
        if !forced && est.saturating_sub(saved) <= self.th.t_target && !cands.is_empty() {
            self.mask_epoch(cands, est)?;
            return Ok(Some("mask"));
        }
        let trigger = if emergency {
            "overflow"
        } else if manual.is_some() {
            "manual"
        } else {
            "auto"
        };
        let keep = if emergency {
            self.th.keep / 2
        } else {
            self.th.keep
        };
        let r = self.compaction_epoch(trigger, manual.flatten(), keep, est)?;
        Ok(Some(r))
    }

    fn overflow(&mut self, est: u64) -> io::Result<ContextError> {
        let m = format!(
            "the context needs about {est} tokens; the window allows {}",
            self.th.hard_limit
        );
        self.error("context_overflow", &m, Some(json!({"est_tokens": est})))?;
        self.display("error", "The context is too large for the model even after compaction, so this turn was stopped. Start a new session with /new, or read large outputs in smaller pieces.")?;
        self.log.sync()?;
        Ok(ContextError::Overflow(m))
    }

    fn mask_count(&self) -> u64 {
        self.log
            .path()
            .iter()
            .filter(|&&p| self.log.index.ents()[p].kind == Kind::Mask)
            .count() as u64
    }

    /// A mask epoch: one `mask` event with every placeholder (sync D3).
    fn mask_epoch(&mut self, cands: Vec<mask::Candidate>, est: u64) -> io::Result<Option<u64>> {
        if cands.is_empty() {
            return Ok(None);
        }
        let saved: u64 = cands.iter().map(|c| c.saved).sum();
        let n = self.mask_count() + 1;
        let id = self.log.append(&EventBody::Mask(MaskEv {
            masked: cands.into_iter().map(|c| c.entry).collect(),
            tokens_before: est,
            tokens_after_est: est.saturating_sub(saved),
            n,
        }))?;
        self.log.sync()?;
        self.rebuild_view()?;
        Ok(Some(id))
    }

    fn mask_all(&mut self, est: u64) -> io::Result<()> {
        let tokens = self.window_tokens();
        let protect = choose_cut(&self.view.window, &tokens, self.th.keep, u64::MAX).unwrap_or(0);
        let cands = mask::candidates(&self.view, protect, &self.calib);
        self.mask_epoch(cands, est)?;
        Ok(())
    }

    fn fork_body(&mut self, msgs: &[Msg]) -> Result<Vec<u8>, ContextError> {
        self.render(msgs, SUMMARY_MAX_OUTPUT, false)
    }

    /// §3.12.3. Returns the epoch kind started (`compaction`, or `mask` on fallback).
    fn compaction_epoch(
        &mut self,
        trigger: &str,
        focus: Option<String>,
        keep: u64,
        est: u64,
    ) -> Result<&'static str, ContextError> {
        let tokens = self.window_tokens();
        let Some(cut) = choose_cut(&self.view.window, &tokens, keep, self.th.max_tail) else {
            self.mask_all(est)?;
            return Ok("mask");
        };
        // The byte cache is only a memo (T-CTX-4: cached and uncached renders are identical) and an
        // epoch drops it anyway, so free it now: the fork's render runs at the window's largest
        // (the M-5 peak) and would otherwise sit beside a second rendered copy of the context.
        self.cache = Vec::new();
        // The previous compaction and its summary.
        let prev = self.view.summary.as_ref().and_then(|l| match &l.ev.body {
            EventBody::Compaction(c) => Some((**c).clone()),
            _ => None,
        });
        let prev_summary: Option<Summary> = prev
            .as_ref()
            .and_then(|c| summary_block(&c.summary).map(parse_summary));
        let over_soft_cap = prev_summary
            .as_ref()
            .map(|s| self.calib.tokens(s.render_sections().len() as u64) > SUMMARY_SOFT_CAP)
            .unwrap_or(false);
        // The fork: exactly the current request plus the summary prompt.
        let mut fork = self.project();
        fork.push(user_msg(
            &summary_prompt(focus.as_deref(), over_soft_cap),
            &self.calib,
        ));
        let mut retries = 0u64;
        let mut format_retry = false;
        let mut id_retry = false;
        let mut summarizer_usage: Option<UsageEv> = None;
        let mut summarizer_model: String;
        let (mut parsed, repaired) = loop {
            let body = self.fork_body(&fork)?;
            let attempt = retries as u32;
            let reply = self.summarizer.summarize(&ForkRequest {
                messages: &fork,
                body: &body,
                max_tokens: crate::provider::models::request_tokens(&self.target.provider, &self.target.model, SUMMARY_MAX_OUTPUT),
                attempt,
            });
            let reply = match reply {
                Ok(r) => r,
                Err(e) => {
                    self.error(
                        "summary_failed",
                        &format!("summarizer call failed: {e}"),
                        None,
                    )?;
                    self.mask_all(est)?;
                    return Ok("mask");
                }
            };
            summarizer_model = reply.model.clone();
            summarizer_usage = reply.usage.clone().or(summarizer_usage);
            let block = summary_block(&reply.text).filter(|_| !reply.tool_use);
            let Some(block) = block else {
                if !format_retry {
                    format_retry = true;
                    retries += 1;
                    self.push_retry(&mut fork, &reply, compact::RETRY_FORMAT_TEXT);
                    continue;
                }
                self.error(
                    "summary_invalid",
                    "the summarizer did not return a text-only <summary> block after one retry",
                    None,
                )?;
                self.mask_all(est)?;
                return Ok("mask");
            };
            let mut s = parse_summary(block);
            let problems = validate(prev_summary.as_ref(), &s);
            if problems.is_empty() {
                break (s, Vec::new());
            }
            if !id_retry {
                id_retry = true;
                retries += 1;
                self.push_retry(&mut fork, &reply, &problems.retry_text());
                continue;
            }
            let rep = repair(prev_summary.as_ref(), &mut s);
            break (s, rep);
        };
        // Borrowed, not cloned: the window is at its largest here (M-5 peak).
        let span: &[Loaded] = &self.view.window[..cut];
        // Body-generated sections.
        let mut owner: Vec<OwnerMsg> = Vec::new();
        if let Some(p) = &prev {
            for id in &p.owner_msgs {
                if let Some(ev) = parse_id(id)
                    .map(|i| self.log.load_id(i))
                    .transpose()?
                    .flatten()
                {
                    let l = Loaded {
                        ev,
                        ppos: 0,
                        tokens: 0,
                    };
                    if let Some(m) = owner_msg_of(&l) {
                        owner.push(m);
                    }
                }
            }
        }
        owner.extend(compact::owner_msgs_in(span));
        let artifacts = prev
            .as_ref()
            .map(|p| Artifacts::from_value(&p.artifacts))
            .unwrap_or_default()
            .merge(span, &self.log.home);
        let n = prev.as_ref().map(|p| p.n + 1).unwrap_or(1);
        let through = self.view.window[cut - 1].ev.id_hex();
        let first_kept = self.view.window[cut].ev.id_hex();
        let retired = std::mem::take(&mut parsed.retired);
        let summary = render_summary(
            n,
            &through,
            &parsed.render_sections(),
            &render_owner_msgs(&owner),
            &artifacts.render(),
            &retired,
        );
        let text = render_summary_text(n, &through, est, self.log.sid(), &summary);
        // Inflation check: prelude + summary + kept tail must be smaller than before.
        let prelude_tokens = span
            .iter()
            .rev()
            .find(|l| l.kind() == Kind::Boot)
            .or(self.view.prelude.as_ref())
            .map(|l| l.tokens)
            .unwrap_or(0);
        // With one huge last step the tail is masked right after the compaction (§3.12.2): count it.
        let huge_tail = tail_tokens(&tokens, cut) > self.th.max_tail;
        let tail_saving: u64 = if huge_tail {
            let last_a = self
                .view
                .window
                .iter()
                .rposition(|l| matches!(&l.ev.body, EventBody::Message(m) if m.is_assistant()));
            let tail_ids: std::collections::HashSet<String> = self.view.window[cut..]
                .iter()
                .map(|l| l.ev.id_hex())
                .collect();
            mask::candidates_in(&self.view, self.view.window.len(), last_a, &self.calib)
                .into_iter()
                .filter(|c| tail_ids.contains(&c.entry.id))
                .map(|c| c.saved)
                .sum()
        } else {
            0
        };
        let after = (self.head_tokens()
            + prelude_tokens
            + self.calib.tokens(text.len() as u64)
            + tail_tokens(&tokens, cut))
        .saturating_sub(tail_saving);
        if after >= est {
            self.error(
                "summary_inflated",
                "the summary did not shrink the context; it was discarded",
                Some(json!({"tokens_before": est, "tokens_after_est": after})),
            )?;
            self.mask_all(est)?;
            return Ok("mask");
        }
        let owner_ids: Vec<String> = owner.iter().map(|m| m.id.clone()).collect();
        let mut summarizer = json!({"model": summarizer_model});
        if let Some(u) = summarizer_usage {
            summarizer["usage"] = icj::to_value(&u)?;
        }
        self.log.append(&EventBody::Compaction(Box::new(
            crate::log::event::CompactionEv {
                summary,
                text,
                first_kept_id: first_kept,
                through_id: through,
                tokens_before: est,
                tokens_after_est: after,
                n,
                trigger: trigger.into(),
                focus,
                artifacts: icj::to_value(&artifacts)?,
                owner_msgs: owner_ids,
                summarizer: Some(summarizer),
                retries,
                repaired_ids: repaired,
            },
        )))?;
        self.log.sync()?; // D3
        self.rebuild_view()?;
        // One huge last step: also mask inside the kept tail, protecting only its assistant message.
        if huge_tail {
            let protect = self
                .view
                .window
                .iter()
                .rposition(|l| matches!(&l.ev.body, EventBody::Message(m) if m.is_assistant()))
                .unwrap_or(0);
            let est2 = self.estimate_now();
            let cands = mask::candidates_in(
                &self.view,
                self.view.window.len(),
                Some(protect),
                &self.calib,
            );
            self.mask_epoch(cands, est2)?;
            let est3 = self.estimate_now();
            if est3 >= self.th.t_trigger {
                return Err(self.overflow(est3)?);
            }
        }
        Ok("compaction")
    }

    fn push_retry(&self, fork: &mut Vec<Msg>, reply: &SummaryReply, correction: &str) {
        let mut blocks = Vec::new();
        if let Some(t) = &reply.thinking {
            blocks.push(Block::Thinking {
                provider: self.target.provider.clone(),
                model: self.target.model.clone(),
                text: t.clone(),
                native: None,
            });
        }
        blocks.push(Block::text(reply.text.clone()));
        let tokens = self.calib.tokens(reply.text.len() as u64);
        fork.push(Msg::Assistant {
            event: 0,
            blocks,
            tokens,
        });
        fork.push(user_msg(correction, &self.calib));
    }

    // -- lifecycle ------------------------------------------------------------------------------

    /// Kernel pause: sync, unlock, return `(log_len, leaf)`.
    pub fn pause(&mut self) -> io::Result<(u64, String)> {
        let (len, leaf) = self.log.writer.unlock()?;
        self.paused = Some((len, leaf));
        Ok((len, leaf.map(id_hex).unwrap_or_default()))
    }

    /// Handoff failed: re-lock, rescan from `log_len`, and with a failure append `update_failed` whose
    /// parent is `leaf` (branch-back), deduplicated by `seq`. The leaf returns to `leaf` either way.
    pub fn resume(&mut self, log_len: u64, leaf: &str, failure: Option<Value>) -> io::Result<()> {
        let rep = self
            .log
            .writer
            .relock_and_rescan(log_len, LOCK_WAIT, &mut self.log.index)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.paused = None;
        let leaf_id = parse_id(leaf);
        self.log.writer.set_leaf(leaf_id);
        if let Some(f) = failure {
            let dup = f
                .get("seq")
                .and_then(Value::as_u64)
                .map(|s| self.log.index.update_failed_seqs.contains(&s))
                .unwrap_or(false);
            if !dup {
                self.log
                    .append(&EventBody::UpdateFailed(update_failed_from(&f, None)))?;
            }
        }
        if !rep.skipped.is_empty() {
            let skipped: Vec<Value> = rep
                .skipped
                .iter()
                .map(|s| json!({"offset": s.offset, "len": s.len}))
                .collect();
            self.log.append(&EventBody::Error(ErrorEv {
                where_: "log".into(),
                code: "corrupt_line".into(),
                message: "lines written by the failed body were skipped".into(),
                data: Some(json!({"skipped": skipped, "newline_appended": rep.tail.is_some()})),
            }))?;
        }
        self.log.sync()?;
        self.rebuild_view()?;
        self.last_owner_input = self.scan_last_owner_input()?;
        Ok(())
    }

    /// The leaf is an owner message, a steer, a harness message, or a complete result set.
    pub fn has_pending_work(&self) -> bool {
        for l in self.view.window.iter().rev() {
            match &l.ev.body {
                EventBody::Message(m) => return !m.is_assistant(),
                EventBody::Steer(_) | EventBody::ToolResult(_) => return true,
                _ => {}
            }
        }
        false
    }
}

fn user_msg(text: &str, calib: &Calib) -> Msg {
    Msg::User {
        blocks: vec![Block::text(text)],
        tokens: calib.tokens(text.len() as u64),
    }
}

/// An `update_failed` event from `update-failed.json`'s shape (`to_sha`, `stage`, `reason`, `seq`,
/// `stderr_tail`, …; kernel/supervise.md §4.3) or from an already-mapped object.
pub fn update_failed_from(v: &Value, text: Option<String>) -> UpdateFailedEv {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let sha = s("sha").or_else(|| s("to_sha")).unwrap_or_default();
    let stage = s("stage").unwrap_or_else(|| "handoff".into());
    let reason = s("reason").unwrap_or_default();
    let mut extra = Map::new();
    for k in ["from_sha", "job", "exit_code", "signal", "report"] {
        if let Some(x) = v.get(k) {
            extra.insert(k.into(), x.clone());
        }
    }
    if let Some(t) = s("log_tail").or_else(|| s("stderr_tail")) {
        let t: String = t
            .chars()
            .rev()
            .take(4096)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        extra.insert("log_tail".into(), t.into());
    }
    let text = text
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| update_failed_text(&stage, &reason, &sha));
    UpdateFailedEv {
        sha,
        stage,
        reason,
        text,
        seq: v.get("seq").and_then(Value::as_u64),
        notice_id: s("notice_id"),
        extra,
    }
}
