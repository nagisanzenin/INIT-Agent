//! ADR-011 §5 (lane X5): attribution. Capability ids, the caps one egress request used (the tap's tool calls,
//! evidence `root`, and `x-egress-caps`, evidence `zone`), the usage ledger (`ROOT_HOME/extend/usage/<date>.jsonl`),
//! the daily `cap_use` digest, the cap table and `INIT_HOME/root/caps.json`. Turns are the sentinel's (X3): it
//! attributes each request's caps to its turn and hands every closed turn here. The public API is in
//! build/status/X5.md.

mod caps; // the cap table fold, caps.json rows, the Root-evidence identity of a change (t0 tar, ext components)
mod usage; // the usage lines, per-day counters and the `cap_use` digest

use crate::link::{Root, lock};
use crate::sentinel::Turn;
use crate::util::{self, icj, rfc3339};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::sync::{Mutex, Once};

pub use caps::{CapRow, change_caps};

/// Kinds of ADR-011 §5.1.
const KINDS: [&str; 10] = ["agent", "dep", "ext", "kernel", "loop", "mem", "mod", "prompt", "skill", "tool"];
/// `x-egress-caps` bounds (§5.2): at most 16 ids in 1 KiB; a longer header is treated as absent.
const ZONE_MAX: usize = 16;
const ZONE_BYTES: usize = 1024;

/// A cap name: `^[a-z0-9][a-z0-9._/-]{0,62}$` (§5.1), with no empty, `.` or `..` component, so it can name no
/// path outside its kind and holds no address or absolute path (root.md §3.10.6).
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=63).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._/-".contains(c))
        && n.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// `<kind>/<name>` with a §5.1 kind. The two prompt files keep their own names (`prompt/GENESIS.md`, `prompt/SELF.md`).
pub fn valid_id(id: &str) -> bool {
    match id.split_once('/') {
        Some(("prompt", n)) => matches!(n, "GENESIS.md" | "SELF.md"),
        Some((k, n)) => KINDS.contains(&k) && valid_name(n),
        None => false,
    }
}

/// The caps one tool call names (§5.2): `tool/<fn>` for every call; `ext/<input.name>` for `ext`; `skill/<n>` and
/// `agent/<n>` for a `read`, `edit` or `write` path or a `bash` command holding `skills/<n>/` or `agents/<n>/` at a
/// path-component boundary (evolution.md §3.11.1's heuristic). Sorted, each once.
pub fn call_caps(tool: &str, arg: Option<&str>) -> Vec<String> {
    let mut v = BTreeSet::new();
    if valid_name(tool) && !tool.contains('/') {
        v.insert(format!("tool/{tool}"));
    }
    match (tool, arg) {
        ("ext", Some(n)) if valid_name(n) => {
            v.insert(format!("ext/{n}"));
        }
        ("read" | "edit" | "write" | "bash", Some(t)) => {
            let b = t.as_bytes();
            for (pre, kind) in [("skills/", "skill"), ("agents/", "agent")] {
                let mut from = 0;
                while let Some(i) = t[from..].find(pre).map(|i| i + from) {
                    from = i + pre.len();
                    let edge = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b"_-.".contains(&b[i - 1]));
                    if let Some(n) = t[from..].split_once('/').map(|x| x.0).filter(|n| edge && valid_name(n)) {
                        v.insert(format!("{kind}/{n}"));
                    }
                }
            }
        }
        _ => {}
    }
    v.into_iter().collect()
}

/// `x-egress-caps: <id>,<id>…` (§5.2): the valid ids, each once; a header over 1 KiB or 16 ids counts as absent.
pub fn zone_caps(h: Option<&str>) -> Vec<String> {
    let Some(h) = h.filter(|h| h.len() <= ZONE_BYTES) else { return Vec::new() };
    let ids: Vec<&str> = h.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if ids.len() > ZONE_MAX {
        return Vec::new();
    }
    ids.into_iter().filter(|i| valid_id(i)).map(String::from).collect::<BTreeSet<_>>().into_iter().collect()
}

/// One forwarded request's caps: the tap's (`root`) and the header's (`zone`); a cap with both is `root`.
pub fn request_caps(calls: BTreeMap<String, u32>, header: Option<&str>) -> Vec<(String, u32, &'static str)> {
    let mut v: Vec<(String, u32, &'static str)> = zone_caps(header).into_iter().filter(|id| !calls.contains_key(id)).map(|id| (id, 1, "zone")).collect();
    v.extend(calls.into_iter().filter(|(id, _)| valid_id(id)).map(|(id, n)| (id, n, "root")));
    v.sort();
    v
}

/// Egress (`Egress::finish`): one forwarded live request's caps go to the sentinel, which puts them on its turn.
pub fn request(root: &Root, req_id: &str, calls: BTreeMap<String, u32>, header: Option<&str>) {
    let caps = request_caps(calls, header);
    if !caps.is_empty() {
        crate::sentinel::req_caps(root, req_id, &caps);
    }
}

/// At egress start: the hooks, and `caps.json` as the lineage left it (so the agent sees its caps before a turn).
pub fn start(root: &Root) {
    register();
    publish(root, &owner_date(root, util::now_ms()));
}

/// Registers X5's sentinel hooks once: the caps of a `change.open` and the closed-turn sink.
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut h = lock(crate::sentinel::hooks());
        h.caps = Some(change_caps);
        h.turn_closed.push(turn_closed);
    });
}

/// The Root's attribution state (`Root::extend`): the cap table, the owner days not yet digested, and the days that
/// are. A leaf lock: nothing is appended while it is held.
#[derive(Default)]
pub struct Extend {
    st: Mutex<St>,
}

#[derive(Default)]
struct St {
    table: caps::Table,
    pending: BTreeMap<String, usage::Day>,
    digested: BTreeSet<String>,
    loaded: bool,
    swept: String,
}

impl Extend {
    /// Folds one lineage record (appended now, or replayed at start): `change_open`, `change_close`, `cap_state`
    /// and `cap_use`, Root-written only.
    pub fn apply(&self, rec: &Value) {
        if rec["by"] != "root" || !matches!(rec["kind"].as_str(), Some("change_open" | "change_close" | "cap_state" | "cap_use")) {
            return;
        }
        let mut st = lock(&self.st);
        if rec["kind"] == "cap_use"
            && let Some(d) = rec["data"]["day"].as_str()
        {
            st.digested.insert(d.to_string());
        }
        st.table.apply(rec["kind"].as_str().unwrap_or(""), &rec["data"]);
    }

    /// X6: the cap table, with the counters of the owner days not yet digested added in.
    pub fn table(&self) -> Vec<CapRow> {
        let st = lock(&self.st);
        st.table.rows(&st.pending)
    }

    /// One closed turn: the usage line, the owner day's counters, and the `cap_use` digests now due (every owner day
    /// before `day` that has turns and no digest), which the caller appends. The usage files are read back once,
    /// at the first turn after a start, for the days without a digest.
    pub(crate) fn turn(&self, rh: &Path, t: &Turn, day: &str) -> io::Result<Vec<Value>> {
        let dir = rh.join("extend/usage");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let line = usage::line(t, day);
        let mut st = lock(&self.st);
        if !st.loaded {
            let St { pending, digested, .. } = &mut *st;
            usage::load(&dir, digested, pending);
            st.loaded = true;
        }
        util::append_line(&dir.join(format!("{}.jsonl", &rfc3339(t.end)[..10])), &line)?;
        usage::add(st.pending.entry(day.to_string()).or_default(), &line);
        let due: Vec<String> = st.pending.keys().filter(|d| d.as_str() < day).cloned().collect();
        let mut out = Vec::new();
        for d in due {
            if let Some(c) = st.pending.remove(&d) {
                out.extend(usage::digest(&d, &c));
            }
        }
        if st.swept != day {
            st.swept = day.to_string();
            usage::sweep(&dir, t.end);
        }
        Ok(out)
    }

    /// A digest the lineage refused goes back to its day's counters, for the next turn to retry.
    fn undigest(&self, d: &Value) {
        let mut st = lock(&self.st);
        if let Some(day) = d["day"].as_str() {
            usage::readd(st.pending.entry(day.to_string()).or_default(), d);
        }
    }

    /// `INIT_HOME/root/caps.json`.
    pub(crate) fn view(&self, day: &str) -> Value {
        json!({"caps": self.table(), "day": day, "v": 1})
    }
}

/// The owner day of `ms` as its local date (`money.day_starts_at_utc_offset`; the owner day starts at local midnight).
fn owner_date(root: &Root, ms: i64) -> String {
    let o = lock(&root.st).charter.as_ref().map(|l| l.ch.money.day_starts_at_utc_offset.clone()).unwrap_or_default();
    let n = |a: usize| o.get(a..a + 2).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let off = (n(1) * 60 + n(4)) * 60_000;
    rfc3339(ms + if o.starts_with('-') { -off } else { off })[..10].to_string()
}

/// The sentinel's closed-turn hook (`Hooks.turn_closed`).
pub fn turn_closed(root: &Root, t: &Turn) {
    let day = owner_date(root, t.end);
    match root.extend.turn(&root.p.rh, t, &day) {
        Ok(digests) => {
            for d in digests {
                if root.append("cap_use", d.clone()).is_none() {
                    root.extend.undigest(&d);
                }
            }
        }
        Err(e) => util::log(&root.p.rh, "error", &format!("extend usage line: {e}")),
    }
    publish(root, &day);
}

/// Writes `INIT_HOME/root/caps.json` (0644, ICJ), the agent's view of its caps (§5.1).
pub fn publish(root: &Root, day: &str) {
    let dir = root.p.i("root");
    if dir.is_dir()
        && let Ok(s) = icj(&root.extend.view(day))
    {
        let _ = util::write_atomic(&dir.join("caps.json"), format!("{s}\n").as_bytes(), 0o644);
    }
}

#[cfg(test)]
mod tests;
