//! ADR-009 from the owner's side (build/specs/ADR-009-spec.md; the Root's protocol is
//! build/status/SBX-009.md §A–§C): the TUI's cwd becomes the workspace right after the first
//! attach (`workspace.set`, before any owner line), `init --full` and `/full on|off` set full mode
//! (`full.set`), and `/undo` restores the workspace's last snapshot (`undo`). ADR-012: full access is
//! the charter's default; `init --workspace` sends `workspace.set{cwd, mode:"workspace"}` to confine
//! this session to the workspace. The Root asks for a typed confirmation (`root.confirm` with
//! `data.phrase`) only when it wants one (`/full on` under a charter `sandbox.mode = "workspace"`,
//! `/undo`); the TUI shows its message, reads the owner's next line and resends the request with
//! `confirm` set to that line.
//!
//! A pure state machine: lines and answers in, [`Out`]s for the front end (TTY or line mode).
//! The requests go on their own connection (`conn::spawn_access`), never on the attach connection.

use crate::conn::ReqErr;
use serde_json::{Value, json};
use std::time::Duration;

/// The no-workspace notice (spec item 2).
pub const NO_WORKSPACE: &str = "no workspace: cd into a project and run `init` to let me edit it";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Workspace,
    Full(bool),
    Undo,
}

impl Op {
    pub fn cmd(&self) -> &'static str {
        match self {
            Op::Workspace => "workspace.set",
            Op::Full(_) => "full.set",
            Op::Undo => "undo",
        }
    }
    /// A workspace change and an undo clone the workspace (APFS clones, up to 2 GiB) before the
    /// Root answers, so the bound is generous.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(120)
    }
    fn label(&self) -> &'static str {
        match self {
            Op::Workspace => "workspace",
            Op::Full(true) => "/full on",
            Op::Full(false) => "/full off",
            Op::Undo => "/undo",
        }
    }
}

/// One request: the op and its ICJ `args`.
#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub op: Op,
    pub args: Value,
}

fn req(op: Op, confirm: Option<&str>) -> Req {
    let mut args = match op {
        Op::Full(on) => json!({"on": on}),
        _ => json!({}),
    };
    if let Some(c) = confirm {
        args["confirm"] = json!(c);
    }
    Req { op, args }
}

/// What the front end does.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Send this request (`conn::spawn_access`).
    Send(Req),
    /// The Root's `AccessView` from an answer: the status bar's workspace and `FULL`.
    Access(Value),
    /// A warning-level notice from the TUI (the no-workspace notice).
    Notice(String),
    Info(String),
    Warn(String),
    Error(String),
    /// Ask the owner to type the phrase (the next line answers it).
    Ask(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ws {
    NotSent,
    InFlight,
    Done,
}

#[derive(Debug)]
pub struct Flow {
    /// The TUI's cwd as the OS gives it; None when it could not be read.
    cwd: Option<String>,
    want_full: bool,
    /// `init --workspace` (ADR-012): this session is confined to the workspace.
    want_workspace: bool,
    ws: Ws,
    inflight: Option<Op>,
    confirm: Option<(Op, String)>,
}

fn err_text(e: &ReqErr) -> String {
    match e {
        ReqErr::Unreachable(e) => format!("the Root is unreachable ({e})"),
        ReqErr::Proto(m) => m.clone(),
        ReqErr::Refused(c, m, _) => format!("{c}: {m}"),
    }
}

/// `~` for the home prefix (display only).
pub fn tilde(p: &str, home: Option<&str>) -> String {
    match home.filter(|h| h.len() > 1) {
        Some(h) if p == h => "~".into(),
        Some(h) => match p.strip_prefix(h).filter(|r| r.starts_with('/')) {
            Some(r) => format!("~{r}"),
            None => p.to_string(),
        },
        None => p.to_string(),
    }
}

impl Flow {
    /// `want_full` (`--full`) and `want_workspace` (`--workspace`) exclude each other (`main`
    /// refuses both); `want_full` wins if both are set.
    pub fn new(cwd: Option<String>, want_full: bool, want_workspace: bool) -> Flow {
        let want_workspace = want_workspace && !want_full;
        Flow { cwd, want_full, want_workspace, ws: Ws::NotSent, inflight: None, confirm: None }
    }

    /// Owner lines wait while the workspace is being set (SBX-009 §A: before any owner line) and
    /// while an access request is in flight (its confirmation may need the next line).
    pub fn holds_lines(&self) -> bool {
        self.ws == Ws::InFlight || self.inflight.is_some()
    }

    /// An access request is in flight or the owner owes a confirmation.
    pub fn busy(&self) -> bool {
        self.holds_lines() || self.confirm.is_some()
    }

    /// The phrase the next line must equal, while a confirmation is pending.
    pub fn awaiting(&self) -> Option<&str> {
        self.confirm.as_ref().map(|(_, p)| p.as_str())
    }

    /// The first attach (later attaches send nothing: once per `init` launch).
    pub fn on_attached(&mut self) -> Vec<Out> {
        if self.ws != Ws::NotSent {
            return vec![];
        }
        match self.cwd.clone() {
            Some(cwd) => {
                self.ws = Ws::InFlight;
                self.inflight = Some(Op::Workspace);
                let mut args = json!({"cwd": cwd});
                if self.want_workspace {
                    args["mode"] = json!("workspace");
                }
                vec![Out::Send(Req { op: Op::Workspace, args })]
            }
            None => {
                self.ws = Ws::Done;
                let mut out = vec![Out::Warn("cannot read the current directory, so there is no workspace".into())];
                out.extend(self.then_full());
                out
            }
        }
    }

    /// After the workspace answer: `--full` asks for full mode. `--workspace` without a cwd (no
    /// `workspace.set` was sent) turns full mode off instead.
    fn then_full(&mut self) -> Vec<Out> {
        let confine = std::mem::take(&mut self.want_workspace) && self.cwd.is_none();
        let on = std::mem::take(&mut self.want_full);
        if !on && !confine {
            return vec![];
        }
        self.inflight = Some(Op::Full(on));
        vec![Out::Send(req(Op::Full(on), None))]
    }

    /// A line the owner submitted. `Some`: the access flow took it (a confirmation answer or an
    /// access command), so it is not sent as a line; `None`: an ordinary line.
    pub fn on_line(&mut self, text: &str) -> Option<Vec<Out>> {
        let t = text.trim();
        if let Some((op, phrase)) = self.confirm.take() {
            if t == phrase {
                self.inflight = Some(op);
                return Some(vec![Out::Send(req(op, Some(t)))]);
            }
            return Some(vec![Out::Info(format!("{}: not confirmed; nothing changed", op.label()))]);
        }
        let op = match t {
            "/undo" => Op::Undo,
            "/full on" => Op::Full(true),
            "/full off" => Op::Full(false),
            _ => return None,
        };
        if self.inflight.is_some() {
            return Some(vec![Out::Warn(format!("{}: another access request is still running; try again in a moment", op.label()))]);
        }
        self.inflight = Some(op);
        Some(vec![Out::Send(req(op, None))])
    }

    /// The Root's answer to `op`.
    pub fn on_answer(&mut self, op: Op, r: Result<Value, ReqErr>) -> Vec<Out> {
        if self.inflight == Some(op) {
            self.inflight = None;
        }
        let mut out = Vec::new();
        match (op, r) {
            (Op::Workspace, Ok(v)) => {
                self.ws = Ws::Done;
                out.push(Out::Access(v["access"].clone()));
                // A change is announced by the Root itself (`code:"access"` notices to every
                // client); an unchanged "no workspace" is not, so the TUI says it.
                if v["access"]["workspace"].is_null() && v["changed"] != true {
                    let why = v["refused"]["message"].as_str().map(|m| format!(" ({m})")).unwrap_or_default();
                    out.push(Out::Notice(format!("{NO_WORKSPACE}{why}")));
                }
                out.extend(self.then_full());
            }
            (Op::Workspace, Err(e)) => {
                self.ws = Ws::Done;
                match &e {
                    // a Root that predates ADR-009: nothing changes for the owner (v0.1 access)
                    ReqErr::Refused(c, ..) if c == "root.unknown_cmd" => {}
                    _ => out.push(Out::Warn(format!("could not set the workspace: {}", err_text(&e)))),
                }
                out.extend(self.then_full());
            }
            (Op::Full(_), Ok(v)) => out.push(Out::Access(v["access"].clone())),
            (Op::Undo, Ok(v)) => {
                let r = &v["restored"];
                let s = |x: &Value, k: &str| x[k].as_str().unwrap_or("?").to_string();
                out.push(Out::Info(format!(
                    "/undo: restored snapshot {} ({}) of {}; the state before it is kept as snapshot {}, so /undo again goes back",
                    s(r, "id"),
                    s(r, "at"),
                    s(r, "workspace"),
                    s(&v["saved"], "id")
                )));
            }
            (op, Err(ReqErr::Refused(code, msg, data))) if code == "root.confirm" => match data["phrase"].as_str().filter(|p| !p.trim().is_empty()) {
                Some(phrase) => {
                    let phrase = phrase.trim().to_string();
                    out.push(Out::Info(msg));
                    out.push(Out::Ask(format!("type {phrase} and press Enter to go ahead with {}; anything else cancels", op.label())));
                    self.confirm = Some((op, phrase));
                }
                None => out.push(Out::Error(format!("{}: the Root asked for a confirmation without a phrase", op.label()))),
            },
            (Op::Undo, Err(ReqErr::Refused(code, _, data))) if code == "root.mode" && data["reason"] == "no_workspace" => {
                out.push(Out::Warn(format!("/undo: {NO_WORKSPACE}")))
            }
            (Op::Undo, Err(ReqErr::Refused(code, ..))) if code == "root.busy" => {
                out.push(Out::Warn("/undo: the agent is in the middle of a turn; try again when it is idle (Esc interrupts it)".into()))
            }
            (Op::Undo, Err(ReqErr::Refused(code, ..))) if code == "root.not_found" => out.push(Out::Warn("/undo: there is no snapshot of this workspace to restore".into())),
            (Op::Full(_), Err(ReqErr::Refused(code, _, data))) if code == "root.mode" && data["reason"] == "charter" => {
                out.push(Out::Warn("full mode is disabled by the charter (sandbox.allow_full = false)".into()))
            }
            (op, Err(e)) => out.push(Out::Error(format!("{}: {}", op.label(), err_text(&e)))),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(code: &str, data: Value) -> Result<Value, ReqErr> {
        Err(ReqErr::Refused(code.into(), "msg".into(), data))
    }

    fn view(ws: Option<&str>, full: bool) -> Value {
        json!({"access": if full { "full" } else if ws.is_some() { "workspace" } else { "read_home" }, "allow_full": true, "full": full,
               "mode": "full", "read_home": true, "restart_pending": false, "snapshot": null, "workspace": ws})
    }

    #[test]
    fn workspace_first_then_lines() {
        let mut f = Flow::new(Some("/Users/o/proj".into()), false, false);
        assert!(!f.holds_lines());
        let out = f.on_attached();
        assert_eq!(out, [Out::Send(Req { op: Op::Workspace, args: json!({"cwd": "/Users/o/proj"}) })]);
        assert!(f.holds_lines(), "no owner line before the workspace answer");
        assert!(f.on_attached().is_empty(), "once per launch");
        let out = f.on_answer(Op::Workspace, Ok(json!({"access": view(Some("/Users/o/proj"), false), "changed": true, "refused": null})));
        assert_eq!(out, [Out::Access(view(Some("/Users/o/proj"), false))]);
        assert!(!f.holds_lines());
        assert_eq!(f.on_line("hello"), None);
    }

    #[test]
    fn no_workspace_notice_only_when_the_root_does_not_announce_it() {
        let mut f = Flow::new(Some("/Users/o".into()), false, false);
        f.on_attached();
        let home = json!({"access": view(None, false), "changed": false, "refused": {"code": "home", "message": "the home itself"}});
        let out = f.on_answer(Op::Workspace, Ok(home));
        assert_eq!(out[1], Out::Notice(format!("{NO_WORKSPACE} (the home itself)")));
        let mut f = Flow::new(Some("/Users/o".into()), false, false);
        f.on_attached();
        let changed = json!({"access": view(None, false), "changed": true, "refused": {"code": "home", "message": "x"}});
        assert_eq!(f.on_answer(Op::Workspace, Ok(changed)).len(), 1, "the Root's code:access notice says it");
    }

    #[test]
    fn full_flag_follows_the_workspace_and_confirms() {
        let mut f = Flow::new(Some("/p".into()), true, false);
        f.on_attached();
        let out = f.on_answer(Op::Workspace, Ok(json!({"access": view(Some("/p"), false), "changed": false})));
        assert_eq!(out.last(), Some(&Out::Send(Req { op: Op::Full(true), args: json!({"on": true}) })));
        assert!(f.holds_lines());
        let out = f.on_answer(Op::Full(true), refused("root.confirm", json!({"phrase": "confirm"})));
        assert_eq!(out[0], Out::Info("msg".into()));
        assert!(matches!(&out[1], Out::Ask(a) if a.starts_with("type confirm ")));
        assert_eq!(f.awaiting(), Some("confirm"));
        assert!(f.busy() && !f.holds_lines(), "the next line is the answer");
        let out = f.on_line(" confirm ").unwrap();
        assert_eq!(out, [Out::Send(Req { op: Op::Full(true), args: json!({"confirm": "confirm", "on": true}) })]);
        let out = f.on_answer(Op::Full(true), Ok(json!({"access": view(Some("/p"), true), "changed": true, "snapshot": null})));
        assert_eq!(out, [Out::Access(view(Some("/p"), true))]);
        assert!(!f.busy());
    }

    /// ADR-012: `--workspace` confines the session in the `workspace.set` itself; `/full on` under a
    /// full-default charter is answered at once (no confirmation), and `--full` asks without one.
    #[test]
    fn workspace_flag_confines_and_full_needs_no_confirmation_by_default() {
        let mut f = Flow::new(Some("/p".into()), false, true);
        assert_eq!(f.on_attached(), [Out::Send(Req { op: Op::Workspace, args: json!({"cwd": "/p", "mode": "workspace"}) })]);
        let out = f.on_answer(Op::Workspace, Ok(json!({"access": view(Some("/p"), false), "changed": true})));
        assert_eq!(out, [Out::Access(view(Some("/p"), false))], "no full.set follows --workspace");
        assert!(!f.busy());
        assert_eq!(f.on_line("/full on").unwrap(), [Out::Send(Req { op: Op::Full(true), args: json!({"on": true}) })]);
        let out = f.on_answer(Op::Full(true), Ok(json!({"access": view(Some("/p"), true), "changed": true, "snapshot": null})));
        assert_eq!(out, [Out::Access(view(Some("/p"), true))]);
        assert!(!f.busy() && f.awaiting().is_none());
        // without a cwd there is no workspace.set, so --workspace turns full mode off directly
        let mut f = Flow::new(None, false, true);
        let out = f.on_attached();
        assert!(matches!(&out[0], Out::Warn(_)));
        assert_eq!(out[1], Out::Send(Req { op: Op::Full(false), args: json!({"on": false}) }));
        // the default launch sends no mode
        let mut f = Flow::new(Some("/p".into()), false, false);
        assert_eq!(f.on_attached(), [Out::Send(Req { op: Op::Workspace, args: json!({"cwd": "/p"}) })]);
        // --full wins over --workspace (main refuses both together)
        let mut f = Flow::new(Some("/p".into()), true, true);
        assert_eq!(f.on_attached(), [Out::Send(Req { op: Op::Workspace, args: json!({"cwd": "/p"}) })]);
    }

    #[test]
    fn undo_asks_then_restores_or_cancels() {
        let mut f = Flow::new(Some("/p".into()), false, false);
        f.on_attached();
        f.on_answer(Op::Workspace, Ok(json!({"access": view(Some("/p"), false), "changed": false})));
        assert_eq!(f.on_line("/undo").unwrap(), [Out::Send(Req { op: Op::Undo, args: json!({}) })]);
        assert!(f.holds_lines(), "the confirmation may need the next line");
        f.on_answer(Op::Undo, refused("root.confirm", json!({"phrase": "yes", "snapshot": {"id": "s1"}})));
        // a wrong answer cancels, and is not sent as a line
        assert_eq!(f.on_line("no").unwrap(), [Out::Info("/undo: not confirmed; nothing changed".into())]);
        assert_eq!(f.awaiting(), None);
        f.on_line("/undo");
        f.on_answer(Op::Undo, refused("root.confirm", json!({"phrase": "yes"})));
        assert_eq!(f.on_line("yes").unwrap(), [Out::Send(Req { op: Op::Undo, args: json!({"confirm": "yes"}) })]);
        let done = json!({"restored": {"at": "2026-10-20T21:00:00Z", "id": "s1", "reason": "session", "workspace": "/p"},
                          "saved": {"at": "2026-10-20T21:05:00Z", "id": "s2", "reason": "pre_undo", "workspace": "/p"}});
        let out = f.on_answer(Op::Undo, Ok(done));
        assert!(matches!(&out[0], Out::Info(t) if t.starts_with("/undo: restored snapshot s1 (2026-10-20T21:00:00Z) of /p") && t.contains("kept as snapshot s2")), "{out:?}");
    }

    #[test]
    fn refusals_and_old_roots() {
        let mut f = Flow::new(Some("/p".into()), true, false);
        f.on_attached();
        let out = f.on_answer(Op::Workspace, refused("root.unknown_cmd", Value::Null));
        assert!(matches!(&out[..], [Out::Send(Req { op: Op::Full(true), .. })]), "an older Root: silent; --full still asked: {out:?}");
        let out = f.on_answer(Op::Full(true), refused("root.mode", json!({"reason": "charter"})));
        assert!(matches!(&out[0], Out::Warn(t) if t.contains("allow_full")));
        f.on_line("/undo");
        assert!(matches!(&f.on_answer(Op::Undo, refused("root.mode", json!({"reason": "no_workspace"})))[0], Out::Warn(t) if t.contains("cd into a project")));
        f.on_line("/undo");
        assert!(matches!(&f.on_answer(Op::Undo, refused("root.not_found", Value::Null))[0], Out::Warn(t) if t.contains("no snapshot")));
        f.on_line("/undo");
        assert!(matches!(&f.on_answer(Op::Undo, refused("root.busy", Value::Null))[0], Out::Warn(t) if t.contains("when it is idle")));
        f.on_line("/full off");
        assert!(matches!(&f.on_line("/undo").unwrap()[0], Out::Warn(t) if t.contains("still running")));
        assert_eq!(f.on_answer(Op::Full(false), Ok(json!({"access": view(Some("/p"), false)}))), [Out::Access(view(Some("/p"), false))]);
        let mut f = Flow::new(None, false, false);
        assert!(matches!(&f.on_attached()[0], Out::Warn(_)));
        assert!(!f.holds_lines());
    }

    #[test]
    fn tilde_paths() {
        assert_eq!(tilde("/Users/o/proj", Some("/Users/o")), "~/proj");
        assert_eq!(tilde("/Users/o", Some("/Users/o")), "~");
        assert_eq!(tilde("/Users/other/x", Some("/Users/o")), "/Users/other/x");
        assert_eq!(tilde("/x", None), "/x");
    }
}
