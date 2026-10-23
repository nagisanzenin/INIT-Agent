//! The stdio-jsonl/1 frames (kernel/supervise.md §4.5; loop.md §4.1), the input thread that turns
//! kernel frames into `Control`, and the output thread that writes body frames.

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::status::Status;
use crate::model::Abort;
use crate::tools::{ExtHostApi, ReloadKind};

/// Output queue bound (loop.md §3.1).
pub const OUT_CAP: usize = 1024;
/// `stop` grace default (supervise §4.5).
pub const STOP_GRACE_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Steer,
    FollowUp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputFrame {
    pub id: String,
    pub seq: u64,
    pub text: String,
    pub ts: Option<String>,
    pub mode: InputMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PauseReq {
    pub token: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReloadJob {
    pub job: String,
    pub kind: ReloadKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub kind: String,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Input(InputFrame),
    Interrupt,
    Pause(PauseReq),
    Resume {
        token: String,
        update_failed: Option<Value>,
    },
    Exit {
        token: String,
        code: i32,
    },
    Stop {
        reason: String,
        grace_ms: u64,
    },
    Reload(ReloadJob),
    Notice(Notice),
    /// stdin closed: the kernel is gone.
    Eof,
}

fn s(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(str::to_string)
}

/// One kernel frame. `Ok(None)`: an unknown `t` (ignored, §4.5).
pub fn parse_frame(line: &str) -> Result<Option<Control>, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Object(m) = v else {
        return Err("not a JSON object".into());
    };
    let t = s(&m, "t").ok_or("missing t")?;
    let need = |k: &str| s(&m, k).ok_or_else(|| format!("{t}: missing {k}"));
    Ok(Some(match t.as_str() {
        "input" => Control::Input(InputFrame {
            id: need("id")?,
            seq: m
                .get("seq")
                .and_then(Value::as_u64)
                .ok_or("input: missing seq")?,
            text: need("text")?,
            ts: s(&m, "ts"),
            mode: match m.get("mode").and_then(Value::as_str) {
                Some("follow_up") => InputMode::FollowUp,
                _ => InputMode::Steer, // missing or unknown mode steers (loop.md §3.4)
            },
        }),
        "interrupt" => Control::Interrupt,
        "pause" => Control::Pause(PauseReq {
            token: need("token")?,
            reason: s(&m, "reason").unwrap_or_else(|| "handoff".into()),
        }),
        "resume" => Control::Resume {
            token: need("token")?,
            update_failed: m.get("update_failed").filter(|v| !v.is_null()).cloned(),
        },
        "exit" => Control::Exit {
            token: need("token")?,
            code: m.get("code").and_then(Value::as_i64).unwrap_or(75) as i32,
        },
        "stop" => Control::Stop {
            reason: s(&m, "reason").unwrap_or_default(),
            grace_ms: m
                .get("grace_ms")
                .and_then(Value::as_u64)
                .unwrap_or(STOP_GRACE_MS),
        },
        "reload" => Control::Reload(ReloadJob {
            job: s(&m, "job").unwrap_or_default(),
            kind: match m.get("kind").and_then(Value::as_str) {
                Some("kept") => ReloadKind::Kept(
                    m.get("components")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                _ => ReloadKind::Request,
            },
        }),
        "notice" => Control::Notice(Notice {
            id: need("id")?,
            kind: s(&m, "kind").unwrap_or_else(|| "notice".into()),
            data: m.get("data").cloned().unwrap_or(json!({})),
        }),
        _ => return Ok(None),
    }))
}

/// Body → kernel frames.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Display {
        kind: &'static str,
        text: String,
        call_id: Option<String>,
        name: Option<String>,
        ok: Option<bool>,
    },
    Ack(u64),
    NoticeAck(String),
    State(bool),
    Paused {
        token: String,
        log_len: u64,
        leaf: String,
    },
    ReloadDone {
        job: String,
        ok: bool,
        prefix_changed: bool,
        components: Vec<String>,
        error: Option<String>,
    },
}

impl Out {
    pub fn display(kind: &'static str, text: impl Into<String>) -> Out {
        Out::Display {
            kind,
            text: text.into(),
            call_id: None,
            name: None,
            ok: None,
        }
    }

    /// One line, sorted keys, `"v":1`.
    pub fn to_value(&self) -> Value {
        let mut v = match self {
            Out::Display {
                kind,
                text,
                call_id,
                name,
                ok,
            } => {
                let mut m = json!({"t": "display", "kind": kind, "text": text});
                if let Some(c) = call_id {
                    m["call_id"] = c.clone().into();
                }
                if let Some(n) = name {
                    m["name"] = n.clone().into();
                }
                if let Some(o) = ok {
                    m["ok"] = (*o).into();
                }
                m
            }
            Out::Ack(seq) => json!({"t": "ack", "seq": seq}),
            Out::NoticeAck(id) => json!({"t": "notice_ack", "id": id}),
            Out::State(busy) => json!({"t": "state", "busy": busy}),
            Out::Paused {
                token,
                log_len,
                leaf,
            } => json!({"t": "paused", "token": token, "log_len": log_len, "leaf": leaf}),
            Out::ReloadDone {
                job,
                ok,
                prefix_changed,
                components,
                error,
            } => json!({"t": "reload_done", "job": job, "ok": ok, "prefix_changed": prefix_changed,
                         "components": components, "error": error}),
        };
        v["v"] = 1.into();
        v
    }
}

/// The bounded output queue. Deltas are dropped (and counted) when full; protocol frames block.
#[derive(Clone)]
pub struct OutTx {
    tx: SyncSender<Out>,
    dropped: Arc<AtomicU64>,
}

impl OutTx {
    pub fn channel(cap: usize) -> (OutTx, Receiver<Out>) {
        let (tx, rx) = mpsc::sync_channel(cap);
        (
            OutTx {
                tx,
                dropped: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }
    /// `display{text|thinking}` deltas: never block the model stream.
    pub fn delta(&self, o: Out) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(o) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// Every other frame: blocking, never lost while the kernel reads.
    pub fn send(&self, o: Out) {
        let _ = self.tx.send(o);
    }
    pub fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }
}

/// The output thread: one frame per line, flushed.
pub fn spawn_output<W: Write + Send + 'static>(
    mut w: W,
    rx: Receiver<Out>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("output".into())
        .spawn(move || {
            for o in rx {
                let line = o.to_value().to_string();
                if writeln!(w, "{line}").and_then(|_| w.flush()).is_err() {
                    // The kernel stopped reading: keep draining so senders never block forever.
                    continue;
                }
            }
        })
}

/// The input thread (loop.md §3.1): kernel frames → `ctl`. It triggers the abort itself on
/// `interrupt` and on EOF, starts reloads itself, and arms the `stop` grace timer.
pub fn spawn_input<R: BufRead + Send + 'static>(
    mut r: R,
    ctl: Sender<Control>,
    abort: Arc<Abort>,
    ext: Option<Arc<dyn ExtHostApi>>,
    status: Arc<Status>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                let c = match parse_frame(l) {
                    Ok(Some(c)) => c,
                    Ok(None) => continue,
                    Err(e) => {
                        eprintln!(
                            "{}",
                            json!({"type": "warning", "step": "stdin", "message": e})
                        );
                        continue;
                    }
                };
                match &c {
                    Control::Interrupt => abort.trigger(),
                    Control::Reload(job) => {
                        if let Some(x) = &ext {
                            x.begin_reload(job.kind.clone());
                        }
                    }
                    Control::Stop { grace_ms, .. } => {
                        let (a, st, g) = (abort.clone(), status.clone(), *grace_ms);
                        let _ = std::thread::Builder::new().name("stop-grace".into()).spawn(
                            move || {
                                std::thread::sleep(Duration::from_millis(g));
                                if st.busy() {
                                    a.trigger();
                                }
                            },
                        );
                    }
                    _ => {}
                }
                if ctl.send(c).is_err() {
                    return;
                }
            }
            // EOF: the kernel is gone. Interrupt, then the loop syncs and exits 0.
            abort.trigger();
            let _ = ctl.send(Control::Eof);
        })
}
