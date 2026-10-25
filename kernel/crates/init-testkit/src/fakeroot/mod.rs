//! `init-fakeroot`: the kernel-facing contract of the Root (testing.md §3.5b; root/root.md §3.5,
//! §3.15, §4.1, §4.2), so every kernel test can run without `init-root`.
//!
//! What it implements:
//! - the **control link** on fd 3 / fd 4 (via [`crate::link`]): `hello` → a typed
//!   `init_proto::root_link::BootBlock`, `ready`, `spawn` (the broker: real processes, each in its
//!   own process group, with `[stdin_w, stdout_r, stderr_r]` handed back over fd 4 before the
//!   response), `kill`, `instances`, `store.put`/`store.get` (blobs under `ROOT_HOME/store/blobs/`),
//!   `tuple.get`, `lineage.append` (hash-chained, `idem`-deduplicated, acknowledged with
//!   `{seq, hash, dup}`), `ledger.get`, `ceilings.get`, `system.swap`, `proposal.file`,
//!   `sealed.*` (level-refused), `alert`; the events `exited`, `level`, `mode`, `ceilings`, `swap`;
//!   the Root → kernel requests `stop`, `prepare_swap` and `owner.in`;
//! - the **owner socket** `ROOT_HOME/run/owner.sock` with the frames of root/root.md §4.2
//!   (`attach`, `line`, `interrupt`, `panic`, `detach`; request frames `status`, `stop`, `start`,
//!   `panic`, `resume`, `ledger`, `shutdown` and the fake-only `test.*` commands), the owner queue
//!   and redelivery after `ready`;
//! - the **mock egress** `ROOT_HOME/run/egress.sock` ([`egress`]): forwarding to `init-mockllm`,
//!   `GET /_root/ceilings`, typed refusals, `x-egress-*` headers, a peak-priced spend counter;
//! - panic from the socket, the latch file `run/panic` and `SIGUSR1`, with the latch surviving a
//!   restart.
//!
//! It **enforces nothing**: no sandbox, no sweep. Its broker stops only the instances it spawned,
//! by held pid and process group (testing.md §3.1a), which is what makes kernel scenarios under it
//! safe on the live machine. Every security property is tested against the real `init-root`.

pub mod charter;
pub mod egress;

use crate::link::{Incoming, Link, Trace};
use crate::util::{self, rfc3339};
use crate::{fault, sys};
use charter::Charter;
use init_proto::envelope::{ErrorBody, Request, Response};
use init_proto::icj::{self, V1};
use init_proto::root_link::{
    ActiveLimit, BootBlock, BootInfo, Ceilings, ExitedData, InstanceInfo, KernelEvent,
    KernelRequest, KillMode, KillResult, LadderInfo, Ledger, LevelCause, LevelData, Limits,
    LineageAppendResult, MoneyBlock, OwnerInArgs, OwnerInFrame, PrepareSwapArgs,
    ProposalFileResult, RootErrorCode, RootPaths, RootVersion, SpawnArgs, SpawnResult, StopArgs,
    StoreGetResult, StorePutResult, SystemSwapResult, TupleBody, TupleExt, TupleIds, TupleKernel,
    ZoneBootReason, ZoneMode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Reported as `root.version`.
pub const VERSION: &str = "0.0.0-fakeroot";
/// The fake's single tuple id (genesis = current = LKG = audited).
pub const FAKE_TUPLE: &str = "t000000000000";
/// The fake provider key the egress adds upstream. It is a constant, never a real key.
pub const FAKE_KEY: &str = "init-fakeroot-test-key";
const ZERO_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const REQ_TIMEOUT: Duration = Duration::from_secs(30);

/// `FAKEROOT_MODE` / `--mode` values (testing.md §3.5b), comma-separated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Grant every spawn within the charter fixture's quotas, accept every lineage kind.
    Ok,
    /// Report level `<n>` in `hello` and `level` events.
    Level(u8),
    /// Start the kernel with `mode:"safe"`.
    Safe,
    /// Refuse spawns of this kind with `root.quota`.
    Refuse(String),
    /// Send `prepare_swap{job}` `after_ms` after `ready`, then restart the kernel (`kernel_upgrade`).
    Swap { job: String, after_ms: u64 },
}

/// Parse `ok,level:2,safe,refuse:build,swap:k-1@500`.
pub fn parse_modes(s: &str) -> Result<Vec<Mode>, String> {
    let mut out = Vec::new();
    for m in s.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        let (k, v) = m.split_once(':').map_or((m, None), |(k, v)| (k, Some(v)));
        out.push(match (k, v) {
            ("ok", None) => Mode::Ok,
            ("safe", None) => Mode::Safe,
            ("level", Some(n)) => Mode::Level(n.parse().ok().filter(|l: &u8| *l <= 4).ok_or_else(|| format!("bad level in `{m}`"))?),
            ("refuse", Some(kind)) if !kind.is_empty() => Mode::Refuse(kind.to_string()),
            ("swap", Some(j)) => {
                let (job, after) = j.split_once('@').unwrap_or((j, "0"));
                Mode::Swap { job: job.to_string(), after_ms: after.parse().map_err(|_| format!("bad delay in `{m}`"))? }
            }
            _ => return Err(format!("unknown FAKEROOT_MODE `{m}` (ok | level:<n> | safe | refuse:<kind> | swap:<job>[@<ms>])")),
        });
    }
    if out.is_empty() {
        out.push(Mode::Ok);
    }
    Ok(out)
}

/// Where the fake Root forwards model requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upstream {
    Tcp(String),
    Unix(PathBuf),
}

impl Upstream {
    /// `http://127.0.0.1:<port>` or `unix:<path>`.
    pub fn parse(s: &str) -> Result<Upstream, String> {
        if let Some(rest) = s.strip_prefix("http://") {
            Ok(Upstream::Tcp(rest.trim_end_matches('/').to_string()))
        } else if let Some(p) = s.strip_prefix("unix:") {
            Ok(Upstream::Unix(PathBuf::from(p)))
        } else {
            Err(format!(
                "upstream `{s}`: expected http://host:port or unix:<path>"
            ))
        }
    }
}

/// Command-line configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub root_home: PathBuf,
    pub home: PathBuf,
    pub kernel: PathBuf,
    pub kernel_args: Vec<String>,
    pub kernel_env: Vec<(String, String)>,
    pub charter: Option<PathBuf>,
    pub upstream: Option<Upstream>,
    pub modes: Vec<Mode>,
    pub autostart: bool,
}

pub const USAGE: &str = "usage: init-fakeroot --root-home <dir> --home <INIT_HOME> --kernel <path> [--kernel-arg <a>]... \
[--kernel-env K=V]... [--charter <file.toml>] [--upstream http://127.0.0.1:<port>|unix:<path>] [--mode <modes>] [--no-start]";

impl Config {
    pub fn from_args(args: &[String]) -> Result<Config, String> {
        let mut c = Config {
            root_home: PathBuf::new(),
            home: PathBuf::new(),
            kernel: PathBuf::new(),
            kernel_args: Vec::new(),
            kernel_env: Vec::new(),
            charter: None,
            upstream: None,
            modes: Vec::new(),
            autostart: true,
        };
        let mut explicit_args = false;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{a} needs a value"))
            };
            match a.as_str() {
                "--root-home" => c.root_home = PathBuf::from(val()?),
                "--home" => c.home = PathBuf::from(val()?),
                "--kernel" => c.kernel = PathBuf::from(val()?),
                "--kernel-arg" => {
                    explicit_args = true;
                    c.kernel_args.push(val()?)
                }
                "--kernel-env" => {
                    let v = val()?;
                    let (k, x) = v.split_once('=').ok_or("--kernel-env needs K=V")?;
                    c.kernel_env.push((k.to_string(), x.to_string()));
                }
                "--charter" => c.charter = Some(PathBuf::from(val()?)),
                "--upstream" => c.upstream = Some(Upstream::parse(&val()?)?),
                "--mode" => c.modes = parse_modes(&val()?)?,
                "--no-start" => c.autostart = false,
                "-h" | "--help" => return Err(USAGE.into()),
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }
        if c.root_home.as_os_str().is_empty()
            || c.home.as_os_str().is_empty()
            || c.kernel.as_os_str().is_empty()
        {
            return Err(USAGE.into());
        }
        for p in [&c.root_home, &c.home, &c.kernel] {
            if !p.is_absolute() {
                return Err(format!("{}: must be an absolute path", p.display()));
            }
        }
        if !explicit_args {
            c.kernel_args = vec!["supervise".into(), "--under-root".into()];
        }
        if c.modes.is_empty() {
            c.modes = parse_modes(&std::env::var("FAKEROOT_MODE").unwrap_or_default())?;
        }
        Ok(c)
    }
}

/// Zone slot (a coarse subset of root/root.md §3.6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
    Exited,
    Panicked,
    ShuttingDown,
}

impl Slot {
    pub fn name(&self) -> &'static str {
        match self {
            Slot::Idle => "idle",
            Slot::Starting => "starting",
            Slot::Running => "running",
            Slot::Stopping => "stopping",
            Slot::Stopped => "stopped",
            Slot::Exited => "exited",
            Slot::Panicked => "panicked",
            Slot::ShuttingDown => "shutting_down",
        }
    }
}

#[derive(Clone, Debug)]
struct Inst {
    kind: String,
    lane: Option<String>,
    pid: i32,
    started_at: String,
    kgen: u64,
    program: String,
}

#[derive(Clone, Debug)]
struct Kernel {
    pid: i32,
    kgen: u64,
    link: Arc<Link>,
    hello: bool,
    ready: bool,
    reason: String,
    started_at: String,
}

/// Root-side money counters (micro-USD), peak-priced (root/egress.md §3.4).
#[derive(Clone, Debug, Default)]
pub struct Money {
    pub day_spent: u64,
    pub day_reserved: u64,
    pub total_spent: u64,
    pub total_reserved: u64,
    pub epoch: u64,
    pub day_start_ms: i64,
    pub requests: u64,
}

struct St {
    slot: Slot,
    level: u8,
    mode: ZoneMode,
    kernel: Option<Kernel>,
    kgen: u64,
    kernel_exits: Vec<Value>,
    instances: BTreeMap<String, Inst>,
    seq: u64,
    head: String,
    last_at: i64,
    idem: HashMap<String, (u64, String)>,
    queue: Vec<(String, Value)>,
    money: Money,
    panicked: Option<(String, String)>,
    last_hb: Option<Value>,
    hb_count: u64,
    owner_out: u64,
    fake_now: Option<i64>,
    clock_offset_ms: i64,
    proposals: u64,
    /// `grant.request`s answered (ADR-011 §7.4; ids `g00000001`…).
    grants: u64,
    swaps: u64,
    req_ids: u64,
    promotions: u64,
}

/// The fake Root.
pub struct Root {
    pub cfg: Config,
    pub charter: Charter,
    st: Mutex<St>,
    cv: Condvar,
    clients: Mutex<Vec<UnixStream>>,
    ring: Mutex<VecDeque<Vec<u8>>>,
    log: Mutex<Option<File>>,
    shutdown: AtomicBool,
    delivering: Mutex<()>,
    kernel_sha256: String,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn root_err(code: RootErrorCode, msg: impl AsRef<str>, data: Option<Value>) -> ErrorBody {
    let e = code.error(msg);
    match data {
        Some(d) => e.with_data(&d),
        None => e,
    }
}

/// A reply to a kernel request: the response plus fds for fd 4.
struct Reply {
    result: Result<Value, ErrorBody>,
    fds: Vec<OwnedFd>,
}

impl Reply {
    fn ok<T: serde::Serialize>(x: &T) -> Reply {
        match icj::to_value(x) {
            Ok(v) => Reply {
                result: Ok(v),
                fds: Vec::new(),
            },
            Err(e) => Reply::err(root_err(RootErrorCode::Internal, e.to_string(), None)),
        }
    }
    fn err(e: ErrorBody) -> Reply {
        Reply {
            result: Err(e),
            fds: Vec::new(),
        }
    }
}

impl Root {
    /// Build the Root (creates the `ROOT_HOME` layout, continues an existing lineage).
    pub fn new(cfg: Config) -> Result<Arc<Root>, String> {
        let charter = match &cfg.charter {
            Some(p) => Charter::load(p)?,
            None => Charter::default(),
        };
        for d in [
            "run",
            "logs",
            "lineage",
            "state/proposals",
            "store/blobs",
            "egress/usage",
        ] {
            fs::create_dir_all(cfg.root_home.join(d))
                .map_err(|e| format!("{}: {e}", cfg.root_home.join(d).display()))?;
        }
        fs::create_dir_all(&cfg.home).map_err(|e| format!("{}: {e}", cfg.home.display()))?;
        let _ = fs::set_permissions(&cfg.root_home, fs::Permissions::from_mode(0o700));
        let kernel_sha256 = util::sha256_file(&cfg.kernel)
            .map(|x| x.0)
            .unwrap_or_else(|_| util::sha256_hex(b""));
        let mut level = 0u8;
        let mut mode = ZoneMode::Normal;
        for m in &cfg.modes {
            match m {
                Mode::Level(n) => level = *n,
                Mode::Safe => mode = ZoneMode::Safe,
                _ => {}
            }
        }
        let fake_now = std::env::var("INIT_ROOT_TEST_NOW")
            .ok()
            .and_then(|s| util::parse_rfc3339(&s));
        let st = St {
            slot: Slot::Idle,
            level: level.min(charter.max_level),
            mode,
            kernel: None,
            kgen: 0,
            kernel_exits: Vec::new(),
            instances: BTreeMap::new(),
            seq: 0,
            head: ZERO_HASH.into(),
            last_at: 0,
            idem: HashMap::new(),
            queue: Vec::new(),
            money: Money::default(),
            panicked: None,
            last_hb: None,
            hb_count: 0,
            owner_out: 0,
            fake_now,
            clock_offset_ms: 0,
            proposals: 0,
            grants: 0,
            swaps: 0,
            req_ids: 0,
            promotions: 0,
        };
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(cfg.root_home.join("logs/fakeroot.jsonl"))
            .ok();
        let root = Arc::new(Root {
            cfg,
            charter,
            st: Mutex::new(st),
            cv: Condvar::new(),
            clients: Mutex::new(Vec::new()),
            ring: Mutex::new(VecDeque::new()),
            log: Mutex::new(log),
            shutdown: AtomicBool::new(false),
            delivering: Mutex::new(()),
            kernel_sha256,
        });
        root.load_lineage();
        root.load_queue();
        {
            let mut st = lock(&root.st);
            let now = root.now_locked(&st);
            st.money.day_start_ms = root.day_start(now);
            if st.seq == 0 && st.head == ZERO_HASH {
                let pubkey = "fake";
                root.append_locked(&mut st, "root", "genesis", json!({"fake": true, "install_id": util::random_hex(8), "pubkey": pubkey, "root_version": VERSION, "test": true}));
            }
            root.append_locked(&mut st, "root", "root_start", json!({"root_sha256": util::sha256_hex(VERSION.as_bytes()), "root_version": VERSION}));
            if let Ok(b) = fs::read(root.r("run/panic")) {
                let v: Value = serde_json::from_slice(&b).unwrap_or(json!({}));
                st.panicked = Some((
                    v["at"].as_str().unwrap_or("").to_string(),
                    v["channel"].as_str().unwrap_or("latch").to_string(),
                ));
                st.slot = Slot::Panicked;
            }
        }
        root.write_status();
        Ok(root)
    }

    /// A path under `ROOT_HOME`.
    pub fn r(&self, rel: &str) -> PathBuf {
        self.cfg.root_home.join(rel)
    }

    fn now_locked(&self, st: &St) -> i64 {
        st.fake_now.unwrap_or_else(util::now_ms) + st.clock_offset_ms
    }

    /// The Root clock (the fake clock when `INIT_ROOT_TEST_NOW` was set; frozen between moves).
    pub fn now_ms(&self) -> i64 {
        let st = lock(&self.st);
        self.now_locked(&st)
    }

    fn day_start(&self, now: i64) -> i64 {
        let off = self.charter.day_offset_min * 60_000;
        (now + off).div_euclid(86_400_000) * 86_400_000 - off
    }

    /// Log one record to `ROOT_HOME/logs/fakeroot.jsonl`.
    pub fn log(&self, rec: Value) {
        let mut rec = rec;
        if let Value::Object(m) = &mut rec {
            m.insert("t".into(), Value::String(util::ts()));
        }
        if let Some(f) = lock(&self.log).as_mut() {
            let mut line = serde_json::to_vec(&rec).unwrap_or_default();
            line.push(b'\n');
            let _ = f.write_all(&line);
        }
    }

    fn trace(self: &Arc<Self>) -> Trace {
        let me = Arc::downgrade(self);
        Arc::new(move |dir: &'static str, line: &[u8]| {
            if let Some(r) = me.upgrade() {
                let frame: Value = serde_json::from_slice(line)
                    .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(line).into()));
                r.log(json!({"dir": dir, "frame": frame, "link": true}));
            }
        })
    }

    // ---------------------------------------------------------------------------------------
    // Lineage (R4 record format, unsigned: the fake never holds a key)
    // ---------------------------------------------------------------------------------------

    fn load_lineage(&self) {
        let recs = util::read_jsonl(&self.r("lineage/lineage.jsonl"));
        let mut st = lock(&self.st);
        if let Some(last) = recs.last() {
            st.seq = last["seq"].as_u64().unwrap_or(0) + 1;
            st.head = last["hash"].as_str().unwrap_or(ZERO_HASH).to_string();
            st.last_at = last["at"]
                .as_str()
                .and_then(util::parse_rfc3339)
                .unwrap_or(0);
            for r in &recs {
                if r["kind"] == "ladder"
                    && let Some(to) = r["data"]["to"].as_u64()
                {
                    st.level = to as u8;
                }
            }
        }
        for i in util::read_jsonl(&self.r("lineage/idem.jsonl")) {
            if let (Some(k), Some(s), Some(h)) =
                (i["idem"].as_str(), i["seq"].as_u64(), i["hash"].as_str())
            {
                st.idem.insert(k.to_string(), (s, h.to_string()));
            }
        }
    }

    fn append_locked(&self, st: &mut St, by: &str, kind: &str, data: Value) -> (u64, String) {
        let now = self.now_locked(st).max(st.last_at);
        st.last_at = now;
        let seq = st.seq;
        let body = json!({"at": rfc3339(now), "by": by, "data": data, "kind": kind, "prev": st.head, "seq": seq, "v": 1});
        let bytes =
            icj::to_vec(&body).unwrap_or_else(|_| serde_json::to_vec(&body).unwrap_or_default());
        let hash = format!("sha256:{}", util::sha256_hex(&bytes));
        let mut rec = body;
        rec["hash"] = Value::String(hash.clone());
        rec["sig"] = Value::String("fake:unsigned".into());
        if let Err(e) = util::append_json(&self.r("lineage/lineage.jsonl"), &rec, true) {
            eprintln!("init-fakeroot: lineage append failed: {e}");
        }
        st.seq += 1;
        st.head = hash.clone();
        (seq, hash)
    }

    /// Append a Root-written record.
    pub fn append(&self, kind: &str, data: Value) -> (u64, String) {
        let mut st = lock(&self.st);
        self.append_locked(&mut st, "root", kind, data)
    }

    // ---------------------------------------------------------------------------------------
    // Owner queue (state/owner-queue.jsonl) and clients
    // ---------------------------------------------------------------------------------------

    fn load_queue(&self) {
        let q: Vec<(String, Value)> = util::read_jsonl(&self.r("state/owner-queue.jsonl"))
            .into_iter()
            .filter_map(|v| Some((v["qid"].as_str()?.to_string(), v["frame"].clone())))
            .collect();
        lock(&self.st).queue = q;
    }

    fn save_queue(&self, q: &[(String, Value)]) {
        let mut buf = Vec::new();
        for (qid, f) in q {
            buf.extend(
                serde_json::to_vec(&json!({"frame": f, "qid": qid, "v": 1})).unwrap_or_default(),
            );
            buf.push(b'\n');
        }
        let _ = util::write_atomic(&self.r("state/owner-queue.jsonl"), &buf, true);
    }

    /// Broadcast a frame to attached owner clients and keep it in the replay ring.
    pub fn emit(&self, frame: &Value) {
        let mut line = serde_json::to_vec(frame).unwrap_or_default();
        line.push(b'\n');
        {
            let mut ring = lock(&self.ring);
            ring.push_back(line.clone());
            while ring.len() > 2000 {
                ring.pop_front();
            }
        }
        let mut cl = lock(&self.clients);
        cl.retain_mut(|c| c.write_all(&line).is_ok());
    }

    fn notice(&self, level: &str, text: &str) {
        self.emit(&json!({"level": level, "t": "notice", "text": text, "ts": util::ts()}));
    }

    /// Queue an owner frame for the kernel (`line` or `interrupt`) and deliver it if a kernel is ready.
    pub fn enqueue(self: &Arc<Self>, frame: Value) -> String {
        let qid = util::random_hex(8);
        let (q, ready) = {
            let mut st = lock(&self.st);
            st.queue.push((qid.clone(), frame.clone()));
            (
                st.queue.clone(),
                st.kernel.as_ref().is_some_and(|k| k.ready),
            )
        };
        self.save_queue(&q);
        if frame["t"] == "line" {
            self.emit(&json!({"qid": qid, "t": "echo", "text": frame["text"]}));
        }
        if ready {
            self.deliver_queue();
        } else {
            self.notice(
                "info",
                &format!(
                    "[root] the system is restarting; your message is queued ({})",
                    q.len()
                ),
            );
        }
        qid
    }

    /// Redeliver every queued owner frame in order (one delivery thread at a time).
    pub fn deliver_queue(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let _g = lock(&me.delivering);
            loop {
                let (next, link) = {
                    let st = lock(&me.st);
                    let link = st
                        .kernel
                        .as_ref()
                        .filter(|k| k.ready)
                        .map(|k| k.link.clone());
                    (st.queue.first().cloned(), link)
                };
                let (Some((qid, frame)), Some(link)) = (next, link) else {
                    return;
                };
                let frame: OwnerInFrame = match serde_json::from_value(frame) {
                    Ok(f) => f,
                    Err(_) => {
                        me.drop_queued(&qid);
                        continue;
                    }
                };
                match link.request_typed(
                    "owner.in",
                    &OwnerInArgs {
                        qid: qid.clone(),
                        frame,
                    },
                    REQ_TIMEOUT,
                ) {
                    Ok(r) if r.is_ok() => me.drop_queued(&qid),
                    Ok(r) => {
                        me.log(json!({"ev": "owner_in_refused", "qid": qid, "resp": r.to_value()}));
                        return;
                    }
                    Err(e) => {
                        me.log(
                            json!({"ev": "owner_in_failed", "qid": qid, "error": e.to_string()}),
                        );
                        return;
                    }
                }
            }
        });
    }

    fn drop_queued(&self, qid: &str) {
        let q = {
            let mut st = lock(&self.st);
            st.queue.retain(|(q, _)| q != qid);
            st.queue.clone()
        };
        self.save_queue(&q);
    }

    // ---------------------------------------------------------------------------------------
    // Status
    // ---------------------------------------------------------------------------------------

    /// The status object (`status` request, `state/root.json`).
    pub fn status(&self) -> Value {
        let st = lock(&self.st);
        let now = self.now_locked(&st);
        let insts: Vec<Value> = st
            .instances
            .iter()
            .map(|(id, i)| json!({"instance": id, "kind": i.kind, "lane": i.lane, "pid": i.pid, "started_at": i.started_at, "program": i.program, "kgen": i.kgen}))
            .collect();
        let bodies = st.instances.values().filter(|i| i.kind == "body").count();
        json!({
            "v": 1,
            "root_version": VERSION,
            "slot": st.slot.name(),
            "level": st.level,
            "cap": self.charter.max_level,
            "mode": st.mode,
            "panicked": st.panicked.as_ref().map(|(at, ch)| json!({"at": at, "channel": ch})),
            "kernel": st.kernel.as_ref().map(|k| json!({"pid": k.pid, "kgen": k.kgen, "hello": k.hello, "ready": k.ready, "reason": k.reason, "started_at": k.started_at})),
            "kernel_exits": st.kernel_exits,
            "instances": insts,
            "bodies": bodies,
            "lineage": {"seq": st.seq.saturating_sub(1), "head": st.head},
            "queued": st.queue.len(),
            "money": {"day_spent_musd": st.money.day_spent, "day_reserved_musd": st.money.day_reserved,
                      "total_spent_musd": st.money.total_spent, "epoch": st.money.epoch, "requests": st.money.requests},
            "hb": {"count": st.hb_count, "last": st.last_hb},
            "owner_out": st.owner_out,
            "now": rfc3339(now),
        })
    }

    fn write_status(&self) {
        let s = self.status();
        let _ = util::write_atomic(
            &self.r("state/root.json"),
            &serde_json::to_vec_pretty(&s).unwrap_or_default(),
            false,
        );
    }

    /// The current level.
    pub fn level(&self) -> u8 {
        lock(&self.st).level
    }

    pub fn is_panicked(&self) -> bool {
        lock(&self.st).panicked.is_some()
    }

    // ---------------------------------------------------------------------------------------
    // Money (used by the egress)
    // ---------------------------------------------------------------------------------------

    fn roll_day(&self, st: &mut St) {
        let now = self.now_locked(st);
        let start = self.day_start(now);
        if start != st.money.day_start_ms {
            st.money.day_start_ms = start;
            st.money.day_spent = 0;
        }
    }

    /// The egress money block (root/egress.md §4.2).
    pub fn money_block(&self) -> MoneyBlock {
        let mut st = lock(&self.st);
        self.roll_day(&mut st);
        MoneyBlock {
            day_limit_musd: self.charter.day_limit_musd,
            day_reserved_musd: st.money.day_reserved,
            day_resets_at: rfc3339(st.money.day_start_ms + 86_400_000),
            day_spent_musd: st.money.day_spent,
            day_starts_at: rfc3339(st.money.day_start_ms),
            sealed_reserve_musd: self.charter.sealed_reserve_musd,
            total_limit_musd: self.charter.total_limit_musd,
            total_spent_musd: st.money.total_spent,
            extra: BTreeMap::new(),
        }
    }

    /// `ceilings.get` / `GET /_root/ceilings` (with the broker counts when `broker` is true).
    pub fn ceilings(&self, broker: bool) -> Ceilings {
        let money = self.money_block();
        let st = lock(&self.st);
        let bodies = st.instances.values().filter(|i| i.kind == "body").count() as u32;
        let classes = vec![
            json!({"id": "providers", "kind": "model", "min_level": 0, "enabled": true, "requests_today": st.money.requests,
                   "requests_limit": 100_000, "bytes_today": 0, "bytes_limit": 8_000_000_000u64, "rate_tokens": 600, "rate_limit": 600}),
            json!({"id": "crates", "kind": "http", "min_level": 0, "enabled": false, "requests_today": 0, "requests_limit": 5000,
                   "bytes_today": 0, "bytes_limit": 2_000_000_000u64, "rate_tokens": 300, "rate_limit": 300}),
        ];
        Ceilings {
            v: Some(V1),
            epoch: Some(st.money.epoch),
            level: Some(st.level),
            money: Some(money),
            models: self.charter.models_json(st.level).as_array().cloned(),
            classes: Some(classes),
            instances: broker.then(|| ActiveLimit {
                active: st.instances.len() as u32,
                limit: self.charter.max_instances,
            }),
            parallel_bodies: broker.then(|| ActiveLimit {
                active: bodies,
                limit: self.body_limit(st.level),
            }),
            extra: BTreeMap::from([("extend".to_string(), json!({"mode": self.charter.extend_mode}))]),
        }
    }

    fn body_limit(&self, level: u8) -> u32 {
        if level >= 2 {
            self.charter.parallel_bodies.max(1)
        } else {
            1
        }
    }

    /// Reserve `r` micro-USD against the day and total ceilings. Returns the epoch the admission
    /// saw, or the refusal code with `(limit, spent)`.
    pub fn reserve(&self, r: u64) -> Result<(u64, u64), (&'static str, u64, u64, u64)> {
        let mut st = lock(&self.st);
        self.roll_day(&mut st);
        let m = &st.money;
        if m.day_spent + m.day_reserved + r > self.charter.day_limit_musd {
            return Err((
                "ceiling.day",
                self.charter.day_limit_musd,
                m.day_spent,
                m.day_reserved,
            ));
        }
        let total_limit = self
            .charter
            .total_limit_musd
            .saturating_sub(self.charter.sealed_reserve_musd);
        if m.total_spent + m.total_reserved + r > total_limit {
            return Err((
                "ceiling.total",
                total_limit,
                m.total_spent,
                m.total_reserved,
            ));
        }
        st.money.day_reserved += r;
        st.money.total_reserved += r;
        st.req_ids += 1;
        Ok((st.money.epoch, st.req_ids))
    }

    /// Settle a reservation with the metered cost.
    pub fn settle(&self, r: u64, cost: u64) {
        {
            let mut st = lock(&self.st);
            st.money.day_reserved = st.money.day_reserved.saturating_sub(r);
            st.money.total_reserved = st.money.total_reserved.saturating_sub(r);
            st.money.day_spent += cost;
            st.money.total_spent += cost;
            st.money.epoch += 1;
            st.money.requests += 1;
        }
        self.write_status();
    }

    // ---------------------------------------------------------------------------------------
    // The kernel process
    // ---------------------------------------------------------------------------------------

    fn kernel_env(&self) -> Vec<(String, String)> {
        let home = self.cfg.home.display().to_string();
        let path = format!(
            "{home}/kernel/bin:{}",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into())
        );
        let mut env = vec![
            ("INIT_HOME".to_string(), home.clone()),
            ("INIT_ROOT_FD".into(), "3".into()),
            ("INIT_ROOT_FDPASS".into(), "4".into()),
            ("PATH".into(), path),
            ("HOME".into(), format!("{home}/cache/kernel-home")),
            ("TMPDIR".into(), format!("{home}/cache/tmp/")),
            (
                "LANG".into(),
                if cfg!(target_os = "macos") {
                    "en_US.UTF-8".into()
                } else {
                    "C.UTF-8".into()
                },
            ),
        ];
        // Test mode is always on in the fake: INIT_TEST, INIT_TEST_* and INIT_FAULT pass through
        // (root/root.md §4.4), plus the fake kernel's and fake body's mode variables.
        for (k, v) in std::env::vars() {
            if k == "INIT_TEST"
                || k.starts_with("INIT_TEST_")
                || k == "INIT_FAULT"
                || k == "FAKEKERNEL_MODE"
                || k == "FAKEBODY_MODE"
            {
                env.push((k, v));
            }
        }
        env.extend(self.cfg.kernel_env.iter().cloned());
        env
    }

    /// Spawn the kernel with the control link on fds 3 and 4.
    pub fn start_kernel(self: &Arc<Self>, reason: &str) -> Result<i32, String> {
        {
            let st = lock(&self.st);
            if st.panicked.is_some() {
                return Err("panicked: resume first".into());
            }
            if st.kernel.is_some() {
                return Err("a kernel is already running".into());
            }
        }
        let home = &self.cfg.home;
        for d in ["cache/kernel-home", "cache/tmp", "run", "state"] {
            let _ = fs::create_dir_all(home.join(d));
        }
        let (s_root, s_kern) = UnixStream::pair().map_err(|e| e.to_string())?;
        let (d_root, d_kern) = UnixDatagram::pair().map_err(|e| e.to_string())?;
        let kgen = {
            let mut st = lock(&self.st);
            st.kgen += 1;
            st.kgen
        };
        let logf = File::create(self.r(&format!("logs/kernel-{kgen}.stderr")))
            .map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&self.cfg.kernel);
        cmd.args(&self.cfg.kernel_args)
            .env_clear()
            .envs(self.kernel_env())
            .current_dir(home)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(logf.try_clone().map_err(|e| e.to_string())?)
            .stderr(logf);
        let (dup3, dup4) = sys::inherit_3_4(&mut cmd, s_kern.as_raw_fd(), d_kern.as_raw_fd())
            .map_err(|e| e.to_string())?;
        // Fds 3 and 4 are the link; any other inherited fd is a leak (crate::watchdog, K-FLAKE).
        let child = crate::watchdog::spawn_keeping(&mut cmd, 5)
            .map_err(|e| format!("{}: {e}", self.cfg.kernel.display()))?;
        drop(cmd);
        drop((dup3, dup4, s_kern, d_kern));
        let pid = child.id() as i32;
        drop(child);
        let (link, rx) =
            Link::new(s_root, d_root, "r-", Some(self.trace())).map_err(|e| e.to_string())?;
        {
            let mut st = lock(&self.st);
            st.kernel = Some(Kernel {
                pid,
                kgen,
                link: link.clone(),
                hello: false,
                ready: false,
                reason: reason.into(),
                started_at: util::ts(),
            });
            st.slot = Slot::Starting;
        }
        self.log(json!({"ev": "kernel_spawn", "pid": pid, "kgen": kgen, "reason": reason}));
        self.write_status();
        let me = self.clone();
        std::thread::spawn(move || me.dispatch(kgen, link, rx));
        let me = self.clone();
        std::thread::spawn(move || me.reap_kernel(pid, kgen));
        Ok(pid)
    }

    fn reap_kernel(self: Arc<Self>, pid: i32, kgen: u64) {
        let _ = sys::wait_exit_nowait(pid);
        let _ = sys::killpg(pid, sys::SIGKILL); // the kernel's own group (held pid)
        let info = sys::reap(pid).unwrap_or(sys::ExitInfo {
            code: None,
            signal: None,
            rusage_ms: 0,
        });
        let (sweep, rec) = {
            let mut st = lock(&self.st);
            let was = st.kernel.take_if(|k| k.kgen == kgen);
            let ready = was.as_ref().is_some_and(|k| k.ready);
            let class = match (info.code, st.slot) {
                (Some(0), Slot::Stopping | Slot::ShuttingDown) => "stop",
                (Some(75), _) => "swap",
                (Some(78), _) if !ready => "config",
                (_, Slot::Panicked) => "panic",
                _ => "crash",
            };
            let rec = json!({"at": util::ts(), "code": info.code, "signal": info.signal, "kgen": kgen, "class": class, "rusage_ms": info.rusage_ms});
            st.kernel_exits.push(rec.clone());
            if !matches!(st.slot, Slot::Panicked | Slot::ShuttingDown) {
                st.slot = if class == "stop" {
                    Slot::Stopped
                } else {
                    Slot::Exited
                };
            }
            let tuple = FAKE_TUPLE;
            self.append_locked(
                &mut st,
                "root",
                "zone_exit",
                json!({"tuple": tuple, "code": info.code, "signal": info.signal, "class": class}),
            );
            // The Root sweeps every zone instance when the kernel ends: here, by held process group.
            let sweep: Vec<i32> = st.instances.values().map(|i| i.pid).collect();
            (sweep, rec)
        };
        for p in sweep {
            let _ = sys::killpg(p, sys::SIGKILL);
        }
        self.log(json!({"ev": "kernel_exit", "exit": rec}));
        self.notice("warn", &format!("[root] kernel exited: {rec}"));
        self.write_status();
        self.cv.notify_all();
    }

    fn dispatch(
        self: Arc<Self>,
        kgen: u64,
        link: Arc<Link>,
        rx: std::sync::mpsc::Receiver<Incoming>,
    ) {
        let mut bad = 0;
        while let Ok(m) = rx.recv() {
            match m {
                Incoming::Request(req) => {
                    bad = 0;
                    let id = req.id.clone();
                    let reply = self.handle(kgen, &link, &req);
                    let resp = match reply.result {
                        Ok(v) => Response::ok_value(id, v),
                        Err(e) => Response::err(Some(id), e),
                    };
                    let raw: Vec<_> = reply.fds.iter().map(|f| f.as_raw_fd()).collect();
                    if let Err(e) = link.respond_with_fds(&resp, &raw) {
                        self.log(json!({"ev": "respond_failed", "error": e.to_string()}));
                    }
                    drop(reply.fds);
                    self.after_request(kgen, &req);
                }
                Incoming::Event(ev) => {
                    bad = 0;
                    match KernelEvent::from_event(&ev) {
                        Ok(KernelEvent::Hb(h)) => {
                            let mut st = lock(&self.st);
                            st.hb_count += 1;
                            st.last_hb = icj::to_value(&h).ok();
                        }
                        Ok(KernelEvent::OwnerOut(o)) => {
                            lock(&self.st).owner_out += 1;
                            if let Ok(f) = icj::to_value(&o.frame) {
                                self.emit(&f);
                            }
                        }
                        Err(e) => self.log(json!({"ev": "bad_event", "error": e.to_string()})),
                    }
                }
                Incoming::Bad(e) => {
                    bad += 1;
                    self.log(json!({"ev": "bad_frame", "error": e, "in_a_row": bad}));
                    if bad >= 3 {
                        // root/root.md §3.6.3: three malformed frames in a row is a crash.
                        let pid = lock(&self.st)
                            .kernel
                            .as_ref()
                            .filter(|k| k.kgen == kgen)
                            .map(|k| k.pid);
                        if let Some(p) = pid {
                            let _ = sys::killpg(p, sys::SIGKILL);
                        }
                    }
                }
                Incoming::Eof => break,
            }
        }
    }

    /// Things to do after a response went out (redelivery after `ready`, scheduled swaps).
    fn after_request(self: &Arc<Self>, kgen: u64, req: &Request) {
        if req.cmd != "ready" {
            return;
        }
        self.deliver_queue();
        for m in self.cfg.modes.clone() {
            if let Mode::Swap { job, after_ms } = m {
                let me = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(after_ms));
                    if lock(&me.st).kernel.as_ref().is_some_and(|k| k.kgen == kgen) {
                        me.prepare_swap(&job, 10);
                    }
                });
            }
        }
    }

    fn boot_block(&self, reason: &str) -> BootBlock {
        let st = lock(&self.st);
        let body = self.cfg.home.join("bin/init-body");
        let body_sha = util::sha256_file(&body)
            .map(|x| x.0)
            .unwrap_or_else(|_| util::sha256_hex(b""));
        BootBlock {
            access: None,
            extend: Some(json!({"mode": self.charter.extend_mode})),
            boot: BootInfo {
                cause: None,
                kstate: None,
                level: st.level,
                mode: st.mode,
                panic: None,
                reason: ZoneBootReason::parse(reason),
                tuple: TupleIds {
                    audited: Some(FAKE_TUPLE.into()),
                    current: Some(FAKE_TUPLE.into()),
                    lkg: Some(FAKE_TUPLE.into()),
                },
                tuple_body: TupleBody {
                    binary: body_sha,
                    ext: TupleExt {
                        components: vec![],
                        set: None,
                    },
                    sha: "0".repeat(40),
                },
                tuple_kernel: TupleKernel {
                    built_from: "0".repeat(40),
                    init_kernel: self.kernel_sha256.clone(),
                    source: "maintainer".into(),
                    version: "0.0.0".into(),
                },
            },
            charter_sha256: self.charter.digest.clone(),
            ladder: LadderInfo {
                cap: self.charter.max_level,
                evolution_share_max_permille: None,
                level: st.level,
            },
            limits: Limits {
                hb_interval_s: self.charter.hb_interval_s,
                max_instances: self.charter.max_instances,
                parallel_bodies: self.body_limit(st.level),
            },
            paths: RootPaths {
                egress_sock: self.r("run/egress.sock").display().to_string(),
                kernel_git: Some(
                    self.cfg
                        .home
                        .join("lineage/kernel.git")
                        .display()
                        .to_string(),
                ),
            },
            root: RootVersion {
                proto: init_proto::ROOT_PROTO,
                version: VERSION.into(),
            },
        }
    }

    fn by_kernel(&self) -> String {
        format!("kernel:{}", &self.kernel_sha256[..12])
    }

    fn handle(self: &Arc<Self>, kgen: u64, link: &Arc<Link>, req: &Request) -> Reply {
        let parsed = match KernelRequest::parse(&req.cmd, &req.args) {
            Ok(p) => p,
            Err(e) => return Reply::err(e),
        };
        let level = self.level();
        match parsed {
            KernelRequest::Hello(h) => {
                if h.proto != init_proto::ROOT_PROTO {
                    return Reply::err(root_err(
                        RootErrorCode::Version,
                        format!(
                            "control-link proto {} unsupported; this Root speaks 1",
                            h.proto
                        ),
                        None,
                    ));
                }
                let reason = {
                    let mut st = lock(&self.st);
                    match st.kernel.as_mut().filter(|k| k.kgen == kgen) {
                        Some(k) => {
                            k.hello = true;
                            k.reason.clone()
                        }
                        None => "cold".into(),
                    }
                };
                let _ = fault::point("fakeroot.hello");
                Reply::ok(&self.boot_block(&reason))
            }
            KernelRequest::Ready => {
                {
                    let mut st = lock(&self.st);
                    if let Some(k) = st.kernel.as_mut().filter(|k| k.kgen == kgen) {
                        k.ready = true;
                    }
                    if st.slot == Slot::Starting {
                        st.slot = Slot::Running;
                    }
                    let (reason, mode, lvl) = (
                        st.kernel.as_ref().map(|k| k.reason.clone()),
                        st.mode,
                        st.level,
                    );
                    self.append_locked(
                        &mut st,
                        "root",
                        "zone_start",
                        json!({"tuple": FAKE_TUPLE, "reason": reason, "mode": mode, "level": lvl}),
                    );
                }
                self.write_status();
                Reply::ok(&json!({}))
            }
            KernelRequest::Spawn(a) => self.spawn(kgen, link, &req.id, &a),
            KernelRequest::Kill(k) => {
                let pid = lock(&self.st).instances.get(&k.instance).map(|i| i.pid);
                match pid {
                    None => Reply::err(root_err(
                        RootErrorCode::NotFound,
                        format!("no instance {}", k.instance),
                        None,
                    )),
                    Some(p) => {
                        let sig = if k.mode == KillMode::Term {
                            sys::SIGTERM
                        } else {
                            sys::SIGKILL
                        };
                        let n = u32::from(sys::killpg(p, sig).is_ok());
                        self.log(
                            json!({"ev": "kill", "instance": k.instance, "mode": k.mode, "pid": p}),
                        );
                        Reply::ok(&KillResult { processes: n })
                    }
                }
            }
            KernelRequest::Instances => {
                let st = lock(&self.st);
                let list: Vec<InstanceInfo> = st
                    .instances
                    .iter()
                    .filter(|(_, i)| i.kind != "kernel")
                    .map(|(id, i)| InstanceInfo {
                        instance: id.clone(),
                        kind: init_proto::root_link::InstanceKind::parse(&i.kind),
                        lane: i.lane.clone(),
                        pid: i.pid,
                        started_at: i.started_at.clone(),
                    })
                    .collect();
                Reply::ok(&list)
            }
            KernelRequest::StorePut(a) => {
                let fds = match link.take_fds(&req.id, Duration::from_secs(5)) {
                    Ok(f) if f.len() == 1 => f,
                    Ok(f) => {
                        return Reply::err(root_err(
                            RootErrorCode::Args,
                            format!("store.put needs 1 fd, got {}", f.len()),
                            None,
                        ));
                    }
                    Err(e) => {
                        return Reply::err(root_err(
                            RootErrorCode::Args,
                            format!("store.put: no fd: {e}"),
                            None,
                        ));
                    }
                };
                let mut bytes = Vec::new();
                if let Err(e) =
                    File::from(fds.into_iter().next().expect("one fd")).read_to_end(&mut bytes)
                {
                    return Reply::err(root_err(
                        RootErrorCode::Store,
                        format!("read failed: {e}"),
                        None,
                    ));
                }
                let sha = util::sha256_hex(&bytes);
                if sha != a.sha256.trim_start_matches("sha256:") || bytes.len() as u64 != a.bytes {
                    return Reply::err(root_err(
                        RootErrorCode::Store,
                        format!("hash or size mismatch: got {sha} ({} bytes)", bytes.len()),
                        Some(json!({"sha256": sha, "bytes": bytes.len()})),
                    ));
                }
                let p = self.r(&format!("store/blobs/{sha}"));
                if !p.exists() && util::write_atomic(&p, &bytes, true).is_err() {
                    return Reply::err(root_err(
                        RootErrorCode::Store,
                        "store full or unwritable",
                        None,
                    ));
                }
                self.log(
                    json!({"ev": "store_put", "kind": a.kind, "sha256": sha, "bytes": bytes.len()}),
                );
                Reply::ok(&StorePutResult { sha256: sha })
            }
            KernelRequest::StoreGet(a) => {
                let sha = a.sha256.trim_start_matches("sha256:").to_string();
                match File::open(self.r(&format!("store/blobs/{sha}"))) {
                    Ok(f) => {
                        let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
                        let mut r = Reply::ok(&StoreGetResult { bytes });
                        r.fds.push(OwnedFd::from(f));
                        r
                    }
                    Err(_) => Reply::err(root_err(
                        RootErrorCode::NotFound,
                        format!("no blob {sha}"),
                        None,
                    )),
                }
            }
            KernelRequest::TupleGet => {
                let bb = self.boot_block("cold");
                let t = json!({"id": FAKE_TUPLE, "body": bb.boot.tuple_body, "kernel": bb.boot.tuple_kernel});
                Reply::ok(&json!({"audited": t, "current": t, "lkg": t}))
            }
            KernelRequest::LineageAppend(a) => self.lineage_append(&a.kind, a.data, &a.idem),
            KernelRequest::LedgerGet => Reply::ok(&self.ledger()),
            KernelRequest::CeilingsGet => Reply::ok(&self.ceilings(true)),
            KernelRequest::SystemSwapStatus(_) => Reply::err(root_err(RootErrorCode::NotFound, "fake Root has no durable swap history", None)),
            KernelRequest::SystemSwap(a) => {
                if level < 3 {
                    return Reply::err(root_err(
                        RootErrorCode::Level,
                        format!("system.swap needs L3; the level is L{level}"),
                        Some(json!({"need": 3, "level": level})),
                    ));
                }
                let job = {
                    let mut st = lock(&self.st);
                    st.swaps += 1;
                    format!("s-{:06}", st.swaps)
                };
                self.log(json!({"ev": "system_swap", "job": job, "version": a.version}));
                let me = self.clone();
                let j = job.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    let link = lock(&me.st).kernel.as_ref().map(|k| k.link.clone());
                    if let Some(l) = link {
                        let _ = l.event("swap", json!({"job": j, "msg": "gate-of-gates skipped (fake Root)", "stage": "gog_pass"}));
                    }
                    me.prepare_swap(&j, 10);
                });
                Reply::ok(&SystemSwapResult { job })
            }
            KernelRequest::ProposalFile(p) => {
                let id = {
                    let mut st = lock(&self.st);
                    st.proposals += 1;
                    format!("p-{:06}", st.proposals)
                };
                let _ = fs::write(
                    self.r(&format!("state/proposals/{id}.md")),
                    format!("# {}\n\n{}\n", p.title, p.text),
                );
                self.append("proposal", json!({"id": id, "title": p.title, "sha256": util::sha256_hex(p.text.as_bytes()), "bytes": p.text.len()}));
                Reply::ok(&ProposalFileResult { id })
            }
            KernelRequest::SealedFamilyEval(_) => {
                if level < 3 {
                    Reply::err(root_err(
                        RootErrorCode::Level,
                        format!("sealed.family_eval needs L3; the level is L{level}"),
                        Some(json!({"need": 3, "level": level})),
                    ))
                } else {
                    Reply::err(root_err(
                        RootErrorCode::Internal,
                        "the fake Root has no sealed evaluator",
                        None,
                    ))
                }
            }
            KernelRequest::SealedG4Slice(a) => self.sealed_g4_slice(&a),
            KernelRequest::SealedHoldoutRotate => {
                if level >= 3 {
                    Reply::err(root_err(
                        RootErrorCode::Level,
                        "sealed.holdout_rotate is for L0-L2",
                        Some(json!({"level": level})),
                    ))
                } else {
                    Reply::ok(&init_proto::root_link::HoldoutRotateResult { count: 0 })
                }
            }
            KernelRequest::ChangeOpen(a) => {
                let r = self.change_open(&a);
                if let Ok(v) = &r.result {
                    let _ = link.event("extend.window", json!({"group": v["group"], "state": "open"}));
                }
                r
            }
            KernelRequest::ChangeVeto(v) => {
                self.log(json!({"ev": "change_veto", "group": v.group, "reason": v.reason}));
                Reply::ok(&json!({}))
            }
            // ADR-013: this fake Root keeps no goals; the real one is tested in root tests/gbal.rs.
            KernelRequest::Goal(cmd, _) => {
                self.log(json!({"ev": "goal", "cmd": cmd}));
                Reply::err(root_err(RootErrorCode::parse("goal.none"), "no active goal (fake Root)", None))
            }
            KernelRequest::GrantRequest(a) => {
                // The real Root's rules in small (root src/grant.rs): §7.4 kinds, 3 requests, `why` off the lineage.
                const KINDS: [&str; 11] = ["money.day_usd", "money.total_usd", "quotas.parallel_bodies", "extend.open_groups", "extend.changes_day",
                    "extend.bloat_factor", "extend.background_share", "extend.snap_max_gib", "fleet.workspace_write", "dep", "autonomy.max_level"];
                let kind = a["kind"].as_str().unwrap_or("");
                if !KINDS.contains(&kind) && !kind.starts_with("actions.") {
                    return Reply::err(root_err(RootErrorCode::Args, format!("{kind:?} is not a grant kind"), None));
                }
                let n = {
                    let mut st = lock(&self.st);
                    if st.grants >= 3 {
                        None
                    } else {
                        st.grants += 1;
                        Some(st.grants)
                    }
                };
                let Some(n) = n else {
                    return Reply::err(root_err(RootErrorCode::Rate, "at most 3 grant requests per rolling 24 h", None));
                };
                let id = format!("g{n:08x}");
                let why = a["why"].as_str().unwrap_or("");
                self.append("grant_request", json!({"duration_h": a["duration_h"].as_i64().unwrap_or(720), "id": id, "kind": kind, "value": a["value"],
                    "why_sha256": util::sha256_hex(why.as_bytes())}));
                self.log(json!({"ev": "grant_request", "id": id, "kind": kind}));
                Reply::ok(&json!({"id": id, "status": "pending"}))
            }
            KernelRequest::Alert(a) => {
                let rec = json!({"at": util::ts(), "code": a.code, "level": a.level, "message": a.message, "source": "kernel"});
                let _ = util::append_json(&self.r("alerts.jsonl"), &rec, false);
                self.notice(
                    match a.level {
                        init_proto::root_link::AlertLevel::Info => "info",
                        init_proto::root_link::AlertLevel::Warn => "warn",
                        init_proto::root_link::AlertLevel::Alert => "alert",
                    },
                    &format!("[kernel] {}: {}", a.code, a.message),
                );
                Reply::ok(&json!({}))
            }
        }
    }

    /// ADR-011 §2.3 (X2): `change.open` on the fake. A scripted refusal first, when
    /// `ROOT_HOME/test/change_open.json` (a JSON array consumed in order) holds
    /// `{"error": {"code", "message"}}` entries; then what the real Root checks before it admits:
    /// the scope, the commit ids, A0 (every candidate blob is in the store). Each admitted change
    /// opens the next group (one change per group; the fake has no owner turns), appends
    /// `change_open`, and answers `{seq, group, snapshot: null, window}`. The caller sends
    /// `extend.window{open}`; close and roll back with `test.event` (`extend.window`,
    /// `extend.rollback`).
    fn change_open(&self, a: &Value) -> Reply {
        self.log(json!({"ev": "change_open", "args": a}));
        let script = self.r("test/change_open.json");
        let mut entries: Vec<Value> = fs::read(&script).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if !entries.is_empty() {
            let next = entries.remove(0);
            let _ = util::write_atomic(&script, &serde_json::to_vec(&entries).unwrap_or_default(), false);
            if let Some(e) = next.get("error") {
                let code = e["code"].as_str().unwrap_or("root.internal");
                return Reply::err(root_err(RootErrorCode::parse(code), e["message"].as_str().unwrap_or("scripted refusal"), e.get("data").cloned()));
            }
        }
        let scope = a["scope"].as_str().unwrap_or("");
        let hexish = |s: &str| (7..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_hexdigit());
        let (sha, from) = (a["sha"].as_str().unwrap_or(""), a["from_sha"].as_str().unwrap_or(""));
        if !["t0", "t1", "t2", "dep", "k"].contains(&scope) || !hexish(sha) || !(from.is_empty() || hexish(from)) {
            return Reply::err(root_err(RootErrorCode::Args, "scope t0|t1|t2|dep|k; sha and from_sha are commit ids", None));
        }
        let c = &a["candidate"];
        let mut blobs = vec![c["body"]["binary"].as_str().unwrap_or("").to_string()];
        blobs.extend(c["body"]["t0"].as_str().map(str::to_string));
        blobs.extend(c["ext"]["components"].as_array().into_iter().flatten().filter_map(|x| x["sha256"].as_str().map(str::to_string)));
        if let Some(b) = blobs.iter().find(|b| b.len() != 64 || !self.r(&format!("store/blobs/{b}")).exists()) {
            return Reply::err(root_err(RootErrorCode::parse("root.boundary"), format!("A0: blob {b:?} is not in the store"), None));
        }
        let group = util::read_jsonl(&self.r("lineage/lineage.jsonl")).iter().filter(|r| r["kind"] == "change_open").count() as u64 + 1;
        let level = self.level();
        let (seq, _) = self.append("change_open", json!({"caps": a["caps"], "checks": {"A0": "pass", "A4": "skip", "A5": "pass"}, "from_sha": from,
            "group": group, "level": level, "open_groups": 1, "parent_tuple": FAKE_TUPLE, "readmit": false, "scope": scope, "sha": sha,
            "snapshot": null, "tuple": FAKE_TUPLE}));
        Reply::ok(&json!({"group": group, "seq": seq, "snapshot": null, "window": {"max_h": 72, "min_h": 2, "quiet": 5, "turns": 20}}))
    }

    /// `sealed.g4_slice` (M7, lane GATE-V2a; wire format pinned in build/status/ROOT-S1a.md). The
    /// fake has no S: it answers from the test script `ROOT_HOME/test/sealed_g4.json`, a JSON
    /// array consumed in order, each entry either a result object (`seq` is filled in from the
    /// `sealed_g4` record it appends) or `{"error": {"code", "message"}}`. No script, or an
    /// exhausted one → `root.sealed_none`. It checks what the real Root checks before running:
    /// the body blob is stored (`root.store`), the incumbent is the LKG tuple and `t0` is null
    /// (`root.args`). Every request is logged (`ev:"sealed_g4_slice"`) with its args.
    fn sealed_g4_slice(&self, a: &Value) -> Reply {
        self.log(json!({"ev": "sealed_g4_slice", "args": a}));
        let script = self.r("test/sealed_g4.json");
        let mut entries: Vec<Value> = fs::read(&script).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if entries.is_empty() {
            return Reply::err(root_err(RootErrorCode::parse("root.sealed_none"), "no S v1 drawn (fake Root)", None));
        }
        let next = entries.remove(0);
        let _ = util::write_atomic(&script, &serde_json::to_vec(&entries).unwrap_or_default(), false);
        if let Some(e) = next.get("error") {
            let code = e["code"].as_str().unwrap_or("root.internal");
            return Reply::err(root_err(RootErrorCode::parse(code), e["message"].as_str().unwrap_or("scripted refusal"), e.get("data").cloned()));
        }
        let body = a["candidate"]["body"].as_str().unwrap_or("");
        if body.len() != 64 || !self.r(&format!("store/blobs/{body}")).exists() {
            return Reply::err(root_err(RootErrorCode::Store, format!("no body blob {body}"), None));
        }
        if a["incumbent"].as_str() != Some(FAKE_TUPLE) || !a["candidate"]["t0"].is_null() {
            return Reply::err(root_err(RootErrorCode::Args, "incumbent must be the lkg tuple; t0 must be null", None));
        }
        let mut res = next;
        let rec = json!({"gate_id": a["gate_id"], "candidate": body, "incumbent": a["incumbent"], "tier": a["tier"]["name"],
                         "n": res["n"], "repeats": res["repeats"], "status": res["status"], "d_permille": res["d_permille"],
                         "m_permille": res["m_permille"], "cpp_ratio_permille": res["cpp_ratio_permille"], "spent_musd": res["spent_musd"]});
        let (seq, _) = self.append("sealed_g4", rec);
        res["seq"] = json!(seq);
        Reply::ok(&res)
    }

    fn lineage_append(&self, kind: &str, data: Value, idem: &str) -> Reply {
        let kind_ok = !kind.is_empty()
            && kind.len() <= 42
            && kind
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
        if !kind_ok || !data.is_object() {
            return Reply::err(root_err(
                RootErrorCode::Lineage,
                format!("bad record: kind `{kind}`, data must be an object"),
                None,
            ));
        }
        if icj::check_integers_only(&data).is_err()
            || icj::depth(&data) > 8
            || serde_json::to_vec(&data).map_or(0, |b| b.len()) > 16 * 1024
        {
            return Reply::err(root_err(
                RootErrorCode::Lineage,
                "data must be ICJ, depth ≤ 8, ≤ 16 KiB",
                None,
            ));
        }
        let by = self.by_kernel();
        let mut st = lock(&self.st);
        if let Some((seq, hash)) = st.idem.get(idem).cloned() {
            return Reply::ok(&LineageAppendResult {
                dup: true,
                hash,
                seq,
            });
        }
        if kind == "promotion" {
            st.promotions += 1;
        }
        let (seq, hash) = self.append_locked(&mut st, &by, kind, data);
        st.idem.insert(idem.to_string(), (seq, hash.clone()));
        let _ = util::append_json(
            &self.r("lineage/idem.jsonl"),
            &json!({"hash": hash, "idem": idem, "seq": seq}),
            true,
        );
        Reply::ok(&LineageAppendResult {
            dup: false,
            hash,
            seq,
        })
    }

    /// `ledger.get` (root/root.md §4.5): the level, cap and the newest ladder change.
    pub fn ledger(&self) -> Ledger {
        let st = lock(&self.st);
        Ledger {
            v: V1,
            cap: self.charter.max_level,
            entered_at: rfc3339(st.last_at),
            entered_seq: st.seq.saturating_sub(1),
            evolution_share_max_permille: None,
            incident: None,
            last_change: None,
            level: st.level,
            max_level: self.charter.max_level,
            next: (st.level < self.charter.max_level).then(|| init_proto::root_link::LedgerNext {
                criteria: vec![init_proto::root_link::Criterion {
                    have: st.promotions as i64,
                    id: "min_promotions".into(),
                    met: false,
                    need: 20,
                }],
                level: st.level + 1,
            }),
            seq: st.seq.saturating_sub(1),
            supported_max_level: 4,
            extra: BTreeMap::new(),
        }
    }

    fn spawn(self: &Arc<Self>, kgen: u64, link: &Arc<Link>, id: &str, a: &SpawnArgs) -> Reply {
        let kind = a.kind.as_str().to_string();
        let (level, mode, panicked, bodies, total) = {
            let st = lock(&self.st);
            let bodies = st
                .instances
                .iter()
                .filter(|(iid, i)| i.kind == "body" && a.replaces.as_deref() != Some(iid.as_str()))
                .count() as u32;
            (
                st.level,
                st.mode,
                st.panicked.is_some(),
                bodies,
                st.instances.len() as u32,
            )
        };
        let refuse = |code: RootErrorCode, msg: String, data: Value| {
            if matches!(code, RootErrorCode::Level | RootErrorCode::Quota) {
                let demotes = code == RootErrorCode::Level;
                self.append("ceiling_hit", json!({"ceiling": data["quota"], "code": code.as_str(), "limit": data["limit"],
                    "attempted": data["used"].as_u64().map(|u| u + 1), "demotes": demotes, "rule": if demotes { "b" } else { "none" }, "source": "broker"}));
            }
            self.log(
                json!({"ev": "spawn_refused", "kind": kind, "code": code.as_str(), "message": msg}),
            );
            Reply::err(root_err(code, msg, Some(data)))
        };
        if panicked {
            return refuse(
                RootErrorCode::Mode,
                "panicked: every spawn is refused".into(),
                json!({"mode": "panicked"}),
            );
        }
        if mode == ZoneMode::Safe && ["build", "check", "probe"].contains(&kind.as_str()) {
            return refuse(
                RootErrorCode::Mode,
                format!("system safe mode refuses `{kind}`"),
                json!({"mode": "safe"}),
            );
        }
        if self
            .cfg
            .modes
            .iter()
            .any(|m| matches!(m, Mode::Refuse(k) if *k == kind))
        {
            return refuse(
                RootErrorCode::Quota,
                format!("refused by FAKEROOT_MODE refuse:{kind}"),
                json!({"quota": "instances", "limit": 0, "used": total}),
            );
        }
        if let Some(k) = a.env.keys().find(|k| k.starts_with("INIT_ROOT_")) {
            return refuse(
                RootErrorCode::Args,
                format!("env key {k} is not allowed (V4)"),
                json!({"key": k}),
            );
        }
        if !Path::new(&a.program).is_absolute() || !Path::new(&a.cwd).is_absolute() {
            return refuse(
                RootErrorCode::Args,
                "program and cwd must be absolute".into(),
                json!({}),
            );
        }
        if kind == "body" {
            let limit = self.body_limit(level);
            if bodies >= limit {
                return if level < 2 {
                    refuse(
                        RootErrorCode::Level,
                        format!("a second body needs L2; the level is L{level}"),
                        json!({"need": 2, "level": level, "quota": "parallel_bodies", "limit": limit, "used": bodies}),
                    )
                } else {
                    refuse(
                        RootErrorCode::Quota,
                        format!("parallel_bodies limit {limit} reached"),
                        json!({"quota": "parallel_bodies", "limit": limit, "used": bodies}),
                    )
                };
            }
        }
        if total >= self.charter.max_instances {
            return refuse(
                RootErrorCode::Quota,
                format!("max_instances {} reached", self.charter.max_instances),
                json!({"quota": "instances", "limit": self.charter.max_instances, "used": total}),
            );
        }
        let pipes =
            (|| -> io::Result<_> { Ok((std::io::pipe()?, std::io::pipe()?, std::io::pipe()?)) })();
        let ((in_r, in_w), (out_r, out_w), (err_r, err_w)) = match pipes {
            Ok(p) => p,
            Err(e) => {
                return Reply::err(root_err(
                    RootErrorCode::Internal,
                    format!("pipe: {e}"),
                    None,
                ));
            }
        };
        let mut cmd = Command::new(&a.program);
        cmd.args(&a.args)
            .env_clear()
            .envs(&a.env)
            .current_dir(&a.cwd)
            .process_group(0)
            .stdin(Stdio::from(in_r))
            .stdout(Stdio::from(out_w))
            .stderr(Stdio::from(err_w));
        // No fd beyond the three pipes, even one another thread had not flagged yet (K-FLAKE).
        let child = match crate::watchdog::spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                return refuse(
                    RootErrorCode::Args,
                    format!("spawn {}: {e}", a.program),
                    json!({}),
                );
            }
        };
        drop(cmd); // closes the child's ends that Command still holds
        let pid = child.id() as i32;
        drop(child);
        let instance = util::random_hex(6);
        let started_at = util::ts();
        {
            let mut st = lock(&self.st);
            st.instances.insert(
                instance.clone(),
                Inst {
                    kind: kind.clone(),
                    lane: a.lane.clone(),
                    pid,
                    started_at: started_at.clone(),
                    kgen,
                    program: a.program.clone(),
                },
            );
        }
        self.log(json!({"ev": "spawn", "instance": instance, "kind": kind, "lane": a.lane, "pid": pid, "program": a.program, "req": id}));
        self.write_status();
        let me = self.clone();
        let (inst, l) = (instance.clone(), link.clone());
        std::thread::spawn(move || me.reap_instance(inst, pid, kgen, l));
        let _ = fault::point("fakeroot.spawn.before_fds");
        let mut r = Reply::ok(&SpawnResult {
            instance,
            pid,
            started_at,
        });
        r.fds = vec![
            OwnedFd::from(in_w),
            OwnedFd::from(out_r),
            OwnedFd::from(err_r),
        ];
        r
    }

    fn reap_instance(self: Arc<Self>, instance: String, pid: i32, kgen: u64, link: Arc<Link>) {
        let _ = sys::wait_exit_nowait(pid);
        // Stop whatever the instance left in its own process group (held: the zombie leader keeps
        // the group id reserved until the reap below).
        let _ = sys::killpg(pid, sys::SIGKILL);
        let info = sys::reap(pid).unwrap_or(sys::ExitInfo {
            code: None,
            signal: None,
            rusage_ms: 0,
        });
        let current = {
            let mut st = lock(&self.st);
            st.instances.remove(&instance);
            st.kernel.as_ref().is_some_and(|k| k.kgen == kgen)
        };
        let data = ExitedData {
            at: util::ts(),
            code: info.code,
            instance: instance.clone(),
            rusage_ms: info.rusage_ms,
            signal: info.signal,
        };
        self.log(
            json!({"ev": "exited", "instance": instance, "code": info.code, "signal": info.signal}),
        );
        if current && let Ok(v) = icj::to_value(&data) {
            let _ = link.event("exited", v);
        }
        self.write_status();
        self.cv.notify_all();
    }

    // ---------------------------------------------------------------------------------------
    // Root → kernel
    // ---------------------------------------------------------------------------------------

    fn kernel_link(&self) -> Option<(Arc<Link>, i32, u64)> {
        lock(&self.st)
            .kernel
            .as_ref()
            .map(|k| (k.link.clone(), k.pid, k.kgen))
    }

    /// Wait until kernel generation `kgen` has exited (true) or `timeout` passes.
    fn wait_kernel_exit(&self, kgen: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = lock(&self.st);
        loop {
            if st.kernel.as_ref().is_none_or(|k| k.kgen != kgen) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self
                .cv
                .wait_timeout(st, left.min(Duration::from_millis(100)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn last_exit(&self) -> Value {
        lock(&self.st)
            .kernel_exits
            .last()
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Send `stop{reason, grace_ms}`, wait for the exit; kill the kernel's group after the grace.
    pub fn stop_kernel(&self, reason: &str, grace_ms: u64) -> Value {
        let Some((link, pid, kgen)) = self.kernel_link() else {
            return json!({"kernel": "not running", "exit": self.last_exit()});
        };
        {
            let mut st = lock(&self.st);
            if st.slot != Slot::ShuttingDown {
                st.slot = Slot::Stopping;
            }
        }
        let args = StopArgs {
            reason: reason.into(),
            grace_ms,
        };
        let answered =
            match link.request_typed("stop", &args, Duration::from_millis(grace_ms + 10_000)) {
                Ok(r) => r.to_value(),
                Err(e) => json!({"error": e.to_string()}),
            };
        let mut killed = false;
        if !self.wait_kernel_exit(kgen, Duration::from_millis(grace_ms + 10_000)) {
            let _ = sys::killpg(pid, sys::SIGKILL);
            killed = true;
            self.wait_kernel_exit(kgen, Duration::from_secs(10));
        }
        json!({"answer": answered, "killed": killed, "exit": self.last_exit()})
    }

    /// Send `prepare_swap{job, deadline_s}`; on exit 75 start the kernel again (`kernel_upgrade`).
    pub fn prepare_swap(self: &Arc<Self>, job: &str, deadline_s: u64) -> Value {
        let Some((link, pid, kgen)) = self.kernel_link() else {
            return json!({"kernel": "not running"});
        };
        let _ = link.event(
            "swap",
            json!({"job": job, "msg": "preparing", "stage": "prepare"}),
        );
        let args = PrepareSwapArgs {
            job: job.into(),
            deadline_s,
        };
        let answered =
            match link.request_typed("prepare_swap", &args, Duration::from_secs(deadline_s + 10)) {
                Ok(r) => r.to_value(),
                Err(e) => json!({"error": e.to_string()}),
            };
        if !self.wait_kernel_exit(kgen, Duration::from_secs(deadline_s + 10)) {
            let _ = sys::killpg(pid, sys::SIGKILL);
            self.wait_kernel_exit(kgen, Duration::from_secs(10));
        }
        let exit = self.last_exit();
        self.log(json!({"ev": "swap_done", "job": job, "answer": answered, "exit": exit}));
        let restarted = if exit["code"] == 75 && !self.shutdown.load(Ordering::SeqCst) {
            self.start_kernel("kernel_upgrade").is_ok()
        } else {
            false
        };
        json!({"answer": answered, "exit": exit, "restarted": restarted})
    }

    /// Change the level and emit a `level` event (fake-only `test.level`).
    pub fn set_level(&self, to: u8, cause: LevelCause) -> Value {
        let to = to.min(self.charter.max_level);
        let (from, seq, link) = {
            let mut st = lock(&self.st);
            let from = st.level;
            let direction = if to >= from { "up" } else { "down" };
            let tseq = st.seq;
            let (seq, _) = self.append_locked(&mut st, "root", "ladder",
                json!({"from": from, "to": to, "direction": direction, "cause": cause, "trigger": {"kind": "test_fixture", "seq": tseq}}));
            st.level = to;
            (from, seq, st.kernel.as_ref().map(|k| k.link.clone()))
        };
        let data = LevelData {
            cap: self.charter.max_level,
            cause,
            evolution_share_max_permille: None,
            from,
            r4_seq: seq,
            to,
            trigger: None,
        };
        if let (Some(l), Ok(v)) = (link, icj::to_value(&data)) {
            let _ = l.event("level", v);
        }
        self.write_status();
        json!({"from": from, "to": to, "r4_seq": seq})
    }

    /// Panic (root/root.md §3.9.2, as far as a fake can): latch, refuse spawns and egress, kill
    /// every instance and the kernel by held process group, record, demote one level.
    pub fn panic(&self, channel: &str) -> Value {
        let (killed, pids) = {
            let mut st = lock(&self.st);
            if st.panicked.is_some() {
                return json!({"already": true});
            }
            let at = rfc3339(self.now_locked(&st));
            st.panicked = Some((at.clone(), channel.to_string()));
            st.slot = Slot::Panicked;
            let latch = self.r("run/panic");
            if !latch.exists() {
                let _ = util::write_atomic(
                    &latch,
                    &serde_json::to_vec(&json!({"at": at, "channel": channel, "v": 1}))
                        .unwrap_or_default(),
                    true,
                );
            }
            let mut pids: Vec<i32> = st.instances.values().map(|i| i.pid).collect();
            if let Some(k) = &st.kernel {
                pids.push(k.pid);
            }
            (
                (st.instances.len() + usize::from(st.kernel.is_some())) as u64,
                pids,
            )
        };
        for p in &pids {
            let _ = sys::killpg(*p, sys::SIGKILL);
        }
        let (from, to) = {
            let mut st = lock(&self.st);
            let (pseq, _) = self.append_locked(&mut st, "root", "panic", json!({"channel": channel, "killed": {"instances": killed, "processes": pids.len()}}));
            let from = st.level;
            let to = from.saturating_sub(1);
            self.append_locked(&mut st, "root", "ladder", json!({"from": from, "to": to, "direction": "down", "cause": "demotion", "trigger": {"kind": "panic", "seq": pseq}}));
            st.level = to;
            (from, to)
        };
        self.notice("alert", &format!("PANIC at {} via {channel}: killed {} instances; egress closed; level {from}→{to}. init-root resume to restart.", util::ts(), killed));
        self.write_status();
        json!({"channel": channel, "killed": killed, "level": {"from": from, "to": to}})
    }

    /// Resume after a panic: delete the latch by its exact path, record, start the kernel.
    pub fn resume(self: &Arc<Self>) -> Result<Value, ErrorBody> {
        {
            let mut st = lock(&self.st);
            if st.panicked.is_none() {
                return Err(root_err(RootErrorCode::Mode, "not panicked", None));
            }
            st.panicked = None;
            st.slot = Slot::Idle;
            self.append_locked(&mut st, "root", "resume", json!({}));
        }
        let _ = fs::remove_file(self.r("run/panic"));
        // The killed kernel must be reaped before a new one starts.
        let _ = util::poll(Duration::from_secs(5), Duration::from_millis(20), || {
            lock(&self.st).kernel.is_none().then_some(())
        });
        let r = self.start_kernel("panic_resume");
        Ok(json!({"started": r.is_ok(), "error": r.err()}))
    }

    // ---------------------------------------------------------------------------------------
    // Owner socket
    // ---------------------------------------------------------------------------------------

    fn owner_request(self: &Arc<Self>, v: &Value) -> Value {
        let id = v["id"].clone();
        let args = &v["args"];
        let r: Result<Value, ErrorBody> = if v["v"] != 1 {
            Err(root_err(
                RootErrorCode::Version,
                "unsupported protocol version",
                None,
            ))
        } else {
            match v["cmd"].as_str().unwrap_or("") {
                "status" => Ok(self.status()),
                "ledger" => icj::to_value(&self.ledger())
                    .map_err(|e| root_err(RootErrorCode::Internal, e.to_string(), None)),
                "lineage" => Ok(Value::Array(util::read_jsonl(
                    &self.r("lineage/lineage.jsonl"),
                ))),
                "panic" => Ok(self.panic("owner_socket")),
                "resume" => self.resume(),
                "stop" => Ok(self.stop_kernel(
                    args["reason"].as_str().unwrap_or("owner"),
                    args["grace_ms"].as_u64().unwrap_or(30_000),
                )),
                "start" => self
                    .start_kernel(args["reason"].as_str().unwrap_or("cold"))
                    .map(|pid| json!({"pid": pid}))
                    .map_err(|e| root_err(RootErrorCode::Mode, e, None)),
                "shutdown" => {
                    self.shutdown.store(true, Ordering::SeqCst);
                    Ok(json!({"shutting_down": true}))
                }
                "test.clock" => {
                    let mut st = lock(&self.st);
                    if let Some(ms) = args["advance_ms"].as_i64().filter(|m| *m >= 0) {
                        st.clock_offset_ms += ms;
                        Ok(json!({"now": rfc3339(self.now_locked(&st))}))
                    } else if let Some(t) = args["set"].as_str().and_then(util::parse_rfc3339) {
                        let now = self.now_locked(&st);
                        if t < now {
                            Err(root_err(
                                RootErrorCode::Args,
                                "the clock never moves back",
                                None,
                            ))
                        } else {
                            st.clock_offset_ms += t - now;
                            Ok(json!({"now": rfc3339(t)}))
                        }
                    } else {
                        Err(root_err(
                            RootErrorCode::Args,
                            "test.clock needs advance_ms ≥ 0 or set",
                            None,
                        ))
                    }
                }
                "test.level" => match args["to"].as_u64() {
                    Some(to) if to <= 4 => {
                        let cause = if args["cause"] == "demotion" {
                            LevelCause::Demotion
                        } else {
                            LevelCause::Unlock
                        };
                        Ok(self.set_level(to as u8, cause))
                    }
                    _ => Err(root_err(
                        RootErrorCode::Args,
                        "test.level needs to: 0..=4",
                        None,
                    )),
                },
                "test.event" => match (self.kernel_link(), args["ev"].as_str()) {
                    (Some((l, _, _)), Some(ev)) => l
                        .event(ev, args["data"].clone())
                        .map(|_| json!({"sent": true}))
                        .map_err(|e| root_err(RootErrorCode::Internal, e.to_string(), None)),
                    (None, _) => Err(root_err(RootErrorCode::Mode, "no kernel", None)),
                    _ => Err(root_err(RootErrorCode::Args, "test.event needs ev", None)),
                },
                "test.request" => match (self.kernel_link(), args["cmd"].as_str()) {
                    (Some((l, _, _)), Some(cmd)) => {
                        let t =
                            Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(10_000));
                        l.request(cmd, args["args"].clone(), t)
                            .map(|r| r.to_value())
                            .map_err(|e| root_err(RootErrorCode::Internal, e.to_string(), None))
                    }
                    (None, _) => Err(root_err(RootErrorCode::Mode, "no kernel", None)),
                    _ => Err(root_err(
                        RootErrorCode::Args,
                        "test.request needs cmd",
                        None,
                    )),
                },
                "test.prepare_swap" => Ok(self.prepare_swap(
                    args["job"].as_str().unwrap_or("s-test"),
                    args["deadline_s"].as_u64().unwrap_or(10),
                )),
                // No approval-type command exists (D4): /approve, /deny, /clear-safe are gone.
                c => Err(root_err(
                    RootErrorCode::UnknownCmd,
                    format!("unknown command {c:?}"),
                    None,
                )),
            }
        };
        let resp = match r {
            Ok(res) => Response::ok_value(id.as_str().unwrap_or("").to_string(), res),
            Err(e) => Response::err(id.as_str().map(str::to_string), e),
        };
        resp.to_value()
    }

    fn owner_line(self: &Arc<Self>, text: &str) {
        match text.trim() {
            "/panic" => {
                self.panic("owner_socket");
            }
            "/resume" if self.is_panicked() => {
                if let Err(e) = self.resume() {
                    self.notice("warn", &format!("[root] {}", e.message));
                }
            }
            "/root status" => self.notice("info", &format!("[root] {}", self.status())),
            _ => {
                self.enqueue(json!({"t": "line", "text": text}));
            }
        }
    }

    fn owner_conn(self: Arc<Self>, s: UnixStream) {
        let Ok(mut w) = s.try_clone() else { return };
        let mut rd = BufReader::new(s);
        let mut buf = String::new();
        loop {
            buf.clear();
            match (&mut rd).take(1024 * 1024 + 1).read_line(&mut buf) {
                Ok(n) if n > 0 && buf.ends_with('\n') => {}
                _ => break,
            }
            let Ok(v) = serde_json::from_str::<Value>(&buf) else {
                let _ = w.write_all(b"{\"error\":{\"code\":\"root.proto\",\"details\":[],\"message\":\"malformed frame\"},\"id\":null,\"ok\":false,\"v\":1}\n");
                continue;
            };
            if v.get("cmd").is_some() {
                let mut r = serde_json::to_vec(&self.owner_request(&v)).unwrap_or_default();
                r.push(b'\n');
                if w.write_all(&r).is_err() {
                    break;
                }
                continue;
            }
            match v["t"].as_str() {
                Some("panic") => {
                    self.panic("owner_socket");
                }
                Some("attach") => {
                    let Ok(mut c) = w.try_clone() else { break };
                    let replay = v["replay"] != false;
                    let ring = lock(&self.ring);
                    let mut cl = lock(&self.clients);
                    if replay {
                        for l in ring.iter() {
                            let _ = c.write_all(l);
                        }
                    }
                    let _ = c.write_all(b"{\"t\":\"replay_end\"}\n");
                    cl.push(c);
                }
                Some("line") => self.owner_line(v["text"].as_str().unwrap_or("")),
                Some("interrupt") => {
                    self.enqueue(json!({"t": "interrupt"}));
                }
                _ => break, // detach or unknown
            }
        }
        let _ = w.shutdown(std::net::Shutdown::Both);
    }

    fn serve_owner(self: Arc<Self>, l: UnixListener) {
        for c in l.incoming().flatten() {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let me = self.clone();
            std::thread::spawn(move || me.owner_conn(c));
        }
    }

    fn watch(self: Arc<Self>) {
        let latch = self.r("run/panic");
        loop {
            std::thread::sleep(Duration::from_millis(100));
            if self.shutdown.load(Ordering::SeqCst) {
                return;
            }
            if sys::term_requested() {
                self.shutdown.store(true, Ordering::SeqCst);
                return;
            }
            if sys::take_usr1() {
                self.panic("signal");
            }
            if !self.is_panicked() && latch.exists() {
                self.panic("latch");
            }
        }
    }

    /// Bind the sockets, start the services and (unless `--no-start`) the kernel; return when a
    /// `shutdown` request or SIGTERM arrives, after an orderly stop. Returns the exit code.
    pub fn run(self: &Arc<Self>) -> i32 {
        sys::install_flag_handlers();
        let _ = fs::write(self.r("run/root.pid"), format!("{}\n", std::process::id()));
        let _ = fs::write(self.r("run/canary"), b"");
        let bind = |name: &str| -> Result<UnixListener, String> {
            let p = self.r(&format!("run/{name}"));
            if p.as_os_str().len() > 103 {
                return Err(format!(
                    "{}: socket path longer than 103 bytes",
                    p.display()
                ));
            }
            let _ = fs::remove_file(&p);
            let l = UnixListener::bind(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
            Ok(l)
        };
        let (owner, egress_l) = match (bind("owner.sock"), bind("egress.sock")) {
            (Ok(o), Ok(e)) => (o, e),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("init-fakeroot: {e}");
                return 74;
            }
        };
        let me = self.clone();
        std::thread::spawn(move || me.serve_owner(owner));
        let me = self.clone();
        std::thread::spawn(move || egress::serve(me, egress_l));
        let me = self.clone();
        std::thread::spawn(move || me.watch());
        self.log(json!({"ev": "root_start", "pid": std::process::id(), "modes": format!("{:?}", self.cfg.modes)}));
        if self.cfg.autostart
            && !self.is_panicked()
            && let Err(e) = self.start_kernel("cold")
        {
            eprintln!("init-fakeroot: cannot start the kernel: {e}");
        }
        while !self.shutdown.load(Ordering::SeqCst) && !sys::term_requested() {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.shutdown.store(true, Ordering::SeqCst);
        lock(&self.st).slot = Slot::ShuttingDown;
        let stopped = self.stop_kernel("shutdown", 5_000);
        self.log(json!({"ev": "shutdown", "stop": stopped}));
        // Anything still held: stop by process group, then wait for the reapers.
        let pids: Vec<i32> = lock(&self.st).instances.values().map(|i| i.pid).collect();
        for p in pids {
            let _ = sys::killpg(p, sys::SIGKILL);
        }
        let _ = util::poll(Duration::from_secs(5), Duration::from_millis(20), || {
            lock(&self.st).instances.is_empty().then_some(())
        });
        self.write_status();
        for name in ["run/owner.sock", "run/egress.sock", "run/root.pid"] {
            let _ = fs::remove_file(self.r(name));
        }
        0
    }
}

/// The `init-fakeroot` entry point.
pub fn main_with_args(args: &[String]) -> i32 {
    let cfg = match Config::from_args(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("init-fakeroot: {e}");
            return 64;
        }
    };
    match Root::new(cfg) {
        Ok(r) => r.run(),
        Err(e) => {
            eprintln!("init-fakeroot: {e}");
            66
        }
    }
}

/// A small client for `ROOT_HOME/run/owner.sock` (the runner and tests use it).
pub struct OwnerClient {
    r: BufReader<UnixStream>,
    n: u64,
}

impl OwnerClient {
    /// Connect, retrying for up to 5 s while the socket is missing or not yet listening (a
    /// socket file exists between `bind` and `listen`).
    pub fn connect(sock: &Path) -> io::Result<OwnerClient> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let s = loop {
            match UnixStream::connect(sock) {
                Ok(s) => break s,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        };
        s.set_read_timeout(Some(Duration::from_secs(120)))?;
        Ok(OwnerClient {
            r: BufReader::new(s),
            n: 0,
        })
    }

    /// Send a channel frame (`line`, `interrupt`, `panic`, `attach`, `detach`).
    pub fn frame(&mut self, f: &Value) -> io::Result<()> {
        let mut b = serde_json::to_vec(f)?;
        b.push(b'\n');
        self.r.get_mut().write_all(&b)
    }

    /// Send a request frame and read its response.
    pub fn request(&mut self, cmd: &str, args: Value) -> io::Result<Value> {
        self.n += 1;
        self.frame(&json!({"args": args, "cmd": cmd, "id": format!("o-{}", self.n), "v": 1}))?;
        loop {
            let v = self.read()?;
            if v.get("ok").is_some() {
                return Ok(v);
            }
        }
    }

    /// Read one line from the socket.
    pub fn read(&mut self) -> io::Result<Value> {
        let mut s = String::new();
        if self.r.read_line(&mut s)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "owner socket closed",
            ));
        }
        serde_json::from_str(&s).map_err(io::Error::other)
    }

    pub fn set_timeout(&self, t: Duration) -> io::Result<()> {
        self.r.get_ref().set_read_timeout(Some(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(parse_modes("").unwrap(), vec![Mode::Ok]);
        assert_eq!(
            parse_modes("level:2,safe,refuse:build,swap:s-1@250").unwrap(),
            vec![
                Mode::Level(2),
                Mode::Safe,
                Mode::Refuse("build".into()),
                Mode::Swap {
                    job: "s-1".into(),
                    after_ms: 250
                }
            ]
        );
        assert!(parse_modes("level:9").is_err());
        assert!(parse_modes("bogus").is_err());
        assert_eq!(
            Upstream::parse("http://127.0.0.1:9/").unwrap(),
            Upstream::Tcp("127.0.0.1:9".into())
        );
        assert!(Upstream::parse("https://x").is_err());
    }

    /// RF-07 (X2) on init-fakeroot: `change.open` refusals from the script, A0's blob check, one group
    /// per change and the `change_open` record; `extend.mode` from the charter reaches the boot block
    /// and the ceilings.
    #[test]
    fn rf07_change_open_on_the_fake() {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/x2");
        fs::create_dir_all(&parent).unwrap();
        let base = fs::canonicalize(&parent).unwrap().join(format!("fakeroot-open-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("charter.toml"), "[extend]\nmode = \"free\"\n").unwrap();
        let argv: Vec<String> = vec!["--root-home".into(), base.join("root").display().to_string(), "--home".into(), base.join("home").display().to_string(),
            "--kernel".into(), "/nonexistent/init-kernel".into(), "--charter".into(), base.join("charter.toml").display().to_string()];
        let root = Root::new(Config::from_args(&argv).expect("config")).unwrap();
        assert_eq!(root.boot_block("cold").extend, Some(json!({"mode": "free"})));
        assert_eq!(root.ceilings(true).extra["extend"], json!({"mode": "free"}));
        let (body, t0) = (util::sha256_hex(b"body"), util::sha256_hex(b"t0"));
        let args = json!({"sha": "a".repeat(40), "from_sha": "b".repeat(40), "scope": "t0", "prefix_change": false, "caps": [{"id": "skill/x", "op": "add"}],
                          "candidate": {"body": {"binary": body, "sha": "a".repeat(40), "t0": t0}, "ext": {"components": [], "set": null}}});
        let code = |r: Reply| r.result.unwrap_err().code.as_str().to_string();
        fs::create_dir_all(root.r("test")).unwrap();
        fs::write(root.r("test/change_open.json"), serde_json::to_vec(&json!([{"error": {"code": "extend.budget", "message": "4 groups open"}}])).unwrap()).unwrap();
        assert_eq!(code(root.change_open(&args)), "extend.budget");
        assert_eq!(code(root.change_open(&args)), "root.boundary", "A0: the blobs are not stored");
        util::write_atomic(&root.r(&format!("store/blobs/{body}")), b"body", true).unwrap();
        util::write_atomic(&root.r(&format!("store/blobs/{t0}")), b"t0", true).unwrap();
        let mut bad = args.clone();
        bad["scope"] = json!("t9");
        assert_eq!(code(root.change_open(&bad)), "root.args");
        let one = root.change_open(&args).result.unwrap();
        let two = root.change_open(&args).result.unwrap();
        assert_eq!((one["group"].as_u64(), two["group"].as_u64()), (Some(1), Some(2)));
        assert!(two["seq"].as_u64() > one["seq"].as_u64());
        let recs = util::read_jsonl(&root.r("lineage/lineage.jsonl"));
        let opens: Vec<&Value> = recs.iter().filter(|r| r["kind"] == "change_open").collect();
        assert_eq!(opens.len(), 2);
        assert_eq!((opens[0]["data"]["scope"].as_str(), opens[0]["data"]["caps"][0]["id"].as_str()), (Some("t0"), Some("skill/x")));
        drop(root);
        assert!(base.to_string_lossy().contains("/target/test-homes/x2/"));
        let _ = fs::remove_dir_all(&base);
    }

    /// GATE-V2a: `sealed.g4_slice` on the fake (the pinned wire format of build/status/ROOT-S1a.md):
    /// no script → `root.sealed_none`; scripted refusals; the store, incumbent and t0 checks; a
    /// scripted answer gets the seq of the `sealed_g4` record it appends.
    #[test]
    fn sealed_g4_slice_script() {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/GATE-V2a");
        fs::create_dir_all(&parent).unwrap();
        let base = fs::canonicalize(&parent).unwrap().join(format!("fakeroot-slice-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let argv: Vec<String> = vec!["--root-home".into(), base.join("root").display().to_string(), "--home".into(), base.join("home").display().to_string(), "--kernel".into(), "/nonexistent/init-kernel".into()];
        let cfg = Config::from_args(&argv).expect("config");
        let root = Root::new(cfg).unwrap();
        let body = util::sha256_hex(b"body");
        let args = json!({"gate_id": "j-000051", "candidate": {"body": body, "ext": {"set": null, "components": []}, "t0": null},
                          "incumbent": FAKE_TUPLE, "tier": {"name": "standard", "n": 32, "repeats": 3}, "budget_musd": 3000000, "wall_s": 5400});
        let code = |r: Reply| r.result.unwrap_err().code.as_str().to_string();
        assert_eq!(code(root.sealed_g4_slice(&args)), "root.sealed_none");
        fs::create_dir_all(root.r("test")).unwrap();
        let ans = json!({"seq": 0, "status": "superior", "ratchet_signal": false, "unconfirmed": 0, "ratchet_k": 4, "tier": "standard",
                         "n": 32, "repeats": 3, "d_permille": 200, "m_permille": 220, "cpp_ratio_permille": 950, "looks_left": null,
                         "s_version": "s1.0", "dup": false, "spent_musd": 371000});
        let script = json!([{"error": {"code": "root.busy", "message": "a slice is running"}}, ans, ans, ans]);
        fs::write(root.r("test/sealed_g4.json"), serde_json::to_vec(&script).unwrap()).unwrap();
        assert_eq!(code(root.sealed_g4_slice(&args)), "root.busy");
        assert_eq!(code(root.sealed_g4_slice(&args)), "root.store", "the body blob is not stored");
        util::write_atomic(&root.r(&format!("store/blobs/{body}")), b"body", true).unwrap();
        let mut bad = args.clone();
        bad["incumbent"] = json!("t111111111111");
        assert_eq!(code(root.sealed_g4_slice(&bad)), "root.args");
        let ok = root.sealed_g4_slice(&args).result.unwrap();
        assert_eq!((ok["status"].as_str(), ok["d_permille"].as_i64()), (Some("superior"), Some(200)));
        assert!(ok["seq"].as_u64().unwrap() > 0);
        let lineage = fs::read_to_string(root.r("lineage/lineage.jsonl")).unwrap();
        assert!(lineage.contains("\"kind\":\"sealed_g4\"") && !lineage.contains("tasks"));
        assert_eq!(code(root.sealed_g4_slice(&args)), "root.sealed_none", "the script is used up");
        drop(root);
        assert!(base.to_string_lossy().contains("/target/test-homes/GATE-V2a/"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn args() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let c = Config::from_args(&a(
            "--root-home /r --home /h --kernel /k --mode level:1 --kernel-env A=b",
        ))
        .unwrap();
        assert_eq!(c.kernel_args, vec!["supervise", "--under-root"]);
        assert_eq!(c.modes, vec![Mode::Level(1)]);
        assert_eq!(c.kernel_env, vec![("A".to_string(), "b".to_string())]);
        assert!(Config::from_args(&a("--root-home r --home /h --kernel /k")).is_err());
        assert!(Config::from_args(&a("--home /h")).is_err());
    }
}
