//! §3.6 admission of agent-authored task families (decisions D2, D3), as a library the kernel gate mirrors
//! (kernel/gate.md §3.7.5 applies it on `initctl task submit`; the Root checks the `task_admit` record).
//! Every rule is mechanical, and every failed rule is reported at once.
#![allow(dead_code)] // the Root's callers arrive with `sealed.family_eval` (L3+, M10); tests use all of it

use super::Sealed;
use super::stats::{Rng, lower};
use serde_json::Value;
use std::collections::BTreeSet;

/// One instance as admission measured it: the check on `solution.tar` and on the no-op answer (the initial
/// workspace, unchanged), `admit_repeats` times each.
pub struct Measured {
    pub solution: Vec<i64>,
    pub noop: Vec<i64>,
}

/// The family size (D2) and rules 1–2. `gen_identical`: for a generated family, whether two `gen/run` runs
/// with the same seed wrote byte-identical instances.
pub fn check(cfg: &Sealed, inst: &[Measured], gen_identical: Option<bool>) -> Vec<&'static str> {
    let reps = cfg.admit_repeats.max(1) as usize;
    let rules = [
        ("family_too_small", (inst.len() as i64) < cfg.family_min_instances),
        ("solution_required", inst.iter().any(|m| m.solution.len() < reps || m.noop.len() < reps)),
        ("broken_check", inst.iter().any(|m| !m.solution.contains(&1000))),
        ("nondeterministic", gen_identical == Some(false) || inst.iter().any(|m| {
            (m.solution.contains(&1000) && m.solution.iter().any(|s| *s != 1000)) || m.noop.windows(2).any(|w| w[0] != w[1])
        })),
        ("trivial", inst.iter().any(|m| m.noop.iter().any(|s| *s > cfg.admit_noop_max_permille))),
    ];
    rules.into_iter().filter(|r| r.1).map(|r| r.0).collect()
}

/// Rule 3 (D2): the sealed half, ⌊n/2⌋ (at least 1) instance indices drawn uniformly; the caller seeds the
/// generator from `/dev/urandom`, never from anything the agent chose.
pub fn seal_half(n: usize, rng: &mut Rng) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut v);
    v.truncate((n / 2).max(1).min(n));
    v.sort_unstable();
    v
}

/// `sealed_commit = sha256(salt ‖ sorted sealed instance ids)`, the ids joined by `\n`.
pub fn sealed_commit(salt: &[u8], ids: &[&str]) -> String {
    let mut s: Vec<&str> = ids.to_vec();
    s.sort_unstable();
    crate::util::sha256_hex(&[salt, s.join("\n").as_bytes()].concat())
}

/// Rules 4–5 (D3): may a family registered at Root lineage seq `reg` and authored at body sha `author` judge
/// a candidate whose LKG `P` was promoted at seq `p_seq`? `ancestry` is P's ancestry, oldest first, ending
/// with P; `promoted` holds the shas that have a `promotion{lkg:true}` record. Promotions count, commits do not.
pub fn eligible(reg: u64, p_seq: u64, author: &str, ancestry: &[&str], promoted: &BTreeSet<&str>, k: i64) -> Result<(), &'static str> {
    if reg >= p_seq {
        return Err("registered_after_lkg");
    }
    match ancestry.iter().position(|s| *s == author) {
        Some(i) if (ancestry[i + 1..].iter().filter(|s| promoted.contains(*s)).count() as i64) < k => Err("author_descendant"),
        _ => Ok(()),
    }
}

/// Rule 6: a `task_admit` record carries the provenance fields, with `n_sealed` = ⌊n/2⌋ (at least 1).
pub fn provenance_ok(d: &Value) -> Result<(), String> {
    let s = |k: &str| d[k].as_str().is_some_and(|v| !v.is_empty());
    let missing: Vec<&str> = ["family", "label", "author_sha", "sealed_commit", "model"].into_iter().filter(|k| !s(k)).collect();
    let (n, ns) = (d["n"].as_u64().unwrap_or(0), d["n_sealed"].as_u64().unwrap_or(0));
    let form = matches!(d["form"].as_str(), Some("enumerated" | "generated"));
    let by = matches!(d["sealed_by"].as_str(), Some("kernel" | "root"));
    match (missing.is_empty(), n >= 2 && ns == (n / 2).max(1), form && by) {
        (true, true, true) => Ok(()),
        _ => Err(format!("task_admit: missing {missing:?}, n {n}, n_sealed {ns}, form/sealed_by valid: {}", form && by)),
    }
}

/// What a family proves (§3.6): a capability gain needs `L > 0` at α = 0.05 over the sealed half's paired
/// differences (candidate − parent, per instance). Returns (`l_permille`, gain).
pub fn gain(d: &[f64], id: &str) -> (i64, bool) {
    let l = lower(d, id, "family");
    ((l * 1000.0).round() as i64, !d.is_empty() && l > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(sol: [i64; 3], noop: [i64; 3]) -> Measured {
        Measured { solution: sol.to_vec(), noop: noop.to_vec() }
    }

    /// SE-30 / TA-2, TA-3, TA-4, TA-5, TA-15: each refused, with every failed rule reported.
    #[test]
    fn family_rules() {
        let c = Sealed::default();
        let good = || m([1000; 3], [0; 3]);
        assert!(check(&c, &[good(), good()], None).is_empty());
        assert_eq!(check(&c, &[good()], None), ["family_too_small"]);
        assert_eq!(check(&c, &[good(), m([1000; 3], [600; 3])], None), ["trivial"]);
        assert_eq!(check(&c, &[good(), m([1000, 0, 1000], [0; 3])], None), ["nondeterministic"]);
        assert_eq!(check(&c, &[good(), m([900; 3], [0; 3])], None), ["broken_check"]);
        assert_eq!(check(&c, &[good(), m([1000; 3], [0, 100, 0])], None), ["nondeterministic"]);
        assert_eq!(check(&c, &[good(), good()], Some(false)), ["nondeterministic"]);
        assert_eq!(check(&c, &[m([1000; 3], [1000; 3])], None), ["family_too_small", "trivial"]);
        let short = Measured { solution: vec![1000], noop: vec![0] };
        assert_eq!(check(&c, &[good(), short], None), ["solution_required"]);
    }

    /// D2: ⌊n/2⌋ sealed, at least 1, uniformly drawn; the commitment ignores order.
    #[test]
    fn sealed_half_and_commit() {
        for n in 1..=8 {
            let h = seal_half(n, &mut Rng(n as u64));
            assert_eq!(h.len(), (n / 2).max(1));
            assert!(h.iter().all(|i| *i < n));
        }
        let mut seen = BTreeSet::new();
        for s in 0..200 {
            seen.insert(seal_half(4, &mut Rng(s)));
        }
        assert_eq!(seen.len(), 6, "every 2-of-4 half occurs");
        assert_eq!(sealed_commit(b"salt", &["f.2", "f.1"]), sealed_commit(b"salt", &["f.1", "f.2"]));
        assert_ne!(sealed_commit(b"salt", &["f.1"]), sealed_commit(b"pepper", &["f.1"]));
    }

    /// SE-31 / TA-8, TA-9 (D3): the seq anchor, and K = 5 promotions (not commits) after the author.
    #[test]
    fn eligibility() {
        let anc = ["a0", "c1", "c2", "p1", "c3", "p2", "p3", "p4", "c4", "p5"];
        let promoted: BTreeSet<&str> = ["p1", "p2", "p3", "p4", "p5"].into();
        assert_eq!(eligible(10, 9, "x", &anc, &promoted, 5), Err("registered_after_lkg"));
        assert_eq!(eligible(9, 9, "x", &anc, &promoted, 5), Err("registered_after_lkg"));
        assert_eq!(eligible(3, 9, "a0", &anc, &promoted, 5), Ok(()), "5 promotions after the author");
        assert_eq!(eligible(3, 9, "c2", &anc, &promoted, 5), Ok(()));
        assert_eq!(eligible(3, 9, "p1", &anc, &promoted, 5), Err("author_descendant"), "4 promotions, many commits");
        assert_eq!(eligible(3, 9, "p5", &anc, &promoted, 5), Err("author_descendant"), "the author is P");
        assert_eq!(eligible(3, 9, "elsewhere", &anc, &promoted, 5), Ok(()), "not an ancestor of P");
    }

    /// Rule 6 and SE-32: provenance fields; a gain on the visible half only is no gain on the sealed half.
    #[test]
    fn provenance_and_gain() {
        let d = json!({"family": "fam-1", "label": "edit-crlf", "n": 4, "n_sealed": 2, "form": "enumerated", "author_sha": "ab",
            "sealed_by": "kernel", "sealed_commit": "cd", "model": "deepseek-flash"});
        assert!(provenance_ok(&d).is_ok());
        let mut bad = d.clone();
        bad["n_sealed"] = json!(1);
        assert!(provenance_ok(&bad).is_err());
        assert!(provenance_ok(&json!({"family": "f"})).is_err());
        assert!(!gain(&[0.0, 0.0], "fam-1").1, "visible-only gain: the sealed half shows none");
        assert!(gain(&[1.0, 1.0, 1.0], "fam-1").1);
        assert!(!gain(&[1.0, -1.0, 0.0], "fam-1").1);
    }
}
