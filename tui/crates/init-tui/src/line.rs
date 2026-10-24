//! Line mode (docs/tui.md §3.12): chosen when stdin or stdout is not a TTY, with `TERM=dumb`, or
//! with `--plain`/`--raw`. One owner line per stdin line; output per the v0.2 attach rules (echoes as
//! `> `); `--raw` prints the frames as JSON lines; `--wait-idle` for scripts.

use crate::access::{self, Out as AccOut};
use crate::app::{Latency, spawn_signals};
use crate::conn::{self, Ev, PollCmd};
use crate::model::{EchoOut, Echoes, Feed, FeedOut, Info, OwnerSt};
use crate::notice;
use crate::proto::{self, DisplayKind, Frame, LineKind};
use crate::render::{Color, Style, sanitize, wrap};
use crate::setup;
use std::collections::{HashSet, VecDeque};
use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub struct LineOpts {
    pub root_home: PathBuf,
    pub raw: bool,
    pub verbose: bool,
    pub wait: bool,
    pub wait_idle: bool,
    pub timeout: Option<Duration>,
    pub replay: bool,
    pub latency: bool,
    /// ANSI styling: stdout is a TTY and `NO_COLOR` is unset.
    pub style: bool,
    /// The `> ` prompt: stdin is a TTY.
    pub prompt: bool,
    /// ADR-009: the cwd sent as the workspace at the first attach, and `--full`; ADR-012:
    /// `--workspace` confines this session to the workspace.
    pub cwd: Option<String>,
    pub full: bool,
    pub workspace: bool,
}

struct Out<W: Write> {
    w: W,
    col: usize,
    style: bool,
}

impl<W: Write> Out<W> {
    fn text(&mut self, s: &str) {
        let p = sanitize::plain(s, self.col);
        if let Some(i) = p.rfind('\n') {
            self.col = wrap::str_width(&p[i + 1..]);
        } else {
            self.col += wrap::str_width(&p);
        }
        let _ = self.w.write_all(p.as_bytes());
    }
    fn styled(&mut self, s: &str, st: Style) {
        if !self.style || st == Style::PLAIN {
            return self.text(s);
        }
        // styled output goes through the TTY writer's commands; the text is sanitized first
        for (i, l) in sanitize::lines(s, st, self.col).into_iter().enumerate() {
            if i > 0 {
                let _ = self.w.write_all(b"\n");
                self.col = 0;
            }
            self.col += l.width();
            let _ = crate::term::write_line(&mut self.w, &l, false);
        }
    }
    fn bol(&mut self) {
        if self.col > 0 {
            let _ = self.w.write_all(b"\n");
            self.col = 0;
        }
    }
    fn line(&mut self, s: &str, st: Style) {
        self.bol();
        self.styled(s, st);
        let _ = self.w.write_all(b"\n");
        self.col = 0;
    }
    fn flush(&mut self) {
        let _ = self.w.flush();
    }
}

/// The tool calls since the last assistant text (§3.12): summarized in one line when they end.
#[derive(Default)]
struct LineAct {
    calls: usize,
    failed: usize,
    names: Vec<(String, usize)>,
}

/// A running call, for the heartbeat.
struct Running {
    call_id: String,
    name: String,
    started: Instant,
    /// The last output or heartbeat line.
    beat: Instant,
}

/// Line mode prints a heartbeat for a call with no output for this long, and again after as long.
const LINE_HEARTBEAT: Duration = Duration::from_secs(30);

struct LineApp<W: Write> {
    o: LineOpts,
    out: Out<W>,
    tx: Sender<Ev>,
    poll: Sender<PollCmd>,
    cn: u64,
    writer: Option<UnixStream>,
    attempt: u32,
    retry_at: Option<Instant>,
    connecting: bool,
    ever_attached: bool,
    feed: Feed,
    echoes: Echoes,
    info: Info,
    printed: HashSet<u64>,
    unsent: VecDeque<String>,
    eof: bool,
    sigint_at: Option<Instant>,
    last_frame: Instant,
    last_confirm: Option<Instant>,
    idle_after_confirm: bool,
    seen_state: bool,
    thinking_open: bool,
    last_boot: Option<u64>,
    started: Instant,
    lat: Latency,
    exit: Option<i32>,
    panic_pending: bool,
    acc: access::Flow,
    attached_once: bool,
    /// UX-KEYMODEL: the first-run setup, `/key`, `/model` (crate::setup).
    setup: setup::Flow,
    act: Option<LineAct>,
    running: Vec<Running>,
    /// This turn's per-request token lines, for `/details`.
    reqs: Vec<String>,
    /// STEER: lines echoed while the agent worked and not read yet: seq → (text, follow-up).
    steers: std::collections::BTreeMap<u64, (String, bool)>,
}

impl<W: Write> LineApp<W> {
    /// The activity ends (the reply's text, an error, the run's end): one summary line when it had
    /// more than one call.
    fn end_act(&mut self) {
        self.running.clear();
        let Some(a) = self.act.take() else { return };
        if a.calls < 2 || self.o.raw {
            return;
        }
        let names: Vec<String> = a.names.iter().map(|(n, k)| format!("{k} {n}")).collect();
        let failed = if a.failed > 0 { format!(", {} failed", a.failed) } else { String::new() };
        let st = if a.failed > 0 { Style::fg(Color::Red) } else { Style::dim() };
        self.out.line(&format!("[tools] ran {} calls ({}){failed}", a.calls, names.join(", ")), st);
    }

    fn act_call(&mut self, name: &str) {
        let a = self.act.get_or_insert_with(LineAct::default);
        a.calls += 1;
        match a.names.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 += 1,
            None => a.names.push((name.to_string(), 1)),
        }
    }

    /// The heartbeat: a running call silent for 30 s gets a line, then one every 30 s.
    fn tick(&mut self, now: Instant) {
        if self.o.raw {
            return;
        }
        let mut lines = Vec::new();
        for r in self.running.iter_mut() {
            if now.duration_since(r.beat) >= LINE_HEARTBEAT {
                r.beat = now;
                lines.push(format!("  … {} still working ({})", r.name, crate::time::span(now.duration_since(r.started).as_secs())));
            }
        }
        for l in lines {
            self.out.line(&l, Style::dim());
        }
        self.out.flush();
    }

    fn start_attach(&mut self) {
        self.cn += 1;
        self.connecting = true;
        self.retry_at = None;
        let replay = self.o.replay || self.feed.attaches > 0;
        conn::spawn_attach(self.cn, conn::sock(&self.o.root_home), replay, self.feed.since(), self.tx.clone());
    }

    fn schedule_retry(&mut self) {
        let d = conn::backoff(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        self.retry_at = Some(Instant::now() + d);
        self.connecting = false;
    }

    fn send(&mut self, frame: &str) -> bool {
        let Some(w) = self.writer.as_mut() else { return false };
        if conn::send(w, frame).is_err() {
            if let Some(w) = self.writer.take() {
                let _ = w.shutdown(std::net::Shutdown::Both);
            }
            self.feed.on_disconnect();
            self.schedule_retry();
            return false;
        }
        true
    }

    /// Carries out the access flow's outputs (ADR-009).
    fn access_outs(&mut self, outs: Vec<AccOut>) {
        for o in outs {
            match o {
                AccOut::Send(req) => conn::spawn_access(conn::sock(&self.o.root_home), req, self.tx.clone()),
                AccOut::Access(v) => self.info.on_access(v),
                _ if self.o.raw => {}
                AccOut::Notice(t) | AccOut::Warn(t) => self.out.line(&format!("[tui] {t}"), Style::fg(Color::Yellow)),
                AccOut::Info(t) | AccOut::Ask(t) => self.out.line(&format!("[tui] {t}"), Style::dim()),
                AccOut::Error(t) => self.out.line(&format!("[tui] {t}"), Style::fg(Color::Red)),
            }
        }
    }

    /// Carries out the setup flow's outputs (UX-KEYMODEL). A secret never reaches the output.
    fn setup_outs(&mut self, outs: Vec<setup::Out>) {
        for o in outs {
            match o {
                setup::Out::Send(req) => conn::spawn_setup(conn::sock(&self.o.root_home), req, self.tx.clone()),
                setup::Out::SendKey { provider, secret } => conn::spawn_key(conn::sock(&self.o.root_home), provider, secret, self.tx.clone()),
                setup::Out::Secret(_) => {}
                _ if self.o.raw => {}
                setup::Out::Info(t) => self.out.line(&format!("[setup] {t}"), Style::dim()),
                setup::Out::Warn(t) => self.out.line(&format!("[setup] {t}"), Style::fg(Color::Yellow)),
                setup::Out::Error(t) => self.out.line(&format!("[setup] {t}"), Style::fg(Color::Red)),
            }
        }
    }

    /// Sends the stdin lines read so far, in order; a line is sent once and never again.
    fn flush_unsent(&mut self) {
        while self.writer.is_some() && self.feed.is_live() {
            let Some(mut text) = self.unsent.front().cloned() else { break };
            let kind = proto::line_kind(&text, self.info.rootish());
            if kind != LineKind::Panic {
                if self.acc.holds_lines() {
                    break; // the workspace or an access request first (ADR-009)
                }
                if let Some(outs) = self.acc.on_line(&text) {
                    self.unsent.pop_front();
                    self.access_outs(outs);
                    continue;
                }
                if self.setup.busy() {
                    break; // a setup request first (UX-KEYMODEL)
                }
                if self.setup.secret_mode() {
                    // the key line: moved into the request and zeroed there; never printed or sent as a line
                    let t = self.unsent.pop_front().unwrap_or_default();
                    setup::zero(&mut std::mem::take(&mut text).into_bytes()); // the clone above
                    let outs = self.setup.on_secret(t.into_bytes());
                    self.setup_outs(outs);
                    continue;
                }
                if let Some(outs) = self.setup.on_line(&text) {
                    self.unsent.pop_front();
                    self.setup_outs(outs);
                    continue;
                }
            }
            let text = proto::canonical(&text);
            let frame = match kind {
                LineKind::Panic => proto::panic_frame(),
                _ => proto::line(&text),
            };
            if frame.len() > proto::LINE_MAX {
                self.unsent.pop_front();
                self.out.line(&format!("[tui] too long to send ({} KiB > 900 KiB); not sent", frame.len() / 1024), Style::fg(Color::Yellow));
                continue;
            }
            if !self.send(&frame) {
                break;
            }
            self.unsent.pop_front();
            if kind != LineKind::Panic {
                self.echoes.sent(&text, kind, Instant::now());
            }
        }
    }

    fn handle(&mut self, ev: Ev) {
        let now = Instant::now();
        match ev {
            Ev::Attached { cn, writer, epoch, started_at } if cn == self.cn => {
                self.writer = Some(writer);
                self.connecting = false;
                self.attempt = 0;
                self.feed.on_attach(epoch, started_at);
                if !self.attached_once {
                    self.attached_once = true;
                    let outs = self.acc.on_attached();
                    self.access_outs(outs);
                    let outs = self.setup.on_attached();
                    self.setup_outs(outs);
                }
            }
            Ev::AttachFailed { cn, unreachable, msg } if cn == self.cn => {
                if !self.ever_attached && !unreachable {
                    eprintln!("init-tui: {msg}: not a Root owner channel");
                    self.exit = Some(76);
                    return;
                }
                if !self.ever_attached && !self.o.wait {
                    eprintln!("init-tui: the Root is unreachable at {} ({msg})", conn::sock(&self.o.root_home).display());
                    self.exit = Some(69);
                    return;
                }
                self.schedule_retry();
            }
            Ev::Disconnected { cn } if cn == self.cn => {
                self.writer = None;
                self.feed.on_disconnect();
                self.lat.disconnects += 1;
                if self.feed.attaches == 0 && !self.o.wait {
                    eprintln!("init-tui: the Root closed the connection before the attach exchange");
                    self.exit = Some(69);
                    return;
                }
                self.schedule_retry();
            }
            Ev::Malformed { cn } if cn == self.cn => {
                if self.feed.attaches == 0 {
                    eprintln!("init-tui: malformed first frames: not a Root owner channel");
                    self.exit = Some(76);
                }
            }
            Ev::Frame { cn, raw, v, at } if cn == self.cn => {
                self.last_frame = now;
                if self.feed.attaches == 0 && matches!(proto::classify(&v), Frame::Answer(_)) {
                    eprintln!("init-tui: the socket answered the attach with a request answer: not a Root owner channel");
                    self.exit = Some(76);
                    return;
                }
                let outs = self.feed.on_frame(&raw, v);
                self.ever_attached |= self.feed.attaches > 0;
                if self.o.raw {
                    for r in std::mem::take(&mut self.feed.raw_out) {
                        let _ = writeln!(self.out.w, "{}", sanitize::raw_json(&r));
                    }
                }
                for o in outs {
                    self.feed_out(o, now);
                }
                self.out.flush();
                Latency::push(&mut self.lat.frame, Instant::now().duration_since(at));
                self.flush_unsent();
            }
            Ev::Status(v) => {
                self.info.on_status(v, now);
                let q = self.info.status.as_ref().and_then(|s| s["queued"].as_u64());
                for id in self.echoes.sweep(now, self.info.ready_since, q) {
                    if !self.o.raw {
                        self.out.line(&format!("[tui] no confirmation seen for line {id} (it may have been delivered before the Root restarted); check before resending"), Style::fg(Color::Yellow));
                    }
                }
            }
            Ev::Egress(v) => self.info.on_egress(v),
            Ev::Access { op, result } => {
                let outs = self.acc.on_answer(op, result);
                self.access_outs(outs);
                self.flush_unsent();
            }
            Ev::Setup { op, result } => {
                let outs = self.setup.on_answer(op, result);
                self.setup_outs(outs);
                self.flush_unsent();
            }
            Ev::Line(l) => {
                if l.trim() == "/details" && !self.o.raw {
                    if self.reqs.is_empty() {
                        self.out.line("[details] no token lines in this turn yet", Style::dim());
                    }
                    for r in self.reqs.clone() {
                        self.out.line(&format!("[details] {r}"), Style::dim());
                    }
                    self.out.flush();
                    return;
                }
                if l.trim() == "/usage" && !self.o.raw {
                    self.out.line(&format!("[usage] {}", crate::render::status::usage_summary(&self.info)), Style::PLAIN);
                    self.out.flush();
                    return;
                }
                if proto::line_kind(&l, false) == LineKind::Panic && !(self.writer.is_some() && self.feed.is_live()) {
                    self.panic_pending = true;
                    conn::panic_fallback(self.o.root_home.clone(), self.tx.clone());
                } else {
                    self.unsent.push_back(l);
                    self.flush_unsent();
                }
            }
            Ev::StdinEof => self.eof = true,
            Ev::Signal(signal_hook::consts::SIGINT) => {
                if self.sigint_at.is_some_and(|t| now.duration_since(t) <= Duration::from_secs(2)) {
                    self.send(&proto::detach());
                    self.exit = Some(0);
                    return;
                }
                self.sigint_at = Some(now);
                self.send(&proto::interrupt());
                if !self.o.raw {
                    let tail = if self.info.kernel_ready() { "" } else { " (no body is running)" };
                    self.out.line(&format!("[interrupt sent{tail}; again within 2 s detaches]"), Style::dim());
                }
            }
            Ev::Signal(_) => {
                self.send(&proto::detach());
                self.exit = Some(0);
            }
            Ev::PanicDone(r) => {
                self.panic_pending = false;
                let s = match r {
                    Ok(m) => format!("[tui] /panic: {m}"),
                    Err(e) => format!("[tui] /panic failed: {e}"),
                };
                self.out.line(&s, Style::fg(Color::Red));
            }
            _ => {}
        }
        self.out.flush();
    }

    fn feed_out(&mut self, o: FeedOut, now: Instant) {
        let raw = self.o.raw;
        match o {
            FeedOut::Divider(d) if !raw => self.out.line(&format!("── {d} ──"), Style::dim()),
            FeedOut::Greeting(v) if !raw => {
                let t = v["text"].as_str().unwrap_or("").to_string();
                self.out.line(&t, Style::dim());
            }
            FeedOut::Frame(v) => {
                match proto::classify(&v) {
                    Frame::Echo { text, qid, seq } => {
                        if seq.is_some() {
                            self.last_confirm = Some(now);
                            self.idle_after_confirm = false;
                            if proto::line_kind(&text, false) == LineKind::Input {
                                self.info.on_owner_delivered();
                            }
                        }
                        let r = self.echoes.on_echo(&text, qid.as_deref(), seq, now);
                        let (id, st) = match r {
                            EchoOut::Update(id, st) | EchoOut::New(id, st) => (id, st),
                            EchoOut::Absorbed(_) => return,
                        };
                        // STEER: a line the kernel holds while the agent works is queued, not read yet
                        let queued_steer = seq.is_some() && self.info.busy() && proto::line_kind(&text, false) == LineKind::Input;
                        if !raw && self.printed.insert(id) {
                            let mut s = format!("> {text}");
                            if st == OwnerSt::Queued {
                                s.push_str(" · queued");
                            }
                            if queued_steer {
                                let tag = if self.echoes.last_follow_up { "[next task queued]" } else { "[steer queued]" };
                                s = format!("{tag} {text}");
                            }
                            self.out.line(&s, if queued_steer { Style::dim() } else { Style::PLAIN });
                        }
                        if let (true, Some(n)) = (queued_steer, seq) {
                            self.steers.insert(n, (text.clone(), self.echoes.last_follow_up));
                        }
                        if !raw && text.trim() == "/new" && st == OwnerSt::Delivered {
                            self.out.line("── new session ──", Style::dim());
                        }
                    }
                    Frame::State(s) => {
                        self.seen_state = true;
                        self.steer_lines(&s);
                        if s["busy"] == false && self.last_confirm.is_some() {
                            self.idle_after_confirm = true;
                        }
                        let was = self.info.busy();
                        self.info.on_state(s, now);
                        if was && !self.info.busy() {
                            // the run ended: the tool summary, then the turn's tokens, cache and cost
                            self.end_act();
                            if !raw && self.info.turn.requests > 0 {
                                let (t, sc) = (self.info.turn_line(), self.info.session.cache().show());
                                self.out.line(&format!("[turn] {} requests: {t} (session cache {sc})", self.info.turn.requests), Style::dim());
                            }
                        }
                    }
                    Frame::Notice { text, code, .. } => {
                        if notice::is_queue_full(&text, proto::src(&v)) {
                            self.echoes.queue_full();
                        }
                        if !raw {
                            self.out.line(&text, Style::PLAIN);
                        }
                        if text.starts_with("[root]") || matches!(code.as_deref(), Some("access" | "snapshot")) {
                            let _ = self.poll.send(PollCmd::Status);
                        }
                    }
                    Frame::Display(d) if !raw => self.display(&d),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// STEER: `[steer seen] …` once the body has logged a queued line (the kernel's `steer_seen`),
    /// `[steer retracted] …` when it was taken back. An older kernel reports neither: nothing more.
    fn steer_lines(&mut self, s: &serde_json::Value) {
        let list = |k: &str| -> Option<Vec<u64>> { Some(s.get(k)?.as_array()?.iter().filter_map(serde_json::Value::as_u64).collect()) };
        let Some(pending) = list("steer_pending") else {
            if s["busy"] == false {
                self.steers.clear();
            }
            return;
        };
        let (seen, retracted) = (list("steer_seen").unwrap_or_default(), list("steer_retracted").unwrap_or_default());
        let done: Vec<u64> = self.steers.keys().copied().filter(|n| !pending.contains(n) || seen.contains(n) || retracted.contains(n)).collect();
        for n in done {
            let Some((text, follow_up)) = self.steers.remove(&n) else { continue };
            if self.o.raw {
                continue;
            }
            let tag = match (retracted.contains(&n), follow_up) {
                (true, _) => "[steer retracted]",
                (false, true) => "[next task started]",
                (false, false) => "[steer seen]",
            };
            self.end_act();
            self.out.line(&format!("{tag} {text}"), Style::dim());
        }
    }

    /// The v0.2 attach rules (kernel/supervise.md v0.2 §4.6), with echoes as `> `.
    fn display(&mut self, d: &proto::Display) {
        if let Some(b) = d.boot_seq {
            if self.last_boot.is_some_and(|p| p != b) {
                self.end_act();
                self.out.line(&format!("── body restarted (boot {b}) ──"), Style::dim());
            }
            self.last_boot = Some(b);
        }
        if d.kind != DisplayKind::Thinking {
            self.thinking_open = false;
        }
        let name = d.name.as_deref().unwrap_or("tool");
        let now = Instant::now();
        match d.kind {
            DisplayKind::Text => {
                if !d.text.trim().is_empty() {
                    self.end_act();
                }
                self.out.text(&d.text)
            }
            DisplayKind::Thinking => {
                if self.o.verbose {
                    self.out.styled(&d.text, Style::dim());
                } else if !self.thinking_open {
                    self.out.line("[thinking…]", Style::dim());
                }
                self.thinking_open = true;
            }
            DisplayKind::ToolStart => {
                self.act_call(name);
                let call_id = d.call_id.clone().unwrap_or_default();
                self.running.push(Running { call_id, name: name.to_string(), started: now, beat: now });
                // a delegate sub-task's call (parent_call_id): indented under its delegate line
                let pad = if d.parent.is_some() { "    " } else { "" };
                self.out.line(&format!("{pad}» {name}: {}", d.text), Style::fg(Color::Cyan))
            }
            DisplayKind::ToolProgress => {
                // no animation in line mode: output only resets the heartbeat's silence
                if let Some(r) = self.running.iter_mut().find(|r| Some(&r.call_id) == d.call_id.as_ref()) {
                    r.beat = now;
                }
            }
            DisplayKind::ToolEnd => {
                match self.running.iter().position(|r| Some(&r.call_id) == d.call_id.as_ref()) {
                    Some(i) => {
                        self.running.remove(i);
                    }
                    None => self.act_call(name), // an end without a start
                }
                let ok = d.ok == Some(true);
                if !ok && let Some(a) = self.act.as_mut() {
                    a.failed += 1;
                }
                let (w, st) = if ok { ("ok", Style::dim()) } else { ("error", Style::fg(Color::Red)) };
                let pad = if d.parent.is_some() { "    " } else { "" };
                self.out.line(&format!("{pad}  « {w} {}", d.text), st);
            }
            DisplayKind::TurnEnd => {
                self.out.bol();
                if self.info.on_tokens(&d.text).is_some() {
                    if self.info.turn.requests == 1 {
                        self.reqs.clear();
                    }
                    self.reqs.push(d.text.clone());
                }
                if self.o.verbose {
                    self.out.line(&d.text, Style::dim());
                }
            }
            DisplayKind::Info | DisplayKind::Other => self.out.line(&format!("[info] {}", d.text), Style::dim()),
            DisplayKind::Error => {
                self.end_act();
                self.out.line(&format!("[error] {}", d.text), Style::fg(Color::Red))
            }
        }
    }

    fn idle_done(&self, now: Instant) -> bool {
        self.eof
            && self.unsent.is_empty()
            && !self.acc.busy()
            && !self.setup.busy()
            && self.echoes.unconfirmed_inputs() == 0
            && (!self.seen_state || self.last_confirm.is_none() || self.idle_after_confirm)
            && !self.info.busy()
            && now.duration_since(self.last_frame) >= Duration::from_secs(1)
    }

    fn prompt(&mut self) {
        if self.o.prompt && !self.o.raw && !self.info.busy() && self.out.col == 0 && !self.eof {
            let _ = self.out.w.write_all(b"> ");
            self.out.col = 2;
            self.out.flush();
        }
    }
}

fn spawn_stdin(tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(Ev::Line(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Ev::StdinEof);
    });
}

pub fn run(o: LineOpts) -> i32 {
    let (tx, rx) = mpsc::channel();
    let (ptx, prx) = mpsc::channel();
    let started = Instant::now();
    let timeout = o.timeout;
    let latency = o.latency;
    let style = o.style;
    conn::spawn_poller(conn::sock(&o.root_home), prx, tx.clone());
    spawn_signals(tx.clone());
    let mut feed = Feed::new();
    feed.want_raw = o.raw;
    let acc = access::Flow::new(o.cwd.clone(), o.full, o.workspace);
    let mut info = Info::default();
    info.home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let mut app = LineApp {
        o,
        out: Out { w: io::BufWriter::new(io::stdout()), col: 0, style },
        tx: tx.clone(),
        poll: ptx.clone(),
        cn: 0,
        writer: None,
        attempt: 0,
        retry_at: None,
        connecting: false,
        ever_attached: false,
        feed,
        echoes: Echoes::default(),
        info,
        printed: HashSet::new(),
        unsent: VecDeque::new(),
        eof: false,
        sigint_at: None,
        last_frame: started,
        last_confirm: None,
        idle_after_confirm: false,
        seen_state: false,
        thinking_open: false,
        last_boot: None,
        started,
        lat: Latency::default(),
        exit: None,
        panic_pending: false,
        acc,
        attached_once: false,
        act: None,
        running: Vec::new(),
        reqs: Vec::new(),
        steers: std::collections::BTreeMap::new(),
        // a key typed into a terminal in line mode would be echoed: only a pipe may carry one
        setup: setup::Flow::new(!std::io::IsTerminal::is_terminal(&std::io::stdin())),
    };
    app.start_attach();
    spawn_stdin(tx);
    let code = loop {
        let now = Instant::now();
        if let Some(t) = timeout
            && now.duration_since(app.started) >= t
        {
            app.send(&proto::detach());
            app.out.flush();
            eprintln!("init-tui: --timeout elapsed");
            break 2;
        }
        if app.retry_at.is_some_and(|t| t <= now) && !app.connecting && app.writer.is_none() {
            app.start_attach();
        }
        if app.eof && app.unsent.is_empty() && !app.panic_pending && !app.acc.holds_lines() && !app.setup.busy() && (app.ever_attached || app.o.wait) {
            if !app.o.wait_idle {
                app.send(&proto::detach());
                break 0;
            }
            if app.writer.is_some() && app.feed.is_live() && app.idle_done(now) {
                app.send(&proto::detach());
                break 0;
            }
        }
        if let Some(c) = app.exit {
            break c;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ev) => app.handle(ev),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break 1,
        }
        if let Some(c) = app.exit {
            break c;
        }
        app.tick(Instant::now());
        app.prompt();
    };
    app.out.bol();
    app.out.flush();
    let _ = ptx.send(PollCmd::Stop);
    if latency {
        eprint!("{}", app.lat.report());
    }
    code
}
