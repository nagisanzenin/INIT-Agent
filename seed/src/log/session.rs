//! A session: its file, writer and index, plus rehydration (log-context.md §3.5, §3.6).
//!
//! Boot order (body/boot.md owns it; WP13 wires it):
//! 1. `SessionLog::open` — §3.6 steps 1–6 and 9 (session id, create if missing, pointer, lock, scan,
//!    torn-tail repair, leaf, path, synthetic results for interrupted tool rounds).
//! 2. `context::Context::new` — steps 7, 8, 10, 11 (epoch, active window, calibration, head + hash).
//! 3. `Context::append_boot` — steps 12 and 13 (boot event + sync D3, corrupt-line error/display).

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::clock::{IdGen, new_session_id, valid_session_id};
use super::event::{
    Block, DisplayEv, ErrorEv, Event, EventBody, Kind, LogRepair, Origin, SessionEv, Skipped,
    ToolResultEv,
};
use super::index::Index;
use super::writer::{LogError, LogWriter, Opened, read_pointer, write_pointer};
use super::{INTERRUPTED_TEXT, K0_PPM, LOCK_WAIT, NOT_STARTED_TEXT, tokens_for_bytes};

/// A call without a result: (call_id, tool name, its `tool_call` marker exists).
type OpenCall = (String, String, bool);

pub fn sessions_dir(home: &Path) -> PathBuf {
    home.join("state").join("sessions")
}

pub fn session_file(home: &Path, sid: &str) -> PathBuf {
    sessions_dir(home).join(format!("{sid}.jsonl"))
}

/// What opening found and did, for the boot event (`log_repair`, `interrupted_calls`, …).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenReport {
    pub sid: String,
    /// `Some("cold")` when the session file was created by this open.
    pub created: Option<String>,
    pub repair: LogRepair,
    /// Some skipped line lies before the last valid event (not just a torn tail): warn the owner.
    pub skipped_before_last_valid: bool,
    pub interrupted: u64,
    pub not_started: u64,
    /// Valid events on the path at open, before synthetic results.
    pub rehydrated_events: u64,
    /// `request.k_ppm` of the newest assistant message on the path (§3.14), if any.
    pub k_ppm: Option<u64>,
    /// `display` warnings to append after the boot event.
    pub warnings: Vec<String>,
}

impl OpenReport {
    pub fn interrupted_calls(&self) -> u64 {
        self.interrupted + self.not_started
    }
}

/// Parameters for opening the session at boot.
pub struct OpenParams<'a> {
    pub home: &'a Path,
    /// `INIT_SESSION`; empty means "start a new session" (tests and manual runs only).
    pub env_session: &'a str,
    /// Body sha for a new `session` header (`INIT_SHA`).
    pub sha: &'a str,
    pub lock_wait: Duration,
}

pub struct SessionLog {
    pub home: PathBuf,
    pub writer: LogWriter,
    pub index: Index,
}

impl SessionLog {
    /// §3.6 steps 1–6 and 9.
    pub fn open(p: &OpenParams) -> Result<(SessionLog, OpenReport), LogError> {
        let dir = sessions_dir(p.home);
        std::fs::create_dir_all(&dir)?;
        let state = p.home.join("state");
        let mut ids = IdGen::new();
        let mut rep = OpenReport::default();
        // Step 1: the session id.
        let sid = if p.env_session.is_empty() {
            new_session_id(&mut ids)
        } else if valid_session_id(p.env_session) {
            p.env_session.to_string()
        } else {
            return Err(LogError::Io(io::Error::other(format!(
                "log: invalid INIT_SESSION {:?}",
                p.env_session
            ))));
        };
        let path = session_file(p.home, &sid);
        let opened = if path.exists() {
            LogWriter::open(&path, &sid, p.lock_wait)?
        } else {
            if !p.env_session.is_empty() {
                rep.warnings.push(format!(
                    "The log of session {sid} was missing, so a new empty log was started under the same id."
                ));
            }
            rep.created = Some("cold".into());
            LogWriter::create(
                &path,
                &sid,
                &session_header(&sid, p.sha, "cold", None),
                p.lock_wait,
            )?
        };
        if read_pointer(&state).as_deref() != Some(sid.as_str()) {
            write_pointer(&state, &sid)?;
        }
        let Opened {
            writer,
            index,
            scan,
            newline_appended,
        } = opened;
        let mut log = SessionLog {
            home: p.home.to_path_buf(),
            writer,
            index,
        };
        // An existing but empty (or fully corrupt) file gets a header, so the session has a root.
        if log.index.is_empty() {
            let h = session_header(&sid, p.sha, "cold", None);
            log.writer.append(&h, None, &mut log.index)?;
            log.writer.sync()?;
        }
        rep.sid = sid;
        rep.repair = LogRepair {
            newline_appended,
            skipped: scan.skipped.clone(),
        };
        let last_valid_off = log.index.last().map(|e| e.off).unwrap_or(0);
        rep.skipped_before_last_valid = scan.skipped.iter().any(|s| s.offset < last_valid_off);
        // Steps 5–6: leaf and path.
        let path = log.index.path(log.writer.leaf());
        rep.rehydrated_events = path.len() as u64;
        // Step 9 (and the k of step 10, needed for tokens_est of the synthetic results).
        log.close_interrupted_rounds(&path, &mut rep)?;
        Ok((log, rep))
    }

    pub fn sid(&self) -> &str {
        self.writer.sid()
    }

    pub fn leaf(&self) -> Option<u64> {
        self.writer.leaf()
    }

    /// Loads the event at index position `pos` from disk.
    pub fn load(&self, pos: usize) -> io::Result<Event> {
        let e = self.index.ents()[pos];
        self.writer.read_event(e.off, e.len)
    }

    pub fn load_id(&self, id: u64) -> io::Result<Option<Event>> {
        match self.index.pos(id) {
            Some(p) => self.load(p).map(Some),
            None => Ok(None),
        }
    }

    pub fn append(&mut self, body: &EventBody) -> io::Result<u64> {
        self.writer.append(body, None, &mut self.index)
    }

    pub fn append_with_parent(&mut self, body: &EventBody, parent: u64) -> io::Result<u64> {
        self.writer.append(body, Some(parent), &mut self.index)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.writer.sync()
    }

    /// Path positions from the root to the current leaf.
    pub fn path(&self) -> Vec<usize> {
        self.index.path(self.writer.leaf())
    }

    /// §3.6 step 9: synthetic results for tool calls without a result, in block order, before boot.
    fn close_interrupted_rounds(&mut self, path: &[usize], rep: &mut OpenReport) -> io::Result<()> {
        // Walk the path backwards. Results and markers between an assistant message and the next one
        // belong to that message (the loop appends a batch's results before the next request).
        let mut results: HashSet<String> = HashSet::new();
        let mut markers: HashSet<(String, u64)> = HashSet::new();
        let mut open: Vec<(u64, Vec<OpenCall>)> = Vec::new();
        for &pos in path.iter().rev() {
            let kind = self.index.ents()[pos].kind;
            if !matches!(kind, Kind::Message | Kind::ToolCall | Kind::ToolResult) {
                continue;
            }
            let ev = self.load(pos)?;
            match &ev.body {
                EventBody::ToolResult(r) => {
                    results.insert(r.call_id.clone());
                }
                EventBody::ToolCall(c) => {
                    if let Some(mid) = super::clock::parse_id(&c.message_id) {
                        markers.insert((c.call_id.clone(), mid));
                    }
                }
                EventBody::Message(m) if m.is_assistant() => {
                    if rep.k_ppm.is_none() {
                        rep.k_ppm = m.request.as_ref().and_then(|r| r.k_ppm);
                    }
                    let missing: Vec<OpenCall> = m
                        .tool_uses()
                        .filter(|(cid, _, _)| !results.contains(*cid))
                        .map(|(cid, name, _)| {
                            (
                                cid.to_string(),
                                name.to_string(),
                                markers.contains(&(cid.to_string(), ev.id)),
                            )
                        })
                        .collect();
                    if !missing.is_empty() {
                        open.push((ev.id, missing));
                    }
                    results.clear();
                    markers.clear();
                }
                _ => {}
            }
        }
        let k = rep.k_ppm.unwrap_or(K0_PPM);
        for (_, calls) in open.into_iter().rev() {
            for (call_id, name, started) in calls {
                let (text, synthetic) = if started {
                    (INTERRUPTED_TEXT, "interrupted")
                } else {
                    (NOT_STARTED_TEXT, "not_started")
                };
                if started {
                    rep.interrupted += 1;
                } else {
                    rep.not_started += 1;
                }
                let body = EventBody::ToolResult(ToolResultEv {
                    call_id,
                    name,
                    output: vec![Block::text(text)],
                    is_error: true,
                    spill_path: None,
                    exit_code: None,
                    signal: None,
                    wall_ms: None,
                    tokens_est: tokens_for_bytes(k, text.len() as u64),
                    synthetic: Some(synthetic.into()),
                });
                self.append(&body)?;
            }
        }
        if rep.interrupted_calls() > 0 {
            self.sync()?;
        }
        Ok(())
    }

    /// §3.6 step 13: after the boot event, report skipped lines (`error`, and a `display` warning when a
    /// skipped line lies before the last valid event) and any open warnings. Then sync.
    pub fn append_repair_reports(&mut self, rep: &OpenReport) -> io::Result<()> {
        if !rep.repair.skipped.is_empty() {
            let skipped: Vec<serde_json::Value> = rep
                .repair
                .skipped
                .iter()
                .map(|s| serde_json::json!({"offset": s.offset, "len": s.len}))
                .collect();
            let n = rep.repair.skipped.len();
            self.append(&EventBody::Error(ErrorEv {
                where_: "log".into(),
                code: "corrupt_line".into(),
                message: format!("{n} unreadable line(s) in the session log were skipped; the bytes are left in place"),
                data: Some(serde_json::json!({"skipped": skipped, "newline_appended": rep.repair.newline_appended})),
            }))?;
            if rep.skipped_before_last_valid {
                self.append(&EventBody::Display(DisplayEv {
                    level: "warn".into(),
                    text: format!(
                        "The session log had {n} corrupt line(s) before its end. They were skipped; the conversation \
                         may have lost the events before the first gap."
                    ),
                }))?;
            }
        }
        for w in &rep.warnings {
            self.append(&EventBody::Display(DisplayEv {
                level: "warn".into(),
                text: w.clone(),
            }))?;
        }
        self.sync()
    }

    /// `/new` (§3.5): a new session file with `origin.reason:"new"`, the pointer updated, the new file
    /// locked and the old one released. Returns the new id. The old session is untouched.
    pub fn new_session(&mut self, sha: &str) -> Result<String, LogError> {
        let mut ids = IdGen::new();
        let mut sid = new_session_id(&mut ids);
        while session_file(&self.home, &sid).exists() {
            sid = new_session_id(&mut ids);
        }
        let from = self.sid().to_string();
        let header = session_header(&sid, sha, "new", Some(&from));
        let Opened { writer, index, .. } =
            LogWriter::create(&session_file(&self.home, &sid), &sid, &header, LOCK_WAIT)?;
        write_pointer(&self.home.join("state"), &sid)?;
        let mut old = std::mem::replace(&mut self.writer, writer);
        self.index = index;
        old.unlock()?;
        Ok(sid)
    }
}

pub fn session_header(sid: &str, sha: &str, reason: &str, from_session: Option<&str>) -> EventBody {
    EventBody::Session(SessionEv {
        format: 1,
        session_id: sid.to_string(),
        origin: Origin {
            reason: reason.to_string(),
            from_session: from_session.map(str::to_string),
            from_event: None,
        },
        sha: sha.to_string(),
    })
}

/// Skipped-line summary for tests and reports.
pub fn skipped_total(s: &[Skipped]) -> u64 {
    s.iter().map(|x| x.len).sum()
}

/// Stores binary content once under `state/blobs/<sha256 hex>.<ext>` (§3.2, content-addressed) and
/// returns `sha256:<hex>`, the `blob` value of an image block. An existing blob is left untouched.
pub fn store_blob(home: &Path, media_type: &str, bytes: &[u8]) -> io::Result<String> {
    use std::io::Write;
    let hex = crate::sha256::hex(&crate::sha256::digest(bytes));
    let ext = match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    };
    let dir = home.join("state").join("blobs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hex}.{ext}"));
    if !path.exists() {
        let tmp = dir.join(format!("{hex}.{ext}.tmp{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
    }
    Ok(format!("sha256:{hex}"))
}
