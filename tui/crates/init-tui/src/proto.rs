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
        // v0.5.2 (R-UNDO): `/undo list`, `/undo <id>` and `/undo <id> confirm` are the Root's lines too
        // (plain `/undo` is the TUI's `undo` request, access.rs)
        _ if t.starts_with("/undo ") => return LineKind::RootCommand,
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

/// The owner commands of §3.8 for the completion popup, with one-line descriptions. v0.5.1 (user
/// sessions): ordered so that the safe, everyday commands come first and the ones that stop the agent
/// come last, `/panic` at the very end: Tab on a bare `/` completes the first candidate, and a
/// destructive command must never be one keystroke from it (`/panic` also asks to confirm, app.rs).
pub const COMMANDS: &[(&str, &str)] = &[
    ("/usage", "show metered cost and tokens locally"),
    ("/details", "print the last tool activity: each call, its result, its output's last lines, tokens per request"),
    ("/model", "list the models and switch (the Root restarts the agent)"),
    ("/new", "start a new session (body)"),
    ("/clear", "start a new session (same as /new)"),
    ("/compact", "compact the context: /compact [focus]"),
    ("/status", "the kernel's status line"),
    ("/workspace", "the workspace and the agent's access"),
    ("/later ", "queue a follow-up: /later <text>"),
    ("/stop", "stop the current run (kernel)"),
    ("/undo", "restore the workspace's last snapshot (asks to confirm)"),
    ("/undo list", "the workspace's snapshots: id, time, what took each one"),
    ("/undo ", "restore one snapshot: /undo <id> (ids from /undo list)"),
    ("/help", "list these commands"),
    ("/root status", "the Root's status"),
    ("/root help", "the Root's lines"),
    ("/root ledger", "the autonomy ledger"),
    ("/key", "set a provider's API key (hidden; kept by the Root)"),
    ("/setup", "choose a provider, key and model"),
    ("/goal ", "the owner's goal (Root): /goal \"<intent>\" done=card:cost_turn<=N usd=1.00 wall=24h rounds=10 guard=<card>; /goal stop|pause|continue"),
    ("/full off", "confine this session: write only the workspace, no network but the provider"),
    ("/full on", "full access: read and write anywhere except INIT's Root, network (the default)"),
    ("/kernel", "the kernel's commands"),
    ("/accept ", "accept a task: /accept <task>"),
    ("/bless ", "bless a job: /bless <job>"),
    ("/reject ", "reject a job: /reject <job>"),
    ("/branch ", "branch: /branch <id> [note]"),
    ("/label ", "label an entry: /label <id> <text>"),
    ("/resume", "leave panic, halt or stop (Root), or resume the kernel"),
    ("/root stop", "stop the agent (Root; /resume starts it again, no demotion)"),
    ("/panic", "emergency stop: kills every agent process and demotes a level (asks to confirm)"),
];

/// The first word of every line some part of INIT reads as a command (v0.5.2): the TUI's own, the
/// Root's (root link.rs `line`, its sentinel, goal, grant, twin and access lines), the kernel's
/// (supervise.md §4.6) and the body's (log-context §4.5). A line starting with any other `/word` is
/// not sent at once ([`slash`]).
pub const KNOWN_WORDS: &[&str] = &[
    // the TUI
    "/usage", "/details", "/help", "/model", "/key", "/setup", "/clear", "/panic",
    // the Root
    "/root", "/resume", "/workspace", "/full", "/undo", "/rollback", "/grants", "/grant", "/deny", "/release", "/twin", "/goal",
    // the kernel
    "/stop", "/status", "/kernel", "/accept", "/bless", "/reject", "/later",
    // the body
    "/new", "/compact", "/branch", "/label",
];

/// The body's own words (log-context §4.5, seed/src/loop/mod.rs `is_command`): an evolved body may
/// change what they take, or add a `/new-…` of its own, so the TUI never judges them (v0.5.3).
pub const BODY_WORDS: &[&str] = &["/new", "/compact", "/branch", "/label"];

/// `/help` (v0.5.2): every command with its description, for the TUI's table and line mode.
pub fn help_rows() -> (String, Vec<(String, String)>) {
    // a command that takes an argument (`/later `, `/undo `) reads `/later …`
    let rows = COMMANDS.iter().map(|(c, d)| (if c.ends_with(' ') { format!("{c}…") } else { c.to_string() }, d.to_string())).collect();
    ("commands (type / for the list as you type; // at the start sends a line beginning with / as a message)".to_string(), rows)
}

/// How the TUI treats a line whose first word looks like a command (§3.8; v0.5.2, reworked in v0.5.3
/// after the v0.5.2 post-release review, finding 10). No frame tells the TUI the body's commands (the
/// owner channel carries display, echo, notice, state and replay_end; the body's are in its own code),
/// so a `/word` it does not know may be a typo or a command an evolved body added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slash {
    /// Sent (or handled) as usual: not a `/word`, a command as its grammar takes it, a body word, a
    /// line of several lines, or a path (`/tmp is full`).
    Pass,
    /// Never sent as it is: a built-in with an argument its fixed grammar refuses (`/stop now`,
    /// `/panic now`, `/full of`: the Root and the kernel match those lines exactly, so today they reach
    /// the model, are billed and stop nothing), or a built-in in another case or joined with `-`
    /// (`/Help`, `/undo-list`). `head` is what the owner typed, `why` the right form.
    Refuse { head: String, why: String },
    /// A `/word` the TUI does not know: not sent on the first Enter; the same line again sends it.
    /// `near` is the closest built-in when one is a typo away (`/hepl`: `/help`).
    Ask { word: String, near: Option<&'static str> },
}

impl Slash {
    /// The TUI's answer, without its "(nothing was sent)" or "(not sent)"; None for [`Slash::Pass`].
    pub fn text(&self, line_mode: bool) -> Option<String> {
        match self {
            Slash::Pass => None,
            Slash::Refuse { head, why } => {
                Some(format!("{head}: {why} — /help lists the commands; start the line with // to send it as a message"))
            }
            Slash::Ask { word, near } => Some(format!(
                "{word}: not a TUI command{} — {} to send it to the model as text (it may be a command your agent added); /help lists the built-in ones",
                near.map(|n| format!("; did you mean {n}?")).unwrap_or_default(),
                if line_mode { "send the same line again" } else { "Enter again" }
            )),
        }
    }
}

/// Whether `p` names something on this machine: a stat, no file opened (T-2). [`slash`] asks it only
/// for a one-segment `/word` (`/tmp`, `/Users`).
pub fn path_exists(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// What a `/word` may end in and still be read as that word (v0.5.3, the release review): `/stop.`,
/// `/help?`, `/undo! list`.
const TRAILING: &[char] = &['.', ',', '!', '?', ';', ':', '…'];

/// v0.5.3: what the TUI does with `text` before sending it ([`Slash`]). In order: a line whose first
/// word is not `/`, a letter, then letters, digits, `-` or `_` (a path such as `/usr/bin/x`, `//…`,
/// prose) passes; a built-in with an argument its fixed grammar refuses is refused; a built-in in
/// another case or joined with `-`/`_` is refused; a word `exists` as a path passes; anything else is
/// asked once. `exists` is [`path_exists`] in the TUI. A word with trailing punctuation that is no
/// path (`/stop.`, `/help?`) is judged without it, and a built-in so written is refused with its form:
/// the Root and the kernel match their words exactly, so such a line reached the model and was billed.
pub fn slash(text: &str, exists: &dyn Fn(&str) -> bool) -> Slash {
    let t = text.trim();
    let word = t.split(char::is_whitespace).next().unwrap_or("");
    let Some(name) = word.strip_prefix('/') else { return Slash::Pass };
    let looks = |n: &str| n.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let head = if t.len() <= 40 && !t.contains('\n') { t.to_string() } else { format!("{word} …") };
    if !looks(name) {
        let bare = name.trim_end_matches(TRAILING);
        if bare.len() == name.len() || !looks(bare) || exists(word) {
            return Slash::Pass;
        }
        let line = format!("/{bare}{}", &t[word.len()..]);
        return match slash(&line, exists) {
            // a message of several lines that starts with a command is text, as without the mark
            Slash::Pass if KNOWN_WORDS.contains(&&line[..bare.len() + 1]) && !t.contains('\n') => {
                Slash::Refuse { head, why: format!("did you mean {line}?") }
            }
            Slash::Pass => Slash::Pass,
            Slash::Refuse { why, .. } => Slash::Refuse { head, why },
            Slash::Ask { near, .. } => Slash::Ask { word: word.to_string(), near },
        };
    }
    if KNOWN_WORDS.contains(&word) {
        // a message of several lines that starts with a command is text to the Root and the kernel too
        return match bad_argument(word, t).filter(|_| !t.contains('\n')) {
            // `/root` is also a directory on Linux: a line about it that is no near miss is text
            Some((why, near)) if near || !exists(word) => Slash::Refuse { head, why },
            _ => Slash::Pass,
        };
    }
    let rest = t[word.len()..].trim_start();
    let lower = word.to_ascii_lowercase();
    if KNOWN_WORDS.contains(&lower.as_str()) {
        let line = if rest.is_empty() { lower.clone() } else { format!("{lower} {rest}") };
        let fixed = if slash(&line, exists) == Slash::Pass { line } else { lower };
        return Slash::Refuse { head, why: format!("commands are lowercase; did you mean {fixed}?") };
    }
    if let Some((first, tail)) = lower.split_once(['-', '_'])
        && KNOWN_WORDS.contains(&first)
        && !BODY_WORDS.contains(&first)
    {
        let spaced = [first, &tail.replace(['-', '_'], " "), rest].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
        let fixed = if slash(&spaced, exists) == Slash::Pass { spaced } else { first.to_string() };
        return Slash::Refuse { head, why: format!("did you mean {fixed}?") };
    }
    if exists(word) {
        return Slash::Pass;
    }
    Slash::Ask { word: word.to_string(), near: nearest(&lower) }
}

/// The built-in a typo away from `word` (lowercase): at an OSA distance of 1, or 2 for a name of six
/// letters or more ([`closest`]).
fn nearest(word: &str) -> Option<&'static str> {
    closest(word, KNOWN_WORDS, if word.len() > 6 { 2 } else { 1 })
}

/// Of `forms`, the one closest to `t` within `max` edits: on a tie, one that `t` begins (`/full of`:
/// `/full off`, not `/full on`), then the first.
fn closest(t: &str, forms: &[&'static str], max: usize) -> Option<&'static str> {
    forms.iter().map(|f| (osa(t, f), !f.starts_with(t), *f)).filter(|(d, ..)| *d <= max).min_by_key(|(d, p, _)| (*d, *p)).map(|(.., f)| f)
}

/// The fixed grammars (v0.5.3): how the TUI's own commands, the Root's (link.rs `line`, access.rs
/// `line`, grant.rs `["/grants"]`) and the kernel's (supervisor.rs `owner_in`) take a line, exactly.
/// Any other line with these words falls through to the model. `t` is trimmed and starts with `word`.
/// Some((why, a near miss was found)) when the line is refused. The body's words, `/model` and `/key`
/// (the TUI's setup reads their argument) and the Root's lines that take any argument (`/goal`, `/twin`,
/// `/rollback`, `/grant`, `/deny`, `/release`, `/undo <id>`) are not judged, except a typo of `/undo list`.
fn bad_argument(word: &str, t: &str) -> Option<(String, bool)> {
    let rest = t[word.len()..].trim_start();
    let near = |forms: &[&'static str]| closest(t, forms, 2);
    let with = |why: &str, n: Option<&str>| (n.map_or_else(|| why.to_string(), |n| format!("{why}; did you mean {n}?")), n.is_some());
    match word {
        "/usage" | "/details" | "/help" | "/clear" | "/panic" | "/stop" | "/status" | "/kernel" | "/resume" | "/workspace" | "/grants" | "/setup" => {
            (t != word).then(|| (format!("{word} takes no argument: type {word} alone"), true))
        }
        "/root" => {
            let forms = ["/root", "/root status", "/root ledger", "/root stop", "/root help"];
            (!forms.contains(&t)).then(|| with("/root takes status, ledger, stop or help", near(&forms[1..])))
        }
        "/full" => {
            let forms = ["/full", "/full on", "/full off", "/full on confirm"];
            (!forms.contains(&t)).then(|| with("/full takes on or off (/full alone shows the access)", near(&forms[1..])))
        }
        "/accept" => rest.is_empty().then(|| ("/accept takes a task id: /accept <task>".to_string(), true)),
        "/bless" | "/reject" => rest.is_empty().then(|| (format!("{word} takes a job id: {word} <job>"), true)),
        "/later" => (rest.is_empty() || !t.starts_with("/later ")).then(|| ("/later takes the text to queue: /later <text>".to_string(), true)),
        "/undo" => {
            // the Root takes `/undo <anything>` (an unknown id gets its answer, and the sentinel takes it
            // as a correction of the current turn); a typo of `list` or `confirm` is caught here first
            let a = rest.split_whitespace().next().unwrap_or("");
            let fix = closest(a, &["list", "confirm"], 2).filter(|f| a != *f && a.len() <= 9);
            fix.map(|f| (format!("did you mean /undo {f}? (/undo takes nothing, list, or a snapshot id from /undo list)"), true))
        }
        _ => None,
    }
}

/// The optimal string alignment distance: Levenshtein with transpositions of neighbours, in chars.
fn osa(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d: Vec<Vec<usize>> = (0..=a.len()).map(|i| (0..=b.len()).map(|j| if i == 0 { j } else if j == 0 { i } else { 0 }).collect()).collect();
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let mut v = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[a.len()][b.len()]
}

/// Commands that stop the agent (TUI-FIX, v0.5.1): listed last in [`COMMANDS`]; `/panic` also asks
/// for a typed confirmation in TTY mode (`app.rs`).
pub const STOPPING: &[&str] = &["/resume", "/root stop", "/panic"];

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

    /// v0.5.3 (the v0.5.2 post-release review, finding 10): the five classes of a `/word` line. A path
    /// (one segment or more), a `//` line, prose and every command as its grammar takes it pass; a
    /// built-in with an argument its fixed grammar refuses, or in another case, or joined with `-`, is
    /// refused with the right form; any other `/word` is asked once, with a built-in a typo away.
    /// Every completion's first word is a known one.
    #[test]
    fn slash_lines_are_classified() {
        let none = |_: &str| false;
        let fs = |p: &str| matches!(p, "/tmp" | "/Users" | "/root");
        let refused = |t: &str, e: &dyn Fn(&str) -> bool| match slash(t, e) {
            Slash::Refuse { why, .. } => why,
            s => panic!("{t}: {s:?}"),
        };
        let asked = |t: &str| match slash(t, &none) {
            Slash::Ask { word, near } => (word, near),
            s => panic!("{t}: {s:?}"),
        };
        for ok in ["/undo list", "/undo 20261028T111910.728Z-project", "/undo 20261028T111910.728Z-project confirm", "/undo confirm",
            "/usr/bin/python3 crashed", "/Users/x/file.go:12 panics", "//undo list", "//stop now", "/ x", "/", "hello /foo", "/later do x",
            "/rollback lkg", "/grant g1 for 2d confirm", "/twin refreeze", "/goal stop", "/goal \"cheaper\" usd=1", "/branch e1 note",
            "/label e1 x", "/compact focus", "/new", "/new session please", "/root help", "/root", "/full on confirm", "/full", "/help",
            "/details", "/über", "/1st", "/model 2", "/key deepseek", "/accept t1", "/reject j1", "/stop", " /stop ", "/stop\nmore lines",
            "/grants", "/deny g1"] {
            assert_eq!(slash(ok, &none), Slash::Pass, "{ok}");
        }
        // 4. a one-segment path goes out as text
        for p in ["/tmp", "/tmp is full", "/Users"] {
            assert_eq!(slash(p, &fs), Slash::Pass, "{p}");
            assert!(matches!(slash(p, &none), Slash::Ask { .. }), "{p}: no such path, asked");
        }
        // 2. a built-in with an argument its fixed grammar refuses
        assert_eq!(refused("/stop now", &none), "/stop takes no argument: type /stop alone");
        assert_eq!(refused("/panic now", &none), "/panic takes no argument: type /panic alone");
        for w in ["/usage", "/details", "/help", "/clear", "/status", "/kernel", "/resume", "/workspace", "/grants", "/setup"] {
            assert!(refused(&format!("{w} x"), &none).starts_with(&format!("{w} takes no argument")), "{w}");
        }
        assert_eq!(refused("/root stauts", &none), "/root takes status, ledger, stop or help; did you mean /root status?");
        assert_eq!(refused("/root  status", &none), "/root takes status, ledger, stop or help; did you mean /root status?", "the Root matches exactly");
        assert_eq!(refused("/root foo", &none), "/root takes status, ledger, stop or help");
        assert_eq!(refused("/full of", &none), "/full takes on or off (/full alone shows the access); did you mean /full off?");
        assert!(refused("/full onn", &none).ends_with("did you mean /full on?"));
        assert!(refused("/full on  confirm", &none).ends_with("did you mean /full on confirm?"));
        assert_eq!(refused("/accept", &none), "/accept takes a task id: /accept <task>");
        assert_eq!(refused("/bless", &none), "/bless takes a job id: /bless <job>");
        assert_eq!(refused("/later", &none), "/later takes the text to queue: /later <text>");
        assert!(refused("/undo lst", &none).starts_with("did you mean /undo list?"));
        assert!(refused("/undo confrim", &none).starts_with("did you mean /undo confirm?"));
        // `/root` is a directory on Linux: a line about it passes, a near miss of a /root line does not
        assert_eq!(slash("/root has the wrong owner", &fs), Slash::Pass);
        assert!(refused("/root stauts", &fs).ends_with("did you mean /root status?"));
        // 3. another case, or joined with `-` or `_`
        assert_eq!(refused("/Help", &none), "commands are lowercase; did you mean /help?");
        assert_eq!(refused("/Undo list", &none), "commands are lowercase; did you mean /undo list?");
        assert_eq!(refused("/STOP now", &none), "commands are lowercase; did you mean /stop?");
        assert_eq!(refused("/undo-list", &none), "did you mean /undo list?");
        assert_eq!(refused("/root_status", &none), "did you mean /root status?");
        assert_eq!(refused("/full-on", &none), "did you mean /full on?");
        assert_eq!(refused("/stop-now", &none), "did you mean /stop?");
        assert_eq!(refused("/goal-stop", &none), "did you mean /goal stop?");
        // 5. anything else: asked once, with a built-in a typo away (1 edit; 2 for six letters or more)
        assert_eq!(asked("/hepl"), ("/hepl".into(), Some("/help")));
        assert_eq!(asked("/stpo now"), ("/stpo".into(), Some("/stop")));
        assert_eq!(asked("/stats"), ("/stats".into(), Some("/status")));
        assert_eq!(asked("/labels e1"), ("/labels".into(), Some("/label")));
        assert_eq!(asked("/workspcae"), ("/workspcae".into(), Some("/workspace")));
        assert_eq!(asked("/wrkspcx"), ("/wrkspcx".into(), None), "three edits");
        assert_eq!(asked("/myskill run"), ("/myskill".into(), None));
        assert_eq!(asked("/foo bar"), ("/foo".into(), None));
        assert_eq!(asked("/new-session"), ("/new-session".into(), None), "the body's words are the body's to extend");
        assert_eq!(asked("/hepl\nmore"), ("/hepl".into(), Some("/help")));
        // the answers
        let s = slash("/stop now", &none).text(false).unwrap();
        assert_eq!(s, "/stop now: /stop takes no argument: type /stop alone — /help lists the commands; start the line with // to send it as a message");
        assert_eq!(slash("/hepl", &none).text(true).unwrap(), "/hepl: not a TUI command; did you mean /help? — send the same line again to send it to \
            the model as text (it may be a command your agent added); /help lists the built-in ones");
        assert!(slash("/foo", &none).text(false).unwrap().starts_with("/foo: not a TUI command — Enter again to send it"));
        assert_eq!(slash("hi", &none).text(false), None);
        // every completion's first word is a known one, and every completion passes as it is completed
        for (c, _) in COMMANDS {
            let w = c.split_whitespace().next().unwrap();
            assert!(KNOWN_WORDS.contains(&w), "the completion {c} is a known command");
            if !c.ends_with(' ') {
                assert_eq!(slash(c, &none), Slash::Pass, "the completion {c}");
            }
        }
        // R-UNDO's lines are the Root's, never chat
        for l in ["/undo list", "/undo 20261028T111910.728Z-project", "/undo 20261028T111910.728Z-project confirm"] {
            assert_eq!(line_kind(l, false), LineKind::RootCommand, "{l}");
        }
        assert!(COMMANDS.iter().any(|c| c.0 == "/undo list") && COMMANDS.iter().any(|c| c.0 == "/undo "));
        assert!(path_exists("/") && !path_exists("/nonexistent-i5-tui-path"));
    }

    /// v0.5.3 (the release review): `/stop.`, `/help?` and `/undo!` went to the model and were billed, since
    /// only letters, digits, `-` and `_` made a `/word`. A word with trailing punctuation is judged without it:
    /// a built-in is refused with its form, a typo is asked, and a path, with the mark or without it, passes.
    #[test]
    fn trailing_punctuation_is_judged_without_it() {
        let none = |_: &str| false;
        let fs = |p: &str| matches!(p, "/tmp" | "/etc." | "/root");
        let refused = |t: &str, e: &dyn Fn(&str) -> bool| match slash(t, e) {
            Slash::Refuse { head, why } => {
                assert_eq!(head, t.trim(), "{t}");
                why
            }
            s => panic!("{t}: {s:?}"),
        };
        for (t, why) in [("/stop.", "did you mean /stop?"), ("/help?", "did you mean /help?"), ("/undo!", "did you mean /undo?"),
            ("/status!!", "did you mean /status?"), (" /panic! ", "did you mean /panic?"), ("/resume…", "did you mean /resume?"),
            ("/undo! list", "did you mean /undo list?"), ("/root, status", "did you mean /root status?"), ("/new.", "did you mean /new?"),
            ("/stop. now", "/stop takes no argument: type /stop alone"), ("/STOP.", "commands are lowercase; did you mean /stop?"),
            ("/undo-list?", "did you mean /undo list?"), ("/root.", "did you mean /root?")] {
            assert_eq!(refused(t, &none), why, "{t}");
        }
        assert_eq!(refused("/root.", &fs), "did you mean /root?", "`/root.` is no path even where /root is one");
        assert_eq!(slash("/stop.", &none).text(false).unwrap(),
            "/stop.: did you mean /stop? — /help lists the commands; start the line with // to send it as a message");
        // a typo with a mark is asked, as without it
        assert_eq!(slash("/hepl?", &none), Slash::Ask { word: "/hepl?".into(), near: Some("/help") });
        assert_eq!(slash("/foo!", &none), Slash::Ask { word: "/foo!".into(), near: None });
        // paths, prose and lines of several lines pass as before
        for ok in ["/tmp/x.", "/usr/bin/python3?", "/tmp.", "/tmp, then /var", "/etc. is full", "/./x", "/.", "/!", "/1st.", "/über!",
            "/help?\nmore lines", "//stop.", "/x/stop."] {
            let e: &dyn Fn(&str) -> bool = if ok.starts_with("/tmp") || ok.starts_with("/etc") { &fs } else { &none };
            assert_eq!(slash(ok, e), Slash::Pass, "{ok}");
        }
        assert!(matches!(slash("/tmp.", &none), Slash::Ask { .. }), "no such path: asked, as /tmp is");
    }

    #[test]
    fn edit_distance() {
        assert_eq!(osa("/hepl", "/help"), 1, "a transposition is one edit");
        assert_eq!(osa("/stats", "/status"), 1);
        assert_eq!(osa("", "list"), 4);
        assert_eq!(osa("/undo", "/undo"), 0);
        assert_eq!(osa("/todo", "/undo"), 2);
    }

    #[test]
    fn clear_is_new() {
        assert_eq!(canonical(" /clear "), "/new");
        assert_eq!(canonical("/new"), "/new");
        assert_eq!(canonical("/clear all"), "/clear all", "only the exact command");
        assert_eq!(line_kind(&canonical("/clear"), false), line_kind("/new", false));
        assert!(COMMANDS.iter().any(|c| c.0 == "/clear") && COMMANDS.iter().any(|c| c.0 == "/new"));
    }

    /// TUI-FIX (v0.5.1 user session): Tab on a bare `/` took the first command, which was `/panic`.
    /// The commands that stop the agent are last, `/panic` at the very end, and every command before
    /// them is a safe one.
    #[test]
    fn stopping_commands_come_last() {
        let n = COMMANDS.len();
        assert_eq!(COMMANDS[n - 1].0, "/panic", "the kill switch is the last completion");
        let tail: Vec<&str> = COMMANDS[n - STOPPING.len()..].iter().map(|c| c.0).collect();
        assert_eq!(tail, STOPPING, "the commands that stop the agent come after every other one");
        assert!(!STOPPING.contains(&COMMANDS[0].0) && COMMANDS[0].0 == "/usage", "the first completion is a safe, read-only one");
        assert!(COMMANDS.iter().all(|c| !c.1.contains("no confirmation")), "no description invites a one-keystroke stop");
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
