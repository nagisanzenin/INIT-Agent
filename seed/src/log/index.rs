//! The in-memory index of a session file (log-context.md §3.7): offsets only, plus the dedup sets.
//! It grows O(events); event contents stay on disk.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};

use super::clock::parse_id;
use super::event::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdxEnt {
    pub id: u64,
    /// 0 = none (ids are never zero).
    pub parent: u64,
    pub off: u64,
    /// Line length in bytes, including the LF terminator.
    pub len: u32,
    pub kind: Kind,
}

impl IdxEnt {
    pub fn parent(&self) -> Option<u64> {
        if self.parent == 0 {
            None
        } else {
            Some(self.parent)
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct Index {
    ents: Vec<IdxEnt>,
    by_id: HashMap<u64, u32>, // never reaches a prompt
    /// Owner input frame ids already logged (`message`, `steer`, `command`).
    pub input_ids: HashSet<String>,
    /// Kernel notice ids already logged (`gate`, `update_failed`, `thinning_done`, `notice`).
    pub notice_ids: HashSet<String>,
    pub update_failed_seqs: HashSet<u64>,
    /// Latest label per target (file-wide; `label: null` clears).
    pub labels: BTreeMap<u64, String>,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ents.is_empty()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.by_id.contains_key(&id)
    }

    pub fn get(&self, id: u64) -> Option<&IdxEnt> {
        self.by_id.get(&id).map(|&i| &self.ents[i as usize])
    }

    pub fn pos(&self, id: u64) -> Option<usize> {
        self.by_id.get(&id).map(|&i| i as usize)
    }

    pub fn ents(&self) -> &[IdxEnt] {
        &self.ents
    }

    pub fn last(&self) -> Option<&IdxEnt> {
        self.ents.last()
    }

    /// Adds an entry. The caller has checked that `id` is new.
    pub fn push(&mut self, id: u64, parent: Option<u64>, off: u64, len: u32, kind: Kind) {
        debug_assert!(!self.by_id.contains_key(&id));
        self.by_id.insert(id, self.ents.len() as u32);
        self.ents.push(IdxEnt {
            id,
            parent: parent.unwrap_or(0),
            off,
            len,
            kind,
        });
    }

    /// Records the dedup keys and labels carried by an event's fields (full object or body fields).
    pub fn note(&mut self, kind: Kind, fields: &Map<String, Value>) {
        let s = |k: &str| fields.get(k).and_then(Value::as_str).map(str::to_string);
        match kind {
            Kind::Message | Kind::Steer | Kind::Command => {
                if let Some(i) = s("input_id") {
                    self.input_ids.insert(i);
                }
            }
            Kind::Gate | Kind::ThinningDone | Kind::Notice => {
                if let Some(n) = s("notice_id") {
                    self.notice_ids.insert(n);
                }
            }
            Kind::UpdateFailed => {
                if let Some(n) = s("notice_id") {
                    self.notice_ids.insert(n);
                }
                if let Some(q) = fields.get("seq").and_then(Value::as_u64) {
                    self.update_failed_seqs.insert(q);
                }
            }
            Kind::Label => {
                if let Some(t) = s("target_id").as_deref().and_then(parse_id) {
                    match fields.get("label").and_then(Value::as_str) {
                        Some(l) => {
                            self.labels.insert(t, l.to_string());
                        }
                        None => {
                            self.labels.remove(&t);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// `path(leaf)`: positions from the root to `leaf`. The walk stops at a null parent, or at a parent
    /// that is not in the index (a corrupt gap, §3.4.4), so the path then starts after the gap.
    pub fn path(&self, leaf: Option<u64>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = leaf.and_then(|l| self.pos(l));
        while let Some(p) = cur {
            out.push(p);
            if out.len() > self.ents.len() {
                break; // a cycle cannot be written by this code; never loop forever on a forged file
            }
            cur = self.ents[p].parent().and_then(|q| self.pos(q));
        }
        out.reverse();
        out
    }

    /// The latest label of an event.
    pub fn label(&self, id: u64) -> Option<&str> {
        self.labels.get(&id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn path_and_gaps() {
        let mut ix = Index::new();
        ix.push(1, None, 0, 10, Kind::Session);
        ix.push(2, Some(1), 10, 10, Kind::Message);
        ix.push(3, Some(2), 20, 10, Kind::Message);
        ix.push(4, Some(2), 30, 10, Kind::Branch); // a branch back to 2
        ix.push(5, Some(4), 40, 10, Kind::Message);
        ix.push(7, Some(6), 50, 10, Kind::Message); // parent 6 is missing (corrupt)
        let ids = |p: Vec<usize>| p.into_iter().map(|i| ix.ents()[i].id).collect::<Vec<_>>();
        assert_eq!(ids(ix.path(Some(5))), vec![1, 2, 4, 5]);
        assert_eq!(ids(ix.path(Some(3))), vec![1, 2, 3]);
        assert_eq!(ids(ix.path(Some(7))), vec![7]);
        assert!(ix.path(None).is_empty());
    }

    #[test]
    fn dedup_and_labels() {
        let mut ix = Index::new();
        let f = |v: serde_json::Value| v.as_object().unwrap().clone();
        ix.note(Kind::Message, &f(json!({"input_id":"00000000000000a1"})));
        ix.note(Kind::Notice, &f(json!({"notice_id":"n1"})));
        ix.note(Kind::UpdateFailed, &f(json!({"seq":7})));
        ix.note(
            Kind::Label,
            &f(json!({"target_id":"00000000000000a1","label":"x"})),
        );
        assert!(ix.input_ids.contains("00000000000000a1"));
        assert!(ix.notice_ids.contains("n1"));
        assert!(ix.update_failed_seqs.contains(&7));
        assert_eq!(ix.label(0xa1), Some("x"));
        ix.note(
            Kind::Label,
            &f(json!({"target_id":"00000000000000a1","label":null})),
        );
        assert_eq!(ix.label(0xa1), None);
    }
}
