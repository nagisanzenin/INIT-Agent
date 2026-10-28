//! TUI-PICKER (docs/tui.md §3.7.2): the picker's wiring in TTY mode. The picker itself (state, keys,
//! rows) is `render::picker`; this child module of `app` opens it for the setup flow and for the
//! agent's questions, and carries out what the owner chose:
//! - a setup choice leaves one committed line (`› provider · DeepSeek`) and goes back to the flow
//!   (`setup::Flow::on_pick`); Esc leaves the flow as `/cancel` does;
//! - an agent question's choice is sent as an ordinary owner line, the chosen labels as text, exactly
//!   as if the owner had typed it (`submit`): no new frame, request or Root action exists for it. The
//!   transcript keeps `· agent asked …` above the answer. Esc, "Type your own answer" or simply typing
//!   closes the picker; the agent keeps waiting, and the hint says so, until the owner sends a line.
//!
//! Nothing here has a timer: a question waits for the owner however long it takes (no default, no
//! auto-pick), and in a `/goal` run the agent stays idle until the answer arrives.

use super::App;
use crate::model::{Kind, OwnerSt};
use crate::render::picker::{Ask, PickOut, Picker, Who};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;
use std::time::Instant;

impl App {
    /// Opens a picker. The TUI's own (setup) comes first: an agent question open at that moment, or one
    /// that arrives while setup asks or the hidden key field is up, waits in `picker_next`.
    pub(super) fn open_picker(&mut self, p: Picker) {
        let busy = self.ed.picker.is_some() || self.secret.is_some();
        if p.who == Who::Agent && busy {
            if let Some(old) = self.ed.picker_next.replace(p) {
                self.info_block(format!("{} · replaced by a newer question", old.record()));
            }
        } else if p.who == Who::Tui && self.ed.picker.as_ref().is_some_and(|o| o.who == Who::Agent) {
            self.ed.picker_next = self.ed.picker.take();
            self.ed.picker = Some(p);
        } else {
            self.ed.picker = Some(p);
        }
        self.dirty = true;
    }

    /// After a picker closed (or the key field went away): the agent question that waited, if any.
    pub(super) fn next_picker(&mut self) {
        if self.ed.picker.is_none() && self.secret.is_none() && self.setup_idle() {
            self.ed.picker = self.ed.picker_next.take();
        }
    }

    fn setup_idle(&self) -> bool {
        !self.setup.busy() && !self.setup.secret_mode()
    }

    /// A display frame with an `ask` (from the agent, always): the picker opens. True when it was one
    /// (the frame then needs no other rendering: its `text` is the fallback for clients without pickers).
    pub(super) fn on_ask(&mut self, v: &Value, now: Instant) -> bool {
        let Some(a) = Ask::from_frame(v) else { return false };
        self.t.last_display = Some(now);
        if self.ed.picker.as_ref().is_some_and(|o| o.who == Who::Agent) {
            let old = self.ed.picker.take().expect("checked");
            self.info_block(format!("{} · replaced by a newer question", old.record()));
        }
        self.ed.waiting = None;
        self.open_picker(a.picker());
        true
    }

    /// Keys while a picker is open. Ctrl-C closes it and interrupts as usual; Ctrl-D closes it and
    /// quits; Ctrl-L redraws; Ctrl-O opens the transcript; the rest go to the picker.
    pub(super) fn picker_key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            match k.code {
                KeyCode::Char('c') => {
                    self.picked(PickOut::Cancel);
                    return self.ctrl_c();
                }
                KeyCode::Char('d') => {
                    self.picked(PickOut::Cancel);
                    return self.quit();
                }
                KeyCode::Char('l') => {
                    self.need_full = true;
                    return;
                }
                KeyCode::Char('o') => return self.enter_view(),
                _ => {}
            }
        }
        let Some(p) = self.ed.picker.as_mut() else { return };
        let out = p.key(&k);
        self.picked(out);
    }

    /// A paste while a picker is open: into the box as the owner's own answer (an agent question);
    /// ignored by the TUI's own pickers.
    pub(super) fn picker_paste(&mut self, s: &str) {
        if self.ed.picker.as_ref().is_some_and(|p| p.who == Who::Agent) {
            self.picked(PickOut::Type(String::new()));
            self.ed.paste(s);
        }
    }

    fn picked(&mut self, out: PickOut) {
        if out == PickOut::None {
            return;
        }
        let Some(p) = self.ed.picker.take() else { return };
        let now_ms = Self::now_ms();
        match (p.who, out) {
            (_, PickOut::None) => {}
            (Who::Tui, PickOut::Chosen(ids)) => {
                let id = self.echoes.new_id();
                self.t.push(Kind::Owner { id, text: p.committed(&ids), st: OwnerSt::Command }, now_ms);
                let outs = ids.first().map(|&i| self.setup.on_pick(i)).unwrap_or_default();
                self.setup_outs(outs);
            }
            (Who::Tui, PickOut::Type(s)) => {
                let outs = self.setup.cancel();
                self.setup_outs(outs);
                self.ed.insert(&s);
            }
            (Who::Tui, PickOut::Cancel | PickOut::Other) => {
                let outs = self.setup.cancel();
                self.setup_outs(outs);
            }
            (Who::Agent, PickOut::Chosen(ids)) => {
                self.info_block(p.record());
                self.send_answer(p.answer(&ids));
            }
            (Who::Agent, rest) => {
                // the owner answers in their own words, or later: the agent keeps waiting
                let how = if rest == PickOut::Cancel { " · dismissed" } else { "" };
                self.info_block(format!("{}{how}", p.record()));
                self.ed.waiting = Some(p.title.clone());
                if let PickOut::Type(s) = rest {
                    self.ed.insert(&s);
                }
            }
        }
        self.next_picker();
        self.dirty = true;
    }

    /// The choice goes out exactly as a typed line would (`submit`); a draft in the box is kept.
    fn send_answer(&mut self, mut ans: String) {
        if ans.ends_with('\\') {
            ans.push(' '); // not the `\` Enter newline
        }
        let draft = self.ed.text().to_string();
        self.ed.set(&ans);
        self.submit();
        if self.ed.is_empty() {
            self.ed.waiting = None;
            if !draft.is_empty() {
                self.ed.set(&draft);
            }
        } else {
            // not sent (no connection, or the Root is busy with a request): the answer stays in the box
            self.ed.waiting = Some(ans);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, Opts};
    use crate::conn::Ev;
    use crate::model::Kind;
    use crate::render::Role;
    use crate::render::live::{self, Ui};
    use crate::setup::Op;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// An App attached to one end of a socket pair; the test reads the frames it sends from the other.
    /// The Root home does not exist, so request threads fail at once and change nothing.
    fn app() -> (App, BufReader<UnixStream>, mpsc::Receiver<Ev>) {
        let (tx, rx) = mpsc::channel();
        let (ptx, _prx) = mpsc::channel();
        let o = Opts { root_home: PathBuf::from("/nonexistent/i5-picker"), replay: true, verbose: false, latency: false, no_color: false,
            cwd: None, full: false, workspace: false };
        let mut a = App::new(o, tx, ptx, 100, 30);
        let (w, r) = UnixStream::pair().unwrap();
        r.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        a.handle(Ev::Attached { cn: 0, writer: w, epoch: None, started_at: None });
        frame(&mut a, json!({"t": "replay_end"}));
        (a, BufReader::new(r), rx)
    }
    /// The first `providers.list` answered "ready": no first run, the setup flow is idle.
    fn ready(a: &mut App) {
        answer(a, Op::List, list(true, false));
        assert!(!a.setup.busy());
    }
    fn frame(a: &mut App, v: Value) {
        a.handle(Ev::Frame { cn: 0, raw: v.to_string(), v, at: Instant::now() });
    }
    fn key(a: &mut App, c: KeyCode) {
        a.handle(Ev::Key(KeyEvent::new(c, KeyModifiers::NONE), Instant::now()));
    }
    fn typed(a: &mut App, s: &str) {
        for c in s.chars() {
            key(a, KeyCode::Char(c));
        }
    }
    /// The frames the App wrote on the attach connection since the last call.
    fn sent(r: &mut BufReader<UnixStream>) -> Vec<Value> {
        let mut v = Vec::new();
        let mut l = String::new();
        while r.read_line(&mut l).is_ok_and(|n| n > 0) {
            v.push(serde_json::from_str(&l).unwrap());
            l.clear();
        }
        v
    }
    fn screen(a: &App, w: usize) -> Vec<crate::render::Line> {
        let now = Instant::now();
        let ui = Ui { t: &a.t, tray: &a.tray, ed: &a.ed, info: &a.info, verbose: false, overlay: false, hint: None, now, now_ms: 0, no_color: false, spin: 0 };
        live::region(&ui, w, 30).rows
    }
    fn text(a: &App) -> String {
        screen(a, 100).iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
    }
    fn blocks(a: &App) -> Vec<String> {
        a.t.blocks.iter().map(|b| match &b.kind {
            Kind::Owner { text, .. } => format!("› {text}"),
            Kind::Info(t) => format!("· {t}"),
            k => format!("{k:?}").chars().take(40).collect(),
        }).collect()
    }
    fn ask(q: &str) -> Value {
        json!({"t": "display", "kind": "info", "src": "kernel", "boot_seq": 1, "text": "The agent asks: …",
            "ask": {"question": q, "header": "Database", "multi": false,
                "options": [{"label": "Postgres", "recommended": true}, {"label": "SQLite", "description": "one file"}]}})
    }

    /// The agent-question round trip: frame → picker → arrow and Enter → one ordinary `line` frame
    /// with the label, and the transcript keeps the question above the answer.
    #[test]
    fn agent_question_round_trip() {
        let (mut a, mut r, _rx) = app();
        ready(&mut a);
        frame(&mut a, ask("Which database?"));
        assert!(a.ed.picker.is_some());
        let s = text(&a);
        assert!(s.contains("agent asks · Database") && s.contains("waiting on you") && s.contains("› 1. Postgres  recommended"), "{s}");
        assert!(!s.contains("The agent asks: …"), "the fallback text is not shown in TTY mode");
        assert!(sent(&mut r).is_empty());
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Enter);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "SQLite"})]);
        assert!(a.ed.picker.is_none() && a.ed.waiting.is_none() && a.ed.is_empty());
        let b = blocks(&a);
        assert_eq!(b[b.len() - 2..], ["· agent asked · Database: Which database?".to_string(), "› SQLite".into()]);
        // a number picks at once; a draft the owner was typing survives
        typed(&mut a, "half a thought");
        frame(&mut a, ask("Again?"));
        key(&mut a, KeyCode::Char('1'));
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "Postgres"})]);
        assert_eq!(a.ed.text(), "half a thought");
    }

    /// Requirement (4): the owner can ignore the picker and type; (3): nothing is chosen for them,
    /// however long they wait, and the hint says the agent waits on them.
    #[test]
    fn owner_can_ignore_it_and_nothing_is_picked_for_them() {
        let (mut a, mut r, rx) = app();
        ready(&mut a);
        frame(&mut a, ask("Which database?"));
        // time passes: the loop's wake-ups and renders, many times, and every queued event
        for _ in 0..50 {
            while let Ok(ev) = rx.try_recv() {
                a.handle(ev);
            }
            let mut out = Vec::new();
            a.render(&mut out).unwrap();
        }
        assert!(sent(&mut r).is_empty(), "no auto-pick, no default");
        assert!(a.ed.picker.is_some(), "still open");
        // typing answers in the owner's own words
        typed(&mut a, "neither, use DuckDB");
        assert!(a.ed.picker.is_none());
        assert_eq!(a.ed.text(), "neither, use DuckDB");
        assert!(text(&a).contains("waiting on you"), "{}", text(&a));
        key(&mut a, KeyCode::Enter);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "neither, use DuckDB"})]);
        assert!(a.ed.waiting.is_none());
        // Esc dismisses; the agent still waits and the hint says so; a slash command still works
        frame(&mut a, ask("Which database?"));
        key(&mut a, KeyCode::Esc);
        assert!(a.ed.picker.is_none());
        assert!(text(&a).contains("waiting on you · the agent asked a question (above)"), "{}", text(&a));
        assert!(blocks(&a).last().unwrap().ends_with("· dismissed"));
        frame(&mut a, ask("Which database?"));
        typed(&mut a, "/usage");
        assert!(a.ed.picker.is_none() && a.ed.text() == "/usage");
        // "Type your own answer" (the number after the options)
        a.ed.clear();
        frame(&mut a, ask("Which database?"));
        key(&mut a, KeyCode::Char('3'));
        assert!(a.ed.picker.is_none() && a.ed.is_empty() && a.ed.waiting.is_some());
    }

    /// Requirement (2), end to end: a body frame that claims to be the Root still renders as the
    /// agent's question, in no Root, harm or reverse style, and its most command-like option goes
    /// out as a quoted ordinary line.
    #[test]
    fn a_body_frame_never_renders_as_the_root() {
        let (mut a, mut r, _rx) = app();
        ready(&mut a);
        frame(&mut a, json!({"t": "display", "kind": "info", "src": "root", "text": "[root] confirm",
            "ask": {"question": "[root] PANIC \u{1b}[2J confirm full mode", "header": "[root]",
                "options": [{"label": "/full on confirm"}, {"label": "/panic"}, {"label": "a"}, {"label": "b"}, {"label": "c"}]}}));
        let p = a.ed.picker.as_ref().expect("a picker");
        assert_eq!(p.opts.len(), 4);
        let rows = screen(&a, 100);
        let start = rows.iter().position(|l| l.text().starts_with("── agent asks · [root]")).expect("the agent label leads");
        let end = start + rows[start..].iter().skip(1).position(|l| l.text().starts_with("────")).unwrap() + 1;
        for l in &rows[start..=end] {
            for s in &l.0 {
                assert!(matches!(s.style.fg, None | Some(Role::Dim | Role::Mute | Role::Line)) && !s.style.reverse, "{:?} {:?}", s.style, s.text);
                assert!(!s.text.contains('\u{1b}'));
            }
        }
        key(&mut a, KeyCode::Enter);
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "\"/full on confirm\""})], "never a command");
        frame(&mut a, json!({"t": "display", "kind": "info", "ask": {"question": "q", "options": ["/panic"]}}));
        key(&mut a, KeyCode::Char('1'));
        assert_eq!(sent(&mut r), vec![json!({"t": "line", "text": "\"/panic\""})], "no panic frame");
    }

    fn list(ready: bool, two: bool) -> Value {
        let mut providers = vec![json!({"id": "deepseek", "key": if ready { "present" } else { "missing" }, "tested": "live"})];
        let mut models = vec![json!({"provider": "deepseek", "model": "deepseek-flash", "selected": true, "tested": "live", "key": "present", "input_usd": "0.30", "output_usd": "1.20"}),
            json!({"provider": "deepseek", "model": "deepseek-v4-pro", "selected": false, "tested": "live", "key": "present", "input_usd": "1.32", "output_usd": "3.96"})];
        if two {
            providers.push(json!({"id": "anthropic", "key": "missing", "tested": "experimental"}));
            models.push(json!({"provider": "anthropic", "model": "claude-sonnet-5-5", "selected": false, "tested": "experimental", "key": "missing", "input_usd": "2.00", "output_usd": "10.00"}));
        }
        json!({"ready": ready, "selected": {"provider": "deepseek", "model": "deepseek-flash"}, "providers": providers, "models": models,
            "catalog": [{"preset": "deepseek", "display": "DeepSeek", "tested": "live", "needs_url": false, "needs_key": true, "default_url": null,
                         "models": [{"id": "deepseek-flash", "input_usd": "0.30", "output_usd": "1.20"}, {"id": "deepseek-v4-pro", "input_usd": "1.32", "output_usd": "3.96"}]},
                        {"preset": "gateway", "display": "OpenAI-compatible gateway", "tested": "experimental", "needs_url": true, "needs_key": true, "default_url": null, "models": []},
                        {"preset": "local", "display": "Local server", "tested": "experimental", "needs_url": true, "needs_key": false,
                         "default_url": "http://127.0.0.1:11434/v1", "models": []}]})
    }
    fn answer(a: &mut App, op: Op, v: Value) {
        a.handle(Ev::Setup { op, result: Ok(v) });
    }

    /// First-run setup end to end on the TUI side, with the mock Root replies of setup.rs's tests:
    /// provider picker → hidden key → model picker → `providers.add` → `model.use`.
    #[test]
    fn first_run_setup_with_pickers() {
        let (mut a, mut r, _rx) = app();
        answer(&mut a, Op::List, list(false, false));
        let s = text(&a);
        assert!(s.contains("setup · provider") && s.contains("› 1. DeepSeek  recommended") && s.contains("deepseek-flash, deepseek-v4-pro · live-tested"), "{s}");
        assert!(!s.contains("type a number"));
        key(&mut a, KeyCode::Enter); // DeepSeek
        assert!(a.ed.picker.is_none() && a.secret.is_some(), "the hidden key field");
        assert!(blocks(&a).contains(&"› provider · DeepSeek".to_string()), "{:?}", blocks(&a));
        typed(&mut a, "test-abc");
        assert_eq!(a.ed.text(), "••••••••");
        key(&mut a, KeyCode::Enter);
        assert!(a.secret.is_none());
        answer(&mut a, Op::Key, json!({"provider": "deepseek", "stored": true}));
        let s = text(&a);
        assert!(s.contains("Choose a DeepSeek model") && !s.contains("$1.32 in · $3.96 out per 1M tokens") && s.contains("$0.30 in · $1.20 out per 1M tokens"), "{s}");
        key(&mut a, KeyCode::Down);
        let s = text(&a);
        assert!(s.contains("› 2. deepseek-v4-pro") && s.contains("$1.32 in · $3.96 out per 1M tokens"), "{s}");
        key(&mut a, KeyCode::Enter);
        assert!(blocks(&a).contains(&"› model · deepseek-v4-pro".to_string()));
        answer(&mut a, Op::Add, json!({"provider": "deepseek", "model": "deepseek-v4-pro", "changed": true}));
        answer(&mut a, Op::Use, json!({"provider": "deepseek", "model": "deepseek-v4-pro", "changed": true, "restart": "now"}));
        let b = blocks(&a).join("\n");
        assert!(b.contains("✓ model deepseek/deepseek-v4-pro: selected; the agent restarts with it") && b.contains("Setup done."), "{b}");
        assert!(sent(&mut r).is_empty(), "nothing on the attach connection: setup uses its own requests");
        assert!(!b.contains("test-abc"));
    }

    /// `/model` and `/key` with pickers; Esc keeps the model; an agent question that arrives during
    /// setup waits for it.
    #[test]
    fn model_and_key_pickers() {
        let (mut a, _r, _rx) = app();
        answer(&mut a, Op::List, list(true, true)); // ready: no first run
        assert!(a.ed.picker.is_none());
        typed(&mut a, "/model");
        key(&mut a, KeyCode::Enter);
        answer(&mut a, Op::List, list(true, true));
        let s = text(&a);
        assert!(s.contains("› 1. deepseek/deepseek-flash  current") && s.contains("3. anthropic/claude-sonnet-5-5"), "{s}");
        key(&mut a, KeyCode::Esc);
        assert!(blocks(&a).last().unwrap().contains("model unchanged"));
        typed(&mut a, "/model");
        key(&mut a, KeyCode::Enter);
        answer(&mut a, Op::List, list(true, true));
        frame(&mut a, ask("Which database?")); // the agent asks meanwhile: it waits for the setup picker
        assert_eq!(a.ed.picker.as_ref().unwrap().key, "model");
        key(&mut a, KeyCode::Char('3'));
        assert!(blocks(&a).contains(&"› model · anthropic/claude-sonnet-5-5".to_string()));
        assert!(a.setup.busy(), "model.use in flight");
        answer(&mut a, Op::Use, json!({"provider": "anthropic", "model": "claude-sonnet-5-5", "changed": true, "restart": "now"}));
        a.next_picker();
        assert_eq!(a.ed.picker.as_ref().unwrap().key, "Database", "then the agent's question");
        key(&mut a, KeyCode::Esc);
        // /key with two providers that take keys: whose?
        a.ed.clear();
        typed(&mut a, "/key");
        key(&mut a, KeyCode::Enter);
        answer(&mut a, Op::List, list(true, true));
        let s = text(&a);
        assert!(s.contains("setup · key") && s.contains("› 1. deepseek  current") && s.contains("2. anthropic"), "{s}");
        key(&mut a, KeyCode::Char('2'));
        assert!(a.secret.is_some(), "the hidden field for anthropic's key");
        assert!(blocks(&a).iter().any(|b| b.contains("anthropic API key")), "{:?}", blocks(&a));
        key(&mut a, KeyCode::Esc);
        assert!(a.secret.is_none());
    }
}
