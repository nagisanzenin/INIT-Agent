//! The durable inbox `state/kernel/inbox.jsonl` (supervise.md §3.10): owner inputs and kernel
//! notices not yet acked by a body.
//!
//! An owner line becomes an input (id = 16 hex, seq = next u64), is appended and fsynced, and
//! only then is the Root's `owner.in` answered `ok`. Inputs are deduplicated by the Root's `qid`;
//! the body drops inputs whose `input_id` is already in its log. At-least-once relay plus those
//! two dedups gives exactly-once delivery into the log. Notices use the same scheme with
//! `notice_ack`.

use crate::fsx;
use init_proto::icj::V1;
use init_proto::stdio::{InputMode, ToBody};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

const COMPACT_BYTES: u64 = 1024 * 1024;
const QID_KEEP: usize = 10_000;

/// One inbox line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Line {
    Input { v: V1, id: String, seq: u64, text: String, ts: String, mode: InputMode, qid: Option<String> },
    Notice { v: V1, id: String, kind: String, data: Value, ts: String },
    Ack { v: V1, seq: u64 },
    NoticeAck { v: V1, id: String },
}

/// A queued owner input.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub id: String,
    pub seq: u64,
    pub text: String,
    pub ts: String,
    pub mode: InputMode,
    /// The Root's `qid` (kept so a compaction preserves dedup of unacked lines).
    pub qid: Option<String>,
}

impl Input {
    pub fn frame(&self) -> ToBody {
        ToBody::Input { id: self.id.clone(), seq: self.seq, text: self.text.clone(), ts: self.ts.clone(), mode: self.mode }
    }
}

/// A queued kernel notice.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub kind: String,
    pub data: Value,
}

impl Notice {
    pub fn frame(&self) -> ToBody {
        ToBody::Notice { id: self.id.clone(), kind: self.kind.clone(), data: self.data.clone() }
    }
}

/// The inbox of one body (the primary's lives in `state/kernel/inbox.jsonl`).
pub struct Inbox {
    path: PathBuf,
    next_seq: u64,
    inputs: BTreeMap<u64, Input>,
    notices: Vec<Notice>,
    qids: BTreeSet<String>,
    qid_order: VecDeque<String>,
}

/// Result of offering an owner line.
#[derive(Clone, Debug, PartialEq)]
pub enum Offer {
    /// Newly queued.
    Queued(Input),
    /// The `qid` was seen before: nothing queued (answer `ok` again).
    Duplicate,
}

impl Inbox {
    /// Load the inbox at `path`.
    pub fn open(path: &Path) -> Inbox {
        let mut ib = Inbox { path: path.to_path_buf(), next_seq: 1, inputs: BTreeMap::new(), notices: Vec::new(), qids: BTreeSet::new(), qid_order: VecDeque::new() };
        for l in fsx::read_lines(path).unwrap_or_default() {
            match serde_json::from_slice::<Line>(&l) {
                Ok(Line::Input { id, seq, text, ts, mode, qid, .. }) => {
                    ib.next_seq = ib.next_seq.max(seq + 1);
                    if let Some(q) = &qid {
                        ib.remember(q.clone());
                    }
                    ib.inputs.insert(seq, Input { id, seq, text, ts, mode, qid });
                }
                Ok(Line::Notice { id, kind, data, .. }) => ib.notices.push(Notice { id, kind, data }),
                Ok(Line::Ack { seq, .. }) => {
                    ib.next_seq = ib.next_seq.max(seq + 1);
                    ib.inputs.remove(&seq);
                }
                Ok(Line::NoticeAck { id, .. }) => ib.notices.retain(|n| n.id != id),
                Err(_) => {}
            }
        }
        ib
    }

    fn remember(&mut self, q: String) {
        if self.qids.insert(q.clone()) {
            self.qid_order.push_back(q);
            if self.qid_order.len() > QID_KEEP {
                if let Some(old) = self.qid_order.pop_front() {
                    self.qids.remove(&old);
                }
            }
        }
    }

    fn append(&self, l: &Line) -> std::io::Result<()> {
        fsx::append_line(&self.path, &init_proto::icj::to_line(l).expect("inbox lines are ICJ"))
    }

    /// Queue an owner line durably (dedup by `qid`).
    pub fn offer(&mut self, qid: Option<&str>, text: &str, mode: InputMode) -> std::io::Result<Offer> {
        if let Some(q) = qid {
            if self.qids.contains(q) {
                return Ok(Offer::Duplicate);
            }
        }
        let input = Input { id: crate::sys::random_hex(8), seq: self.next_seq, text: text.to_string(), ts: crate::clock::now_rfc3339(), mode, qid: qid.map(str::to_string) };
        self.append(&Line::Input { v: V1, id: input.id.clone(), seq: input.seq, text: input.text.clone(), ts: input.ts.clone(), mode, qid: input.qid.clone() })?;
        self.next_seq += 1;
        if let Some(q) = qid {
            self.remember(q.to_string());
        }
        self.inputs.insert(input.seq, input.clone());
        Ok(Offer::Queued(input))
    }

    /// Queue a kernel notice durably.
    pub fn notice(&mut self, kind: &str, data: Value) -> std::io::Result<Notice> {
        let n = Notice { id: crate::sys::random_hex(8), kind: kind.to_string(), data };
        self.append(&Line::Notice { v: V1, id: n.id.clone(), kind: n.kind.clone(), data: n.data.clone(), ts: crate::clock::now_rfc3339() })?;
        self.notices.push(n.clone());
        Ok(n)
    }

    /// The body acked input `seq` (it is durably in the log).
    pub fn ack(&mut self, seq: u64) {
        if self.inputs.remove(&seq).is_some() {
            let _ = self.append(&Line::Ack { v: V1, seq });
            self.maybe_compact();
        }
    }

    /// The body acked notice `id`.
    pub fn notice_ack(&mut self, id: &str) {
        let before = self.notices.len();
        self.notices.retain(|n| n.id != id);
        if self.notices.len() != before {
            let _ = self.append(&Line::NoticeAck { v: V1, id: id.to_string() });
            self.maybe_compact();
        }
    }

    /// Every unacked input, in seq order (redelivered when a body becomes ready).
    pub fn pending_inputs(&self) -> Vec<Input> {
        self.inputs.values().cloned().collect()
    }

    /// Every unacked notice, in order.
    pub fn pending_notices(&self) -> Vec<Notice> {
        self.notices.clone()
    }

    /// Counts for `boot.json.pending`.
    pub fn counts(&self) -> (u64, u64) {
        (self.inputs.len() as u64, self.notices.len() as u64)
    }

    fn maybe_compact(&mut self) {
        if std::fs::metadata(&self.path).map(|m| m.len() <= COMPACT_BYTES).unwrap_or(true) {
            return;
        }
        let mut out = Vec::new();
        for i in self.inputs.values() {
            let l = Line::Input { v: V1, id: i.id.clone(), seq: i.seq, text: i.text.clone(), ts: i.ts.clone(), mode: i.mode, qid: i.qid.clone() };
            out.extend(init_proto::icj::to_line(&l).unwrap());
        }
        for n in &self.notices {
            let l = Line::Notice { v: V1, id: n.id.clone(), kind: n.kind.clone(), data: n.data.clone(), ts: crate::clock::now_rfc3339() };
            out.extend(init_proto::icj::to_line(&l).unwrap());
        }
        // Acked lines' qids are dropped: the Root removed them from its queue when it got `ok`.
        // An ack marker keeps next_seq monotonic.
        let marker = Line::Ack { v: V1, seq: self.next_seq.saturating_sub(1) };
        out.extend(init_proto::icj::to_line(&marker).unwrap());
        let _ = fsx::write_atomic(&self.path, &out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn durable_dedup_ack_and_redelivery_order() {
        let d = crate::testutil::test_dir("inbox");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("inbox.jsonl");
        let mut ib = Inbox::open(&p);
        let Offer::Queued(a) = ib.offer(Some("q1"), "hello", InputMode::Steer).unwrap() else { panic!() };
        assert_eq!(ib.offer(Some("q1"), "hello", InputMode::Steer).unwrap(), Offer::Duplicate);
        let Offer::Queued(b) = ib.offer(Some("q2"), "x", InputMode::FollowUp).unwrap() else { panic!() };
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(a.id.len(), 16);
        let n = ib.notice("lkg", json!({"sha": "x"})).unwrap();
        ib.ack(1);
        let mut ib2 = Inbox::open(&p);
        assert_eq!(ib2.pending_inputs(), vec![b.clone()]);
        assert_eq!(ib2.pending_notices().len(), 1);
        assert_eq!(ib2.offer(Some("q2"), "x", InputMode::Steer).unwrap(), Offer::Duplicate, "qid survives a restart");
        ib2.notice_ack(&n.id);
        ib2.ack(2);
        let Offer::Queued(c) = ib2.offer(None, "y", InputMode::Steer).unwrap() else { panic!() };
        assert_eq!(c.seq, 3);
        assert_eq!(Inbox::open(&p).counts(), (1, 0));
        crate::testutil::remove_test_dir(&d);
    }
}
