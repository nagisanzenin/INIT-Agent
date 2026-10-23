//! R4: the off-machine mirror (root/root.md §3.11), the mirror-ref panic channel (§3.9.1), the daily
//! full verification (§3.10.4) and verify rule 8. The mirror is **off** unless the charter sets
//! `mirror.url`; with `root.test_mode` only `file://` URLs are used, so a test never reaches a network.
//! The daemon starts the lineage thread with `spawn(&root)` after cold start.

use crate::charter::Charter;
use crate::install::{self, Paths};
use crate::link::{Root, lock};
use crate::sup::Slot;
use crate::util::{self, icj, now_ms, rfc3339};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CHUNK: usize = 10_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const DAY_MS: i64 = 86_400_000;

/// The mirror's charter settings, and the git binary of `install.json`.
pub struct Cfg {
    pub url: String,
    pub branch: String,
    interval_ms: i64,
    poll_ms: Option<i64>,
    pub git: String,
}

impl Cfg {
    /// None when the mirror is off: no `mirror.url`, or a non-`file://` URL in test mode (and a
    /// `file://` URL outside it, which C9 already refuses).
    pub fn of(p: &Paths, c: &Charter) -> Option<Cfg> {
        let u = &c.mirror.url;
        if u.is_empty() || u.starts_with("file://") != c.root.test_mode {
            return None;
        }
        let info: Value = std::fs::read_to_string(p.r("install.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Some(Cfg { url: u.clone(), branch: c.mirror.branch.clone(), interval_ms: c.mirror.push_interval_s.max(1) * 1000,
            poll_ms: c.panic.mirror_ref.then_some(c.panic.mirror_poll_s.max(1) * 1000),
            git: info["git"].as_str().unwrap_or("/usr/bin/git").to_string() })
    }
}

/// The hardened Root git prefix `$RG` (§3.11) on `lineage/mirror.git`: no owner git or SSH config,
/// the mirror deploy key only, UTC, the Root's own identity.
fn rg(p: &Paths, git: &str) -> Command {
    let q = |x: &Path| format!("'{}'", x.display().to_string().replace('\'', r"'\''"));
    let ssh = format!("ssh -F /dev/null -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20 -o UserKnownHostsFile={} \
        -o StrictHostKeyChecking=accept-new", q(&p.r("keys/mirror_ed25519")), q(&p.r("keys/known_hosts")));
    let mut c = install::git(p, git);
    c.env("GIT_ASKPASS", "/usr/bin/false").arg("-c").arg(format!("core.sshCommand={ssh}"))
        .args(["-c", "protocol.allow=never", "-c", "protocol.ssh.allow=always", "-c", "protocol.file.allow=always"])
        .arg(format!("--git-dir={}", p.r("lineage/mirror.git").display()));
    c
}

/// One git call fed `input`, killed (our own child, by its handle) after 120 s. Ok(stdout) on exit 0,
/// else Err(stdout and stderr).
pub(super) fn run(mut c: Command, input: &[u8]) -> Result<String, String> {
    let mut ch = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("git: {e}"))?;
    let (mut i, input) = (ch.stdin.take(), input.to_vec());
    let w = std::thread::spawn(move || i.as_mut().map(|i| i.write_all(&input)));
    let reader = |r: Option<Box<dyn Read + Send>>| std::thread::spawn(move || {
        let mut s = String::new();
        r.map(|mut r| r.read_to_string(&mut s));
        s
    });
    let (o, e) = (reader(ch.stdout.take().map(|x| Box::new(x) as _)), reader(ch.stderr.take().map(|x| Box::new(x) as _)));
    let t = Instant::now();
    let st = loop {
        match ch.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t.elapsed() < GIT_TIMEOUT => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = ch.kill();
                let _ = ch.wait();
                break None;
            }
        }
    };
    let _ = w.join();
    let (out, err) = (o.join().unwrap_or_default(), e.join().unwrap_or_default());
    match st {
        Some(s) if s.success() => Ok(out),
        Some(s) => Err(format!("git exited {:?}: {} {}", s.code(), out.trim(), err.trim())),
        None => Err("git timed out after 120 s".into()),
    }
}

fn ensure_repo(p: &Paths, git: &str) -> Result<(), String> {
    let dir = p.r("lineage/mirror.git");
    if dir.join("HEAD").is_file() {
        return Ok(());
    }
    let mut c = install::git(p, git);
    c.args(["init", "--bare", "-q", "--template="]).arg(&dir);
    run(c, b"").map(drop)
}

pub enum Fail {
    Diverged(String),
    Other(String),
}

/// One push (§3.11): the chain's complete lines as `lineage-NNNNNN.jsonl` chunk blobs (full chunks
/// reused from the parent tree), `HEAD.json` and `lineage.pub`; a commit on `refs/heads/lineage`
/// (CAS); `push` without force; `ls-remote` must show the commit. Ok: the pushed (seq, commit).
pub fn push(p: &Paths, c: &Cfg) -> Result<(u64, String), Fail> {
    let o = Fail::Other;
    ensure_repo(p, &c.git).map_err(o)?;
    let git = |args: &[&str], input: &[u8]| {
        let mut x = rg(p, &c.git);
        x.args(args);
        run(x, input)
    };
    let bytes = std::fs::read(p.r("lineage/lineage.jsonl")).map_err(|e| o(e.to_string()))?;
    let end = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let lines: Vec<&[u8]> = bytes[..end].split_inclusive(|&b| b == b'\n').collect();
    let last: Value = lines.last().and_then(|l| serde_json::from_slice(l).ok()).ok_or_else(|| o("the lineage is empty".into()))?;
    let (seq, hash) = (last["seq"].as_u64().unwrap_or(0), last["hash"].as_str().unwrap_or("").to_string());
    let parent = git(&["rev-parse", "-q", "--verify", "refs/heads/lineage^{commit}"], b"").ok().map(|s| s.trim().to_string());
    let old: HashMap<String, String> = match &parent {
        Some(pc) => git(&["ls-tree", pc], b"").map_err(o)?.lines()
            .filter_map(|l| l.split_once('\t').and_then(|(m, n)| Some((n.to_string(), m.split(' ').nth(2)?.to_string())))).collect(),
        None => HashMap::new(),
    };
    let blob = |b: &[u8]| git(&["hash-object", "-w", "--stdin"], b).map(|s| s.trim().to_string());
    let head = format!("{}\n", icj(&json!({"at": last["at"], "hash": hash, "seq": seq})).unwrap_or_default());
    let mut tree = format!("100644 blob {}\tHEAD.json\n", blob(head.as_bytes()).map_err(o)?);
    for (i, ch) in lines.chunks(CHUNK).enumerate() {
        let name = format!("lineage-{i:06}.jsonl");
        let id = match old.get(&name) {
            Some(id) if old.contains_key(&format!("lineage-{:06}.jsonl", i + 1)) => id.clone(), // it was full already
            _ => blob(&ch.concat()).map_err(o)?,
        };
        tree += &format!("100644 blob {id}\t{name}\n");
    }
    let key = std::fs::read(p.r("keys/lineage.pub")).map_err(|e| o(e.to_string()))?;
    tree += &format!("100644 blob {}\tlineage.pub\n", blob(&key).map_err(o)?);
    let t = git(&["mktree"], tree.as_bytes()).map_err(o)?.trim().to_string();
    let same = parent.as_ref().and_then(|pc| git(&["rev-parse", &format!("{pc}^{{tree}}")], b"").ok()).is_some_and(|x| x.trim() == t);
    let commit = match &parent {
        Some(pc) if same => pc.clone(),
        _ => {
            let msg = format!("lineage {seq} {}", hash.strip_prefix("sha256:").and_then(|h| h.get(..12)).unwrap_or(""));
            let mut a = vec!["commit-tree", t.as_str(), "-m", msg.as_str()];
            a.extend(parent.iter().flat_map(|pc| ["-p", pc.as_str()]));
            let cm = git(&a, b"").map_err(o)?.trim().to_string();
            git(&["update-ref", "refs/heads/lineage", &cm, parent.as_deref().unwrap_or("")], b"").map_err(o)?;
            cm
        }
    };
    super::env_fault("root.mirror.after_commit").map_err(|e| o(e.to_string()))?;
    let spec = format!("refs/heads/lineage:refs/heads/{}", c.branch);
    if let Err(e) = git(&["push", "--porcelain", &c.url, &spec], b"") {
        let rejected = ["non-fast-forward", "fetch first", "[rejected]"].iter().any(|k| e.contains(k));
        return Err(if rejected { Fail::Diverged(e) } else { Fail::Other(e) });
    }
    let remote = git(&["ls-remote", &c.url, &format!("refs/heads/{}", c.branch)], b"").map_err(o)?;
    if remote.split_whitespace().next() != Some(commit.as_str()) {
        return Err(o(format!("after the push the remote shows {:?}", remote.split_whitespace().next())));
    }
    let _ = git(&["gc", "--auto", "--quiet"], b"");
    Ok((seq, commit))
}

pub enum Prefix {
    Unreachable(String),
    Bad(u64, &'static str),
}

/// Rule 8: the remote's `HEAD.json` names a record of the local chain with the same hash, so the remote
/// is a prefix and the local file was not rewritten. Ok(None): the remote has no branch yet.
pub fn check_prefix(p: &Paths, c: &Cfg, text: &str) -> Result<Option<u64>, Prefix> {
    let un = Prefix::Unreachable;
    ensure_repo(p, &c.git).map_err(un)?;
    let git = |args: &[&str]| {
        let mut x = rg(p, &c.git);
        x.args(args);
        run(x, b"")
    };
    let r = git(&["ls-remote", &c.url, &format!("refs/heads/{}", c.branch)]).map_err(un)?;
    let Some(id) = r.split_whitespace().next() else { return Ok(None) };
    if git(&["cat-file", "-e", &format!("{id}^{{commit}}")]).is_err() {
        git(&["fetch", "--no-tags", "--no-write-fetch-head", &c.url, &format!("+refs/heads/{}:refs/mirror/remote", c.branch)]).map_err(un)?;
    }
    let hj: Value = git(&["cat-file", "blob", &format!("{id}:HEAD.json")]).ok().and_then(|s| serde_json::from_str(&s).ok())
        .ok_or(Prefix::Bad(0, "8: the mirror has no readable HEAD.json"))?;
    let seq = hj["seq"].as_u64().ok_or(Prefix::Bad(0, "8: the mirror's HEAD.json has no seq"))?;
    match text.lines().nth(seq as usize).and_then(|l| serde_json::from_str::<Value>(l).ok()) {
        Some(r) if r["hash"] == hj["hash"] => Ok(Some(seq)),
        Some(_) => Err(Prefix::Bad(seq, "8: the mirror holds another record at this seq (a history was rewritten)")),
        None => Err(Prefix::Bad(seq, "8: the mirror is ahead of the local chain (the local file was truncated)")),
    }
}

/// The panic branch on the mirror remote (`panic.mirror_ref`): its commit id, if the branch exists.
pub fn panic_ref(p: &Paths, c: &Cfg) -> Result<Option<String>, String> {
    ensure_repo(p, &c.git)?;
    let mut x = rg(p, &c.git);
    x.args(["ls-remote", &c.url, "refs/heads/panic"]);
    Ok(run(x, b"")?.split_whitespace().next().map(String::from))
}

/// §3.9.5 step 1 (build-deviations §5 L5): the commit of the mirror's `refs/heads/panic` as the channel's
/// last `ls-remote` saw it, while the channel is on. `resume` asks this instead of calling `ls-remote`
/// itself, because a git call may take up to 120 s and the owner's CLI waits 10 s. Asking also moves the next
/// poll to now, so a branch the owner has just deleted is seen within a second (the thread ticks every
/// second). With no mirror (`mirror.url = ""` until M0, or a URL the mode refuses, see `Cfg::of`) or
/// `panic.mirror_ref = false` there is no branch to watch: nothing is polled, and nothing is checked.
pub fn panic_ref_seen(c: Option<&Cfg>) -> Option<String> {
    c.filter(|c| c.poll_ms.is_some())?;
    let mut s = lock(&ST);
    s.poll_next = 0;
    s.panic_seen.clone()
}

/// The thread's state, served in the owner `status` as `mirror`.
struct St {
    state: &'static str,
    seq: Option<u64>,
    at: Option<i64>,
    error: Option<String>,
    failing_since: Option<i64>,
    alerted: bool,
    backoff_ms: i64,
    next_ms: i64,
    poll_next: i64,
    panic_seen: Option<String>,
}

static ST: Mutex<St> = Mutex::new(St { state: "off", seq: None, at: None, error: None, failing_since: None, alerted: false,
    backoff_ms: 0, next_ms: 0, poll_next: 0, panic_seen: None });

/// `status.mirror`: `{state: off|idle|ok|failing|diverged, pushed_seq, last_push_at, last_error}`.
#[allow(dead_code)] // the owner status calls it (link.rs, WP21)
pub fn status() -> Value {
    let s = lock(&ST);
    json!({"last_error": s.error, "last_push_at": s.at.map(rfc3339), "pushed_seq": s.seq, "state": s.state})
}

/// What the daily verification calls on a corrupt chain: the supervisor's `Halted{lineage}` (WP21).
pub type Halt = fn(&Arc<Root>, &str);

/// Starts the lineage thread after cold start: pushes (every `push_interval_s` when there are new records,
/// at once after the urgent kinds), the mirror-ref panic poll, the daily full verification.
#[allow(dead_code)] // the daemon calls it (cli.rs `start`, WP21)
pub fn spawn(root: &Arc<Root>, halt: Halt) {
    let (tx, rx) = mpsc::channel();
    if let Some(l) = lock(&root.lin).as_mut() {
        l.kick = Some(tx);
    }
    let root = root.clone();
    std::thread::spawn(move || {
        let mut urgent = true;
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) => urgent = true,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_secs(1)),
            }
            tick(&root, &mut urgent, halt);
        }
    });
}

/// One round of the lineage thread.
pub(super) fn tick(root: &Arc<Root>, urgent: &mut bool, halt: Halt) {
    let now = now_ms();
    daily(root, now, halt);
    let cfg = lock(&root.st).charter.as_ref().and_then(|l| Cfg::of(&root.p, &l.ch));
    let Some(c) = cfg else {
        lock(&ST).state = "off";
        return;
    };
    poll_panic(root, &c, now);
    let head = lock(&root.lin).as_ref().and_then(|l| l.head.as_ref().map(|h| h.seq));
    {
        let mut s = lock(&ST);
        if s.state == "off" {
            s.state = "idle";
        }
        if head.is_none() || head <= s.seq {
            *urgent = false;
        }
        if s.state == "diverged" || head.is_none() || head <= s.seq || !(*urgent || now >= s.next_ms) {
            return;
        }
    }
    *urgent = false;
    let r = push(&root.p, &c);
    let mut s = lock(&ST);
    match r {
        Ok((seq, _)) => {
            (s.state, s.seq, s.at, s.error, s.failing_since, s.alerted, s.backoff_ms) = ("ok", Some(seq), Some(now), None, None, false, 0);
            s.next_ms = now + c.interval_ms;
        }
        Err(Fail::Diverged(e)) => {
            (s.state, s.error) = ("diverged", Some(e.chars().take(200).collect()));
            drop(s);
            util::log(&root.p.rh, "error", &format!("mirror diverged: {e}"));
            root.alert("mirror_diverged", "[root] the lineage mirror rejected a fast-forward push: its history was rewritten. \
                Pushes stopped (never forced); compare the mirror with the local chain, then restart the Root.");
        }
        Err(Fail::Other(e)) => {
            (s.state, s.error) = ("failing", Some(e.chars().take(200).collect()));
            let since = *s.failing_since.get_or_insert(now);
            s.backoff_ms = (s.backoff_ms * 2).clamp(60_000, 3_600_000);
            s.next_ms = now + s.backoff_ms;
            let alert = now - since >= DAY_MS && !s.alerted;
            s.alerted |= alert;
            drop(s);
            util::log(&root.p.rh, "warn", &format!("mirror push failed: {e}"));
            if alert {
                root.alert("mirror_failing", "[root] the lineage mirror has not accepted a push for 24 h (see logs/root.jsonl)");
            }
        }
    }
}

/// The mirror-ref panic channel: a new commit on the remote's `refs/heads/panic` panics the Root.
fn poll_panic(root: &Arc<Root>, c: &Cfg, now: i64) {
    let Some(every) = c.poll_ms else { return };
    {
        let mut s = lock(&ST);
        if now < s.poll_next {
            return;
        }
        s.poll_next = now + every;
    }
    let Ok(id) = panic_ref(&root.p, c) else { return };
    let fire = {
        let mut s = lock(&ST);
        let fire = id.is_some() && id != s.panic_seen;
        s.panic_seen = id;
        fire
    };
    if fire && !matches!(lock(&root.st).slot, Slot::Panicked { .. }) {
        crate::panic::panic(root, "mirror_ref");
    }
}

/// §3.10.4: the whole chain once a day. On a failure the lineage handle is dropped (nothing more is
/// appended), the alert `lineage_corrupt` goes out, and `halt` puts the Root in `Halted{lineage}`.
fn daily(root: &Arc<Root>, now: i64, halt: Halt) {
    let vk = {
        let l = lock(&root.lin);
        match l.as_ref() {
            Some(l) if l.verified_full_at().is_none_or(|t| now - t >= DAY_MS) => l.sk.verifying_key(),
            _ => return,
        }
    };
    let text = std::fs::read_to_string(root.p.r("lineage/lineage.jsonl")).unwrap_or_default();
    let end = text.rfind('\n').map_or(0, |i| i + 1); // an append in progress is not a torn tail
    match super::verify::chain(&text[..end], Some(&vk), 0, &HashSet::new()) {
        Ok(_) => {
            if let Some(l) = lock(&root.lin).as_mut()
                && let Err(e) = l.set_verified_full(now)
            {
                util::log(&root.p.rh, "warn", &format!("head.json: {e}"));
            }
        }
        Err((seq, rule)) => {
            *lock(&root.lin) = None;
            let why = format!("the daily verification failed at seq {seq}: {rule}");
            util::log(&root.p.rh, "error", &format!("lineage: {why}"));
            root.alert("lineage_corrupt", &format!("[root] lineage record {seq} fails rule {rule}: Halted{{lineage}}. \
                Compare lineage.jsonl with the mirror."));
            halt(root, &why);
            root.publish();
        }
    }
}
