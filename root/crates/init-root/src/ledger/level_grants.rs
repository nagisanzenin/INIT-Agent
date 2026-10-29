//! L3 (v0.5.3): every permission a level grants has a Root cap or a Root rollback path. ADR-011 §7.2: "a level grants
//! rate and blast radius under Root-held reversibility, never an exemption from the sentinel"; foundations.md INV-8 (no
//! level grants a write to the Root) and N3. Static: the test reads the Root's own source for every comparison with a
//! level (an identifier ending in `level` or `lvl` beside `<`, `<=`, `>` or `>=`) and every per-level budget lookup,
//! and fails on one that the tables below do not name. A new level permission must therefore say here what bounds it:
//! the cap that refuses past it, or the rollback that undoes it, each with the source text that implements it, which
//! the test also checks is still there. Comparisons that grant nothing (the ladder itself, display) are listed with
//! the reason.

use std::path::{Path, PathBuf};

/// A level-gated grant: (file under src/, text of the gating line, what it grants, the cap, the rollback path). Cap
/// and rollback are (file, text that implements it); at least one is present.
type Grant = (&'static str, &'static str, &'static str, Option<(&'static str, &'static str)>, Option<(&'static str, &'static str)>);

const GRANTS: &[Grant] = &[
    ("sentinel/engine.rs", "pub fn budget(level: i64)", "the blast-radius defaults by level: open groups, changes per owner day, group_max, kernel changes per day",
        Some(("sentinel/engine.rs", "change groups are open (L{} budget)")), Some(("sentinel/engine.rs", "fn rollback_from("))),
    ("grant.rs", "extend(root).budget(level)", "the charter's per-level budget with the owner's grants, which A5 and the sentinel read",
        Some(("sentinel/engine.rs", "changes were admitted this owner day")), Some(("sentinel/engine.rs", "fn rollback_from("))),
    ("charter.rs", "kernel_open: i64::from(level >= 1)", "the kernel lane from L1 (one open kernel group)",
        Some(("sentinel/engine.rs", "kernel changes per day at L{}")), Some(("sup/kupgrade.rs", "\"probation_fail\""))),
    ("sentinel/engine.rs", "if kernel && (cx.level < 1", "a kernel change group from L1, alone",
        Some(("sentinel/engine.rs", "kernel changes per day at L{}")), Some(("sup/kupgrade.rs", "\"probation_fail\""))),
    ("sup/kupgrade.rs", "if level < need", "`system.swap`, a kernel swap: L1 on the free path, L3 gated",
        Some(("sup/kupgrade.rs", "root.unwatched_change")), Some(("sup/kupgrade.rs", "\"probation_fail\""))),
    ("extend/select.rs", "x.budget(level).bloat_permille", "a larger bloat budget (the level's factor)",
        Some(("extend/select.rs", "extend.bloat")), None),
    ("egress.rs", "if i64::from(c.min_level) > level", "external-action classes up to the level",
        Some(("egress/money.rs", "pub fn admit(&mut self, r: u64)")), None),
    ("egress.rs", "if i64::from(md.min_level) > if safe { 0 } else { level }", "models up to the level",
        Some(("egress/money.rs", "pub fn admit(&mut self, r: u64)")), None),
    ("egress/lane.rs", "i64::from(m.min_level) <= cx.level", "models up to the level on a sealed or twin lane",
        Some(("egress/lane.rs", "Pool::Reserve(money::Limits::of(ch).reserve)")), None),
    ("sup.rs", "\"sealed.family_eval\" if lock(&root.st).level.level < 3", "the sealed family evaluation from L3 (it writes no tuple)",
        Some(("sealed/mod.rs", "reserve_usd")), None),
    ("zone.rs", "if level < 2 && !safe && !free", "more than one live body from L2 on the gated path",
        Some(("zone.rs", "l.ch.quotas.parallel_bodies")), None),
    ("zone.rs", "(st.level.level < 2 && !free)", "the parallel-body limit from L2 on the gated path",
        Some(("zone.rs", "l.ch.quotas.parallel_bodies")), None),
];

/// Level comparisons that grant nothing: (file, text of the line, why).
const NOT_GRANTS: &[(&str, &str, &str)] = &[
    ("ledger.rs", "self.level >= 1 =>", "the ladder: the harm-rate demotion trigger (§7.3)"),
    ("ledger.rs", "if self.level >= cap", "the ladder: no climb past the cap"),
    ("ledger.rs", "(self.level < cap)", "the ladder's view: the next level's criteria"),
    ("link.rs", "if l.ch.autonomy.max_level < level", "the charter's max_level caps the level (a limit, not a grant)"),
    ("egress.rs", "\"routable\": i64::from(md.min_level) <= eff", "`egress stats` display"),
];

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The Root's non-test source files under src/, as (path relative to src/, text without inline test modules).
fn sources() -> Vec<(String, String)> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "tests") {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&src(), &mut files);
    files.sort();
    files.into_iter().filter_map(|p| {
        let rel = p.strip_prefix(src()).unwrap().to_string_lossy().into_owned();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        // test-only files: unit-test modules, the simulations, this file, the fake clock and test ledgers
        if name == "tests.rs" || name.ends_with("_tests.rs") || name == "sim.rs" || name == "level_grants.rs" || name == "testfix.rs" {
            return None;
        }
        Some((rel, strip_test_mods(&std::fs::read_to_string(&p).unwrap())))
    }).collect()
}

/// The text without its inline `#[cfg(test)] mod … { … }` blocks (braces counted; the Root's test modules keep their
/// braces balanced, which the stale-row check below would expose if one did not).
fn strip_test_mods(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("#[cfg(test)]\n") {
        let after = &rest[i + "#[cfg(test)]\n".len()..];
        let head = after.trim_start();
        let opens = head.starts_with("mod ") && head.lines().next().is_some_and(|l| l.trim_end().ends_with('{'));
        if !opens {
            out.push_str(&rest[..i + "#[cfg(test)]\n".len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..i]);
        let start = after.len() - head.len() + head.find('{').unwrap();
        let mut depth = 0i64;
        let mut end = after.len();
        for (j, c) in after[start..].char_indices() {
            depth += match c { '{' => 1, '}' => -1, _ => 0 };
            if depth == 0 {
                end = start + j + 1;
                break;
            }
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// A line compares a level (an identifier ending in `level` or `lvl`, possibly a field, beside `<`, `<=`, `>`, `>=`),
/// or looks a budget up by level. A level copied into a variable of another name escapes this; the Root's code names
/// its levels `level` or `lvl`, and a review of a new grant should keep it so.
fn gates(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") {
        return false;
    }
    if t.contains("budget(level") || t.contains(".budget(level") {
        return true;
    }
    let b = line.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
    for (i, w) in line.match_indices("level").chain(line.match_indices("lvl")) {
        let end = i + w.len();
        if end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
            continue; // `levels`, `level_x`: another word
        }
        // right: `level < 1`, `level) > x`
        let mut j = end;
        while j < b.len() && (b[j] == b' ' || b[j] == b')') {
            j += 1;
        }
        let right = j < b.len() && (b[j] == b'<' || b[j] == b'>') && !(j + 1 < b.len() && (b[j + 1] == b'<' || b[j + 1] == b'>'));
        // left: `x > level`, `x <= cx.level`
        let mut k = i;
        while k > 0 && ident(b[k - 1]) {
            k -= 1;
        }
        while k > 0 && (b[k - 1] == b' ' || b[k - 1] == b'(') {
            k -= 1;
        }
        let k = if k > 0 && b[k - 1] == b'=' && k > 1 && (b[k - 2] == b'<' || b[k - 2] == b'>') { k - 1 } else { k };
        let left = k > 0 && (b[k - 1] == b'<' || b[k - 1] == b'>') && !(k > 1 && (b[k - 2] == b'-' || b[k - 2] == b'=' || b[k - 2] == b'<' || b[k - 2] == b'>'));
        if right || left {
            return true;
        }
    }
    false
}

#[test]
fn every_level_permission_has_a_root_cap_or_a_rollback_path() {
    let files = sources();
    let text = |f: &str| files.iter().find(|(p, _)| p == f).map(|(_, t)| t.as_str()).unwrap_or_else(|| panic!("no source file {f}"));
    // 1. every level-gated line is named, as a grant or as a non-grant
    let mut found: Vec<(String, String)> = Vec::new();
    for (path, t) in &files {
        for line in t.lines().filter(|l| gates(l)) {
            let named = GRANTS.iter().any(|g| g.0 == path && line.contains(g.1)) || NOT_GRANTS.iter().any(|n| n.0 == path && line.contains(n.1));
            assert!(named, "{path}: a level-gated line that no table names; add it to GRANTS with its Root cap or rollback path, or to NOT_GRANTS with why:\n  {}", line.trim());
            found.push((path.clone(), line.to_string()));
        }
    }
    // 2. every entry still names a real line (no stale rows), and every grant's cap or rollback is still in the source
    for (f, m) in GRANTS.iter().map(|g| (g.0, g.1)).chain(NOT_GRANTS.iter().map(|n| (n.0, n.1))) {
        assert!(found.iter().any(|(p, l)| p == f && l.contains(m)), "{f}: the table names {m:?}, which is no longer a level-gated line");
    }
    for (f, line, what, cap, rollback) in GRANTS {
        assert!(cap.is_some() || rollback.is_some(), "{f}: {what} has neither a Root cap nor a rollback path");
        for (cf, ct) in cap.iter().chain(rollback.iter()) {
            assert!(text(cf).contains(ct), "{f} ({line}): {what}: its bound {ct:?} is no longer in {cf}");
        }
    }
    // 3. the budgets are bounded at every level, and none exceeds L4's (a level never lifts a cap to "unlimited")
    let top = crate::sentinel::engine::budget(4);
    for level in 0..=4 {
        let b = crate::sentinel::engine::budget(level);
        assert!(b.0 > 0 && b.0 <= top.0 && b.1 <= top.1 && b.2 <= top.2 && b.3 <= top.3, "L{level}: {b:?}");
        let c = crate::charter::Extend::default().budget(level);
        assert!(c.open_groups > 0 && c.kernel_open <= 1 && c.changes_day > 0, "L{level}: {c:?}");
    }
    assert_eq!(strip_test_mods("a\n#[cfg(test)]\nmod t {\n fn f() { if level < 2 {} }\n}\nb\n#[cfg(test)]\nfn g() {}\n"), "a\n\nb\n#[cfg(test)]\nfn g() {}\n");
    // the matcher itself (so a refactor of it cannot silently match nothing)
    for (yes, l) in [(true, "if level < 2 && x"), (true, "if i64::from(c.min_level) > level {"), (true, "m.min_level <= cx.level"), (true, "budget(level)"),
        (true, "let _ = lvl >= 2;"), (false, "format!(\"L{level}\")"), (false, "fn f(level: i64) -> i64"), (false, "// level < 2"), (false, "levels >= 3"), (false, "x => level")] {
        assert_eq!(gates(l), yes, "{l}");
    }
}
