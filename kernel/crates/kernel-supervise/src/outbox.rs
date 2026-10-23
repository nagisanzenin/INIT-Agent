//! Durable lineage records (supervise.md §3.13.3, §4.7; D7).
//!
//! Each record is first appended to the kernel-local copy `vault/lineage.jsonl`, then to
//! `state/kernel/root-outbox.jsonl` (`O_APPEND`, `sync_data`), then sent as
//! `lineage.append{kind, data, idem}`. `idem` = `<kboot>:<outbox seq>` (root/root.md §4.1.3
//! names the field `idem`; supervise.md calls it `dedup_key`), where `kboot` is 16 random hex per
//! kernel start, stored with the entry so a resend reuses it. An entry is answered when the Root
//! returns `{seq, hash, dup}`; unanswered entries are resent at cold start with their original
//! key, so a resend never duplicates a record.

use crate::fsx;
use crate::link::{LinkError, RootLink};
use init_proto::files::LineageLocalRecord;
use init_proto::icj::V1;
use init_proto::root_link::{KernelRequest, LineageAppendArgs, LineageAppendResult, RootErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Compact the outbox file once it passes 1 MiB (§6 rank 5).
const COMPACT_BYTES: u64 = 1024 * 1024;
/// F26: more unanswered entries than this, or older than 60 s, refuses new jobs.
pub const BACKLOG_MAX: usize = 1000;
pub const BACKLOG_AGE_MS: i64 = 60_000;

/// One outbox entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub v: V1,
    pub seq: u64,
    pub idem: String,
    pub kind: String,
    pub data: Value,
    pub at: String,
}

/// A line marking an entry answered (or permanently refused).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Answered {
    v: V1,
    answered: u64,
    root_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused: Option<String>,
}

#[derive(Default)]
struct State {
    next_seq: u64,
    unanswered: BTreeMap<u64, Entry>,
    last_root_seq: Option<u64>,
}

/// The outbox.
pub struct Outbox {
    local: PathBuf,
    file: PathBuf,
    kboot: String,
    st: Mutex<State>,
    link: Mutex<Option<Arc<RootLink>>>,
    tx: Mutex<Option<Sender<u64>>>,
}

impl Outbox {
    /// The attached control link, if any (G4 v2's slice client calls `store.put`, `tuple.get` and
    /// `sealed.g4_slice` on it; promote/g4v2.rs).
    pub fn root_link(&self) -> Option<Arc<RootLink>> {
        self.link.lock().unwrap().clone()
    }

    /// Open the outbox of `home` (creating nothing until the first record).
    pub fn open(home: &Path, kboot: &str) -> Arc<Outbox> {
        let local = home.join(init_proto::files::paths::LINEAGE_LOCAL);
        let file = home.join(init_proto::files::paths::ROOT_OUTBOX);
        let mut st = State::default();
        let mut max_seq = 0u64;
        for l in fsx::read_lines(&file).unwrap_or_default() {
            if let Ok(e) = serde_json::from_slice::<Entry>(&l) {
                max_seq = max_seq.max(e.seq);
                st.unanswered.insert(e.seq, e);
            } else if let Ok(a) = serde_json::from_slice::<Answered>(&l) {
                st.unanswered.remove(&a.answered);
                if a.root_seq.is_some() {
                    st.last_root_seq = a.root_seq;
                }
            }
        }
        // The local copy's last line carries the highest seq ever used.
        if let Ok(tail) = fsx::read_nofollow(&local, 64 * 1024, true) {
            for l in tail.split(|b| *b == b'\n').rev() {
                if let Ok(r) = serde_json::from_slice::<LineageLocalRecord>(l) {
                    max_seq = max_seq.max(r.seq);
                    break;
                }
            }
        }
        st.next_seq = max_seq + 1;
        Arc::new(Outbox { local, file, kboot: kboot.to_string(), st: Mutex::new(st), link: Mutex::new(None), tx: Mutex::new(None) })
    }

    /// Attach the control link and start the background sender.
    pub fn attach(self: &Arc<Self>, link: Arc<RootLink>) {
        *self.link.lock().unwrap() = Some(link);
        let (tx, rx) = mpsc::channel::<u64>();
        *self.tx.lock().unwrap() = Some(tx);
        let me = self.clone();
        std::thread::Builder::new()
            .name("outbox".into())
            .spawn(move || {
                while let Ok(seq) = rx.recv() {
                    let _ = me.send_now(seq);
                }
            })
            .expect("spawn outbox thread");
    }

    /// Append a record durably (local copy, then outbox). It is not sent yet.
    pub fn record(&self, kind: &str, data: Value) -> std::io::Result<Entry> {
        let mut st = self.st.lock().unwrap();
        let seq = st.next_seq;
        let at = crate::clock::now_rfc3339();
        let idem = format!("{}:{seq}", self.kboot);
        let local = LineageLocalRecord { v: V1, at: at.clone(), data: data.clone(), dedup_key: idem.clone(), kind: kind.to_string(), seq };
        if let Some(dir) = self.local.parent() {
            std::fs::create_dir_all(dir)?;
        }
        fsx::append_line(&self.local, &line(&local))?;
        let e = Entry { v: V1, seq, idem, kind: kind.to_string(), data, at };
        fsx::append_line(&self.file, &line(&e))?;
        st.next_seq += 1;
        st.unanswered.insert(seq, e.clone());
        Ok(e)
    }

    /// A kernel-local line in `vault/lineage.jsonl` only (e.g. `handoff_failed`: the Root records
    /// no failed swaps, which are non-events).
    pub fn local_only(&self, kind: &str, data: Value) -> std::io::Result<()> {
        let mut st = self.st.lock().unwrap();
        let seq = st.next_seq;
        let local = LineageLocalRecord { v: V1, at: crate::clock::now_rfc3339(), data, dedup_key: format!("{}:{seq}", self.kboot), kind: kind.to_string(), seq };
        if let Some(dir) = self.local.parent() {
            std::fs::create_dir_all(dir)?;
        }
        fsx::append_line(&self.local, &line(&local))?;
        st.next_seq += 1;
        Ok(())
    }

    /// Record and queue for sending; the caller does not wait (the local step is already
    /// irreversible, K-F6).
    pub fn post(&self, kind: &str, data: Value) -> std::io::Result<Entry> {
        let e = self.record(kind, data)?;
        crate::fault::point("supervise.root.after_outbox_before_response");
        self.queue(e.seq);
        Ok(e)
    }

    /// Record, send and wait for the Root's answer (`store.put`-gated steps: LKG commit,
    /// `task_admit`).
    pub fn post_wait(&self, kind: &str, data: Value) -> Result<LineageAppendResult, LinkError> {
        let e = self.record(kind, data).map_err(|e| LinkError::Other(e.to_string()))?;
        crate::fault::point("supervise.root.after_outbox_before_response");
        self.send_now(e.seq)
    }

    fn queue(&self, seq: u64) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(seq);
        }
    }

    /// Send one entry now and mark it answered on success.
    fn send_now(&self, seq: u64) -> Result<LineageAppendResult, LinkError> {
        let entry = self.st.lock().unwrap().unanswered.get(&seq).cloned();
        let Some(e) = entry else { return Err(LinkError::Other(format!("outbox entry {seq} already answered"))) };
        let link = self.link.lock().unwrap().clone().ok_or(LinkError::Lost)?;
        let req = KernelRequest::LineageAppend(LineageAppendArgs { kind: e.kind.clone(), data: e.data.clone(), idem: e.idem.clone() });
        match link.call(&req, link.request_timeout.max(Duration::from_secs(2))) {
            Ok(v) => {
                let r: LineageAppendResult = serde_json::from_value(v).map_err(|x| LinkError::Other(format!("lineage.append result: {x}")))?;
                self.mark(seq, Some(r.seq), None);
                Ok(r)
            }
            Err(LinkError::Root(b)) if RootErrorCode::of(&b) == RootErrorCode::Lineage => {
                // A permanent refusal (kind, fields, privacy): never resend it.
                crate::klog::log("outbox", &format!("lineage.append {} refused: {}", e.kind, b.message));
                self.mark(seq, None, Some(b.message.clone()));
                Err(LinkError::Root(b))
            }
            Err(x) => Err(x),
        }
    }

    fn mark(&self, seq: u64, root_seq: Option<u64>, refused: Option<String>) {
        let mut st = self.st.lock().unwrap();
        if st.unanswered.remove(&seq).is_none() {
            return;
        }
        if root_seq.is_some() {
            st.last_root_seq = root_seq;
        }
        let a = Answered { v: V1, answered: seq, root_seq, refused };
        let _ = fsx::append_line(&self.file, &line(&a));
        if std::fs::metadata(&self.file).map(|m| m.len() > COMPACT_BYTES).unwrap_or(false) {
            let mut bytes = Vec::new();
            for e in st.unanswered.values() {
                bytes.extend(line(e));
            }
            let _ = fsx::write_atomic(&self.file, &bytes);
        }
    }

    /// Queue every unanswered entry again (cold start, and the watcher's periodic retry).
    pub fn resend_unanswered(&self) {
        let seqs: Vec<u64> = self.st.lock().unwrap().unanswered.keys().copied().collect();
        for s in seqs {
            self.queue(s);
        }
    }

    /// Unanswered entries.
    pub fn unanswered(&self) -> usize {
        self.st.lock().unwrap().unanswered.len()
    }

    /// F26: too many or too old unanswered entries.
    pub fn backlogged(&self) -> bool {
        let st = self.st.lock().unwrap();
        if st.unanswered.len() > BACKLOG_MAX {
            return true;
        }
        let now = crate::clock::now_ms();
        st.unanswered.values().next().and_then(|e| crate::clock::parse_rfc3339(&e.at)).map(|t| now - t > BACKLOG_AGE_MS).unwrap_or(false)
    }

    /// The Root's `seq` of the newest answered record.
    pub fn last_root_seq(&self) -> Option<u64> {
        self.st.lock().unwrap().last_root_seq
    }

    /// This kernel start's `kboot`.
    pub fn kboot(&self) -> &str {
        &self.kboot
    }
}

fn line<T: Serialize>(x: &T) -> Vec<u8> {
    init_proto::icj::to_line(x).expect("outbox lines are ICJ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_survive_a_restart_with_their_keys() {
        let d = crate::testutil::test_dir("outbox");
        std::fs::create_dir_all(d.join("vault")).unwrap();
        std::fs::create_dir_all(d.join("state/kernel")).unwrap();
        let ob = Outbox::open(&d, "aaaaaaaaaaaaaaaa");
        let a = ob.record("handoff", json!({"from": "x", "to": "y"})).unwrap();
        let b = ob.record("body_safe", json!({"on": true})).unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(a.idem, "aaaaaaaaaaaaaaaa:1");
        ob.mark(1, Some(100), None);
        // Restart: entry 2 is still unanswered with its original key; seq continues.
        let ob2 = Outbox::open(&d, "bbbbbbbbbbbbbbbb");
        assert_eq!(ob2.unanswered(), 1);
        assert_eq!(ob2.st.lock().unwrap().unanswered[&2].idem, "aaaaaaaaaaaaaaaa:2");
        assert_eq!(ob2.last_root_seq(), Some(100));
        let c = ob2.record("halted", json!({})).unwrap();
        assert_eq!((c.seq, c.idem.as_str()), (3, "bbbbbbbbbbbbbbbb:3"));
        let local = fsx::read_lines(&d.join("vault/lineage.jsonl")).unwrap();
        assert_eq!(local.len(), 3);
        let r: LineageLocalRecord = serde_json::from_slice(&local[2]).unwrap();
        assert_eq!((r.kind.as_str(), r.dedup_key.as_str()), ("halted", "bbbbbbbbbbbbbbbb:3"));
        crate::testutil::remove_test_dir(&d);
    }
}
