//! Hand-written matchers for the ABNF of evolution.md §3.8 and §3.10.1 (no regex crate).
//!
//! Rules the docs use but leave undefined are pinned here, each to the one definition the docs give
//! elsewhere (see each function). ABNF quoted strings are matched case-sensitively (RFC 7405 `%s`),
//! so lineage stays greppable; that is the spirit of `trailer.noncanonical_key` (deviation from
//! RFC 5234's case-insensitive literals).

/// Exactly `n` hex digits (ABNF `HEXDIG`, either case).
pub fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `task-id`: `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}` (kernel/proxy.md §3, `x-init-task`).
pub fn is_task_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `session-id`: `^[0-9A-Za-z._-]{1,64}$` (body/log-context.md §3, supervise.md §3.4).
pub fn is_session_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `<prefix> 6DIGIT`: the kernel's sequential ids (`j-000043`, `f-000003`, `p-000007`).
fn is_seq_id(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// `gate-id = "j-" 6DIGIT`: the initctl job id (kernel/gate.md §3.1, evolution-v2.md §4.2).
pub fn is_gate_id(s: &str) -> bool {
    is_seq_id(s, "j-")
}

/// `run-id`: `<arm>-<task_id>-r<k>`, arm `c` or `p`, k = 1..R (kernel/gate.md §3.1 "Identifiers",
/// `g4::run_id`). `k` is taken as 1–4 digits without a leading zero; the redacted holdout form
/// `c-holdout-r1` (gate.md §3.7) fits the same rule.
pub fn is_run_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("c-").or_else(|| s.strip_prefix("p-")) else {
        return false;
    };
    let Some((task, k)) = rest.rsplit_once("-r") else {
        return false;
    };
    is_task_id(task)
        && (1..=4).contains(&k.len())
        && k.bytes().all(|b| b.is_ascii_digit())
        && !k.starts_with('0')
}

/// `"f-" 6DIGIT`: a friction cluster id (evolution-v2.md §3.4.4, §4.2).
pub fn is_friction_id(s: &str) -> bool {
    is_seq_id(s, "f-")
}

/// `proposal-id = "p-" 6DIGIT` (evolution-v2.md §4.2).
pub fn is_proposal_id(s: &str) -> bool {
    is_seq_id(s, "p-")
}

fn lower_digit_hyphen(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'
}

/// `label = 1*40( %x61-7A / DIGIT / "-" )` (§3.10.1).
pub fn is_label(s: &str) -> bool {
    (1..=40).contains(&s.len()) && s.bytes().all(lower_digit_hyphen)
}

/// `slug = 3*40( %x61-7A / DIGIT / "-" )` (§3.8).
pub fn is_slug(s: &str) -> bool {
    (3..=40).contains(&s.len()) && s.bytes().all(lower_digit_hyphen)
}

/// `name`: the extension name rule `^[a-z][a-z0-9-]{0,31}$` (ext/authoring.md §3).
pub fn is_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|&c| lower_digit_hyphen(c))
}

/// `number = 1*DIGIT [ "." 1*DIGIT ]` (undefined in §3.10.1; a plain non-negative decimal).
pub fn is_number(s: &str) -> bool {
    let all_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    match s.split_once('.') {
        Some((i, f)) => all_digits(i) && all_digits(f),
        None => all_digits(s),
    }
}

/// True if a [`is_number`] string is greater than zero.
pub fn number_is_positive(s: &str) -> bool {
    s.bytes().any(|b| (b'1'..=b'9').contains(&b))
}

fn is_numeric_ident(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && (p == "0" || !p.starts_with('0'))
}

fn is_alnum_ident(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// SemVer 2.0.0: `MAJOR.MINOR.PATCH[-pre][+build]`, no leading zeros in numeric identifiers.
pub fn is_semver(s: &str) -> bool {
    let (core_pre, build) = match s.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let (core, pre) = match core_pre.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (core_pre, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| is_numeric_ident(p))
        && pre.is_none_or(|p| {
            p.split('.').all(|id| {
                is_alnum_ident(id)
                    && (!id.bytes().all(|b| b.is_ascii_digit()) || is_numeric_ident(id))
            })
        })
        && build.is_none_or(|b| b.split('.').all(is_alnum_ident))
}

/// `tag = "seed-v" semver` (§3.8 `upstream`).
pub fn is_seed_tag(s: &str) -> bool {
    s.strip_prefix("seed-v").is_some_and(is_semver)
}

/// `component-id` (§3.8). `hook/<name>/<name>` uses [`is_name`] for both parts.
pub fn is_component_id(s: &str) -> bool {
    for kind in ["ext/", "skill/", "tool/", "mod/", "profile/"] {
        if let Some(rest) = s.strip_prefix(kind) {
            return (1..=128).contains(&rest.len())
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b'.'));
        }
    }
    if let Some(rest) = s.strip_prefix("prompt/") {
        return ["GENESIS.md/", "SELF.md/"]
            .iter()
            .any(|f| rest.strip_prefix(f).is_some_and(is_slug));
    }
    if let Some(rest) = s.strip_prefix("hook/") {
        return rest
            .split_once('/')
            .is_some_and(|(a, b)| is_name(a) && is_name(b));
    }
    false
}

/// The capability kinds of ADR-011 §5.1.
pub const CAP_KINDS: [&str; 10] = [
    "agent", "dep", "ext", "kernel", "loop", "mem", "mod", "prompt", "skill", "tool",
];

/// A cap name (ADR-011 §5.1): `^[a-z0-9][a-z0-9._/-]{0,62}$`, with no empty, `.` or `..` component
/// (the Root's `extend::valid_name`, so both sides accept the same ids).
pub fn is_cap_name(n: &str) -> bool {
    let b = n.as_bytes();
    (1..=63).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._/-".contains(c))
        && n.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// `cap-id = kind "/" name` (ADR-011 §5.1, the `Cap:` trailer). The two prompt files keep their own
/// names (`prompt/GENESIS.md`, `prompt/SELF.md`), as the Root's `extend::valid_id` does.
pub fn is_cap_id(id: &str) -> bool {
    match id.split_once('/') {
        Some(("prompt", n)) => matches!(n, "GENESIS.md" | "SELF.md"),
        Some((k, n)) => CAP_KINDS.contains(&k) && is_cap_name(n),
        None => false,
    }
}

/// A task family id (`Measure: exp:<family>`; evolution.md §3.15.4 `work/tasks/<family>/`): the
/// task-id rule.
pub fn is_family_id(s: &str) -> bool {
    is_task_id(s)
}

/// `1*<max>text`: `text` is undefined in §3.8; taken as any Unicode scalar except control characters,
/// counted in characters.
pub fn is_text(s: &str, max: usize) -> bool {
    let n = s.chars().count();
    (1..=max).contains(&n) && !s.chars().any(char::is_control)
}

/// `<min>*<max>VCHAR-or-SP`: printable ASCII (%x20-7E), counted in bytes.
pub fn is_vchar_or_sp(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Implements serde through `Display` / `FromStr<Err = String>`, so a value serializes as its trailer
/// spelling.
macro_rules! string_serde {
    ($t:ty) => {
        impl serde::Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as serde::Deserialize>::deserialize(d)?;
                s.parse::<$t>()
                    .map_err(<D::Error as serde::de::Error>::custom)
            }
        }
    };
}
pub(crate) use string_serde;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_labels() {
        assert!(is_task_id("2026-10-20-03"));
        assert!(is_task_id("A.b_c-1"));
        assert!(!is_task_id(".hidden"));
        assert!(!is_task_id("a/b"));
        assert!(!is_task_id(&"a".repeat(65)));
        assert!(is_session_id("20261019-203015-9f3a2c"));
        assert!(!is_session_id("a/b"));
        assert!(is_label("interactive"));
        assert!(!is_label("Interactive"));
        assert!(!is_label(""));
        assert!(is_slug("loop-detection"));
        assert!(!is_slug("ab"));
        assert!(is_name("loopguard"));
        assert!(!is_name("1abc"));
    }

    #[test]
    fn engine_ids() {
        assert!(is_gate_id("j-000031"));
        for bad in [
            "j-00031",
            "j-0000311",
            "J-000031",
            "j-00003a",
            "p-000031",
            "j000031",
        ] {
            assert!(!is_gate_id(bad), "{bad}");
        }
        assert!(is_friction_id("f-000003"));
        assert!(!is_friction_id("f-3"));
        assert!(is_proposal_id("p-000007"));
        assert!(!is_proposal_id("p-00000x"));
        assert!(!is_proposal_id("p-000007 "));
        for ok in [
            "p-repo-03-r1",
            "c-2026-10-20-03-r1",
            "c-holdout-r2",
            "c-task-r2-r10",
            "p-a-r9999",
        ] {
            assert!(is_run_id(ok), "{ok}");
        }
        for bad in [
            "x-repo-03-r1",
            "c-repo-03",
            "c-repo-03-r0",
            "c-repo-03-r01",
            "c-repo-03-r10000",
            "c--r1",
            "c-.hidden-r1",
            "c-a/b-r1",
            "c-repo-03-r1x",
            "C-repo-03-r1",
        ] {
            assert!(!is_run_id(bad), "{bad}");
        }
    }

    #[test]
    fn numbers_hex_semver() {
        assert!(is_number("30"));
        assert!(is_number("0.5"));
        assert!(!is_number(".5"));
        assert!(!is_number("5."));
        assert!(!is_number("-5"));
        assert!(number_is_positive("0.01"));
        assert!(!number_is_positive("0.00"));
        assert!(is_hex("e4f1a2b3c4d5e6f7", 16));
        assert!(is_hex("E4F1A2B3C4D5E6F7", 16));
        assert!(!is_hex("e4f1a2b3c4d5e6f", 16));
        assert!(!is_hex("g4f1a2b3c4d5e6f7", 16));
        assert!(is_seed_tag("seed-v0.3.0"));
        assert!(is_seed_tag("seed-v1.2.3-rc.1+build.5"));
        assert!(!is_seed_tag("seed-v01.2.3"));
        assert!(!is_seed_tag("seed-v1.2"));
        assert!(!is_seed_tag("v1.2.3"));
        assert!(!is_seed_tag("seed-v1.2.3-01"));
    }

    #[test]
    fn component_ids() {
        for ok in [
            "ext/pty",
            "ext/semver-sort",
            "skill/harness-patterns",
            "tool/edit",
            "mod/src/context.rs",
            "profile/deepseek",
            "prompt/SELF.md/loop-detection",
            "prompt/GENESIS.md/abc",
            "hook/loopguard/after-tool",
        ] {
            assert!(is_component_id(ok), "{ok}");
        }
        for bad in [
            "ext/",
            "prompt/OTHER.md/abc",
            "prompt/SELF.md/AB",
            "hook/x",
            "thing/x",
            "ext/a b",
            "Ext/x",
        ] {
            assert!(!is_component_id(bad), "{bad}");
        }
    }

    #[test]
    fn cap_ids_and_families() {
        for ok in [
            "ext/pty",
            "skill/harness-patterns",
            "agent/reviewer",
            "prompt/GENESIS.md",
            "prompt/SELF.md",
            "tool/edit",
            "mod/src/context.rs",
            "mem/episodic.v2",
            "loop/plan-act",
            "kernel/kernel-gate",
            "dep/serde",
            "skill/0day",
        ] {
            assert!(is_cap_id(ok), "{ok}");
        }
        for bad in [
            "ext/",
            "ext",
            "Ext/pty",
            "ext/Pty",
            "prompt/OTHER.md",
            "prompt/genesis",
            "thing/x",
            "mod/../etc",
            "mod/a//b",
            "mod/./a",
            "skill/-x",
            "skill/a b",
            &format!("skill/{}", "a".repeat(64)),
        ] {
            assert!(!is_cap_id(bad), "{bad}");
        }
        assert!(is_cap_name(&"a".repeat(63)));
        assert!(is_family_id("semver-sort"));
        assert!(!is_family_id("a/b"));
        assert!(!is_family_id(""));
    }

    #[test]
    fn text_rules() {
        assert!(is_text("new capability", 200));
        assert!(is_text("ünïcode ok", 200));
        assert!(!is_text("", 200));
        assert!(!is_text("tab\there", 200));
        assert!(!is_text(&"x".repeat(201), 200));
        assert!(is_vchar_or_sp("0123456789", 10, 500));
        assert!(!is_vchar_or_sp("012345678", 10, 500));
        assert!(!is_vchar_or_sp("non-ascii é here", 10, 500));
    }
}
