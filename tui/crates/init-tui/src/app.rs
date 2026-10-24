//! TTY mode (docs/tui.md §3.2, §3.4–§3.11): the main loop owns the state, the write half of the
//! attach connection and the terminal. It renders at most once per 8 ms when something changed;
//! finished blocks are printed once into the main screen and only the live region is redrawn.

use crate::access::{self, Out};
use crate::conn::{self, Ev, PollCmd};
use crate::input::Editor;
use crate::keys::{self, Action, View};
use crate::model::{self, Conn, EchoOut, Echoes, Feed, FeedOut, Info, Kind, OwnerSt, Transcript, Tray, TrayOut};
use crate::notice::{self, Class};
use crate::proto::{self, Frame, LineKind};
use crate::render::live::{self, Ui};
use crate::render::transcript::{self, ViewState};
use crate::render::Line;
use crate::setup;
use crate::{term, time};
use crossterm::QueueableCommand;
use crossterm::cursor::{MoveTo, MoveToColumn, MoveToPreviousLine, Show};
use crossterm::event::{self, Event};
use crossterm::terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Opts {
    pub root_home: PathBuf,
    pub replay: bool,
    pub verbose: bool,
    pub latency: bool,
    pub no_color: bool,
    /// ADR-009: the cwd sent as the workspace at the first attach, and `--full`; ADR-012:
    /// `--workspace` confines this session to the workspace.
    pub cwd: Option<String>,
    pub full: bool,
    pub workspace: bool,
}

/// Latency samples (§6), in microseconds.
#[derive(Default, Debug)]
pub struct Latency {
    pub key: Vec<u32>,
    pub frame: Vec<u32>,
    pub first_paint_ms: Option<u64>,
    pub disconnects: u32,
    pub renders: u64,
}

fn pct(v: &mut [u32], p: f64) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i.min(v.len() - 1)]
}

impl Latency {
    pub(crate) fn push(v: &mut Vec<u32>, d: Duration) {
        if v.len() < 1_000_000 {
            v.push(d.as_micros().min(u32::MAX as u128) as u32);
        }
    }
    pub fn report(&mut self) -> String {
        let mut s = String::new();
        for (name, v) in [("keystroke->flush", &mut self.key), ("frame->flush", &mut self.frame)] {
            let n = v.len();
            let (p50, p90, p99, max) = (pct(v, 0.5), pct(v, 0.9), pct(v, 0.99), pct(v, 1.0));
            s.push_str(&format!("init-tui latency {name}: n={n} p50={p50}us p90={p90}us p99={p99}us max={max}us\n"));
        }
        s.push_str(&format!("init-tui first_paint_ms={} disconnects={} renders={}\n", self.first_paint_ms.map_or(-1, |v| v as i64), self.disconnects, self.renders));
        s
    }
}

pub struct App {
    pub o: Opts,
    tx: Sender<Ev>,
    poll: Sender<PollCmd>,
    cn: u64,
    writer: Option<UnixStream>,
    attempt: u32,
    retry_at: Option<Instant>,
    connecting: bool,
    pub feed: Feed,
    pub echoes: Echoes,
    pub t: Transcript,
    /// STEER: lines typed while the agent works, until the body reads them (§3.7.1).
    pub tray: Tray,
    /// STEER: the last Esc that interrupted and steered (a second Esc within 1 s just stops).
    esc_steer_at: Option<Instant>,
    pub ed: Editor,
    pub info: Info,
    pub verbose: bool,
    overlay: bool,
    view: Option<ViewState>,
    hint: Option<(String, Instant)>,
    ctrl_c_at: Option<Instant>,
    t0: Instant,
    pub w: u16,
    pub h: u16,
    prev: Vec<Line>,
    prev_cursor_row: usize,
    need_full: bool,
    dirty: bool,
    last_render: Option<Instant>,
    pub lat: Latency,
    pending_keys: Vec<Instant>,
    pending_frames: Vec<Instant>,
    malformed_at: Option<Instant>,
    pub exit: Option<i32>,
    unreachable_told: bool,
    ever_attached: bool,
    need_first_paint: bool,
    pub acc: access::Flow,
    /// UX-KEYMODEL: the first-run setup, `/key`, `/model` (crate::setup).
    pub setup: setup::Flow,
    /// The hidden key field: preallocated, never in the editor, the history or the transcript; zeroed.
    secret: Option<Vec<u8>>,
}

impl App {
    pub fn new(o: Opts, tx: Sender<Ev>, poll: Sender<PollCmd>, w: u16, h: u16) -> App {
        let verbose = o.verbose;
        let acc = access::Flow::new(o.cwd.clone(), o.full, o.workspace);
        let mut info = Info::default();
        info.home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
        App {
            o,
            tx,
            poll,
            cn: 0,
            writer: None,
            attempt: 0,
            retry_at: Some(Instant::now()),
            connecting: false,
            feed: Feed::new(),
            echoes: Echoes::default(),
            t: Transcript::default(),
            tray: Tray::default(),
            esc_steer_at: None,
            ed: Editor::default(),
            info,
            verbose,
            overlay: false,
            view: None,
            hint: None,
            ctrl_c_at: None,
            t0: Instant::now(),
            w,
            h,
            prev: Vec::new(),
            prev_cursor_row: 0,
            need_full: true,
            dirty: true,
            last_render: None,
            lat: Latency::default(),
            pending_keys: Vec::new(),
            pending_frames: Vec::new(),
            malformed_at: None,
            exit: None,
            unreachable_told: false,
            ever_attached: false,
            need_first_paint: false,
            acc,
            setup: setup::Flow::new(true),
            secret: None,
        }
    }

    fn now_ms() -> i64 {
        time::unix_ms()
    }

    fn set_hint(&mut self, s: impl Into<String>, secs: u64) {
        self.hint = Some((s.into(), Instant::now() + Duration::from_secs(secs)));
        self.dirty = true;
    }

    fn info_block(&mut self, text: impl Into<String>) {
        self.t.push(Kind::Info(text.into()), Self::now_ms());
    }

    // ---------------------------------------------------------------------------------------
    // Connection
    // ---------------------------------------------------------------------------------------

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
        let at = Instant::now() + d;
        self.retry_at = Some(at);
        self.connecting = false;
        self.info.conn = Conn::Down { retry_at: at };
        self.dirty = true;
    }

    fn drop_conn(&mut self) {
        if let Some(w) = self.writer.take() {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
        self.feed.on_disconnect();
    }

    fn send_frame(&mut self, frame: &str) -> bool {
        let Some(w) = self.writer.as_mut() else { return false };
        if conn::send(w, frame).is_err() {
            self.drop_conn();
            self.lat.disconnects += 1;
            self.schedule_retry();
            return false;
        }
        true
    }

    // ---------------------------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------------------------

    pub fn handle(&mut self, ev: Ev) {
        let now = Instant::now();
        match ev {
            Ev::Attached { cn, writer, epoch, started_at } if cn == self.cn => {
                self.writer = Some(writer);
                self.connecting = false;
                self.attempt = 0;
                self.feed.on_attach(epoch, started_at);
                self.info.conn = Conn::Live;
                self.ever_attached = true;
                let outs = self.acc.on_attached();
                self.access_outs(outs);
                let outs = self.setup.on_attached();
                self.setup_outs(outs);
                let _ = self.poll.send(PollCmd::Egress);
                self.dirty = true;
            }
            Ev::AttachFailed { cn, unreachable, msg } if cn == self.cn => {
                if !unreachable && !self.ever_attached {
                    self.info_block(format!("the socket did not answer like a Root ({msg})"));
                    self.exit = Some(76);
                    return;
                }
                if !self.unreachable_told {
                    self.unreachable_told = true;
                    let s = conn::sock(&self.o.root_home);
                    self.info_block(format!(
                        "the Root is not reachable at {} ({msg}); start it with `init-root start` (or its launchd job). Retrying.",
                        s.display()
                    ));
                }
                self.schedule_retry();
            }
            Ev::Disconnected { cn } if cn == self.cn => {
                self.drop_conn();
                self.lat.disconnects += 1;
                self.schedule_retry();
            }
            Ev::Malformed { cn } if cn == self.cn => {
                if !self.ever_attached || self.feed.attaches == 0 && !self.feed.is_live() && self.t.is_empty() {
                    // the first frames are not an attach exchange
                    self.exit = Some(76);
                    return;
                }
                if self.malformed_at.is_none_or(|t| now.duration_since(t) > Duration::from_secs(60)) {
                    self.malformed_at = Some(now);
                    self.info_block("[tui] malformed frame ignored");
                }
            }
            Ev::Frame { cn, raw, v, at } if cn == self.cn => {
                if self.feed.attaches == 0 && !self.feed.is_live() && matches!(proto::classify(&v), Frame::Answer(_)) {
                    self.info_block("the socket answered the attach with a request answer: not a Root owner channel");
                    self.exit = Some(76);
                    return;
                }
                let was_first = self.feed.attaches == 0;
                for out in self.feed.on_frame(&raw, v) {
                    self.feed_out(out, at, now);
                }
                if was_first && self.feed.attaches == 1 && self.lat.first_paint_ms.is_none() {
                    self.need_first_paint = true;
                }
                self.dirty = true;
            }
            Ev::Status(v) => {
                self.info.on_status(v, now);
                let queued = self.info.status.as_ref().and_then(|s| s["queued"].as_u64());
                for id in self.echoes.sweep(now, self.info.ready_since, queued) {
                    self.t.set_owner(id, OwnerSt::Unconfirmed);
                }
                self.dirty = true;
            }
            Ev::Egress(v) => {
                self.info.on_egress(v);
                for s in self.info.ceiling_notices(Self::now_ms()) {
                    self.t.push(Kind::Notice { class: Class::Ceiling, level: "alert".into(), text: s }, Self::now_ms());
                }
                self.dirty = true;
            }
            Ev::Access { op, result } => {
                let outs = self.acc.on_answer(op, result);
                self.access_outs(outs);
                let _ = self.poll.send(PollCmd::Status);
                self.dirty = true;
            }
            Ev::Setup { op, result } => {
                let outs = self.setup.on_answer(op, result);
                self.setup_outs(outs);
                let _ = self.poll.send(PollCmd::Status);
            }
            Ev::ReqFailed { cmd } => {
                if cmd == "egress.stats" {
                    self.info.egress_failed = true;
                    self.dirty = true;
                }
            }
            Ev::PanicDone(r) => {
                match r {
                    Ok(m) => self.info_block(format!("/panic: {m}")),
                    Err(e) => {
                        self.t.push(Kind::Error(format!("/panic failed: {e}")), Self::now_ms());
                    }
                };
                let _ = self.poll.send(PollCmd::Status);
            }
            Ev::Key(k, at) => {
                self.pending_keys.push(at);
                self.key(k);
                self.dirty = true;
            }
            Ev::Paste(s, at) => {
                self.pending_keys.push(at);
                if self.secret.is_some() {
                    self.secret_input(s.trim());
                } else if self.view.is_none() {
                    self.ed.paste(&s);
                }
                self.dirty = true;
            }
            Ev::Resize(w, h) => self.resize(w, h),
            Ev::Signal(sig) => match sig {
                signal_hook::consts::SIGINT => self.ctrl_c(),
                _ => self.quit(),
            },
            Ev::Line(_) | Ev::StdinEof => {}
            _ => {} // events of an old connection
        }
    }

    fn feed_out(&mut self, out: FeedOut, at: Instant, now: Instant) {
        let now_ms = Self::now_ms();
        match out {
            FeedOut::Divider(d) => {
                self.t.push(Kind::Divider(d), now_ms);
            }
            FeedOut::Greeting(v) => {
                let text = v["text"].as_str().unwrap_or("");
                self.t.notice("info", text, proto::src(&v), None, now_ms);
            }
            FeedOut::Reconnected => self.info.reconnected_at = Some(now),
            FeedOut::Frame(v) => match proto::classify(&v) {
                Frame::Display(d) => {
                    self.pending_frames.push(at);
                    let turn_end = d.kind == proto::DisplayKind::TurnEnd;
                    self.t.display(&d, now, now_ms);
                    if turn_end {
                        self.info.on_tokens(&d.text);
                        let _ = self.poll.send(PollCmd::Egress);
                    }
                }
                Frame::Echo { text, qid, seq } => {
                    let r = self.echoes.on_echo(&text, qid.as_deref(), seq, now);
                    let new_session = text.trim() == "/new";
                    if seq.is_some() && proto::line_kind(&text, false) == LineKind::Input {
                        self.info.on_owner_delivered();
                    }
                    match r {
                        EchoOut::Update(id, st) if self.tray.has(id) => {
                            if let Some(o) = self.tray.on_echo(id, st, seq) {
                                self.tray_out(o);
                            }
                        }
                        EchoOut::Update(id, st) => {
                            self.t.set_owner(id, st);
                            if new_session && st == OwnerSt::Delivered {
                                self.t.push(Kind::Divider("new session".into()), now_ms);
                            }
                        }
                        EchoOut::New(id, st) => {
                            self.t.push(Kind::Owner { id, text, st }, now_ms);
                            if new_session && st == OwnerSt::Delivered {
                                self.t.push(Kind::Divider("new session".into()), now_ms);
                            }
                        }
                        EchoOut::Absorbed(_) => {}
                    }
                }
                Frame::Notice { level, text, code, .. } => {
                    let src = proto::src(&v);
                    if notice::is_queue_full(&text, src)
                        && let Some(id) = self.echoes.queue_full()
                    {
                        match self.tray.remove(id) {
                            Some(it) => {
                                self.t.push(Kind::Owner { id, text: it.text, st: OwnerSt::NotQueued }, now_ms);
                            }
                            None => self.t.set_owner(id, OwnerSt::NotQueued),
                        }
                    }
                    // STEER: the kernel's answer to a retract is a hint, not a transcript line
                    if src != Some("root")
                        && let Some((taken, seq)) = model::retract_notice(&text)
                    {
                        if !taken && self.tray.retract_refused(seq) {
                            self.set_hint("not taken back: the agent already read it", 4);
                        }
                        return;
                    }
                    self.t.notice(&level, &text, src, code.as_deref(), now_ms);
                    if src != Some("kernel") && (text.starts_with("[root]") || text.starts_with("PANIC")) {
                        let _ = self.poll.send(PollCmd::Status);
                    }
                    if code.as_deref() == Some("ceiling_hit") {
                        let _ = self.poll.send(PollCmd::Egress);
                    }
                    if matches!(code.as_deref(), Some("access" | "snapshot")) {
                        let _ = self.poll.send(PollCmd::Status); // the workspace and FULL (ADR-009)
                    }
                }
                Frame::State(s) => {
                    let was = self.info.busy();
                    let outs = self.tray.on_state(&s);
                    self.info.on_state(s, now);
                    for o in outs {
                        self.tray_out(o);
                    }
                    if was && !self.info.busy() {
                        self.t.end_activity(now); // the run ended: the activity freezes to its summary
                        for o in self.tray.flush_untracked() {
                            self.tray_out(o);
                        }
                    }
                }
                Frame::ReplayEnd { .. } | Frame::Answer(_) | Frame::Unknown => {}
            },
        }
    }

    /// STEER: a line leaves the tray: read (into the transcript, marked seen), delivered (an older
    /// kernel), or taken back (its text returns to the box when the box is empty).
    fn tray_out(&mut self, o: TrayOut) {
        let now_ms = Self::now_ms();
        match o {
            TrayOut::Seen(it) => {
                let st = if it.follow_up { OwnerSt::Delivered } else { OwnerSt::Seen };
                self.t.push(Kind::Owner { id: it.id, text: it.text, st }, now_ms);
            }
            TrayOut::Delivered(it) => {
                self.t.push(Kind::Owner { id: it.id, text: it.text, st: OwnerSt::Delivered }, now_ms);
            }
            TrayOut::Retracted(it) => {
                if self.ed.is_empty() {
                    self.ed.set(&it.text);
                    self.set_hint("taken back: it was not read · edit it, Enter sends it again", 4);
                } else {
                    self.ed.push_history(&it.text);
                    self.set_hint("taken back: it was not read (Up recalls it)", 4);
                }
            }
        }
        self.dirty = true;
    }

    /// Esc while busy: with a steer still pending, interrupt *and* steer (the body aborts, then
    /// continues the run with the steer); a second Esc within 1 s, or nothing pending: just stop.
    fn esc(&mut self) {
        let now = Instant::now();
        let again = self.esc_steer_at.is_some_and(|t| now.duration_since(t) <= Duration::from_secs(1));
        if self.tray.has_pending_steer() && !again {
            self.esc_steer_at = Some(now);
            self.send_frame(&proto::interrupt_steer());
            self.set_hint("interrupting · your note goes in now (Esc again: just stop)", 3);
        } else {
            self.esc_steer_at = None;
            self.send_frame(&proto::interrupt());
            self.set_hint("interrupt sent", 2);
        }
    }

    /// Up on an empty box with lines waiting: take back the newest one the body has not read.
    fn retract(&mut self) {
        match self.tray.retract_newest() {
            Ok(seq) => {
                if self.send_frame(&proto::retract(seq)) {
                    self.set_hint("taking back the newest line…", 3);
                } else {
                    self.tray.retract_refused(seq);
                    self.set_hint("the connection dropped: nothing was taken back", 4);
                }
            }
            Err(why) => self.set_hint(why, 4),
        }
    }

    fn ctrl_c(&mut self) {
        let now = Instant::now();
        if self.ctrl_c_at.is_some_and(|t| now.duration_since(t) <= Duration::from_secs(2)) {
            return self.quit();
        }
        self.ctrl_c_at = Some(now);
        self.send_frame(&proto::interrupt());
        let tail = if self.info.kernel_ready() { "" } else { " (no body is running)" };
        self.set_hint(format!("interrupt sent{tail} · Ctrl-C again within 2 s quits init (the agent keeps running)"), 2);
    }

    fn quit(&mut self) {
        self.send_frame(&proto::detach());
        self.exit = Some(0);
    }

    fn key(&mut self, k: crossterm::event::KeyEvent) {
        if let Some(vs) = self.view.as_mut() {
            let page = transcript::page(self.h as usize);
            let last = self.t.len().saturating_sub(1);
            match keys::view(&k) {
                View::Close => self.leave_view(),
                View::Up => vs.sel = vs.sel.saturating_sub(1),
                View::Down => vs.sel = (vs.sel + 1).min(last),
                View::PageUp => vs.sel = vs.sel.saturating_sub(page),
                View::PageDown => vs.sel = (vs.sel + page).min(last),
                View::Top => vs.sel = self.t.base,
                View::Bottom => vs.sel = last,
                View::Toggle => {
                    let sel = vs.sel;
                    if let Some(b) = self.t.get_mut(sel) {
                        b.expanded = !b.expanded;
                    }
                }
                View::ExpandAll | View::CollapseAll => {
                    let e = keys::view(&k) == View::ExpandAll;
                    for b in self.t.blocks.iter_mut() {
                        b.expanded = e;
                    }
                }
                View::None => {}
            }
            return;
        }
        let cx = keys::Ctx { empty: self.ed.is_empty(), busy: self.info.busy(), popup: self.ed.popup_open(), overlay: self.overlay };
        let a = keys::map(&k, &cx);
        if self.secret.is_some() {
            return self.secret_key(&k, a);
        }
        if self.overlay && a != Action::None {
            self.overlay = false;
            if matches!(a, Action::ClosePopup | Action::Help) {
                return;
            }
        }
        match a {
            Action::Insert(c) => {
                let mut b = [0u8; 4];
                self.ed.insert(c.encode_utf8(&mut b));
            }
            Action::Newline => self.ed.newline(),
            Action::Submit => self.submit(),
            Action::Interrupt => self.ctrl_c(),
            Action::EscInterrupt => self.esc(),
            Action::QueueLater => self.submit_later(),
            Action::ClosePopup => self.ed.popup_dismissed = true,
            Action::Detach => self.quit(),
            Action::Backspace => self.ed.backspace(),
            Action::Delete => self.ed.delete(),
            Action::Left => self.ed.left(),
            Action::Right => self.ed.right(),
            Action::Home => self.ed.home(),
            Action::End => self.ed.end(),
            Action::WordLeft => self.ed.word_left(),
            Action::WordRight => self.ed.word_right(),
            Action::DeleteWord => self.ed.delete_word(),
            Action::KillToStart => self.ed.kill_to_start(),
            Action::KillToEnd => self.ed.kill_to_end(),
            Action::Up => {
                if self.ed.popup_open() {
                    self.ed.popup_move(false)
                } else if self.ed.is_empty() && !self.tray.is_empty() {
                    self.retract()
                } else {
                    self.ed.up()
                }
            }
            Action::Down => {
                if self.ed.popup_open() {
                    self.ed.popup_move(true)
                } else {
                    self.ed.down()
                }
            }
            Action::Complete => {
                self.ed.complete();
            }
            Action::Transcript => self.enter_view(),
            Action::ToggleThinking => {
                self.verbose = !self.verbose;
                self.set_hint(if self.verbose { "thinking shown" } else { "thinking hidden" }, 2);
            }
            Action::Redraw => self.need_full = true,
            Action::Help => self.overlay = true,
            Action::None => {}
        }
    }

    /// The hidden key field (UX-KEYMODEL): typed and pasted bytes go to `secret`; the box shows only a mask.
    fn secret_key(&mut self, k: &crossterm::event::KeyEvent, a: Action) {
        match a {
            _ if k.code == crossterm::event::KeyCode::Esc => {
                let outs = self.setup.cancel();
                self.setup_outs(outs);
            }
            Action::Insert(c) => {
                let mut b = [0u8; 4];
                self.secret_input(c.encode_utf8(&mut b));
            }
            Action::Backspace => {
                if let Some(s) = self.secret.as_mut() {
                    while let Some(b) = s.pop() {
                        if b & 0xC0 != 0x80 {
                            break; // a whole UTF-8 character removed
                        }
                    }
                }
                self.mask();
            }
            Action::Submit => {
                let s = self.secret.take().unwrap_or_default();
                self.ed.clear();
                let outs = self.setup.on_secret(s);
                self.setup_outs(outs);
            }
            Action::Interrupt => {
                let outs = self.setup.cancel();
                self.setup_outs(outs);
                self.ctrl_c();
            }
            Action::Detach => {
                let outs = self.setup.cancel();
                self.setup_outs(outs);
                self.quit();
            }
            _ => {}
        }
    }

    fn secret_input(&mut self, s: &str) {
        if let Some(b) = self.secret.as_mut() {
            // never past the preallocated buffer, so it never reallocates and leaves no copy (ux-v1 §3.4.5)
            let room = b.capacity().saturating_sub(b.len());
            b.extend_from_slice(&s.as_bytes()[..s.len().min(room)]);
        }
        self.mask();
    }

    fn mask(&mut self) {
        let n = self.secret.as_ref().map_or(0, |b| String::from_utf8_lossy(b).chars().count());
        self.ed.set(&"•".repeat(n));
    }

    /// Carries out the setup flow's outputs (UX-KEYMODEL).
    fn setup_outs(&mut self, outs: Vec<setup::Out>) {
        let now_ms = Self::now_ms();
        for o in outs {
            match o {
                setup::Out::Send(req) => conn::spawn_setup(conn::sock(&self.o.root_home), req, self.tx.clone()),
                setup::Out::SendKey { provider, secret } => conn::spawn_key(conn::sock(&self.o.root_home), provider, secret, self.tx.clone()),
                setup::Out::Info(t) => self.info_block(format!("[setup] {t}")),
                setup::Out::Warn(t) => self.t.notice("warn", &format!("[setup] {t}"), Some("tui"), None, now_ms),
                setup::Out::Error(t) => {
                    self.t.push(Kind::Error(format!("[setup] {t}")), now_ms);
                }
                setup::Out::Secret(true) => {
                    self.ed.clear();
                    self.secret = Some(Vec::with_capacity(1024));
                }
                setup::Out::Secret(false) => {
                    if let Some(mut b) = self.secret.take() {
                        setup::zero(&mut b);
                    }
                    self.ed.clear();
                }
            }
        }
        if self.setup.secret_mode() && self.secret.is_none() {
            self.secret = Some(Vec::with_capacity(1024)); // asked again (an empty or refused key)
            self.ed.clear();
        }
        self.dirty = true;
    }

    fn submit(&mut self) {
        if self.ed.backslash_newline() {
            return;
        }
        let text = proto::canonical(self.ed.text());
        if text.trim().is_empty() {
            return;
        }
        if text.trim() == "/usage" {
            self.ed.push_history(&text);
            self.ed.clear();
            self.info_block(crate::render::status::usage_summary(&self.info));
            return;
        }
        if text.trim() == "/details" {
            self.ed.push_history(&text);
            self.ed.clear();
            self.details();
            return;
        }
        let kind = proto::line_kind(&text, self.info.rootish());
        let now = Instant::now();
        if kind != LineKind::Panic && self.acc.holds_lines() {
            self.set_hint("waiting for the Root (workspace or access request); your text is kept", 3);
            return;
        }
        if kind != LineKind::Panic
            && let Some(outs) = self.acc.on_line(&text)
        {
            let id = self.echoes.new_id();
            self.t.push(Kind::Owner { id, text: text.clone(), st: OwnerSt::Command }, Self::now_ms());
            self.ed.push_history(&text);
            self.ed.clear();
            self.hint = None;
            self.access_outs(outs);
            return;
        }
        if kind != LineKind::Panic && self.setup.busy() {
            self.set_hint("waiting for the Root (setup request); your text is kept", 3);
            return;
        }
        if kind != LineKind::Panic
            && let Some(outs) = self.setup.on_line(&text)
        {
            let id = self.echoes.new_id();
            self.t.push(Kind::Owner { id, text: text.clone(), st: OwnerSt::Command }, Self::now_ms());
            self.ed.push_history(&text);
            self.ed.clear();
            self.hint = None;
            self.setup_outs(outs);
            return;
        }
        if kind == LineKind::Panic {
            self.ed.push_history(&text);
            self.ed.clear();
            let id = self.echoes.new_id();
            self.t.push(Kind::Owner { id, text: "/panic".into(), st: OwnerSt::Command }, Self::now_ms());
            if !(self.feed.is_live() && self.send_frame(&proto::panic_frame())) {
                conn::panic_fallback(self.o.root_home.clone(), self.tx.clone());
                self.info_block("/panic: the attach connection is down; sending the panic request on a fresh connection");
            }
            let _ = self.poll.send(PollCmd::Status);
            return;
        }
        let frame = proto::line(&text);
        if frame.len() > proto::LINE_MAX {
            self.set_hint(format!("too long to send ({} KiB > 900 KiB); split it", frame.len() / 1024), 5);
            return;
        }
        if self.writer.is_none() {
            self.set_hint("not connected to the Root: nothing was sent; your text is kept", 4);
            return;
        }
        if !self.send_frame(&frame) {
            self.set_hint("the connection dropped: nothing was sent; your text is kept", 4);
            return;
        }
        let (id, st) = self.echoes.sent(&text, kind, now);
        if kind == LineKind::Input && self.info.busy() {
            // STEER: a line typed while the agent works waits in the tray until the body reads it
            match text.trim().strip_prefix("/later ") {
                Some(rest) => self.tray.add(id, rest, true, now),
                None => self.tray.add(id, &text, false, now),
            }
        } else {
            self.t.push(Kind::Owner { id, text: text.clone(), st }, Self::now_ms());
        }
        self.ed.push_history(&text);
        self.ed.clear();
        if kind == LineKind::RootCommand {
            let _ = self.poll.send(PollCmd::Status);
        }
    }

    /// STEER: Tab while busy: the box goes out as `/later <text>`, the next task after this run.
    fn submit_later(&mut self) {
        let text = proto::canonical(self.ed.text());
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let line = format!("/later {body}");
        let frame = proto::line(&line);
        if frame.len() > proto::LINE_MAX {
            self.set_hint(format!("too long to send ({} KiB > 900 KiB); split it", frame.len() / 1024), 5);
            return;
        }
        if self.writer.is_none() || !self.send_frame(&frame) {
            self.set_hint("not connected to the Root: nothing was sent; your text is kept", 4);
            return;
        }
        let now = Instant::now();
        let (id, _) = self.echoes.sent(&line, LineKind::Input, now);
        self.tray.add(id, body, true, now);
        self.ed.push_history(&text);
        self.ed.clear();
        self.set_hint("queued as the next task", 2);
    }

    /// Carries out the access flow's outputs (ADR-009).
    fn access_outs(&mut self, outs: Vec<Out>) {
        let now_ms = Self::now_ms();
        for o in outs {
            match o {
                Out::Send(req) => conn::spawn_access(conn::sock(&self.o.root_home), req, self.tx.clone()),
                Out::Access(v) => self.info.on_access(v),
                Out::Notice(t) => self.t.notice("warn", &t, Some("tui"), None, now_ms),
                Out::Info(t) => self.info_block(t),
                Out::Warn(t) => self.t.notice("warn", &t, Some("tui"), None, now_ms),
                Out::Error(t) => {
                    self.t.push(Kind::Error(t), now_ms);
                }
                Out::Ask(t) => self.info_block(t),
            }
        }
        self.dirty = true;
    }

    /// `/details`: the last tool activity printed in full (§3.6).
    fn details(&mut self) {
        if let Err(e) = self.t.details(Self::now_ms()) {
            self.set_hint(e, 4);
        }
    }

    fn enter_view(&mut self) {
        let mut out = io::stdout();
        let _ = out.queue(EnterAlternateScreen);
        let _ = out.flush();
        term::set_alt(true);
        // Ctrl-O opens on the last tool activity, expanded (§3.6); otherwise on the newest block
        let sel = match self.t.last_activity() {
            Some(i) => {
                if let Some(b) = self.t.get_mut(i) {
                    b.expanded = true;
                }
                i
            }
            None => self.t.len().saturating_sub(1),
        };
        self.view = Some(ViewState { sel, top: sel });
    }

    fn leave_view(&mut self) {
        let mut out = io::stdout();
        let _ = out.queue(LeaveAlternateScreen);
        let _ = out.flush();
        term::set_alt(false);
        self.view = None;
        self.need_full = true;
    }

    fn resize(&mut self, w: u16, h: u16) {
        let old_w = self.w.max(1) as usize;
        self.w = w;
        self.h = h;
        if self.view.is_some() {
            self.dirty = true;
            return;
        }
        // the rows above the caret, after the terminal's own reflow of the old live region
        let nw = (w as usize).max(1);
        let above: usize = self.prev.iter().take(self.prev_cursor_row).map(|l| if nw < old_w { l.width().div_ceil(nw).max(1) } else { 1 }).sum();
        let mut out = io::stdout();
        match crossterm::cursor::position() {
            Ok((_, cy)) => {
                let top = (cy as usize).saturating_sub(above) as u16;
                let _ = out.queue(MoveTo(0, top));
            }
            Err(_) => {
                let _ = out.queue(MoveToColumn(0));
                if above > 0 {
                    let _ = out.queue(MoveToPreviousLine(above.min(u16::MAX as usize) as u16));
                }
            }
        }
        let _ = out.queue(Clear(ClearType::FromCursorDown));
        let _ = out.flush();
        self.prev.clear();
        self.prev_cursor_row = 0;
        self.need_full = true;
        self.dirty = true;
    }

    // ---------------------------------------------------------------------------------------
    // Rendering
    // ---------------------------------------------------------------------------------------

    fn animating(&self) -> bool {
        self.info.busy() || self.t.activity_live() || self.t.live().any(|(_, b)| matches!(&b.kind, Kind::Thinking { done: false, .. }))
    }

    pub fn render<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        let now = Instant::now();
        let now_ms = Self::now_ms();
        let (w, h) = (self.w.max(1) as usize, self.h.max(1) as usize);
        let spin = live::spin_at(self.t0, now);
        if self.hint.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.hint = None;
        }
        if let Some(mut vs) = self.view {
            let rows = transcript::render(&self.t, &mut vs, w, h, self.verbose, now);
            self.view = Some(vs);
            out.queue(BeginSynchronizedUpdate)?;
            out.queue(MoveTo(0, 0))?;
            for (i, r) in rows.iter().enumerate() {
                term::write_line(out, r, self.o.no_color)?;
                out.queue(Clear(ClearType::UntilNewLine))?;
                if i + 1 < rows.len() {
                    out.write_all(b"\r\n")?;
                }
            }
            out.queue(Clear(ClearType::FromCursorDown))?;
            out.queue(EndSynchronizedUpdate)?;
            out.flush()?;
            self.after_flush(now);
            return Ok(());
        }
        let max_live = {
            let ui = Ui { t: &self.t, tray: &self.tray, ed: &self.ed, info: &self.info, verbose: self.verbose, overlay: self.overlay, hint: None, now, now_ms, no_color: self.o.no_color, spin };
            live::max_live(&ui, w, h)
        };
        let commits = live::commit(&mut self.t, w, max_live, self.verbose, now, spin);
        // while the Root waits for a typed confirmation, the hint says so (ADR-009)
        let hint = self.hint.as_ref().map(|(s, _)| s.clone()).or_else(|| self.acc.awaiting().map(|p| format!("type {p} and press Enter to confirm; anything else cancels")));
        let ui = Ui { t: &self.t, tray: &self.tray, ed: &self.ed, info: &self.info, verbose: self.verbose, overlay: self.overlay, hint: hint.as_deref(), now, now_ms, no_color: self.o.no_color, spin };
        let region = live::region(&ui, w, h);
        out.queue(BeginSynchronizedUpdate)?;
        term::hide_cursor(out)?;
        if self.prev_cursor_row > 0 {
            out.queue(MoveToPreviousLine(self.prev_cursor_row as u16))?;
        } else {
            out.queue(MoveToColumn(0))?;
        }
        let full = self.need_full || !commits.is_empty();
        for l in &commits {
            term::write_line(out, l, self.o.no_color)?;
            out.queue(Clear(ClearType::UntilNewLine))?;
            out.write_all(b"\r\n")?;
        }
        let n = region.rows.len();
        for (i, l) in region.rows.iter().enumerate() {
            if full || self.prev.get(i) != Some(l) {
                term::write_line(out, l, self.o.no_color)?;
                out.queue(Clear(ClearType::UntilNewLine))?;
            }
            if i + 1 < n {
                out.write_all(b"\r\n")?;
            }
        }
        out.queue(Clear(ClearType::FromCursorDown))?;
        let (cx, cy) = region.cursor.unwrap_or((0, n.saturating_sub(1)));
        let up = n.saturating_sub(1).saturating_sub(cy);
        if up > 0 {
            out.queue(MoveToPreviousLine(up as u16))?;
        }
        out.queue(MoveToColumn(cx as u16))?;
        out.queue(Show)?;
        out.queue(EndSynchronizedUpdate)?;
        out.flush()?;
        self.prev = region.rows;
        self.prev_cursor_row = cy.min(n.saturating_sub(1));
        self.need_full = false;
        self.after_flush(now);
        Ok(())
    }

    fn after_flush(&mut self, started: Instant) {
        let done = Instant::now();
        for at in self.pending_keys.drain(..) {
            Latency::push(&mut self.lat.key, done.duration_since(at));
        }
        for at in self.pending_frames.drain(..) {
            Latency::push(&mut self.lat.frame, done.duration_since(at));
        }
        if self.need_first_paint {
            self.need_first_paint = false;
            self.lat.first_paint_ms = Some(done.duration_since(self.t0).as_millis() as u64);
        }
        self.lat.renders += 1;
        self.last_render = Some(started);
        self.dirty = false;
        self.t.dirty = false;
    }

    /// When the loop should wake up next, if nothing arrives.
    fn next_wake(&self, now: Instant) -> Duration {
        let mut d = Duration::from_secs(1);
        if self.dirty || self.t.dirty {
            let since = self.last_render.map_or(Duration::MAX, |t| now.duration_since(t));
            d = d.min(Duration::from_millis(8).saturating_sub(since));
        }
        if self.animating() {
            d = d.min(Duration::from_millis(100));
        }
        if let Some(t) = self.retry_at {
            d = d.min(t.saturating_duration_since(now));
        }
        if let Some((_, t)) = &self.hint {
            d = d.min(t.saturating_duration_since(now));
        }
        if matches!(self.info.conn, Conn::Down { .. }) {
            d = d.min(Duration::from_millis(100));
        }
        d
    }

    /// The loop: wait, drain, render (at most once per 8 ms).
    pub fn run_loop<W: Write>(&mut self, rx: &Receiver<Ev>, out: &mut W) -> i32 {
        loop {
            let now = Instant::now();
            if self.retry_at.is_some_and(|t| t <= now) && !self.connecting && self.writer.is_none() {
                self.start_attach();
            }
            let wake = self.next_wake(now);
            match rx.recv_timeout(wake) {
                Ok(ev) => {
                    self.handle(ev);
                    let mut n = 0;
                    while let Ok(ev) = rx.try_recv() {
                        self.handle(ev);
                        n += 1;
                        if n > 10_000 || self.exit.is_some() {
                            break;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.animating() || matches!(self.info.conn, Conn::Down { .. }) {
                        self.dirty = true;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return 1,
            }
            if let Some(code) = self.exit {
                let _ = self.render(out);
                return code;
            }
            let now = Instant::now();
            // frames are coalesced (one render per 8 ms); a keystroke renders at once (§6: p50 ≤ 4 ms)
            let due = self.last_render.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(8)) || !self.pending_keys.is_empty();
            if (self.dirty || self.t.dirty || self.need_full) && due && self.render(out).is_err() {
                return 1;
            }
        }
    }
}

/// The input thread: crossterm events, polled so that a cursor-position query can take the reader
/// in between (the resize path).
fn spawn_input(tx: Sender<Ev>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match event::poll(Duration::from_millis(20)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    let _ = tx.send(Ev::Signal(signal_hook::consts::SIGHUP)); // the terminal went away
                    return;
                }
            }
            let ev = match event::read() {
                Ok(Event::Key(k)) => Ev::Key(k, Instant::now()),
                Ok(Event::Paste(s)) => Ev::Paste(s, Instant::now()),
                Ok(Event::Resize(w, h)) => Ev::Resize(w, h),
                Ok(_) => continue,
                Err(_) => Ev::Signal(signal_hook::consts::SIGHUP),
            };
            if tx.send(ev).is_err() {
                return;
            }
        }
    });
}

pub fn spawn_signals(tx: Sender<Ev>) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let Ok(mut sigs) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) else { return };
    std::thread::spawn(move || {
        for s in sigs.forever() {
            if tx.send(Ev::Signal(s)).is_err() {
                return;
            }
        }
    });
}

/// TTY mode. Returns the exit code.
pub fn run(o: Opts) -> i32 {
    let latency = o.latency;
    let (tx, rx) = mpsc::channel();
    let (ptx, prx) = mpsc::channel();
    term::install_panic_hook();
    if let Err(e) = term::enter() {
        term::restore();
        eprintln!("init-tui: cannot set up the terminal: {e}");
        return 1;
    }
    let (w, h) = terminal::size().unwrap_or((80, 24));
    let mut app = App::new(o, tx.clone(), ptx.clone(), w, h);
    conn::spawn_poller(conn::sock(&app.o.root_home), prx, tx.clone());
    let stop = Arc::new(AtomicBool::new(false));
    spawn_input(tx.clone(), stop.clone());
    spawn_signals(tx);
    let mut out = io::BufWriter::with_capacity(64 * 1024, io::stdout());
    let code = app.run_loop(&rx, &mut out);
    let _ = out.flush();
    stop.store(true, Ordering::SeqCst);
    let _ = ptx.send(PollCmd::Stop);
    term::restore();
    // leave the cursor below the live region
    println!();
    if latency {
        eprint!("{}", app.lat.report());
    }
    code
}

