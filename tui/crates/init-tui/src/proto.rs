//! The owner channel as the TUI sees it (docs/tui.md §3.3, root/root.md §4.2, kernel/supervise.md
//! §4.6): frame classification over `serde_json::Value` and the five client frames. Unknown `t`
//! values and unknown fields are ignored. Outgoing frames are ICJ: sorted keys (serde_json's map is
//! a BTreeMap without `preserve_order`), no whitespace.

use serde_json::{Value, json};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The Root and the kernel stop at 1 MiB per line; the TUI sends at most 900 KiB (§3.7).
pub const LINE_MAX: usize = 900 * 1024;
/// The reader's limit for one incoming frame (§5).
pub const FRAME_IN_MAX: u64 = 2 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayKind {
    Text,
    Thinking,
    ToolStart,
    ToolEnd,
    /// The newest output of a running call (proposed, §9 Q4): `call_id` and `text`.
    ToolProgress,
    TurnEnd,
    Info,
    Error,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub kind: DisplayKind,
    pub text: String,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub ok: Option<bool>,
    pub boot_seq: Option<u64>,
    /// An optional longer result (§3.6), rendered under the summary when present.
    pub detail: Option<String>,
    /// `parent_call_id`: the `delegate` call a sub-task's tool frame belongs to; such a call is
    /// shown nested and collapsed under that call's row.
    pub parent: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Display(Display),
    /// `qid` (Root: queued while no kernel runs) or `seq` (kernel: durable in its inbox).
    Echo { text: String, qid: Option<String>, seq: Option<u64> },
    /// `data` (TUI-OBSERVE, §3.10): a kernel notice's structured payload (`{"kind": …}`), an
    /// object or nothing; the gate track reads it (`crate::render::gates`).
    Notice { level: String, text: String, ts: Option<String>, code: Option<String>, data: Option<Value> },
    State(Value),
    ReplayEnd { epoch: Option<String>, gap: bool },
    /// A request answer that arrived on the attach connection (only in a broken exchange).
    Answer(Value),
    Unknown,
}

/// Who wrote a frame, when the Root stamps it (§9 Q1 `src`); None as built.
pub fn src(v: &Value) -> Option<&str> {
    v.get("src").and_then(Value::as_str)
}

pub fn rseq(v: &Value) -> Option<u64> {
    v.get("rseq").and_then(Value::as_u64)
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

pub fn classify(v: &Value) -> Frame {
    match v.get("t").and_then(Value::as_str) {
        Some("display") => {
            let kind = match v.get("kind").and_then(Value::as_str) {
                Some("text") => DisplayKind::Text,
                Some("thinking") => DisplayKind::Thinking,
                Some("tool_start") => DisplayKind::ToolStart,
                Some("tool_end") => DisplayKind::ToolEnd,
                Some("tool_progress") => DisplayKind::ToolProgress,
                Some("turn_end") => DisplayKind::TurnEnd,
                Some("info") => DisplayKind::Info,
                Some("error") => DisplayKind::Error,
                _ => DisplayKind::Other,
            };
            Frame::Display(Display {
                kind,
                text: s(v, "text").unwrap_or_default(),
                call_id: s(v, "call_id"),
                name: s(v, "name"),
                ok: v.get("ok").and_then(Value::as_bool),
                boot_seq: v.get("boot_seq").and_then(Value::as_u64),
                detail: s(v, "detail"),
                parent: s(v, "parent_call_id"),
            })
        }
        Some("echo") => Frame::Echo { text: s(v, "text").unwrap_or_default(), qid: s(v, "qid"), seq: v.get("seq").and_then(Value::as_u64) },
        Some("notice") => Frame::Notice {
            level: s(v, "level").unwrap_or_else(|| "info".into()),
            text: s(v, "text").unwrap_or_default(),
            ts: s(v, "ts"),
            code: s(v, "code"),
            data: v.get("data").filter(|d| d.is_object()).cloned(),
        },
        Some("state") => Frame::State(v.clone()),
        Some("replay_end") => Frame::ReplayEnd { epoch: s(v, "epoch"), gap: v.get("gap").and_then(Value::as_bool).unwrap_or(false) },
        None if v.get("ok").is_some() => Frame::Answer(v.clone()),
        _ => Frame::Unknown,
    }
}

/// The ICJ encoding of a value (sorted keys, compact).
pub fn icj(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// `attach`, with §9 Q1's `since` and `epoch` once the Root has shown it numbers its frames.
pub fn attach(replay: bool, since: Option<(u64, &str)>) -> String {
    let mut v = json!({"client": format!("init-tui/{VERSION}"), "replay": replay, "t": "attach"});
    if let Some((since, epoch)) = since {
        v["since"] = json!(since);
        v["epoch"] = json!(epoch);
    }
    icj(&v)
}

pub fn line(text: &str) -> String {
    icj(&json!({"t": "line", "text": text}))
}

pub fn interrupt() -> String {
    icj(&json!({"t": "interrupt"}))
}

/// STEER: interrupt *and* steer (Esc with a steer pending): the body aborts, then continues the
/// run with the steer instead of going idle. An older Root, kernel or body treats it as a plain
/// interrupt (the agent stops; the steer starts the next run), which is safe.
pub fn interrupt_steer() -> String {
    icj(&json!({"t": "interrupt", "then": "continue"}))
}

/// STEER: take back the pending line the kernel numbered `seq` (its echo's `seq`).
pub fn retract(seq: u64) -> String {
    icj(&json!({"seq": seq, "t": "retract"}))
}

pub fn panic_frame() -> String {
    icj(&json!({"t": "panic"}))
}

pub fn detach() -> String {
    icj(&json!({"t": "detach"}))
}

pub fn request(id: u64, cmd: &str) -> String {
    request_with(id, cmd, json!({}))
}

/// A request frame with arguments (ADR-009's `workspace.set`, `full.set`, `undo`).
pub fn request_with(id: u64, cmd: &str, args: Value) -> String {
    icj(&json!({"args": args, "cmd": cmd, "id": format!("t-{id}"), "v": 1}))
}

/// The line as sent: `/clear` is an alias of `/new` (UX-KEYMODEL, docs/ux-v1.md §3.6.1), in TTY and line mode.
pub fn canonical(text: &str) -> String {
    if text.trim() == "/clear" { "/new".into() } else { text.to_string() }
}

/// How a line typed by the owner is handled (§3.8). Commands produce no echo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `/panic` alone: the panic frame, never a line.
    Panic,
    /// A Root line (`/root …`, `/resume` while the Root is panicked, halted or stopped).
    RootCommand,
    /// A kernel command (`/stop`, `/status`, …).
    KernelCommand,
    /// An input to the body (echoed by the kernel), including `/later`, `/new`, `/compact`.
    Input,
}

/// Mirrors the Root's `line` and the kernel's `owner_in` parsing (exact match after trim).
pub fn line_kind(text: &str, root_rootish: bool) -> LineKind {
    let t = text.trim();
    match t {
        "/panic" => return LineKind::Panic,
        "/root status" | "/root ledger" | "/root stop" | "/root help" | "/root" => return LineKind::RootCommand,
        // ADR-009 lines the Root answers itself with notices (root SBX-009 §B)
        "/workspace" | "/full" | "/full on confirm" | "/undo confirm" => return LineKind::RootCommand,
        "/resume" if root_rootish => return LineKind::RootCommand,
        // ADR-013: the owner's goal lines (`/goal`, `/goal "<intent>" …`, `/goal stop`): only the Root reads them
        _ if t == "/goal" || t.starts_with("/goal ") => return LineKind::RootCommand,
        _ => {}
    }
    let (word, rest) = match t.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim()),
        None => (t, ""),
    };
    match (word, rest.is_empty()) {
        ("/stop" | "/resume" | "/status" | "/kernel", true) => LineKind::KernelCommand,
        ("/accept" | "/bless" | "/reject", false) => LineKind::KernelCommand,
        _ => LineKind::Input,
    }
}

/// The owner commands of §3.8 for the completion popup, with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/panic", "stop everything now (Root; no confirmation)"),
    ("/resume", "leave panic, halt or stop (Root), or resume the kernel"),
    ("/root status", "the Root's status"),
    ("/root ledger", "the autonomy ledger"),
    ("/root stop", "stop the zone"),
    ("/root help", "the Root's lines"),
    ("/undo", "restore the workspace's last snapshot (asks to confirm)"),
    ("/full on", "full access: read and write anywhere except INIT's Root, network (the default)"),
    ("/full off", "confine this session: write only the workspace, no network but the provider"),
    ("/workspace", "the workspace and the agent's access"),
    ("/goal ", "the owner's goal (Root): /goal \"<intent>\" done=card:cost_turn<=N usd=1.00 wall=24h rounds=10 guard=<card>; /goal stop|pause|continue"),
    ("/stop", "stop the current run (kernel)"),
    ("/usage", "show metered cost and tokens locally"),
    ("/details", "print the last tool activity in full (calls, tokens per request)"),
    ("/status", "the kernel's status line"),
    ("/kernel", "the kernel's commands"),
    ("/accept ", "accept a task: /accept <task>"),
    ("/bless ", "bless a job: /bless <job>"),
    ("/reject ", "reject a job: /reject <job>"),
    ("/later ", "queue a follow-up: /later <text>"),
    ("/new", "start a new session (body)"),
    ("/clear", "start a new session (same as /new)"),
    ("/model", "list the models and switch (the Root restarts the agent)"),
    ("/key", "set a provider's API key (hidden; kept by the Root)"),
    ("/setup", "choose a provider, key and model"),
    ("/compact", "compact the context: /compact [focus]"),
    ("/branch ", "branch: /branch <id> [note]"),
    ("/label ", "label an entry: /label <id> <text>"),
];

/// Does a kernel echo `echo` confirm the line `sent` (§3.11)? The kernel echoes a follow-up
/// without its `/later ` prefix.
pub fn echo_matches(sent: &str, echo: &str) -> bool {
    sent == echo || sent.trim().strip_prefix("/later ").is_some_and(|r| r == echo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_frames_are_icj() {
        assert_eq!(line("hi \"x\""), r#"{"t":"line","text":"hi \"x\""}"#);
        assert_eq!(interrupt(), r#"{"t":"interrupt"}"#);
        assert_eq!(attach(true, None), format!(r#"{{"client":"init-tui/{VERSION}","replay":true,"t":"attach"}}"#));
        assert_eq!(attach(true, Some((7, "e"))), format!(r#"{{"client":"init-tui/{VERSION}","epoch":"e","replay":true,"since":7,"t":"attach"}}"#));
        assert_eq!(request(3, "status"), r#"{"args":{},"cmd":"status","id":"t-3","v":1}"#);
        assert_eq!(request_with(4, "workspace.set", json!({"cwd": "/p"})), r#"{"args":{"cwd":"/p"},"cmd":"workspace.set","id":"t-4","v":1}"#);
    }

    #[test]
    fn line_kinds() {
        assert_eq!(line_kind(" /panic ", false), LineKind::Panic);
        assert_eq!(line_kind("/root status", false), LineKind::RootCommand);
        assert_eq!(line_kind("/resume", true), LineKind::RootCommand);
        assert_eq!(line_kind("/resume", false), LineKind::KernelCommand);
        assert_eq!(line_kind("/stop", false), LineKind::KernelCommand);
        assert_eq!(line_kind("/stop now", false), LineKind::Input);
        assert_eq!(line_kind("/accept t1", false), LineKind::KernelCommand);
        assert_eq!(line_kind("/accept", false), LineKind::Input);
        assert_eq!(line_kind("/later do x", false), LineKind::Input);
        assert_eq!(line_kind("/new", false), LineKind::Input);
        assert_eq!(line_kind("/stop\nmore", false), LineKind::Input);
        assert_eq!(line_kind("/workspace", false), LineKind::RootCommand);
        assert_eq!(line_kind("/goal", false), LineKind::RootCommand);
        assert_eq!(line_kind("/goal \"cheaper turns\" usd=0.50", false), LineKind::RootCommand);
        assert_eq!(line_kind("/goals are nice", false), LineKind::Input);
        assert_eq!(line_kind(" /full ", false), LineKind::RootCommand);
        assert_eq!(line_kind("/undo confirm", false), LineKind::RootCommand);
    }

    #[test]
    fn clear_is_new() {
        assert_eq!(canonical(" /clear "), "/new");
        assert_eq!(canonical("/new"), "/new");
        assert_eq!(canonical("/clear all"), "/clear all", "only the exact command");
        assert_eq!(line_kind(&canonical("/clear"), false), line_kind("/new", false));
        assert!(COMMANDS.iter().any(|c| c.0 == "/clear") && COMMANDS.iter().any(|c| c.0 == "/new"));
    }

    #[test]
    fn echo_matching() {
        assert!(echo_matches("hi", "hi"));
        assert!(echo_matches("/later do x", "do x"));
        assert!(!echo_matches("hi", "hi "));
    }

    #[test]
    fn classify_frames() {
        let d = classify(&json!({"t":"display","kind":"tool_end","text":"exit 0","call_id":"c1","ok":true,"boot_seq":2,"extra":1}));
        assert!(matches!(d, Frame::Display(Display { kind: DisplayKind::ToolEnd, ok: Some(true), boot_seq: Some(2), parent: None, .. })));
        // DELEGATE: a sub-task's call names its delegate call
        let s = classify(&json!({"t":"display","kind":"tool_start","text":"ls","call_id":"d1/c1","parent_call_id":"d1","boot_seq":2}));
        assert!(matches!(s, Frame::Display(Display { parent: Some(ref p), .. }) if p == "d1"));
        assert_eq!(classify(&json!({"t":"zzz"})), Frame::Unknown);
        assert!(matches!(classify(&json!({"t":"echo","text":"x","seq":3})), Frame::Echo { seq: Some(3), .. }));
        assert!(matches!(classify(&json!({"ok":false,"id":null})), Frame::Answer(_)));
    }

    /// TUI-OBSERVE: a notice's `data` is read when it is an object; an older kernel sends none.
    #[test]
    fn notice_data() {
        let n = classify(&json!({"t":"notice","level":"warn","text":"[kernel] gate G2 fail","ts":"t","src":"kernel",
            "data":{"kind":"gate","stage":"G2","pass":false}}));
        assert!(matches!(n, Frame::Notice { data: Some(ref d), ref level, .. } if d["stage"] == "G2" && level == "warn"));
        let old = classify(&json!({"t":"notice","level":"info","text":"[kernel] promoted","ts":"t"}));
        assert!(matches!(old, Frame::Notice { data: None, .. }));
        assert!(matches!(classify(&json!({"t":"notice","text":"x","data":"not an object"})), Frame::Notice { data: None, .. }));
    }
}
