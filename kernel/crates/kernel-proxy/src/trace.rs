//! The trace store `vault/traces/v1/` (kernel/proxy.md §3.13), the kernel's spend ledger
//! (§3.10.4) and the readers behind the agent's redacted view (§3.14).
//!
//! One std thread owns every file of the store and takes jobs over a bounded channel
//! (`traces.writer_queue`). Ledger records are sent with a blocking send; a blob payload rides
//! along only when the queue has room, otherwise it is dropped and the record says
//! `"trace":"dropped"`. Blobs live in append-only packs, deduplicated by sha256 within the month;
//! request records (`req/<YYYY-MM-DD>.jsonl`) hold their addresses, so readers need no index.
//! Readers never panic on corrupt files: unparsable lines are skipped and bad blobs are io errors.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `[traces]` (kernel/config.md §4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TracesCfg {
    pub store_bodies: bool,
    pub retention_days: u32,
    pub pack_max_bytes: u64,
    pub checkpoint_every: u32,
    pub writer_queue: usize,
    pub fsync_interval_ms: u64,
    pub max_response_bytes: usize,
}

impl Default for TracesCfg {
    fn default() -> Self {
        TracesCfg {
            store_bodies: true,
            retention_days: 0,
            pack_max_bytes: 268_435_456,
            checkpoint_every: 64,
            writer_queue: 4096,
            fsync_interval_ms: 1000,
            max_response_bytes: 16_777_216,
        }
    }
}

/// The request bytes as forwarded, with the message list located (from `ReqView` offsets).
#[derive(Clone, Debug)]
pub struct RequestBody {
    pub bytes: Arc<[u8]>,
    /// Byte range `[start, end)` of the message-list value, brackets included (`messages`, or
    /// `input` when it is an array). `None`: the whole request is the head.
    pub list: Option<(usize, usize)>,
    /// Byte range of each element of the list, in order.
    pub elements: Vec<(usize, usize)>,
}

/// A `start` record. `fields` holds every field except `t`, `head`, `msgs` and `trace`, which the
/// writer adds.
#[derive(Clone, Debug)]
pub struct StartJob {
    pub req_id: String,
    /// The chain key, `<scope id>` + `\0` + `<x-init-session or _none>`.
    pub chain_key: String,
    pub fields: Map<String, Value>,
    pub body: Option<RequestBody>,
}

/// An `end` record. `fields` holds every field except `t` and `resp`, which the writer adds.
#[derive(Clone, Debug)]
pub struct EndJob {
    pub req_id: String,
    pub fields: Map<String, Value>,
    /// Raw upstream response bytes, already capped at `max_response_bytes`.
    pub response: Option<Vec<u8>>,
}

/// Handle to the writer thread (one std thread owns all files). Cheap to clone.
#[derive(Clone)]
pub struct TraceWriter {
    /// `None`: the disabled writer.
    inner: Option<Arc<Inner>>,
}

struct Inner {
    tx: SyncSender<Job>,
    dropped: Arc<AtomicU64>,
    store_bodies: bool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl TraceWriter {
    /// Start the writer on `root` (= `$INIT_HOME/vault/traces/v1`), rebuilding the dedup index.
    pub fn start(root: &Path, cfg: &TracesCfg) -> io::Result<TraceWriter> {
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Writer::open(root, cfg, dropped.clone())?;
        let (tx, rx) = mpsc::sync_channel(cfg.writer_queue.max(1));
        let handle = std::thread::Builder::new()
            .name("init-trace-writer".into())
            .spawn(move || writer.run(rx))?;
        Ok(TraceWriter {
            inner: Some(Arc::new(Inner {
                tx,
                dropped,
                store_bodies: cfg.store_bodies,
                thread: Mutex::new(Some(handle)),
            })),
        })
    }
    /// A writer that writes nothing (`judge` mode, proxy.md P-25).
    pub fn disabled() -> TraceWriter {
        TraceWriter { inner: None }
    }
    pub fn start_record(&self, mut job: StartJob) {
        let Some(inner) = &self.inner else { return };
        if !inner.store_bodies {
            job.body = None;
        }
        inner.send(Job::Start(job, false));
    }
    pub fn end_record(&self, mut job: EndJob) {
        let Some(inner) = &self.inner else { return };
        if !inner.store_bodies {
            job.response = None;
        }
        inner.send(Job::End(job, false));
    }
    /// One line of `scopes.jsonl`.
    pub fn scope_record(&self, line: Value) {
        let Some(inner) = &self.inner else { return };
        let _ = inner.tx.send(Job::Scope(line));
    }
    /// Block until everything queued so far is written and fsynced.
    pub fn flush(&self) {
        let Some(inner) = &self.inner else { return };
        let (ack, done) = mpsc::sync_channel(1);
        if inner.tx.send(Job::Flush(ack)).is_ok() {
            let _ = done.recv();
        }
    }
    /// Blob payloads dropped because the queue was full (`trace_dropped`).
    pub fn dropped(&self) -> u64 {
        self.inner
            .as_ref()
            .map_or(0, |i| i.dropped.load(Ordering::Relaxed))
    }
    /// Flush, fsync and stop the thread.
    pub fn shutdown(&self) {
        let Some(inner) = &self.inner else { return };
        let (ack, done) = mpsc::sync_channel(1);
        if inner.tx.send(Job::Shutdown(ack)).is_ok() {
            let _ = done.recv();
        }
        let handle = inner
            .thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

impl Inner {
    /// Ledger records always go (blocking send); the payload only if the queue has room.
    fn send(&self, job: Job) {
        if !job.has_payload() {
            let _ = self.tx.send(job);
            return;
        }
        match self.tx.try_send(job) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(job)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                let _ = self.tx.send(job.strip());
            }
        }
    }
}

/// One unit of work for the writer thread. The `bool` marks a record whose payload was dropped.
enum Job {
    Start(StartJob, bool),
    End(EndJob, bool),
    Scope(Value),
    Flush(SyncSender<()>),
    Shutdown(SyncSender<()>),
    /// Test hook: park the writer until the receiver yields.
    #[cfg(test)]
    Pause(Receiver<()>),
}

impl Job {
    fn has_payload(&self) -> bool {
        match self {
            Job::Start(j, _) => j.body.is_some(),
            Job::End(j, _) => j.response.is_some(),
            _ => false,
        }
    }
    fn strip(self) -> Job {
        match self {
            Job::Start(mut j, _) => {
                j.body = None;
                Job::Start(j, true)
            }
            Job::End(mut j, _) => {
                j.response = None;
                Job::End(j, true)
            }
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pack format
// ---------------------------------------------------------------------------------------------

const MAGIC: u32 = 0x3142_4e49;
/// magic u32 + kind u8 + len u32 + sha256 [u8; 32].
const HDR_LEN: usize = 41;
const MAX_BLOB: u64 = 64 << 20;
const KIND_HEAD: u8 = 1;
const KIND_MESSAGE: u8 = 2;
const KIND_RESPONSE: u8 = 3;
/// Hard bound on `base` hops while reading (cycle guard; the writer keeps chains far shorter).
const MAX_CHAIN: usize = 65_536;
/// Chains remembered by the writer; the least recently used is forgotten beyond this.
const MAX_CHAINS: usize = 4096;

fn header(kind: u8, len: u32, h: &[u8; 32]) -> [u8; HDR_LEN] {
    let mut hdr = [0u8; HDR_LEN];
    hdr[..4].copy_from_slice(&MAGIC.to_le_bytes());
    hdr[4] = kind;
    hdr[5..9].copy_from_slice(&len.to_le_bytes());
    hdr[9..].copy_from_slice(h);
    hdr
}

/// `(kind, len, sha256)` of a record header, `None` if it is not one.
fn parse_header(hdr: &[u8; HDR_LEN]) -> Option<(u8, u64, [u8; 32])> {
    let magic = u32::from_le_bytes(hdr[0..4].try_into().ok()?);
    let kind = hdr[4];
    if magic != MAGIC || !(1..=5).contains(&kind) {
        return None;
    }
    let len = u64::from(u32::from_le_bytes(hdr[5..9].try_into().ok()?));
    let h: [u8; 32] = hdr[9..].try_into().ok()?;
    Some((kind, len, h))
}

fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(char::from(H[usize::from(x >> 4)]));
        s.push(char::from(H[usize::from(x & 15)]));
    }
    s
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (nib(b[2 * i])? << 4) | nib(b[2 * i + 1])?;
    }
    Some(out)
}

fn pack_name(num: u32) -> String {
    format!("{num:06}.pack")
}

/// `<YYYYMM>/<NNNNNN>`, digits only (paths are built from it).
fn valid_pack_ref(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() == 13
        && b[6] == b'/'
        && b[..6].iter().all(u8::is_ascii_digit)
        && b[7..].iter().all(u8::is_ascii_digit)
}

/// The pack numbers present in a month directory, ascending.
fn pack_nums(dir: &Path) -> io::Result<Vec<u32>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut nums = Vec::new();
    for entry in rd {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(stem) = name.strip_suffix(".pack")
            && stem.len() == 6
            && stem.bytes().all(|c| c.is_ascii_digit())
            && let Ok(n) = stem.parse::<u32>()
        {
            nums.push(n);
        }
    }
    nums.sort_unstable();
    Ok(nums)
}

/// Index one pack (headers only, seeking over payloads; the last record's payload is hashed) and
/// truncate a torn tail. Returns the pack's length afterwards.
fn scan_pack(path: &Path, num: u32, index: &mut HashMap<[u8; 32], BlobLoc>) -> io::Result<u64> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let hdr_len = HDR_LEN as u64;
    let mut r = BufReader::new(&file);
    let mut off = 0u64;
    while off < len {
        if len - off < hdr_len {
            break;
        }
        let mut hdr = [0u8; HDR_LEN];
        r.read_exact(&mut hdr)?;
        let Some((_, plen, h)) = parse_header(&hdr) else {
            break;
        };
        if plen > MAX_BLOB || len - off - hdr_len < plen {
            break;
        }
        let next = off + hdr_len + plen;
        if next == len {
            let mut payload = vec![0u8; plen as usize];
            r.read_exact(&mut payload)?;
            if sha256(&payload) != h {
                break;
            }
        } else {
            r.seek_relative(plen as i64)?;
        }
        index.entry(h).or_insert(BlobLoc {
            num,
            off,
            len: plen,
        });
        off = next;
    }
    drop(r);
    if off < len {
        file.set_len(off)?;
        file.sync_all()?;
    }
    Ok(off)
}

// ---------------------------------------------------------------------------------------------
// Dates (UTC, proleptic Gregorian; H. Hinnant's civil-from-days algorithms)
// ---------------------------------------------------------------------------------------------

const DAY_MS: u64 = 86_400_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let (m, d) = (i64::from(m), i64::from(d));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn day_of_ms(ms: u64) -> i64 {
    (ms / DAY_MS) as i64
}

/// `YYYY-MM-DD`.
fn date_str(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYYMM`, the pack month.
fn month_str(ms: u64) -> String {
    let (y, m, _) = civil_from_days(day_of_ms(ms));
    format!("{y:04}{m:02}")
}

/// RFC 3339 `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn ts_str(ms: u64) -> String {
    let rem = ms % DAY_MS;
    format!(
        "{}T{:02}:{:02}:{:02}.{:03}Z",
        date_str(day_of_ms(ms)),
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

/// Days since the epoch of a `YYYY-MM-DD` date.
fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(i64::from(y), m, d);
    (civil_from_days(days) == (i64::from(y), m, d)).then_some(days)
}

fn is_req_id(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Unix ms of a `req_id` (its first 12 hex digits).
fn req_ms(id: &str) -> Option<u64> {
    if is_req_id(id) {
        u64::from_str_radix(&id[..12], 16).ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

fn mkdirs(p: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p)
}

/// Open a JSONL file for appending (0600, parents 0700). A torn last line gets its newline first,
/// so the next record starts on a line of its own.
fn open_line_file(p: &Path) -> io::Result<File> {
    if let Some(dir) = p.parent() {
        mkdirs(dir)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .open(p)?;
    let len = f.metadata()?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        f.seek(SeekFrom::Start(len - 1))?;
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            f.write_all(b"\n")?;
        }
    }
    Ok(f)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("trace store: {what}"))
}

fn not_found(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("trace store: {what}"))
}

fn check_id(id: &str) -> io::Result<u64> {
    req_ms(id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid req_id"))
}

fn str_field<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

// ---------------------------------------------------------------------------------------------
// Writer thread
// ---------------------------------------------------------------------------------------------

/// Where a blob of the current month lives.
#[derive(Clone, Copy, Debug)]
struct BlobLoc {
    num: u32,
    off: u64,
    len: u64,
}

/// The previous request of a chain.
struct Chain {
    prev: String,
    hashes: Vec<[u8; 32]>,
    /// Records read to reconstruct `prev` (1 for a full list).
    depth: u32,
    /// Recency, for eviction.
    seq: u64,
}

struct Writer {
    root: PathBuf,
    cfg: TracesCfg,
    dropped: Arc<AtomicU64>,
    /// `YYYYMM` of the index and the current pack.
    month: String,
    index: HashMap<[u8; 32], BlobLoc>,
    pack_num: u32,
    pack: Option<File>,
    pack_size: u64,
    pack_dirty: bool,
    chains: HashMap<String, Chain>,
    seq: u64,
    req: Option<(i64, File)>,
    req_dirty: bool,
    scopes: Option<File>,
    scopes_dirty: bool,
    last_sync: Instant,
}

impl Writer {
    fn open(root: &Path, cfg: &TracesCfg, dropped: Arc<AtomicU64>) -> io::Result<Writer> {
        let month = month_str(now_ms());
        let dir = root.join("packs").join(&month);
        let mut index = HashMap::new();
        let (mut pack_num, mut pack_size) = (1, 0);
        for n in pack_nums(&dir)? {
            pack_size = scan_pack(&dir.join(pack_name(n)), n, &mut index)?;
            pack_num = n;
        }
        Ok(Writer {
            root: root.to_path_buf(),
            cfg: cfg.clone(),
            dropped,
            month,
            index,
            pack_num,
            pack: None,
            pack_size,
            pack_dirty: false,
            chains: HashMap::new(),
            seq: 0,
            req: None,
            req_dirty: false,
            scopes: None,
            scopes_dirty: false,
            last_sync: Instant::now(),
        })
    }

    fn run(mut self, rx: Receiver<Job>) {
        let interval = Duration::from_millis(self.cfg.fsync_interval_ms);
        loop {
            let msg = if self.dirty() {
                rx.recv_timeout(interval.saturating_sub(self.last_sync.elapsed()))
            } else {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match msg {
                Ok(Job::Start(job, dropped)) => self.on_start(job, dropped),
                Ok(Job::End(job, dropped)) => self.on_end(job, dropped),
                Ok(Job::Scope(line)) => self.on_scope(&line),
                Ok(Job::Flush(ack)) => {
                    self.sync();
                    let _ = ack.send(());
                }
                Ok(Job::Shutdown(ack)) => {
                    self.sync();
                    let _ = ack.send(());
                    return;
                }
                #[cfg(test)]
                Ok(Job::Pause(until)) => {
                    let _ = until.recv();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.sync();
                    return;
                }
            }
            if self.dirty() && self.last_sync.elapsed() >= interval {
                self.sync();
            }
        }
    }

    fn dirty(&self) -> bool {
        self.pack_dirty || self.req_dirty || self.scopes_dirty
    }

    /// fsync packs before records, so a durable record never points at a lost blob.
    fn sync_pack(&mut self) {
        if self.pack_dirty {
            if let Some(f) = &self.pack {
                let _ = f.sync_data();
            }
            self.pack_dirty = false;
        }
    }

    fn sync(&mut self) {
        self.sync_pack();
        if self.req_dirty {
            if let Some((_, f)) = &self.req {
                let _ = f.sync_data();
            }
            self.req_dirty = false;
        }
        if self.scopes_dirty {
            if let Some(f) = &self.scopes {
                let _ = f.sync_data();
            }
            self.scopes_dirty = false;
        }
        self.last_sync = Instant::now();
    }

    fn close_pack(&mut self) {
        self.sync_pack();
        self.pack = None;
    }

    fn close_req(&mut self) {
        if self.req_dirty {
            self.sync_pack();
            if let Some((_, f)) = &self.req {
                let _ = f.sync_data();
            }
            self.req_dirty = false;
        }
        self.req = None;
    }

    /// Switch to a new month: new pack directory, empty index.
    fn ensure_month(&mut self, month: String) {
        if month == self.month {
            return;
        }
        self.close_pack();
        self.index.clear();
        let nums = pack_nums(&self.root.join("packs").join(&month)).unwrap_or_default();
        self.pack_num = nums.last().map_or(1, |n| n + 1);
        self.pack_size = 0;
        self.month = month;
    }

    fn addr(&self, h: &[u8; 32], loc: BlobLoc) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("h".into(), Value::String(hex(h)));
        m.insert("n".into(), loc.len.into());
        m.insert("o".into(), loc.off.into());
        m.insert(
            "p".into(),
            Value::String(format!("{}/{:06}", self.month, loc.num)),
        );
        m
    }

    /// The address of a blob, appending it to the current pack unless the index has it.
    fn blob(&mut self, kind: u8, payload: &[u8], h: [u8; 32]) -> io::Result<Map<String, Value>> {
        if let Some(loc) = self.index.get(&h) {
            return Ok(self.addr(&h, *loc));
        }
        let len = payload.len() as u64;
        if len > MAX_BLOB {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "blob over 64 MiB",
            ));
        }
        let rec = HDR_LEN as u64 + len;
        loop {
            if self.pack.is_none() {
                let dir = self.root.join("packs").join(&self.month);
                mkdirs(&dir)?;
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(dir.join(pack_name(self.pack_num)))?;
                self.pack_size = f.metadata()?.len();
                self.pack = Some(f);
            }
            if self.pack_size > 0 && self.pack_size + rec > self.cfg.pack_max_bytes {
                self.close_pack();
                self.pack_num += 1;
                continue;
            }
            break;
        }
        let Some(f) = self.pack.as_mut() else {
            return Err(io::Error::other("no open pack"));
        };
        let off = self.pack_size;
        let res = f
            .write_all(&header(kind, len as u32, &h))
            .and_then(|()| f.write_all(payload));
        if let Err(e) = res {
            // Cut the partial record; if even that fails, never append to this pack again.
            if f.set_len(off).is_err() {
                self.pack = None;
                self.pack_dirty = false;
                self.pack_num += 1;
                self.pack_size = 0;
            }
            return Err(e);
        }
        self.pack_size += rec;
        self.pack_dirty = true;
        let loc = BlobLoc {
            num: self.pack_num,
            off,
            len,
        };
        self.index.insert(h, loc);
        Ok(self.addr(&h, loc))
    }

    /// Write the head and the new message blobs. Returns `(head, msgs, chain state)`.
    fn write_request(
        &mut self,
        req_id: &str,
        chain_key: &str,
        body: &RequestBody,
    ) -> io::Result<(Value, Value, Option<Chain>)> {
        let b: &[u8] = &body.bytes;
        let Some((l0, l1)) = split(body) else {
            let mut head = self.blob(KIND_HEAD, b, sha256(b))?;
            head.insert("list_at".into(), Value::Null);
            return Ok((Value::Object(head), Value::Null, None));
        };
        let mut head_bytes = Vec::with_capacity(b.len() - (l1 - l0) + 2);
        head_bytes.extend_from_slice(&b[..l0]);
        head_bytes.extend_from_slice(b"[]");
        head_bytes.extend_from_slice(&b[l1..]);
        let hashes: Vec<[u8; 32]> = body
            .elements
            .iter()
            .map(|&(s, e)| sha256(&b[s..e]))
            .collect();
        let (base, keep, depth) = match self.chains.get(chain_key) {
            Some(c) if c.depth < self.cfg.checkpoint_every => {
                let keep = c
                    .hashes
                    .iter()
                    .zip(&hashes)
                    .take_while(|(x, y)| x == y)
                    .count();
                if keep > 0 {
                    (Some(c.prev.clone()), keep, c.depth + 1)
                } else {
                    (None, 0, 1)
                }
            }
            _ => (None, 0, 1),
        };
        let mut head = self.blob(KIND_HEAD, &head_bytes, sha256(&head_bytes))?;
        head.insert("list_at".into(), l0.into());
        let mut add = Vec::with_capacity(hashes.len() - keep);
        for (&(s, e), h) in body.elements.iter().zip(&hashes).skip(keep) {
            add.push(Value::Object(self.blob(KIND_MESSAGE, &b[s..e], *h)?));
        }
        let mut msgs = Map::new();
        msgs.insert("add".into(), Value::Array(add));
        msgs.insert("base".into(), base.map_or(Value::Null, Value::String));
        msgs.insert("keep".into(), keep.into());
        self.seq += 1;
        let chain = Chain {
            prev: req_id.to_string(),
            hashes,
            depth,
            seq: self.seq,
        };
        Ok((Value::Object(head), Value::Object(msgs), Some(chain)))
    }

    fn on_start(&mut self, job: StartJob, dropped: bool) {
        let StartJob {
            req_id,
            chain_key,
            fields: mut m,
            body,
        } = job;
        for k in ["t", "head", "msgs", "trace"] {
            m.remove(k);
        }
        m.insert("t".into(), "start".into());
        m.insert("req_id".into(), Value::String(req_id.clone()));
        if !m.contains_key("ts") {
            m.insert("ts".into(), ts_str(now_ms()).into());
        }
        let (mut head, mut msgs, mut chain, mut lost) = (Value::Null, Value::Null, None, dropped);
        if let Some(body) = body.filter(|_| self.cfg.store_bodies) {
            self.ensure_month(month_str(now_ms()));
            match self.write_request(&req_id, &chain_key, &body) {
                Ok((h, ms, c)) => (head, msgs, chain) = (h, ms, c),
                Err(_) => {
                    lost = true;
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if lost {
            m.insert("trace".into(), "dropped".into());
        }
        m.insert("head".into(), head);
        m.insert("msgs".into(), msgs);
        let written = self.append_record(m).is_ok();
        match chain {
            Some(c) if written => {
                if self.chains.len() >= MAX_CHAINS
                    && !self.chains.contains_key(&chain_key)
                    && let Some(oldest) = self
                        .chains
                        .iter()
                        .min_by_key(|(_, c)| c.seq)
                        .map(|(k, _)| k.clone())
                {
                    self.chains.remove(&oldest);
                }
                self.chains.insert(chain_key, c);
            }
            // No list, no body, or an unwritten record: the next request starts a full list.
            _ => {
                self.chains.remove(&chain_key);
            }
        }
    }

    fn on_end(&mut self, job: EndJob, dropped: bool) {
        let EndJob {
            req_id,
            fields: mut m,
            response,
        } = job;
        for k in ["t", "resp", "trace"] {
            m.remove(k);
        }
        m.insert("t".into(), "end".into());
        m.insert("req_id".into(), Value::String(req_id));
        if !m.contains_key("ts") {
            m.insert("ts".into(), ts_str(now_ms()).into());
        }
        let (mut resp, mut lost) = (Value::Null, dropped);
        if let Some(mut r) = response.filter(|_| self.cfg.store_bodies) {
            let cap = (self.cfg.max_response_bytes as u64).min(MAX_BLOB) as usize;
            if r.len() > cap {
                r.truncate(cap);
                m.insert("resp_truncated".into(), true.into());
            }
            self.ensure_month(month_str(now_ms()));
            match self.blob(KIND_RESPONSE, &r, sha256(&r)) {
                Ok(a) => resp = Value::Object(a),
                Err(_) => {
                    lost = true;
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if lost {
            m.insert("trace".into(), "dropped".into());
        }
        m.insert("resp".into(), resp);
        let _ = self.append_record(m);
    }

    fn on_scope(&mut self, line: &Value) {
        let Ok(mut bytes) = serde_json::to_vec(line) else {
            return;
        };
        bytes.push(b'\n');
        if self.scopes.is_none() {
            match open_line_file(&self.root.join("scopes.jsonl")) {
                Ok(f) => self.scopes = Some(f),
                Err(_) => return,
            }
        }
        if let Some(f) = self.scopes.as_mut() {
            if f.write_all(&bytes).is_ok() {
                self.scopes_dirty = true;
            } else {
                self.scopes = None;
            }
        }
    }

    /// Append one record to `req/<UTC date of ts>.jsonl` (sorted keys: `Map` is a `BTreeMap`).
    fn append_record(&mut self, m: Map<String, Value>) -> io::Result<()> {
        let day = str_field(&m, "ts")
            .and_then(|s| s.get(..10))
            .and_then(parse_date)
            .unwrap_or_else(|| day_of_ms(now_ms()));
        let mut line = serde_json::to_vec(&Value::Object(m)).map_err(io::Error::other)?;
        line.push(b'\n');
        if self.req.as_ref().is_none_or(|(d, _)| *d != day) {
            self.close_req();
            let path = self
                .root
                .join("req")
                .join(format!("{}.jsonl", date_str(day)));
            self.req = Some((day, open_line_file(&path)?));
        }
        let Some((_, f)) = self.req.as_mut() else {
            return Err(io::Error::other("no open request file"));
        };
        if let Err(e) = f.write_all(&line) {
            // Reopening re-checks the tail and ends a torn line first.
            self.close_req();
            return Err(e);
        }
        self.req_dirty = true;
        Ok(())
    }
}

/// The validated message-list range, or `None` when the body has no usable list.
fn split(body: &RequestBody) -> Option<(usize, usize)> {
    let (l0, l1) = body.list?;
    let b = &body.bytes;
    if l0 >= l1 || l1 > b.len() || b[l0] != b'[' || b[l1 - 1] != b']' {
        return None;
    }
    let mut prev = l0 + 1;
    for &(s, e) in &body.elements {
        if s < prev || e < s || e >= l1 {
            return None;
        }
        prev = e;
    }
    Some((l0, l1))
}

// ---------------------------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------------------------

const REQ_ID_KEY: &str = "\"req_id\":\"";

/// One `req/<date>.jsonl` file, with lines located by the req ids they mention.
struct DayFile {
    text: String,
    ids: HashMap<String, Vec<(usize, usize)>>,
}

impl DayFile {
    fn load(root: &Path, day: i64) -> io::Result<Option<DayFile>> {
        let bytes = match fs::read(req_path(root, day)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut ids: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        let mut pos = 0;
        for line in text.split('\n') {
            let range = (pos, pos + line.len());
            pos += line.len() + 1;
            for (i, _) in line.match_indices(REQ_ID_KEY) {
                let rest = &line[i + REQ_ID_KEY.len()..];
                if let Some(id) = rest.get(..16)
                    && is_req_id(id)
                    && rest.as_bytes().get(16) == Some(&b'"')
                {
                    let v = ids.entry(id.to_string()).or_default();
                    if v.last() != Some(&range) {
                        v.push(range);
                    }
                }
            }
        }
        Ok(Some(DayFile { text, ids }))
    }

    fn find(&self, id: &str, t: &str) -> Option<Value> {
        for &(s, e) in self.ids.get(id)? {
            if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&self.text[s..e])
                && str_field(&m, "req_id") == Some(id)
                && str_field(&m, "t") == Some(t)
            {
                return Some(Value::Object(m));
            }
        }
        None
    }
}

fn req_path(root: &Path, day: i64) -> PathBuf {
    root.join("req").join(format!("{}.jsonl", date_str(day)))
}

/// Finds records by req id; a start is in the file of its id's UTC date (±1 day for clock skew),
/// an end up to two days later.
struct Finder<'a> {
    root: &'a Path,
    days: HashMap<i64, Option<DayFile>>,
}

impl<'a> Finder<'a> {
    fn new(root: &'a Path) -> Self {
        Finder {
            root,
            days: HashMap::new(),
        }
    }

    fn find(&mut self, id: &str, t: &str, days: &[i64]) -> io::Result<Option<Value>> {
        for &d in days {
            if let Entry::Vacant(e) = self.days.entry(d) {
                e.insert(DayFile::load(self.root, d)?);
            }
            if let Some(Some(f)) = self.days.get(&d)
                && let Some(v) = f.find(id, t)
            {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    fn start(&mut self, id: &str) -> io::Result<Option<Value>> {
        let d = day_of_ms(check_id(id)?);
        self.find(id, "start", &[d, d + 1, d - 1])
    }

    fn end(&mut self, id: &str) -> io::Result<Option<Value>> {
        let d = day_of_ms(check_id(id)?);
        self.find(id, "end", &[d, d + 1, d + 2, d - 1])
    }
}

/// Reads blobs by address, verifying the record header and the payload hash.
struct BlobReader<'a> {
    root: &'a Path,
    files: HashMap<String, File>,
}

impl<'a> BlobReader<'a> {
    fn new(root: &'a Path) -> Self {
        BlobReader {
            root,
            files: HashMap::new(),
        }
    }

    fn read(&mut self, a: &Map<String, Value>) -> io::Result<Vec<u8>> {
        let h = str_field(a, "h")
            .and_then(unhex32)
            .ok_or_else(|| bad("address h"))?;
        let p = str_field(a, "p")
            .filter(|p| valid_pack_ref(p))
            .ok_or_else(|| bad("address p"))?;
        let o = a
            .get("o")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("address o"))?;
        let n = a
            .get("n")
            .and_then(Value::as_u64)
            .filter(|&n| n <= MAX_BLOB)
            .ok_or_else(|| bad("address n"))?;
        let f = match self.files.entry(p.to_string()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(File::open(
                self.root.join("packs").join(format!("{p}.pack")),
            )?),
        };
        f.seek(SeekFrom::Start(o))?;
        let mut hdr = [0u8; HDR_LEN];
        f.read_exact(&mut hdr)?;
        let (_, len, hh) = parse_header(&hdr).ok_or_else(|| bad("pack record header"))?;
        if len != n || hh != h {
            return Err(bad("pack record does not match its address"));
        }
        let mut payload = vec![0u8; n as usize];
        f.read_exact(&mut payload)?;
        if sha256(&payload) != h {
            return Err(bad("blob hash mismatch"));
        }
        Ok(payload)
    }
}

fn parse_addr(v: &Value) -> io::Result<Map<String, Value>> {
    v.as_object().cloned().ok_or_else(|| bad("address"))
}

/// The element addresses of a request's message list, following `base`/`keep` to a full list.
fn resolve_list(finder: &mut Finder, start: &Value) -> io::Result<Vec<Map<String, Value>>> {
    let mut levels: Vec<(usize, Vec<Map<String, Value>>)> = Vec::new();
    let mut cur = start.clone();
    loop {
        if levels.len() >= MAX_CHAIN {
            return Err(bad("chain too long"));
        }
        let Some(Value::Object(msgs)) = cur.get("msgs") else {
            return Err(bad("record without msgs in a chain"));
        };
        let keep = msgs
            .get("keep")
            .and_then(Value::as_u64)
            .and_then(|k| usize::try_from(k).ok())
            .ok_or_else(|| bad("msgs.keep"))?;
        let add = match msgs.get("add") {
            Some(Value::Array(a)) => a.iter().map(parse_addr).collect::<io::Result<Vec<_>>>()?,
            _ => return Err(bad("msgs.add")),
        };
        let base = match msgs.get("base") {
            None | Some(Value::Null) => None,
            Some(Value::String(b)) => Some(b.clone()),
            Some(_) => return Err(bad("msgs.base")),
        };
        match base {
            None => {
                levels.push((0, add));
                break;
            }
            Some(b) => {
                levels.push((keep, add));
                cur = finder
                    .start(&b)?
                    .ok_or_else(|| not_found("base record missing"))?;
            }
        }
    }
    let mut list = Vec::new();
    for (keep, add) in levels.into_iter().rev() {
        if keep > list.len() {
            return Err(bad("msgs.keep beyond its base"));
        }
        list.truncate(keep);
        list.extend(add);
    }
    Ok(list)
}

/// Every `req/<date>.jsonl` file, oldest first.
fn req_files(root: &Path) -> io::Result<Vec<(i64, PathBuf)>> {
    let rd = match fs::read_dir(root.join("req")) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(day) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".jsonl"))
            .and_then(parse_date)
        {
            out.push((day, entry.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// The parsable object lines of a records file (a torn or corrupt line is skipped).
fn read_records(path: &Path) -> io::Result<Vec<Map<String, Value>>> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect())
}

/// One request as the ledger sees it (a `start` joined with its `end`, if any).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LedgerEntry {
    pub req_id: String,
    /// Unix ms of the `start` record.
    pub ts_ms: u64,
    pub scope: String,
    pub job_id: Option<String>,
    pub task: Option<String>,
    pub provider: String,
    pub model: String,
    pub vis: String,
    pub reserved_pusd: u64,
    /// From the `end` record; `None` when there is no `end` (charge the reservation).
    pub cost_pusd: Option<u64>,
    pub cost_ref_pusd: Option<u64>,
}

/// The `start` records whose time is in `[from_ms, to_ms)`, joined with their `end` records.
pub fn read_ledger(root: &Path, from_ms: u64, to_ms: u64) -> io::Result<Vec<LedgerEntry>> {
    if to_ms <= from_ms {
        return Ok(vec![]);
    }
    let (lo, hi) = (day_of_ms(from_ms) - 1, day_of_ms(to_ms - 1) + 2);
    let mut starts: BTreeMap<String, (u64, Map<String, Value>)> = BTreeMap::new();
    let mut ends: HashMap<String, Map<String, Value>> = HashMap::new();
    for (day, path) in req_files(root)? {
        if day < lo || day > hi {
            continue;
        }
        for m in read_records(&path)? {
            let Some(id) = str_field(&m, "req_id").filter(|id| is_req_id(id)) else {
                continue;
            };
            let id = id.to_string();
            match str_field(&m, "t") {
                Some("start") => {
                    if let Some(ms) = req_ms(&id)
                        && (from_ms..to_ms).contains(&ms)
                    {
                        starts.entry(id).or_insert((ms, m));
                    }
                }
                Some("end") => {
                    ends.insert(id, m);
                }
                _ => {}
            }
        }
    }
    let s = |m: &Map<String, Value>, k: &str| str_field(m, k).unwrap_or_default().to_string();
    let o = |m: &Map<String, Value>, k: &str| str_field(m, k).map(str::to_string);
    Ok(starts
        .into_iter()
        .map(|(id, (ts_ms, m))| {
            let end = ends.get(&id);
            LedgerEntry {
                ts_ms,
                scope: s(&m, "scope"),
                job_id: o(&m, "job_id"),
                task: o(&m, "task"),
                provider: s(&m, "provider"),
                model: s(&m, "model"),
                vis: s(&m, "vis"),
                reserved_pusd: m.get("reserved_pusd").and_then(Value::as_u64).unwrap_or(0),
                cost_pusd: end.and_then(|e| e.get("cost_pusd")).and_then(Value::as_u64),
                cost_ref_pusd: end
                    .and_then(|e| e.get("cost_ref_pusd"))
                    .and_then(Value::as_u64),
                req_id: id,
            }
        })
        .collect())
}

/// The reconstructed request bytes and its message-list element bytes.
pub fn reconstruct(root: &Path, req_id: &str) -> io::Result<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut finder = Finder::new(root);
    let start = finder
        .start(req_id)?
        .ok_or_else(|| not_found("no start record"))?;
    let Some(Value::Object(head_addr)) = start.get("head") else {
        return Err(not_found("request body not stored"));
    };
    let mut blobs = BlobReader::new(root);
    let head = blobs.read(head_addr)?;
    let at = match head_addr.get("list_at") {
        None | Some(Value::Null) => return Ok((head, vec![])),
        Some(v) => v
            .as_u64()
            .and_then(|a| usize::try_from(a).ok())
            .filter(|&a| head.get(a..a.saturating_add(2)) == Some(b"[]".as_slice()))
            .ok_or_else(|| bad("head.list_at"))?,
    };
    let addrs = resolve_list(&mut finder, &start)?;
    let mut elems = Vec::with_capacity(addrs.len());
    for a in &addrs {
        elems.push(blobs.read(a)?);
    }
    let total = head.len() + elems.iter().map(Vec::len).sum::<usize>() + elems.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&head[..at]);
    out.push(b'[');
    for (i, e) in elems.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(e);
    }
    out.push(b']');
    out.extend_from_slice(&head[at + 2..]);
    Ok((out, elems))
}

/// The raw response bytes of a request.
pub fn response(root: &Path, req_id: &str) -> io::Result<Vec<u8>> {
    let end = Finder::new(root)
        .end(req_id)?
        .ok_or_else(|| not_found("no end record"))?;
    let Some(Value::Object(a)) = end.get("resp") else {
        return Err(not_found("response not stored"));
    };
    BlobReader::new(root).read(a)
}

/// The `start` and (if written) `end` record of a request.
pub fn records(root: &Path, req_id: &str) -> io::Result<(Value, Option<Value>)> {
    let mut finder = Finder::new(root);
    let start = finder
        .start(req_id)?
        .ok_or_else(|| not_found("no start record"))?;
    Ok((start, finder.end(req_id)?))
}

/// Filter for [`summaries`].
#[derive(Clone, Debug, Default)]
pub struct SummaryQuery {
    pub after: Option<String>,
    pub limit: usize,
    pub task: Option<String>,
    /// `Some("agent")` for the agent view.
    pub vis: Option<String>,
}

/// One summary per matching request, oldest first: the `end` fields merged into `start`, minus
/// blob addresses, with every `*_pusd` field replaced by `*_musd` (truncated).
///
/// `limit = 0` means no limit. On a key present in both records the `start` value wins (`ts` is
/// the start time); `t` is removed.
pub fn summaries(root: &Path, q: &SummaryQuery) -> io::Result<Vec<Value>> {
    let after_day = match &q.after {
        Some(a) => Some(day_of_ms(check_id(a)?) - 1),
        None => None,
    };
    let after = q.after.as_deref().unwrap_or("");
    let limit = if q.limit == 0 { usize::MAX } else { q.limit };
    let mut starts: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut ends: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut reached = false;
    for (day, path) in req_files(root)? {
        if after_day.is_some_and(|a| day < a) {
            continue;
        }
        for m in read_records(&path)? {
            let Some(id) = str_field(&m, "req_id").filter(|id| is_req_id(id) && *id > after) else {
                continue;
            };
            let id = id.to_string();
            match str_field(&m, "t") {
                Some("start") => {
                    let vis_ok = q
                        .vis
                        .as_deref()
                        .is_none_or(|v| str_field(&m, "vis") == Some(v));
                    let task_ok = q
                        .task
                        .as_deref()
                        .is_none_or(|t| str_field(&m, "task") == Some(t));
                    if vis_ok && task_ok {
                        starts.entry(id).or_insert(m);
                    }
                }
                Some("end") => {
                    ends.insert(id, m);
                }
                _ => {}
            }
        }
        // Records of one id can sit a day apart (ts vs id clock, ends after midnight): read one
        // more file once the limit is reached, then keep the smallest ids.
        if starts.len() >= limit {
            if reached {
                break;
            }
            reached = true;
        }
    }
    Ok(starts
        .into_iter()
        .take(limit)
        .map(|(id, mut m)| {
            if let Some(end) = ends.remove(&id) {
                for (k, v) in end {
                    m.entry(k).or_insert(v);
                }
            }
            for k in ["t", "head", "msgs", "resp"] {
                m.remove(k);
            }
            let mut v = Value::Object(m);
            pusd_to_musd(&mut v);
            v
        })
        .collect())
}

/// Replace every `*_pusd` key (at any depth) by `*_musd` = value / 1,000,000, truncated.
fn pusd_to_musd(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let keys: Vec<String> = m.keys().filter(|k| k.ends_with("_pusd")).cloned().collect();
            for k in keys {
                let musd = match m.remove(&k) {
                    Some(Value::Number(n)) => n
                        .as_u64()
                        .map(|u| Value::from(u / 1_000_000))
                        .or_else(|| n.as_i64().map(|i| Value::from(i / 1_000_000)))
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                m.insert(format!("{}_musd", &k[..k.len() - 5]), musd);
            }
            for x in m.values_mut() {
                pusd_to_musd(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(pusd_to_musd),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    /// `<repo>/kernel/target/test-homes/wp5/trace-<name>/vault/traces/v1`, emptied first.
    fn test_root(name: &str) -> PathBuf {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|a| a.join("kernel").join("Cargo.toml").is_file())
            .expect("repo root above CARGO_MANIFEST_DIR")
            .to_path_buf();
        let home = repo
            .join("kernel")
            .join("target")
            .join("test-homes")
            .join("wp5")
            .join(format!("trace-{name}"));
        if home.exists() {
            fs::remove_dir_all(&home).unwrap();
        }
        fs::create_dir_all(&home).unwrap();
        home.join("vault").join("traces").join("v1")
    }

    fn cfg() -> TracesCfg {
        TracesCfg {
            fsync_interval_ms: 50,
            ..TracesCfg::default()
        }
    }

    fn rid(ms: u64, n: u16) -> String {
        format!("{ms:012x}{n:04x}")
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn range(&mut self, lo: usize, hi: usize) -> usize {
            lo + (self.next() % (hi - lo) as u64) as usize
        }
    }

    const WORDS: &[&str] = &[
        "cargo",
        "test",
        "error[E0502]:",
        "cannot",
        "borrow",
        "`self.index`",
        "as",
        "mutable",
        "because",
        "it",
        "is",
        "also",
        "borrowed",
        "fn",
        "main()",
        "{",
        "}",
        "let",
        "mut",
        "src/",
        "lib.rs:42:17",
        "warning",
        "the",
        "proxy",
        "trace",
        "writer",
        "\"quoted\"",
        "\n",
        "ok.",
        "running",
        "12",
        "tests",
        "passed;",
        "0",
        "failed",
        "=>",
        "Some(x)",
        "None",
    ];

    fn text(rng: &mut Rng, n: usize) -> String {
        let mut s = String::new();
        while s.len() < n {
            s.push_str(WORDS[rng.range(0, WORDS.len())]);
            s.push(' ');
        }
        s
    }

    fn message(rng: &mut Rng, role: &str, lo: usize, hi: usize) -> Vec<u8> {
        let n = rng.range(lo, hi);
        let content = text(rng, n);
        serde_json::to_vec(&json!({"content": content, "role": role})).unwrap()
    }

    /// A request `prefix [e0,e1,…] suffix` (pretty: newlines between elements) plus the bytes the
    /// store must give back (no whitespace between elements).
    fn build(
        prefix: &[u8],
        elems: &[Vec<u8>],
        suffix: &[u8],
        pretty: bool,
    ) -> (RequestBody, Vec<u8>) {
        let mut b = prefix.to_vec();
        let l0 = b.len();
        b.push(b'[');
        if pretty {
            b.extend_from_slice(b"\n  ");
        }
        let mut offs = Vec::new();
        for (i, e) in elems.iter().enumerate() {
            if i > 0 {
                b.extend_from_slice(if pretty { b",\n  " } else { b"," });
            }
            offs.push((b.len(), b.len() + e.len()));
            b.extend_from_slice(e);
        }
        if pretty {
            b.push(b'\n');
        }
        b.push(b']');
        let l1 = b.len();
        b.extend_from_slice(suffix);
        let mut compact = prefix.to_vec();
        compact.push(b'[');
        compact.extend_from_slice(&elems.join(&b','));
        compact.push(b']');
        compact.extend_from_slice(suffix);
        let body = RequestBody {
            bytes: b.into(),
            list: Some((l0, l1)),
            elements: offs,
        };
        (body, compact)
    }

    fn suffix(rng: &mut Rng) -> Vec<u8> {
        let tools: Vec<Value> = (0..8)
            .map(|i| {
                json!({"function": {"description": text(rng, 600), "name": format!("tool_{i}"),
                       "parameters": {"properties": {"arg": {"type": "string"}}, "type": "object"}},
                       "type": "function"})
            })
            .collect();
        let tail = serde_json::to_string(&json!({"model": "deepseek-flash", "stream": true,
            "stream_options": {"include_usage": true}, "tools": tools}))
        .unwrap();
        format!(",{}", &tail[1..]).into_bytes()
    }

    const PREFIX: &[u8] = b"{\"max_tokens\":8192,\"messages\":";

    fn start_job(
        id: &str,
        chain: &str,
        ms: u64,
        body: Option<RequestBody>,
        extra: Value,
    ) -> StartJob {
        let mut fields = Map::new();
        fields.insert("req_id".into(), id.into());
        fields.insert("ts".into(), ts_str(ms).into());
        fields.insert("scope".into(), "live".into());
        fields.insert("vis".into(), "agent".into());
        fields.insert("provider".into(), "deepseek".into());
        fields.insert("model".into(), "deepseek-flash".into());
        fields.insert("job_id".into(), Value::Null);
        fields.insert("reserved_pusd".into(), 700_427_640_000u64.into());
        if let Value::Object(x) = extra {
            fields.extend(x);
        }
        StartJob {
            req_id: id.into(),
            chain_key: chain.into(),
            fields,
            body,
        }
    }

    fn end_job(id: &str, ms: u64, response: Option<Vec<u8>>, extra: Value) -> EndJob {
        let mut fields = Map::new();
        fields.insert("req_id".into(), id.into());
        fields.insert("ts".into(), ts_str(ms).into());
        fields.insert("status".into(), 200.into());
        fields.insert("resp_truncated".into(), false.into());
        if let Value::Object(x) = extra {
            fields.extend(x);
        }
        EndJob {
            req_id: id.into(),
            fields,
            response,
        }
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(files_under(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    fn bytes_under(dir: &Path) -> u64 {
        files_under(dir)
            .iter()
            .map(|p| fs::metadata(p).unwrap().len())
            .sum()
    }

    /// Records read to reconstruct `id`, from every start record of the store.
    fn chain_depth(starts: &HashMap<String, Map<String, Value>>, id: &str) -> usize {
        let mut depth = 1;
        let mut cur = &starts[id];
        while let Some(b) = cur["msgs"]["base"].as_str() {
            cur = &starts[b];
            depth += 1;
        }
        depth
    }

    fn all_starts(root: &Path) -> HashMap<String, Map<String, Value>> {
        let mut out = HashMap::new();
        for (_, path) in req_files(root).unwrap() {
            for m in read_records(&path).unwrap() {
                if str_field(&m, "t") == Some("start") {
                    out.insert(str_field(&m, "req_id").unwrap().to_string(), m);
                }
            }
        }
        out
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    fn pack_bytes_all(root: &Path) -> Vec<u8> {
        files_under(&root.join("packs"))
            .iter()
            .flat_map(|p| fs::read(p).unwrap())
            .collect()
    }

    /// proxy.md P-10.
    #[test]
    fn p10_round_trip_compaction_and_ratio() {
        let root = test_root("p10");
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let sfx = suffix(&mut rng);
        let sys = message(&mut rng, "system", 3000, 3001);
        let mut conv = vec![sys.clone(), message(&mut rng, "user", 500, 1500)];
        let t0 = now_ms();
        let mut naive = 0usize;
        let mut expected = Vec::new();
        for step in 0..200u64 {
            if step == 120 {
                conv = vec![sys.clone(), message(&mut rng, "user", 2500, 3000)];
            }
            let (body, compact) = build(PREFIX, &conv, &sfx, step % 3 == 0);
            naive += body.bytes.len();
            let id = rid(t0 + step, step as u16);
            w.start_record(start_job(&id, "live\0s1", t0 + step, Some(body), json!({})));
            w.end_record(end_job(&id, t0 + step, None, json!({"cost_pusd": 1})));
            expected.push((id, compact, conv.clone()));
            conv.push(message(&mut rng, "assistant", 500, 1500));
            conv.push(message(&mut rng, "tool", 500, 3000));
        }
        w.flush();
        assert_eq!(w.dropped(), 0);
        let starts = all_starts(&root);
        let mut max_depth = 0;
        for (id, compact, elems) in &expected {
            let (bytes, got) = reconstruct(&root, id).unwrap();
            assert!(bytes == *compact, "request {id} differs");
            assert_eq!(&got, elems);
            max_depth = max_depth.max(chain_depth(&starts, id));
        }
        assert_eq!(
            max_depth, 64,
            "chain depth reaches but never passes checkpoint_every"
        );
        let compacted = records(&root, &expected[120].0).unwrap().0;
        assert_eq!(compacted["msgs"]["keep"], 1);
        assert!(compacted["msgs"]["base"].is_string());
        let pack = bytes_under(&root.join("packs"));
        let total = pack + bytes_under(&root.join("req"));
        let ratio = pack as f64 / naive as f64;
        eprintln!(
            "P-10: naive {naive} B, packs {pack} B ({:.2}%), packs+records {total} B ({:.2}%)",
            ratio * 100.0,
            total as f64 / naive as f64 * 100.0
        );
        assert!(ratio < 0.03, "pack bytes {ratio} of naive");
        assert!(
            (total as f64) < naive as f64 * 0.03,
            "packs + records under 3% too"
        );
        w.shutdown();
    }

    #[test]
    fn dedup_across_requests_and_chains() {
        let root = test_root("dedup");
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let sys =
            br#"{"content":"SYSTEM-MARKER-7f3a you are a careful agent","role":"system"}"#.to_vec();
        let u1 = br#"{"content":"first user turn","role":"user"}"#.to_vec();
        let u2 = br#"{"content":"second user turn","role":"user"}"#.to_vec();
        let t0 = now_ms();
        let (a, b) = (rid(t0, 1), rid(t0 + 1, 2));
        let (ba, _) = build(PREFIX, &[sys.clone(), u1], b"}", false);
        let (bb, _) = build(PREFIX, &[sys.clone(), u2], b"}", false);
        w.start_record(start_job(&a, "live\0s1", t0, Some(ba), json!({})));
        w.start_record(start_job(&b, "live\0s2", t0 + 1, Some(bb), json!({})));
        w.flush();
        let (ra, rb) = (records(&root, &a).unwrap().0, records(&root, &b).unwrap().0);
        assert_eq!(ra["msgs"]["add"][0], rb["msgs"]["add"][0]);
        assert_eq!(ra["head"], rb["head"]);
        assert_ne!(ra["msgs"]["add"][1], rb["msgs"]["add"][1]);
        assert!(
            rb["msgs"]["base"].is_null(),
            "another chain starts a full list"
        );
        assert_eq!(count(&pack_bytes_all(&root), b"SYSTEM-MARKER-7f3a"), 1);
        // Sorted keys, one object per line.
        let line = fs::read_to_string(req_path(&root, day_of_ms(t0))).unwrap();
        let first = line.lines().next().unwrap();
        let pos = |k: &str| first.find(&format!("\"{k}\":")).unwrap();
        assert!(
            pos("head") < pos("msgs") && pos("msgs") < pos("req_id") && pos("req_id") < pos("t")
        );
        // Permissions: dirs 0700, files 0600.
        for d in [root.clone(), root.join("req"), root.join("packs")] {
            assert_eq!(
                fs::metadata(&d).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for f in files_under(&root) {
            assert_eq!(
                fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        w.shutdown();
    }

    #[test]
    fn pack_rotation_keeps_addresses_valid() {
        let root = test_root("rotate");
        let c = TracesCfg {
            pack_max_bytes: 4096,
            ..cfg()
        };
        let w = TraceWriter::start(&root, &c).unwrap();
        let mut rng = Rng(7);
        let mut conv = vec![message(&mut rng, "system", 1000, 2000)];
        let t0 = now_ms();
        let mut ids = Vec::new();
        for i in 0..20u64 {
            conv.push(message(&mut rng, "user", 1000, 2000));
            let (body, compact) = build(PREFIX, &conv, b"}", false);
            let id = rid(t0 + i, i as u16);
            w.start_record(start_job(&id, "live\0r", t0 + i, Some(body), json!({})));
            ids.push((id, compact));
        }
        w.flush();
        assert!(files_under(&root.join("packs")).len() > 5);
        for (id, compact) in &ids {
            assert_eq!(&reconstruct(&root, id).unwrap().0, compact);
        }
        w.shutdown();
    }

    #[test]
    fn restart_rebuilds_index_without_rewriting() {
        let root = test_root("restart");
        let mut rng = Rng(11);
        let conv: Vec<Vec<u8>> = (0..5)
            .map(|_| message(&mut rng, "user", 500, 900))
            .collect();
        let t0 = now_ms();
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        for i in 1..=5u64 {
            let (body, _) = build(PREFIX, &conv[..i as usize], b"}", false);
            w.start_record(start_job(
                &rid(t0 + i, i as u16),
                "live\0a",
                t0 + i,
                Some(body),
                json!({}),
            ));
        }
        w.shutdown();
        let packs = files_under(&root.join("packs"));
        let size = bytes_under(&root.join("packs"));
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let (body, compact) = build(PREFIX, &conv, b"}", false);
        let id = rid(t0 + 10, 10);
        w.start_record(start_job(&id, "live\0b", t0 + 10, Some(body), json!({})));
        w.flush();
        assert_eq!(
            bytes_under(&root.join("packs")),
            size,
            "every blob deduplicated"
        );
        assert_eq!(reconstruct(&root, &id).unwrap().0, compact);
        let extra = message(&mut rng, "user", 700, 701);
        let mut more = conv.clone();
        more.push(extra.clone());
        let (body, compact2) = build(PREFIX, &more, b"}", false);
        let id2 = rid(t0 + 11, 11);
        w.start_record(start_job(&id2, "live\0b", t0 + 11, Some(body), json!({})));
        w.shutdown();
        assert_eq!(
            files_under(&root.join("packs")),
            packs,
            "appends to the same pack"
        );
        assert_eq!(
            bytes_under(&root.join("packs")),
            size + HDR_LEN as u64 + extra.len() as u64
        );
        assert_eq!(reconstruct(&root, &id2).unwrap().0, compact2);
        assert_eq!(records(&root, &id2).unwrap().0["msgs"]["keep"], 5);
        for i in 1..=5u64 {
            assert!(reconstruct(&root, &rid(t0 + i, i as u16)).is_ok());
        }
    }

    #[test]
    fn torn_tails_are_truncated() {
        let root = test_root("torn");
        let t0 = now_ms();
        let msg = |s: &str| format!(r#"{{"content":"{s}","role":"user"}}"#).into_bytes();
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let (b1, c1) = build(PREFIX, &[msg("one"), msg("two")], b"}", false);
        let id1 = rid(t0, 1);
        w.start_record(start_job(&id1, "live\0t", t0, Some(b1), json!({})));
        w.shutdown();
        let packs = files_under(&root.join("packs"));
        assert_eq!(packs.len(), 1);
        let pack = packs[0].clone();
        let good = fs::metadata(&pack).unwrap().len();
        let append = |bytes: &[u8]| {
            let mut f = OpenOptions::new().append(true).open(&pack).unwrap();
            f.write_all(bytes).unwrap();
        };
        let h = sha256(b"hello");
        let mut short = header(KIND_MESSAGE, 1000, &h).to_vec();
        short.extend_from_slice(b"0123456789");
        let mut mismatch = header(KIND_MESSAGE, 5, &h).to_vec();
        mismatch.extend_from_slice(b"hellX");
        let cases: [(&str, Vec<u8>); 4] = [
            ("short payload", short),
            ("short header", b"INB1\x02\x05".to_vec()),
            ("bad magic", vec![0xAB; 64]),
            ("hash mismatch", mismatch),
        ];
        for (what, junk) in cases {
            append(&junk);
            let w = TraceWriter::start(&root, &cfg()).unwrap();
            assert_eq!(fs::metadata(&pack).unwrap().len(), good, "{what}");
            w.shutdown();
        }
        // A torn record line: the next record still lands on its own line.
        let req = req_path(&root, day_of_ms(t0));
        let mut f = OpenOptions::new().append(true).open(&req).unwrap();
        f.write_all(br#"{"t":"start","req_id":"0"#).unwrap();
        drop(f);
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let (b2, c2) = build(PREFIX, &[msg("one"), msg("two"), msg("three")], b"}", false);
        let id2 = rid(t0 + 1, 2);
        w.start_record(start_job(&id2, "live\0t", t0 + 1, Some(b2), json!({})));
        w.shutdown();
        assert_eq!(reconstruct(&root, &id1).unwrap().0, c1);
        assert_eq!(reconstruct(&root, &id2).unwrap().0, c2);
        let new_blob = HDR_LEN as u64 + msg("three").len() as u64;
        assert_eq!(
            fs::metadata(&pack).unwrap().len(),
            good + new_blob,
            "index rebuilt"
        );
        let ledger = read_ledger(&root, t0, t0 + 2).unwrap();
        assert_eq!(ledger.len(), 2);
        // A corrupt payload is an error, not a panic.
        let mut bytes = fs::read(&pack).unwrap();
        let last = bytes.len() - 2;
        bytes[last] ^= 0xff;
        fs::write(&pack, &bytes).unwrap();
        assert_eq!(
            reconstruct(&root, &id2).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn full_queue_drops_payloads_not_records() {
        let root = test_root("dropped");
        let c = TracesCfg {
            writer_queue: 1,
            ..cfg()
        };
        let w = TraceWriter::start(&root, &c).unwrap();
        let (go, wait) = mpsc::channel();
        w.inner.as_ref().unwrap().tx.send(Job::Pause(wait)).unwrap();
        let t0 = now_ms();
        let msg = br#"{"content":"x","role":"user"}"#.to_vec();
        let ids: Vec<String> = (0..6).map(|i| rid(t0 + i, i as u16)).collect();
        let burst = {
            let (w, ids, msg) = (w.clone(), ids.clone(), msg.clone());
            std::thread::spawn(move || {
                for (i, id) in ids.iter().enumerate() {
                    let (body, _) = build(PREFIX, &vec![msg.clone(); i + 1], b"}", false);
                    w.start_record(start_job(
                        id,
                        "live\0q",
                        t0 + i as u64,
                        Some(body),
                        json!({}),
                    ));
                }
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        go.send(()).unwrap();
        burst.join().unwrap();
        w.flush();
        assert!(w.dropped() > 0);
        let mut dropped = 0;
        for (i, id) in ids.iter().enumerate() {
            let start = records(&root, id).unwrap().0;
            if start.get("trace") == Some(&json!("dropped")) {
                dropped += 1;
                assert!(start["head"].is_null() && start["msgs"].is_null());
                assert_eq!(
                    start["reserved_pusd"], 700_427_640_000u64,
                    "ledger fields kept"
                );
                assert_eq!(
                    reconstruct(&root, id).unwrap_err().kind(),
                    io::ErrorKind::NotFound
                );
            } else {
                let (_, elems) = reconstruct(&root, id).unwrap();
                assert_eq!(elems.len(), i + 1);
            }
        }
        assert_eq!(dropped as u64, w.dropped());
        w.shutdown();
    }

    #[test]
    fn store_bodies_false_writes_no_blobs() {
        let root = test_root("nobodies");
        let c = TracesCfg {
            store_bodies: false,
            ..cfg()
        };
        let w = TraceWriter::start(&root, &c).unwrap();
        let t0 = now_ms();
        let id = rid(t0, 1);
        let (body, _) = build(PREFIX, &[b"{}".to_vec()], b"}", false);
        w.start_record(start_job(&id, "live\0n", t0, Some(body), json!({})));
        w.end_record(end_job(
            &id,
            t0 + 5,
            Some(b"data: {}\n\n".to_vec()),
            json!({}),
        ));
        w.flush();
        assert!(!root.join("packs").exists());
        let (start, end) = records(&root, &id).unwrap();
        let end = end.unwrap();
        assert!(start["head"].is_null() && start["msgs"].is_null() && end["resp"].is_null());
        assert!(start.get("trace").is_none() && end.get("trace").is_none());
        assert_eq!(
            reconstruct(&root, &id).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            response(&root, &id).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(w.dropped(), 0);
        w.shutdown();
    }

    #[test]
    fn ledger_joins_across_midnight() {
        let root = test_root("ledger");
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let midnight = days_from_civil(2026, 10, 20) as u64 * DAY_MS;
        let (a, b, c) = (
            rid(midnight - 100, 1),
            rid(midnight + 1000, 2),
            rid(midnight - 5000, 3),
        );
        let task =
            json!({"task": "2026-10-20-03", "scope": "gate:7", "job_id": "j1", "vis": "kernel"});
        w.start_record(start_job(&a, "x\0a", midnight - 100, None, task.clone()));
        w.start_record(start_job(&c, "x\0c", midnight - 5000, None, json!({})));
        w.end_record(end_job(
            &a,
            midnight + 500,
            None,
            json!({"cost_pusd": 7_195_144_000u64, "cost_ref_pusd": 9_000_000_000u64}),
        ));
        w.start_record(start_job(&b, "x\0b", midnight + 1000, None, json!({})));
        w.end_record(end_job(
            &b,
            midnight + 2000,
            None,
            json!({"cost_pusd": 5u64, "cost_ref_pusd": 6u64}),
        ));
        w.shutdown();
        assert!(
            fs::read_to_string(root.join("req/2026-10-19.jsonl"))
                .unwrap()
                .contains(&a)
        );
        let next = fs::read_to_string(root.join("req/2026-10-20.jsonl")).unwrap();
        assert!(next.contains(&format!("\"req_id\":\"{a}\"")) && next.contains("\"t\":\"end\""));
        let day1 = read_ledger(&root, midnight - DAY_MS, midnight).unwrap();
        assert_eq!(day1.len(), 2);
        assert_eq!(day1[0].req_id, c);
        assert_eq!(day1[0].cost_pusd, None);
        assert_eq!(day1[0].reserved_pusd, 700_427_640_000);
        let ea = &day1[1];
        assert_eq!(ea.req_id, a);
        assert_eq!(ea.ts_ms, midnight - 100);
        assert_eq!(ea.cost_pusd, Some(7_195_144_000));
        assert_eq!(ea.cost_ref_pusd, Some(9_000_000_000));
        assert_eq!((ea.scope.as_str(), ea.vis.as_str()), ("gate:7", "kernel"));
        assert_eq!(ea.job_id.as_deref(), Some("j1"));
        assert_eq!(ea.task.as_deref(), Some("2026-10-20-03"));
        assert_eq!(
            (ea.provider.as_str(), ea.model.as_str()),
            ("deepseek", "deepseek-flash")
        );
        let day2 = read_ledger(&root, midnight, midnight + DAY_MS).unwrap();
        assert_eq!(day2.len(), 1);
        assert_eq!(day2[0].req_id, b);
        assert_eq!(day2[0].cost_pusd, Some(5));
        assert_eq!(day2[0].job_id, None);
        let (_, end) = records(&root, &a).unwrap();
        assert_eq!(end.unwrap()["ts"], ts_str(midnight + 500));
    }

    #[test]
    fn summaries_filter_and_convert_money() {
        let root = test_root("summaries");
        let w = TraceWriter::start(&root, &cfg()).unwrap();
        let t0 = now_ms();
        let mut ids = Vec::new();
        for i in 0..6u64 {
            let id = rid(t0 + i, i as u16);
            let vis = if i % 3 == 2 { "kernel" } else { "agent" };
            let task = if i < 3 { "t1" } else { "t2" };
            let (body, _) = build(PREFIX, &[b"{}".to_vec()], b"}", false);
            w.start_record(start_job(
                &id,
                "s\0x",
                t0 + i,
                Some(body),
                json!({"vis": vis, "task": task}),
            ));
            w.end_record(end_job(&id, t0 + i + 1, Some(b"{}".to_vec()),
                json!({"cost_pusd": 7_195_144_000u64, "cost_ref_pusd": 1_999_999u64, "root_cost_musd": 7195,
                       "usage": {"nested_pusd": 3_000_000u64}})));
            ids.push(id);
        }
        w.shutdown();
        let q = |after: Option<&String>, limit, task: Option<&str>| SummaryQuery {
            after: after.cloned(),
            limit,
            task: task.map(str::to_string),
            vis: Some("agent".into()),
        };
        let all = summaries(&root, &q(None, 500, None)).unwrap();
        let got: Vec<&str> = all.iter().map(|v| v["req_id"].as_str().unwrap()).collect();
        assert_eq!(got, [&ids[0], &ids[1], &ids[3], &ids[4]]);
        let s = &all[0];
        for k in ["head", "msgs", "resp", "t"] {
            assert!(s.get(k).is_none(), "{k} removed");
        }
        let text = s.to_string();
        assert!(!text.contains("_pusd"), "{text}");
        assert_eq!(s["reserved_musd"], 700_427);
        assert_eq!(s["cost_musd"], 7195);
        assert_eq!(s["cost_ref_musd"], 1);
        assert_eq!(s["root_cost_musd"], 7195);
        assert_eq!(s["usage"]["nested_musd"], 3);
        assert_eq!(s["status"], 200);
        assert_eq!(s["ts"], ts_str(t0), "start time kept");
        let t2 = summaries(&root, &q(None, 500, Some("t2"))).unwrap();
        assert_eq!(t2.len(), 2);
        assert_eq!(t2[0]["req_id"], ids[3].as_str());
        let after = summaries(&root, &q(Some(&ids[1]), 1, None)).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["req_id"], ids[3].as_str());
        let kernel = SummaryQuery {
            vis: None,
            ..q(None, 2, None)
        };
        assert_eq!(summaries(&root, &kernel).unwrap().len(), 2);
        let unlimited = SummaryQuery::default();
        assert_eq!(summaries(&root, &unlimited).unwrap().len(), 6);
    }

    #[test]
    fn response_round_trip_and_scope_records() {
        let root = test_root("response");
        let c = TracesCfg {
            max_response_bytes: 64,
            ..cfg()
        };
        let w = TraceWriter::start(&root, &c).unwrap();
        let t0 = now_ms();
        let (a, b) = (rid(t0, 1), rid(t0 + 1, 2));
        let sse = b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n".to_vec();
        let big = vec![b'z'; 200];
        let (body, _) = build(PREFIX, &[b"{}".to_vec()], b"}", false);
        w.start_record(start_job(&a, "s\0r", t0, Some(body), json!({})));
        w.end_record(end_job(&a, t0 + 9, Some(sse.clone()), json!({})));
        w.start_record(start_job(&b, "s\0r", t0 + 1, None, json!({})));
        w.end_record(end_job(&b, t0 + 9, Some(big), json!({})));
        w.scope_record(json!({"t": "open", "scope": "live"}));
        w.scope_record(json!({"t": "close", "scope": "live"}));
        w.flush();
        assert_eq!(response(&root, &a).unwrap(), sse);
        assert_eq!(response(&root, &b).unwrap(), vec![b'z'; 64]);
        let (_, end_b) = records(&root, &b).unwrap();
        assert_eq!(end_b.unwrap()["resp_truncated"], true);
        let (_, end_a) = records(&root, &a).unwrap();
        let end_a = end_a.unwrap();
        assert_eq!(end_a["resp"]["n"], sse.len());
        assert_eq!(end_a["resp_truncated"], false);
        let scopes = fs::read_to_string(root.join("scopes.jsonl")).unwrap();
        assert_eq!(scopes.lines().count(), 2);
        assert_eq!(
            scopes.lines().next().unwrap(),
            r#"{"scope":"live","t":"open"}"#
        );
        assert_eq!(
            records(&root, "zz").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            records(&root, &rid(t0 + 99, 9)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        w.shutdown();
        w.shutdown();
        w.flush();
    }

    #[test]
    fn disabled_writer_writes_nothing() {
        let root = test_root("disabled");
        let w = TraceWriter::disabled();
        let t0 = now_ms();
        let id = rid(t0, 1);
        let (body, _) = build(PREFIX, &[b"{}".to_vec()], b"}", false);
        w.start_record(start_job(&id, "s\0d", t0, Some(body), json!({})));
        w.end_record(end_job(&id, t0, Some(b"x".to_vec()), json!({})));
        w.scope_record(json!({"t": "open"}));
        w.flush();
        w.shutdown();
        assert_eq!(w.dropped(), 0);
        assert!(!root.exists());
        let home = root.ancestors().nth(3).unwrap();
        assert_eq!(fs::read_dir(home).unwrap().count(), 0);
        assert!(read_ledger(&root, 0, u64::MAX).unwrap().is_empty());
        assert!(
            summaries(&root, &SummaryQuery::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn dates_round_trip() {
        for day in [-1i64, 0, 59, 365, 11_016, 20_745, 20_746, 2_932_896] {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
            if (0..=9999).contains(&y) {
                assert_eq!(parse_date(&date_str(day)), Some(day));
            }
        }
        assert_eq!(date_str(20_745), "2026-10-19");
        assert_eq!(ts_str(1_760_880_247_123), "2025-10-19T13:24:07.123Z");
        assert_eq!(parse_date("2026-02-30"), None);
        assert_eq!(
            month_str(days_from_civil(2026, 10, 1) as u64 * DAY_MS),
            "202610"
        );
        assert_eq!(req_ms("019a1c2e3f4b0007"), Some(0x019a_1c2e_3f4b));
        assert_eq!(req_ms("019A1C2E3F4B0007"), None);
    }
}
