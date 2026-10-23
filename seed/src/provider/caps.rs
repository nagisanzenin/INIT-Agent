//! Zone-evidence capabilities (ADR-011 §5.2, X5): a code-level capability that shaped the next
//! model request marks itself here, e.g. a memory structure that put context in, a planning loop or
//! a sub-agent pattern. The request then carries the marks as `x-init-caps: <id>,<id>…`. The kernel
//! proxy forwards that header to the Root as `x-egress-caps`, and the Root counts each id once per
//! request with `zone` evidence.
//!
//! Seed components are `core`: they never mark. An agent-written module calls
//! `crate::provider::caps::mark("mem/<name>")` while it builds the request's context. Ids follow the
//! Root's grammar (`<kind>/<name>`; name `^[a-z0-9][a-z0-9._/-]{0,62}$`, with no empty, `.` or `..`
//! component). The Root ignores a header with more than 16 ids or over 1 KiB, so a mark that would
//! cross either bound is refused here.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// At most this many ids per request (the Root ignores the header beyond it).
pub const MAX_IDS: usize = 16;
/// At most this many header bytes.
pub const MAX_BYTES: usize = 1024;
const KINDS: [&str; 9] = ["agent", "dep", "ext", "kernel", "loop", "mem", "mod", "skill", "tool"];

/// A cap id the Root accepts as zone evidence.
pub fn valid_id(id: &str) -> bool {
    let Some((kind, n)) = id.split_once('/') else {
        return false;
    };
    let b = n.as_bytes();
    KINDS.contains(&kind)
        && (1..=63).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._/-".contains(c))
        && n.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// The marks of the next request.
#[derive(Default)]
pub struct Marks(Mutex<BTreeSet<String>>);

impl Marks {
    pub const fn new() -> Marks {
        Marks(Mutex::new(BTreeSet::new()))
    }

    /// Marks `id` for the next request. `false` if it is not a valid id or would cross a bound.
    pub fn mark(&self, id: &str) -> bool {
        if !valid_id(id) {
            return false;
        }
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.contains(id) {
            return true;
        }
        let bytes: usize = g.iter().map(|s| s.len() + 1).sum::<usize>() + id.len();
        if g.len() >= MAX_IDS || bytes > MAX_BYTES {
            return false;
        }
        g.insert(id.to_string());
        true
    }

    /// The header value (sorted ids, comma-separated) and clears the marks; `None` when unmarked.
    pub fn take(&self) -> Option<String> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let ids = std::mem::take(&mut *g);
        (!ids.is_empty()).then(|| ids.into_iter().collect::<Vec<_>>().join(","))
    }
}

static MARKS: Marks = Marks::new();

/// Marks `id` for the body's next model request (see the module docs).
pub fn mark(id: &str) -> bool {
    MARKS.mark(id)
}

/// The next request's `x-init-caps` value; clears the marks (`provider::client::cx` calls it).
pub fn take() -> Option<String> {
    MARKS.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_bounds_and_take() {
        for ok in ["mem/notes", "loop/plan-2", "mod/src/loop.rs", "tool/x", "agent/a.b_c"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["mem", "mem/", "prompt/SELF.md", "Mem/x", "mem/X", "mem/../x", "mem//x", "mem/-x", "web/x", &format!("mem/{}", "a".repeat(64))] {
            assert!(!valid_id(bad), "{bad}");
        }
        let m = Marks::new();
        assert_eq!(m.take(), None);
        assert!(m.mark("mem/notes") && m.mark("loop/plan") && m.mark("mem/notes"));
        assert!(!m.mark("mem/../etc"));
        assert_eq!(m.take().as_deref(), Some("loop/plan,mem/notes"));
        assert_eq!(m.take(), None, "taken once");
        // At most 16 ids, and at most 1 KiB.
        for i in 0..MAX_IDS {
            assert!(m.mark(&format!("mem/n{i:02}")));
        }
        assert!(!m.mark("mem/one-too-many"));
        assert_eq!(m.take().unwrap().split(',').count(), MAX_IDS);
        let long = |i: usize| format!("mem/{i:x}{}", "a".repeat(61));
        let mut n = 0;
        while m.mark(&long(n)) {
            n += 1;
        }
        let v = m.take().unwrap();
        assert!(v.len() <= MAX_BYTES && n < MAX_IDS, "{} bytes, {n} ids", v.len());
    }
}
