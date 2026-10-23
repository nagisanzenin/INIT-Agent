//! The single writer of a session file: lock, scan, torn-tail repair, append, durability
//! (log-context.md §3.4).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::clock::{IdGen, id_hex, now_rfc3339_ms};
use super::event::{Event, EventBody, Kind, Skipped};
use super::icj;
use super::index::Index;
use super::{LOCK_POLL, MAX_LINE_BYTES};

#[derive(Debug)]
pub enum LogError {
    /// `try_lock` kept failing with `WouldBlock` for the whole budget.
    Locked(String),
    Io(io::Error),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::Locked(sid) => write!(f, "log: session {sid} locked by another writer"),
            LogError::Io(e) => write!(f, "log: {e}"),
        }
    }
}

impl std::error::Error for LogError {}

impl From<io::Error> for LogError {
    fn from(e: io::Error) -> Self {
        LogError::Io(e)
    }
}

/// Bytes after the last LF at scan time.
#[derive(Clone, Debug, PartialEq)]
pub struct Tail {
    pub off: u64,
    pub len: u64,
    /// The tail parsed as a complete, new event (the crash hit just before the terminator).
    pub event: Option<(Event, Map<String, Value>)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    pub from: u64,
    pub file_len: u64,
    /// Valid, LF-terminated events added to the index by this scan.
    pub valid: u64,
    /// Invalid complete lines: `{offset, len}` with `len` including the LF.
    pub skipped: Vec<Skipped>,
    pub tail: Option<Tail>,
}

/// Scans `[from, EOF)` of `file` in 1 MiB chunks, adding every valid LF-terminated line to `index`.
/// Invalid lines (unparsable JSON, invalid UTF-8, a bad envelope, a duplicate id) are skipped and
/// reported. Bytes after the last LF are returned as `tail` and never indexed here.
pub fn scan(file: &File, from: u64, index: &mut Index) -> io::Result<ScanReport> {
    const CHUNK: usize = 1 << 20;
    let file_len = file.metadata()?.len();
    let mut rep = ScanReport {
        from,
        file_len,
        ..Default::default()
    };
    let mut pos = from;
    let mut line: Vec<u8> = Vec::new();
    let mut line_off = from;
    let mut buf = vec![0u8; CHUNK];
    while pos < file_len {
        let want = ((file_len - pos) as usize).min(CHUNK);
        let n = file.read_at(&mut buf[..want], pos)?;
        if n == 0 {
            break;
        }
        let mut start = 0;
        for i in 0..n {
            if buf[i] == b'\n' {
                line.extend_from_slice(&buf[start..i]);
                let len = line.len() as u64 + 1;
                handle_line(&line, line_off, len, index, &mut rep);
                line.clear();
                line_off = pos + i as u64 + 1;
                start = i + 1;
            }
        }
        line.extend_from_slice(&buf[start..n]);
        pos += n as u64;
    }
    if !line.is_empty() {
        let event = Event::parse_line(&line).filter(|(e, _)| !index.contains(e.id));
        rep.tail = Some(Tail {
            off: line_off,
            len: line.len() as u64,
            event,
        });
    }
    Ok(rep)
}

fn handle_line(line: &[u8], off: u64, len: u64, index: &mut Index, rep: &mut ScanReport) {
    match Event::parse_line(line) {
        Some((ev, obj)) if !index.contains(ev.id) && len <= u32::MAX as u64 => {
            let kind = ev.kind();
            index.push(ev.id, ev.parent, off, len as u32, kind);
            index.note(kind, &obj);
            rep.valid += 1;
        }
        _ => rep.skipped.push(Skipped { offset: off, len }),
    }
}

/// Opens with `try_lock`, polling every 50 ms for up to `wait`.
fn lock_with_budget(file: &File, sid: &str, wait: Duration) -> Result<(), LogError> {
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {
                if start.elapsed() >= wait {
                    return Err(LogError::Locked(sid.to_string()));
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::Error(e)) => return Err(LogError::Io(e)),
        }
    }
}

/// Syncs a directory; on macOS a failure is ignored (UNVERIFIED whether F_FULLFSYNC works on a dir fd).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir).and_then(|f| f.sync_all()) {
        Ok(()) => Ok(()),
        Err(e) if cfg!(target_os = "macos") => {
            eprintln!("log: directory sync of {} ignored: {e}", dir.display());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub struct LogWriter {
    file: File,
    path: PathBuf,
    sid: String,
    len: u64,
    leaf: Option<u64>,
    dirty: bool,
    locked: bool,
    poisoned: bool,
    ids: IdGen,
}

/// The result of opening a session file for writing.
pub struct Opened {
    pub writer: LogWriter,
    pub index: Index,
    pub scan: ScanReport,
    /// A `\n` was appended because the file ended in an unterminated line (§3.4.4 case 2).
    pub newline_appended: bool,
}

impl LogWriter {
    /// Opens (creating if missing) and locks a session file, scans it, and repairs an unterminated tail
    /// by appending one `\n` (then `sync_data`). Nothing is ever truncated (P1).
    pub fn open(path: &Path, sid: &str, wait: Duration) -> Result<Opened, LogError> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        lock_with_budget(&file, sid, wait)?;
        let mut index = Index::new();
        let scan = scan(&file, 0, &mut index)?;
        let mut w = LogWriter {
            file,
            path: path.to_path_buf(),
            sid: sid.to_string(),
            len: scan.file_len,
            leaf: None,
            dirty: false,
            locked: true,
            poisoned: false,
            ids: IdGen::new(),
        };
        let mut scan = scan;
        let newline_appended = w.repair_tail(&mut scan, &mut index)?;
        w.leaf = index.last().map(|e| e.id);
        Ok(Opened {
            writer: w,
            index,
            scan,
            newline_appended,
        })
    }

    /// Creates a new session file (`create_new`), writes the `session` header, syncs the file and the
    /// directory. The file stays locked.
    pub fn create(
        path: &Path,
        sid: &str,
        header: &EventBody,
        wait: Duration,
    ) -> Result<Opened, LogError> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .open(path)?;
        lock_with_budget(&file, sid, wait)?;
        let mut w = LogWriter {
            file,
            path: path.to_path_buf(),
            sid: sid.to_string(),
            len: 0,
            leaf: None,
            dirty: false,
            locked: true,
            poisoned: false,
            ids: IdGen::new(),
        };
        let mut index = Index::new();
        w.append(header, None, &mut index)?;
        w.sync()?;
        if let Some(dir) = path.parent() {
            sync_dir(dir)?;
        }
        let scan = ScanReport {
            file_len: w.len,
            valid: 1,
            ..Default::default()
        };
        Ok(Opened {
            writer: w,
            index,
            scan,
            newline_appended: false,
        })
    }

    /// §3.4.4 case 2: append `\n` after an unterminated tail. A tail that parses as a complete new
    /// event becomes valid; otherwise it becomes one skipped line.
    fn repair_tail(&mut self, scan: &mut ScanReport, index: &mut Index) -> io::Result<bool> {
        let Some(tail) = scan.tail.take() else {
            return Ok(false);
        };
        self.write_raw(b"\n")?;
        self.sync()?;
        let len = tail.len + 1;
        match &tail.event {
            Some((ev, obj)) if len <= u32::MAX as u64 && !index.contains(ev.id) => {
                index.push(ev.id, ev.parent, tail.off, len as u32, ev.kind());
                index.note(ev.kind(), obj);
                scan.valid += 1;
            }
            _ => scan.skipped.push(Skipped {
                offset: tail.off,
                len,
            }),
        }
        scan.tail = Some(tail);
        Ok(true)
    }

    fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other(
                "log: writer poisoned by an earlier write error",
            ));
        }
        if !self.locked {
            return Err(io::Error::other("log: write while unlocked (paused)"));
        }
        if let Err(e) = self.file.write_all(bytes) {
            self.poisoned = true; // a partial write may be on disk: the process must exit and rescan
            return Err(e);
        }
        self.len += bytes.len() as u64;
        self.dirty = true;
        Ok(())
    }

    /// Appends one event whose parent is the current leaf (or `parent`, for `branch` and branch-back).
    pub fn append(
        &mut self,
        body: &EventBody,
        parent: Option<u64>,
        index: &mut Index,
    ) -> io::Result<u64> {
        let fields = body.to_fields()?;
        self.append_fields(body.type_tag(), fields, parent, index)
    }

    /// Appends a raw body map (used for `Other` and by tests).
    pub fn append_fields(
        &mut self,
        ty: &str,
        mut fields: Map<String, Value>,
        parent: Option<u64>,
        index: &mut Index,
    ) -> io::Result<u64> {
        let id = self.ids.next(&|x| index.contains(x));
        let parent = parent.or(self.leaf);
        fields.insert("id".into(), id_hex(id).into());
        fields.insert(
            "parent_id".into(),
            parent.map(id_hex).map(Value::from).unwrap_or(Value::Null),
        );
        fields.insert("ts".into(), now_rfc3339_ms().into());
        fields.insert("type".into(), ty.into());
        let kind = Kind::parse(ty);
        let obj = Value::Object(fields);
        let mut line = icj::to_vec(&obj)?;
        if line.len() > MAX_LINE_BYTES {
            return Err(io::Error::other("log: event too large; use a blob"));
        }
        line.push(b'\n');
        let off = self.len;
        self.write_raw(&line)?; // O_APPEND: lands at EOF
        if let Value::Object(m) = &obj {
            index.note(kind, m);
        }
        index.push(id, parent, off, line.len() as u32, kind);
        self.leaf = Some(id);
        Ok(id)
    }

    /// Durability point (D1–D3). On macOS std's `sync_data` is `fcntl(F_FULLFSYNC)`.
    /// An error here is fatal for the process (§3.4.3): the caller logs it and exits 1.
    pub fn sync(&mut self) -> io::Result<()> {
        if self.dirty {
            if let Err(e) = self.file.sync_data() {
                self.poisoned = true;
                return Err(e);
            }
            self.dirty = false;
        }
        Ok(())
    }

    /// Handoff pause: sync and release the lock. Returns `(log_len, leaf)`.
    pub fn unlock(&mut self) -> io::Result<(u64, Option<u64>)> {
        self.sync()?;
        if self.locked {
            self.file.unlock()?;
            self.locked = false;
        }
        Ok((self.len, self.leaf))
    }

    /// Resume after a failed handoff: re-lock (≤ `wait`), then rescan from `from` (events another body
    /// appended meanwhile), repairing a torn tail it left. The leaf is set back to `leaf` by the caller.
    pub fn relock_and_rescan(
        &mut self,
        from: u64,
        wait: Duration,
        index: &mut Index,
    ) -> Result<ScanReport, LogError> {
        if !self.locked {
            lock_with_budget(&self.file, &self.sid, wait)?;
            self.locked = true;
        }
        let mut rep = scan(&self.file, from, index)?;
        self.len = rep.file_len;
        self.repair_tail(&mut rep, index)?;
        Ok(rep)
    }

    /// Reads one line back (offset and length from the index, LF included) and parses it.
    pub fn read_event(&self, off: u64, len: u32) -> io::Result<Event> {
        read_event_at(&self.file, off, len)
    }

    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn leaf(&self) -> Option<u64> {
        self.leaf
    }
    pub fn set_leaf(&mut self, leaf: Option<u64>) {
        self.leaf = leaf;
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sid(&self) -> &str {
        &self.sid
    }
    pub fn is_locked(&self) -> bool {
        self.locked
    }
    pub fn file(&self) -> &File {
        &self.file
    }
}

pub fn read_event_at(file: &File, off: u64, len: u32) -> io::Result<Event> {
    let mut buf = vec![0u8; len as usize];
    file.read_exact_at(&mut buf, off)?;
    let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
    Event::parse_line(line)
        .map(|(e, _)| e)
        .ok_or_else(|| io::Error::other(format!("log: no valid event at offset {off}")))
}

/// Reader contract (§4.6): every valid, LF-terminated event of a file, in file order, without a lock.
pub fn read_all(path: &Path) -> io::Result<Vec<Event>> {
    let file = File::open(path)?;
    let mut index = Index::new();
    scan(&file, 0, &mut index)?;
    index
        .ents()
        .iter()
        .map(|e| read_event_at(&file, e.off, e.len))
        .collect()
}

/// Writes `state/session` atomically: `session.tmp`, sync, rename, sync the directory.
pub fn write_pointer(state_dir: &Path, sid: &str) -> io::Result<()> {
    let tmp = state_dir.join("session.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(format!("{sid}\n").as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, state_dir.join("session"))?;
    sync_dir(state_dir)
}

/// Reads `state/session` (trimmed), if present and valid.
pub fn read_pointer(state_dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(state_dir.join("session")).ok()?;
    let s = s.trim_end_matches('\n').to_string();
    if super::clock::valid_session_id(&s) {
        Some(s)
    } else {
        None
    }
}
