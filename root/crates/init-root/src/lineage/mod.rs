//! R4: hash-chained, Ed25519-signed lineage (root/root.md §3.10, §3.11, §3.8.1).
//! Here: the key, the record format and sealing, appending (`O_APPEND` + `F_FULLFSYNC`, the view copy,
//! `head.json`), torn-tail repair and the tail check at open, the `idem` index (D7), kernel-reported
//! records (checks, privacy, rate) and the Root-level wrapper for the control link.
//! `verify`: rules 1–8 and `audit verify`. `kinds`: the kind registry and the privacy rules.
//! `mirror`: the off-machine mirror and the mirror-ref panic channel. `kgit`: `lineage/kernel.git`.

#[cfg(feature = "fault-injection")]
mod fault;
pub mod kgit;
mod kinds;
pub mod mirror;
pub mod owner_view; // TUI-OBSERVE: `lineage.tail` on owner.sock (read-only, bounded)
#[cfg(test)]
mod tests;
pub mod verify;

use crate::link::{Refusal, Root, lock};
use crate::util::{self, bad, hex, icj, rfc3339, unhex};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
pub(crate) use kinds::private;
use verify::check_head;

pub const AREA: &str = "lineage";
pub const DOMAIN: &[u8] = b"INIT-ROOT-LINEAGE-V1\n";
const DATA_MAX: usize = 16 * 1024;
const HOUR_MS: i64 = 3_600_000;
const IDEM_KEEP_MS: i64 = 7 * 24 * HOUR_MS;
/// `root.lineage_verify_tail`: signatures checked at every open; the daily and `--full` checks do all.
pub const TAIL: u64 = 1000;
/// Kinds after which the mirror pushes at once (§3.11 schedule).
const URGENT: [&str; 8] = ["panic", "ladder", "rollback", "kernel_upgrade", "audit", "charter", "halt", "crash_loop"];

fn zero_hash() -> String {
    format!("sha256:{}", "0".repeat(64))
}

/// The newest record: what the next `prev`, `seq` and `at` build on.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
    pub at: i64,
    pub prev: String,
}

/// Generates `keys/lineage.ed25519` (the 32-byte seed, R-F4) and `keys/lineage.pub` unless present.
pub fn ensure_key(root_home: &Path) -> io::Result<VerifyingKey> {
    let kp = root_home.join("keys/lineage.ed25519");
    if fs::symlink_metadata(&kp).is_err() {
        let mut seed = util::random::<32>()?;
        util::write_atomic(&kp, &seed, 0o600)?;
        let pk = SigningKey::from_bytes(&seed).verifying_key();
        crate::sys::zero(&mut seed);
        util::write_atomic(&root_home.join("keys/lineage.pub"), format!("{}\n", hex(&pk.to_bytes())).as_bytes(), 0o644)?;
    }
    Ok(load_key(root_home)?.verifying_key())
}

fn load_key(root_home: &Path) -> io::Result<SigningKey> {
    let mut b = util::read_secret(&root_home.join("keys/lineage.ed25519"))?;
    let mut seed: [u8; 32] = b.as_slice().try_into().map_err(|_| bad("the lineage key must be 32 bytes"))?;
    crate::sys::zero(&mut b);
    let sk = SigningKey::from_bytes(&seed);
    crate::sys::zero(&mut seed);
    Ok(sk)
}

/// A published key file (`keys/lineage.pub`, `INIT_HOME/root/lineage.pub`): 64 hex and a newline.
pub fn read_pub(path: &Path) -> Option<VerifyingKey> {
    let s = fs::read_to_string(path).ok()?;
    VerifyingKey::from_bytes(&unhex::<32>(s.trim())?).ok()
}

fn hash_of(b: &[u8]) -> [u8; 32] {
    let mut h = [0u8; 32];
    h.copy_from_slice(&Sha256::digest(b));
    h
}

/// Keeps the timestamps of the last hour; false (and nothing kept) when `max` are already there.
fn window(q: &mut VecDeque<i64>, now: i64, max: usize) -> bool {
    while q.front().is_some_and(|t| *t <= now - HOUR_MS) {
        q.pop_front();
    }
    if q.len() >= max {
        return false;
    }
    q.push_back(now);
    true
}

/// What a kernel record is checked against: the reporting build, the strings no record may hold
/// (the owner's home, ROOT_HOME, INIT_HOME), and the owner lines the Root relayed, by `qid` (§3.15).
pub struct KernelCtx<'a> {
    pub build: &'a str,
    pub deny: &'a [String],
    pub relayed: &'a dyn Fn(&str) -> Option<String>,
}

pub struct Lineage {
    file: PathBuf,
    view: Option<PathBuf>,
    sk: SigningKey,
    pub head: Option<Head>,
    len: u64,
    full_at: Value,
    idem: HashMap<String, (u64, String)>,
    kernel_q: VecDeque<i64>,
    hits_q: VecDeque<i64>,
    /// `root.kernel_lineage_per_hour`, also the cap on demoting `ceiling_hit` records per hour.
    pub per_hour: usize,
    pub rate_alert_at: i64,
    /// The mirror thread, woken after the kinds of `URGENT`.
    pub kick: Option<Sender<()>>,
    #[cfg(feature = "fault-injection")]
    pub fault: String,
    #[cfg(feature = "fault-injection")]
    fault_hits: HashMap<String, u32>,
}

impl Lineage {
    /// §3.6.5 step 3: loads the key, repairs a torn tail, verifies the chain (signatures over the
    /// last `TAIL` records), checks `head.json` (rule 7), loads the `idem` index and rebuilds the
    /// view. `view` is `INIT_HOME/root/lineage.jsonl`, the published copy, when INIT_HOME exists.
    /// The caller holds `run/root.lock` (or no Root can be running yet).
    pub fn open(root_home: &Path, view: Option<PathBuf>) -> io::Result<Lineage> {
        let sk = load_key(root_home)?;
        let dir = root_home.join("lineage");
        let file = dir.join("lineage.jsonl");
        let mut bytes = fs::read(&file).or_else(|e| if e.kind() == io::ErrorKind::NotFound { Ok(Vec::new()) } else { Err(e) })?;
        let keep = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        if keep < bytes.len() {
            let torn = dir.join(format!("torn-{}.bin", util::now_ms()));
            util::write_atomic(&torn, &bytes[keep..], 0o600)?;
            let f = OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(&file)?;
            f.set_len(keep as u64)?;
            f.sync_all()?;
            bytes.truncate(keep);
            util::log(root_home, "warn", &format!("lineage: torn tail ({} bytes) copied to {} and truncated", keep, torn.display()));
        }
        let text = String::from_utf8(bytes).map_err(|_| bad("lineage: not UTF-8"))?;
        let idem_text = fs::read_to_string(dir.join("idem.jsonl")).unwrap_or_default();
        let entries: Vec<Value> = idem_text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let want: HashSet<u64> = entries.iter().filter_map(|e| e["seq"].as_u64()).collect();
        let n = text.lines().count() as u64;
        let c = verify::chain(&text, Some(&sk.verifying_key()), n.saturating_sub(TAIL), &want)
            .map_err(|(s, r)| bad(format!("lineage: seq {s}: {r}")))?;
        let hj: Option<Value> = fs::read_to_string(dir.join("head.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
        let stale = check_head(hj.as_ref(), c.head.as_ref()).map_err(|r| bad(format!("lineage: {r}")))?;
        let mut l = Lineage { file, view, sk, head: c.head, len: text.len() as u64, idem: HashMap::new(),
            full_at: hj.map_or(Value::Null, |h| h["verified_full_at"].clone()), kernel_q: VecDeque::new(),
            hits_q: VecDeque::new(), per_hour: 600, rate_alert_at: i64::MIN, kick: None,
            #[cfg(feature = "fault-injection")]
            fault: std::env::var("INIT_FAULT").unwrap_or_default(),
            #[cfg(feature = "fault-injection")]
            fault_hits: HashMap::new() };
        if stale {
            l.write_head()?;
        }
        // idem: the last 7 days, and only keys whose record is in the chain with that hash (write-ahead)
        let since = util::now_ms() - IDEM_KEEP_MS;
        for e in &entries {
            let at = e["at"].as_str().and_then(util::parse_rfc3339).unwrap_or(i64::MIN);
            if let (Some(k), Some(s), Some(h)) = (e["idem"].as_str(), e["seq"].as_u64(), e["hash"].as_str())
                && at >= since && c.hashes.get(&s).map(String::as_str) == Some(h)
            {
                l.idem.insert(k.to_string(), (s, h.to_string()));
            }
        }
        if l.idem.len() != entries.len() || entries.len() != idem_text.lines().count() {
            let live = |e: &&Value| e["idem"].as_str().and_then(|k| l.idem.get(k)) == Some(&(e["seq"].as_u64().unwrap_or(u64::MAX), e["hash"].as_str().unwrap_or("").to_string()));
            let keep: String = entries.iter().filter(live)
                .map(|e| icj(e).map(|s| s + "\n")).collect::<io::Result<_>>()?;
            util::write_atomic(&dir.join("idem.jsonl"), keep.as_bytes(), 0o600)?;
        }
        l.rebuild_view(Some(&text))?;
        Ok(l)
    }

    /// The published copy (§3.10.6), byte-identical; rewritten when it differs.
    fn rebuild_view(&self, text: Option<&str>) -> io::Result<()> {
        let Some(v) = self.view.as_ref().filter(|v| v.parent().is_some_and(Path::is_dir)) else { return Ok(()) };
        let owned;
        let text = match text {
            Some(t) => t,
            None => {
                owned = fs::read_to_string(&self.file)?;
                &owned
            }
        };
        if fs::read(v).ok().as_deref() != Some(text.as_bytes()) {
            util::write_atomic(v, text.as_bytes(), 0o644)?;
        }
        Ok(())
    }

    fn write_head(&self) -> io::Result<()> {
        let h = self.head.as_ref();
        let head = json!({"hash": h.map(|h| &h.hash), "seq": h.map(|h| h.seq), "v": 1, "verified_full_at": self.full_at});
        util::write_atomic(&self.file.with_file_name("head.json"), format!("{}\n", icj(&head)?).as_bytes(), 0o600)
    }

    /// After a full verification (the daily check of the mirror thread): `head.json.verified_full_at`.
    pub fn set_verified_full(&mut self, at: i64) -> io::Result<()> {
        self.full_at = json!(rfc3339(at));
        self.write_head()
    }

    pub fn verified_full_at(&self) -> Option<i64> {
        self.full_at.as_str().and_then(util::parse_rfc3339)
    }

    /// A Root-written record (§3.10.5). Kinds outside the Root's list are refused, and demoting
    /// `ceiling_hit` records beyond `per_hour` an hour are dropped (`root.rate`; the egress usage
    /// ledger still has every refusal), so a hostile kernel cannot flood the chain through the egress.
    pub fn append(&mut self, by: &str, kind: &str, data: Value) -> io::Result<Value> {
        if by == "root" && !kinds::ROOT.contains(&kind) {
            return Err(bad(format!("{kind:?} is not a Root-written lineage kind (root/root.md §3.10.5)")));
        }
        let now = util::now_ms();
        if kind == "ceiling_hit" && data["demotes"] == true && !window(&mut self.hits_q, now, self.per_hour) {
            return Err(bad(format!("root.rate: more than {} demoting ceiling_hit records in an hour; not recorded", self.per_hour)));
        }
        self.append_at(now, by, kind, data)
    }

    /// Any kind at a given Root time (`at` never decreases); test ledgers use it directly.
    pub fn append_at(&mut self, now: i64, by: &str, kind: &str, data: Value) -> io::Result<Value> {
        let (rec, line, head) = self.seal(now, by, kind, data)?;
        self.write(&line, head, kind)?;
        Ok(rec)
    }

    /// §3.10.1: the record with its `hash` and `sig`, its line, and the head it makes.
    fn seal(&self, now: i64, by: &str, kind: &str, data: Value) -> io::Result<(Value, String, Head)> {
        if !data.is_object() || icj(&data)?.len() > DATA_MAX {
            return Err(bad("lineage data must be an ICJ object of at most 16 KiB"));
        }
        let (seq, prev, at) = match &self.head {
            None => (0, zero_hash(), now),
            Some(h) => (h.seq + 1, h.hash.clone(), now.max(h.at)),
        };
        let mut rec = json!({"at": rfc3339(at), "by": by, "data": data, "kind": kind, "prev": prev, "seq": seq, "v": 1});
        let h = hash_of(icj(&rec)?.as_bytes());
        let sig = self.sk.sign(&[DOMAIN, &h[..]].concat());
        let hash = format!("sha256:{}", hex(&h));
        rec["hash"] = json!(hash);
        rec["sig"] = json!(format!("ed25519:{}", hex(&sig.to_bytes())));
        let line = format!("{}\n", icj(&rec)?);
        Ok((rec, line, Head { seq, hash, at, prev }))
    }

    /// §3.10.3: one `write_all` with `O_APPEND`, `sync_data` (`F_FULLFSYNC` on Apple), the view, `head.json`.
    fn write(&mut self, line: &str, head: Head, kind: &str) -> io::Result<()> {
        let open = |p: &Path| OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(p);
        let mut f = open(&self.file)?;
        self.fault("root.lineage.mid_append", || {
            f.write_all(&line.as_bytes()[..line.len() / 2])?;
            f.sync_data()
        })?;
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        self.fault("root.lineage.after_append", || Ok(()))?;
        let old = self.len;
        self.len += line.len() as u64;
        self.head = Some(head);
        if let Some(v) = self.view.as_ref().filter(|v| v.parent().is_some_and(Path::is_dir)) {
            let r = match fs::symlink_metadata(v) {
                Ok(m) if m.len() == old => open(v).and_then(|mut w| w.write_all(line.as_bytes())),
                _ => self.rebuild_view(None),
            };
            if let Err(e) = r {
                util::log(self.file.parent().and_then(Path::parent).unwrap_or(Path::new("/")), "warn", &format!("lineage view: {e}"));
            }
        }
        self.write_head()?;
        if let Some(k) = self.kick.as_ref().filter(|_| URGENT.contains(&kind)) {
            let _ = k.send(());
        }
        Ok(())
    }

    /// `lineage.append` from the kernel (§3.10.3, §3.10.5, D7): `{dup, hash, seq}` and the new record.
    /// The `idem` key is indexed before the record is written, so a resend after a crash anywhere
    /// never duplicates: a key whose record never landed is dropped at the next open.
    pub fn append_kernel(&mut self, k: &KernelCtx, kind: &str, data: Value, idem: &str) -> Result<(Value, Option<Value>), Refusal> {
        if !(1..=64).contains(&idem.len()) || !idem.bytes().all(|c| c.is_ascii_graphic()) {
            return Err(("root.args", "idem must be 1–64 printable ASCII bytes".into()));
        }
        if let Some((seq, hash)) = self.idem.get(idem) {
            return Ok((json!({"dup": true, "hash": hash, "seq": seq}), None));
        }
        if k.build.len() != 12 || !k.build.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(("root.internal", "the kernel build id must be 12 lowercase hex".into()));
        }
        kinds::check_kernel(kind, &data, k).map_err(|m| ("root.lineage", m))?;
        let now = util::now_ms();
        if !window(&mut self.kernel_q, now, self.per_hour) {
            return Err(("root.rate", format!("more than {} kernel lineage records in an hour", self.per_hour)));
        }
        let ie = |e: io::Error| ("root.internal", e.to_string());
        let (rec, line, head) = self.seal(now, &format!("kernel:{}", k.build), kind, data).map_err(|e| ("root.lineage", e.to_string()))?;
        let (seq, hash) = (head.seq, head.hash.clone());
        let entry = json!({"at": rec["at"], "hash": hash, "idem": idem, "seq": seq, "v": 1});
        util::append_line(&self.file.with_file_name("idem.jsonl"), &entry).map_err(ie)?;
        self.fault("root.lineage.after_idem", || Ok(())).map_err(ie)?;
        self.write(&line, head, kind).map_err(ie)?;
        self.idem.insert(idem.to_string(), (seq, hash.clone()));
        Ok((json!({"dup": false, "hash": hash, "seq": seq}), Some(rec)))
    }

    /// `INIT_FAULT` points (root/root.md §4.7), test builds only: `before` runs, then the action.
    fn fault(&mut self, point: &str, before: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        #[cfg(feature = "fault-injection")]
        if let Some(a) = fault::fires(&self.fault, point, &mut self.fault_hits) {
            before()?;
            return fault::act(&a);
        }
        let _ = (point, before);
        Ok(())
    }
}

/// A fault point outside a `Lineage` (mirror, `kernel.git`): `INIT_FAULT` from the environment.
pub(crate) fn env_fault(point: &str) -> io::Result<()> {
    #[cfg(feature = "fault-injection")]
    {
        let spec = std::env::var("INIT_FAULT").unwrap_or_default();
        let mut g = fault::HITS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = fault::fires(&spec, point, g.get_or_insert_with(HashMap::new)) {
            drop(g);
            return fault::act(&a);
        }
    }
    let _ = point;
    Ok(())
}

/// Every record of a chain file (the caller has verified it).
pub fn records(root_home: &Path) -> io::Result<Vec<Value>> {
    let text = fs::read_to_string(root_home.join("lineage/lineage.jsonl")).or_else(|e| match e.kind() {
        io::ErrorKind::NotFound => Ok(String::new()),
        _ => Err(e),
    })?;
    text.lines().map(|l| serde_json::from_str(l).map_err(|e| bad(e.to_string()))).collect()
}

/// The control link's `lineage.append{kind, data, idem}` (§4.1.3), for the dispatcher (WP21): `build`
/// is the first 12 hex of the reporting kernel build's sha256; `relayed(qid)` returns the owner line
/// the Root relayed under that `qid`. Result: `{dup, hash, seq}`.
#[allow(dead_code)] // the control link's dispatcher calls it (link.rs, WP21)
pub fn kernel_append(root: &Root, build: &str, args: &Value, relayed: &dyn Fn(&str) -> Option<String>) -> Result<Value, Refusal> {
    let per_hour = lock(&root.st).charter.as_ref().map_or(600, |l| l.ch.root.kernel_lineage_per_hour.max(1) as usize);
    let deny = [&root.p.home, &root.p.rh, &root.p.ih].map(|p| p.display().to_string());
    let k = KernelCtx { build, deny: &deny, relayed };
    let (kind, idem) = (args["kind"].as_str().unwrap_or(""), args["idem"].as_str().unwrap_or(""));
    let mut lg = lock(&root.lin);
    let lin = lg.as_mut().ok_or(("root.mode", "the lineage is unavailable (Halted)".to_string()))?;
    lin.per_hour = per_hour;
    let r = lin.append_kernel(&k, kind, args["data"].clone(), idem);
    let now = util::now_ms();
    let alert = matches!(&r, Err(("root.rate", _))) && now - lin.rate_alert_at >= HOUR_MS;
    if alert {
        lin.rate_alert_at = now;
    }
    if let Ok((_, Some(rec))) = &r {
        lock(&root.st).level.apply(rec);
    }
    drop(lg);
    if alert {
        root.alert("lineage_rate", &format!("[root] the kernel exceeded {per_hour} lineage records an hour; further records are refused (root.rate)"));
    }
    r.map(|(v, _)| v)
}
