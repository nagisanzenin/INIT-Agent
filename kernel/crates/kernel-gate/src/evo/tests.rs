//! Tests against the real local git (evolution.md §8 rows 1–2): the §3.9 git-behaviour table through
//! [`GitTrailerParser`] and the strict check, every mandatory-matrix cell, and scratch lineage repos.
//!
//! Safety: everything lives under `kernel/target/test-homes/WP8/evo-*` and is removed by exact path.
//! Every git process runs with `env_clear`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`,
//! `HOME=<test home>` and the identity `INIT Agent <agent@init.local>`. No network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use super::validate::{KEYS, Need, need, need_in};
use super::*;

const GIT: &str = "/usr/bin/git";
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HOMES_MARK: &str = "/target/test-homes/WP8/evo-";

/// A fresh directory under `kernel/target/test-homes/WP8/`, removed by exact path on drop.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-homes/WP8")
            .join(format!("evo-{name}-{}-{n}", std::process::id()));
        assert!(dir.to_string_lossy().contains(HOMES_MARK));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove stale test home");
        }
        fs::create_dir_all(&dir).expect("create test home");
        Home(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if self.0.to_string_lossy().contains(HOMES_MARK) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn parser(home: &Home) -> GitTrailerParser {
    let empty = home.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    GitTrailerParser::new(GIT, empty)
}

fn codes<T>(r: &Result<T, Vec<ValidationError>>) -> Vec<String> {
    match r {
        Ok(_) => Vec::new(),
        Err(e) => e.iter().map(|e| e.code.clone()).collect(),
    }
}

fn has(r: &Result<impl Sized, Vec<ValidationError>>, code: &str) -> bool {
    codes(r).iter().any(|c| c == code)
}

fn render(subject: &str, trailers: &[(&str, String)]) -> String {
    let mut s = format!("{subject}\n\nThe body explains the change in free text.\n\n");
    for (k, v) in trailers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    s
}

/// The smallest valid trailer set for `t`.
fn minimal(t: ChangeType) -> Vec<(&'static str, String)> {
    use ChangeType::*;
    let mut v = vec![
        (
            "Why",
            "task t-1 hung for 600 s on an interactive REPL".to_string(),
        ),
        (
            "Predicts",
            match t {
                Perf => "cost_usd < parent",
                Feat | Fix | Guard => "pass_rate@label:interactive > parent",
                _ => "pass_rate >= parent",
            }
            .to_string(),
        ),
        (
            "Compensates-for",
            if matches!(t, Guard | Thin) {
                "loop-detection"
            } else {
                "none"
            }
            .to_string(),
        ),
        ("Prefix-change", "no".to_string()),
    ];
    match t {
        Fix | Guard => v.push(("Repro", "task:t-1".into())),
        Retire => v.push(("Retires", "ext/old".into())),
        Thin => v.push(("Thins", "ext/old".into())),
        Upstream => v.push(("Upstream", format!("seed-v0.3.0 {SHA_A}"))),
        Revert => v.push(("Reverts", SHA_B.into())),
        _ => {}
    }
    v
}

fn valid_value(key: &str) -> String {
    match key {
        "Why" => "a reason that is long enough".into(),
        "Predicts" => "pass_rate >= parent".into(),
        "Compensates-for" => "none".into(),
        "Prefix-change" => "no".into(),
        "Repro" => "task:t-1".into(),
        "Retires" | "Thins" => "ext/old".into(),
        "Upstream" => format!("seed-v0.3.0 {SHA_A}"),
        "Reverts" => SHA_B.into(),
        "Proposal" => "p-000001".into(),
        "Cap" => "ext/pty".into(),
        "Measure" => "g4".into(),
        _ => unreachable!(),
    }
}

fn subject(t: ChangeType) -> String {
    format!("{}: change something", t.as_str())
}

fn with(t: ChangeType, f: impl FnOnce(&mut Vec<(&'static str, String)>)) -> String {
    let mut tr = minimal(t);
    f(&mut tr);
    render(&subject(t), &tr)
}

fn set(tr: &mut Vec<(&'static str, String)>, key: &'static str, value: &str) {
    match tr.iter_mut().find(|(k, _)| *k == key) {
        Some(e) => e.1 = value.to_string(),
        None => tr.push((key, value.to_string())),
    }
}

const SPEC_EXAMPLE: &str = "feat(ext): add pty component for interactive programs

The bash tool cannot drive programs that check isatty(); task 2026-10-20-03
hung for 600 s on an interactive REPL.

Why: task 2026-10-20-03 hung on an interactive REPL (events e4f1a2b3c4d5e6f7).
Predicts: pass_rate@label:interactive > parent; gap_ms_p50@label:interactive <= parent-30%
Compensates-for: none (new capability)
Prefix-change: no
Repro: task:2026-10-20-03
";

// ---- §3.8 example and the §3.9 git-behaviour table (real git 2.39) ----

#[test]
fn spec_example_validates() {
    let h = Home::new("example");
    let v = validate_message(&parser(&h), SPEC_EXAMPLE).expect("the §3.8 example validates");
    assert_eq!(v.change_type, ChangeType::Feat);
    assert_eq!(v.scope.as_deref(), Some("ext"));
    assert_eq!(v.summary, "add pty component for interactive programs");
    assert_eq!(
        v.trailers.why,
        "task 2026-10-20-03 hung on an interactive REPL (events e4f1a2b3c4d5e6f7)."
    );
    assert_eq!(v.trailers.predicts.len(), 2);
    assert_eq!(
        v.trailers.compensates_for,
        CompTag {
            tag: "none".into(),
            note: Some("new capability".into())
        }
    );
    assert!(!v.trailers.prefix_change);
    assert_eq!(
        v.trailers.repro,
        vec![ReproRef::Task("2026-10-20-03".into())]
    );
    assert_eq!(v.capability_labels, vec!["interactive".to_string()]);
}

#[test]
fn git_table_continuations_are_unfolded() {
    let h = Home::new("unfold");
    let p = parser(&h);
    let msg = render(
        "feat: add x",
        &[
            (
                "Why",
                "task t-1 hung on an\n  interactive REPL\n\tfor 600 s".into(),
            ),
            ("Predicts", "pass_rate > parent".into()),
            ("Compensates-for", "none".into()),
            ("Prefix-change", "no".into()),
        ],
    );
    let l = p.parse(&msg).unwrap();
    assert_eq!(
        l[0],
        (
            "Why".to_string(),
            "task t-1 hung on an interactive REPL for 600 s".to_string()
        )
    );
    let v = validate_message(&p, &msg).unwrap();
    assert_eq!(
        v.trailers.why,
        "task t-1 hung on an interactive REPL for 600 s"
    );
}

#[test]
fn git_table_mixed_block_without_git_trailer_is_loud() {
    let h = Home::new("mixed");
    let p = parser(&h);
    let msg = "feat: add x\n\nbody\n\nWhy: a reason that is long enough\nsee also the old loop\n\
               Predicts: pass_rate > parent\nCompensates-for: none\nPrefix-change: no\n";
    assert!(
        p.parse(msg).unwrap().is_empty(),
        "git drops the whole block silently"
    );
    let r = validate_message(&p, msg);
    let errs = r.as_ref().unwrap_err();
    assert!(errs.contains(&ValidationError::new(
        codes::BLOCK_MIXED,
        "line 6: 'see also the old loop'"
    )));
    assert!(has(&r, codes::PARSER_MISMATCH));
    assert!(has(&r, &codes::missing("Predicts")));
}

#[test]
fn git_table_mixed_block_with_signed_off_by_is_loud() {
    let h = Home::new("mixed-sob");
    let p = parser(&h);
    let msg = "feat: add x\n\nbody\n\nWhy: a reason that is long enough\nsee also the old loop\n\
               Predicts: pass_rate > parent\nCompensates-for: none\nPrefix-change: no\n\
               Signed-off-by: INIT Agent <agent@init.local>\n";
    let l = p.parse(msg).unwrap();
    assert_eq!(
        l.len(),
        5,
        "git keeps the trailer lines and drops the prose: {l:?}"
    );
    assert!(l.iter().all(|(_, v)| !v.contains("old loop")));
    let r = validate_message(&p, msg);
    assert_eq!(codes(&r), vec![codes::BLOCK_MIXED]);
}

#[test]
fn git_table_divider_in_body_is_ignored() {
    let h = Home::new("divider");
    let p = parser(&h);
    let tr = minimal(ChangeType::Feat);
    let mut msg = String::from("feat: add x\n\nbefore\n---\nafter the divider\n\n");
    for (k, v) in &tr {
        msg.push_str(&format!("{k}: {v}\n"));
    }
    let v = validate_message(&p, &msg).expect("--no-divider keeps the trailers after ---");
    assert_eq!(v.trailers.why, tr[0].1);
}

#[test]
fn git_table_empty_value_is_rejected() {
    let h = Home::new("empty-value");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| set(tr, "Why", ""));
    assert!(msg.contains("Why: \n"));
    let msg = msg.replace("Why: \n", "Why:\n");
    assert_eq!(
        p.parse(&msg).unwrap()[0],
        ("Why".to_string(), String::new())
    );
    assert_eq!(codes(&validate_message(&p, &msg)), vec![codes::EMPTY]);
}

#[test]
fn git_table_separator_spacing_is_normalized() {
    let h = Home::new("normalize");
    let p = parser(&h);
    let msg = "feat: add x\n\nbody\n\nWhy : a reason that is long enough\nPredicts: pass_rate > parent\n\
               Compensates-for: none\nPrefix-change: yes\nRepro:task:t-1\n";
    let l = p.parse(msg).unwrap();
    assert_eq!(
        l[0],
        (
            "Why".to_string(),
            "a reason that is long enough".to_string()
        )
    );
    assert_eq!(l[4], ("Repro".to_string(), "task:t-1".to_string()));
    let v = validate_message(&p, msg).unwrap();
    assert_eq!(v.trailers.repro, vec![ReproRef::Task("t-1".into())]);
    assert!(v.trailers.prefix_change);
}

#[test]
fn git_table_key_with_space_is_loud() {
    let h = Home::new("key-space");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| {
        tr.push(("Why not", "because".into()))
    });
    assert!(
        p.parse(&msg).unwrap().is_empty(),
        "git does not recognize the block at all"
    );
    let r = validate_message(&p, &msg);
    assert!(r.as_ref().unwrap_err().contains(&ValidationError::new(
        codes::BLOCK_MIXED,
        "line 9: 'Why not: because'"
    )));
    assert!(has(&r, codes::PARSER_MISMATCH));
    assert!(has(&r, &codes::missing("Why")));
}

#[test]
fn git_table_single_paragraph_has_no_block() {
    let h = Home::new("one-para");
    let p = parser(&h);
    let r = validate_message(&p, "feat: add x\n");
    assert!(has(&r, codes::BLOCK_MISSING));
    assert!(!has(&r, codes::SUBJECT));
    // Trailers glued to the subject: one paragraph, so git parses nothing.
    let msg = "feat: add x\nWhy: a reason that is long enough\nPredicts: pass_rate > parent\n";
    assert!(p.parse(msg).unwrap().is_empty());
    let r = validate_message(&p, msg);
    assert!(has(&r, codes::BLOCK_MISSING));
    assert!(has(&r, codes::SUBJECT), "the subject must be a single line");
}

#[test]
fn git_table_crlf_is_accepted() {
    let h = Home::new("crlf");
    let p = parser(&h);
    let lf = with(ChangeType::Fix, |_| {});
    let crlf = lf.replace('\n', "\r\n");
    let a = validate_message(&p, &lf).unwrap();
    let b = validate_message(&p, &crlf).expect("CRLF validates");
    assert_eq!(a, b);
}

// ---- keys, values and the matrix ----

#[test]
fn matrix_every_cell_present_and_absent() {
    let h = Home::new("matrix");
    let p = parser(&h);
    for t in ChangeType::ALL {
        let base = minimal(t);
        let ok = validate_message(&p, &render(&subject(t), &base));
        assert!(ok.is_ok(), "{t:?} minimal: {:?}", codes(&ok));
        for key in KEYS {
            let present = base.iter().any(|(k, _)| *k == key);
            match need(t, key) {
                Need::Mandatory => {
                    assert!(present, "{t:?} {key}");
                    let without: Vec<_> = base.iter().filter(|(k, _)| *k != key).cloned().collect();
                    let r = validate_message(&p, &render(&subject(t), &without));
                    assert_eq!(codes(&r), vec![codes::missing(key)], "{t:?} without {key}");
                }
                Need::Forbidden => {
                    assert!(!present, "{t:?} {key}");
                    let mut extra = base.clone();
                    extra.push((key, valid_value(key)));
                    let r = validate_message(&p, &render(&subject(t), &extra));
                    assert_eq!(codes(&r), vec![codes::forbidden(key)], "{t:?} with {key}");
                }
                Need::Conditional | Need::Optional => {
                    assert!(!present, "{t:?} {key}");
                    let mut extra = base.clone();
                    extra.push((key, valid_value(key)));
                    let r = validate_message(&p, &render(&subject(t), &extra));
                    assert!(r.is_ok(), "{t:?} with optional {key}: {:?}", codes(&r));
                }
            }
        }
    }
}

#[test]
fn noncanonical_key_is_rejected() {
    let h = Home::new("noncanonical");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |_| {}).replace("Prefix-change:", "prefix-change:");
    let r = validate_message(&p, &msg);
    assert_eq!(
        codes(&r),
        vec![
            codes::NONCANONICAL_KEY.to_string(),
            codes::missing("Prefix-change")
        ]
    );
}

#[test]
fn duplicates_are_rejected_except_repeatable_keys() {
    let h = Home::new("dup");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| {
        tr.push(("Why", "a second reason, also long".into()))
    });
    assert_eq!(codes(&validate_message(&p, &msg)), vec![codes::DUPLICATE]);
    let msg = with(ChangeType::Retire, |tr| {
        tr.push(("Retires", "skill/unused".into()))
    });
    let v = validate_message(&p, &msg).unwrap();
    assert_eq!(
        v.trailers.retires,
        vec!["ext/old".to_string(), "skill/unused".to_string()]
    );
}

#[test]
fn six_predicts_clauses_are_too_many() {
    let h = Home::new("six");
    let p = parser(&h);
    let six = "pass_rate > parent; cost_usd <= parent; wall_s <= parent; turns < parent; tokens_in < parent; tokens_out < parent";
    let msg = with(ChangeType::Feat, |tr| set(tr, "Predicts", six));
    assert_eq!(
        codes(&validate_message(&p, &msg)),
        vec![codes::PREDICTS_TOO_MANY]
    );
    let five = "pass_rate > parent; cost_usd <= parent; wall_s <= parent; turns < parent; tokens_in < parent";
    assert!(validate_message(&p, &with(ChangeType::Feat, |tr| set(tr, "Predicts", five))).is_ok());
}

#[test]
fn predicts_parse_errors_are_reported() {
    let h = Home::new("predicts-parse");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| {
        set(tr, "Predicts", "pass_rate > parent; speed < parent")
    });
    assert_eq!(
        codes(&validate_message(&p, &msg)),
        vec![codes::PREDICTS_PARSE]
    );
}

#[test]
fn baseline_only_predicts_on_feat_is_trivial() {
    let h = Home::new("baseline");
    let p = parser(&h);
    let baseline = "pass_rate >= parent; cost_usd <= parent; wall_s <= parent";
    for t in [
        ChangeType::Feat,
        ChangeType::Fix,
        ChangeType::Guard,
        ChangeType::Perf,
    ] {
        let r = validate_message(&p, &with(t, |tr| set(tr, "Predicts", baseline)));
        assert_eq!(codes(&r), vec![codes::PREDICTS_TRIVIAL_ONLY], "{t:?}");
    }
    for t in [
        ChangeType::Refactor,
        ChangeType::Skill,
        ChangeType::Retire,
        ChangeType::Thin,
        ChangeType::Upstream,
        ChangeType::Revert,
    ] {
        assert!(
            validate_message(&p, &with(t, |tr| set(tr, "Predicts", baseline))).is_ok(),
            "{t:?}"
        );
    }
}

#[test]
fn perf_needs_a_strict_cost_or_time_clause() {
    let h = Home::new("perf");
    let p = parser(&h);
    for (pred, ok) in [
        ("cost_usd < parent", true),
        ("cache_hit > parent+5%", true),
        ("gap_ms_p50@pool < parent-10%", true),
        ("pass_rate > parent", false),
        ("cost_usd <= parent-10%", false),
        ("cost_usd > parent", false),
    ] {
        let r = validate_message(&p, &with(ChangeType::Perf, |tr| set(tr, "Predicts", pred)));
        if ok {
            assert!(r.is_ok(), "{pred}: {:?}", codes(&r));
        } else {
            assert_eq!(codes(&r), vec![codes::PREDICTS_TRIVIAL_ONLY], "{pred}");
        }
    }
}

#[test]
fn prefix_change_yes_without_repro_on_feat() {
    let h = Home::new("prefix");
    let p = parser(&h);
    let r = validate_message(
        &p,
        &with(ChangeType::Feat, |tr| set(tr, "Prefix-change", "yes")),
    );
    assert_eq!(codes(&r), vec![codes::REPRO_REQUIRED_FOR_PREFIX]);
    let r = validate_message(
        &p,
        &with(ChangeType::Feat, |tr| {
            set(tr, "Prefix-change", "yes");
            set(tr, "Repro", "task:t-1");
        }),
    );
    assert!(r.is_ok());
    // Removal types need no Repro for a prefix change (thinning a prompt block).
    assert!(
        validate_message(
            &p,
            &with(ChangeType::Thin, |tr| set(tr, "Prefix-change", "yes"))
        )
        .is_ok()
    );
    let r = validate_message(
        &p,
        &with(ChangeType::Feat, |tr| set(tr, "Prefix-change", "Yes")),
    );
    assert_eq!(codes(&r), vec![codes::invalid("Prefix-change")]);
}

#[test]
fn trace_repro_needs_16_hex() {
    let h = Home::new("trace");
    let p = parser(&h);
    let v = validate_message(
        &p,
        &with(ChangeType::Fix, |tr| {
            set(tr, "Repro", "trace:0123456789ABCDEF")
        }),
    )
    .unwrap();
    assert_eq!(
        v.trailers.repro,
        vec![ReproRef::Trace("0123456789abcdef".into())]
    );
    for bad in [
        "trace:0123456789abcde",
        "trace:0123456789abcdef0",
        "trace:0123456789abcdeg",
    ] {
        let r = validate_message(&p, &with(ChangeType::Fix, |tr| set(tr, "Repro", bad)));
        assert_eq!(codes(&r), vec![codes::invalid("Repro")], "{bad}");
    }
    let refs = "task:t-1, event:20261019-203015-9f3a2c#e4f1a2b3c4d5e6f7, trace:0123456789abcdef";
    assert_eq!(
        validate_message(&p, &with(ChangeType::Fix, |tr| set(tr, "Repro", refs)))
            .unwrap()
            .trailers
            .repro
            .len(),
        3
    );
}

#[test]
fn run_and_friction_repro_refs() {
    let h = Home::new("engine-refs");
    let p = parser(&h);
    let refs = "run:j-000031/p-repo-03-r1, friction:f-000003";
    let v = validate_message(&p, &with(ChangeType::Fix, |tr| set(tr, "Repro", refs))).unwrap();
    assert_eq!(
        v.trailers.repro,
        vec![
            ReproRef::Run {
                gate: "j-000031".into(),
                run: "p-repo-03-r1".into()
            },
            ReproRef::Friction("f-000003".into())
        ]
    );
    // Display, FromStr and serde all use the trailer spelling.
    assert_eq!(v.trailers.repro[0].to_string(), "run:j-000031/p-repo-03-r1");
    let json = serde_json::to_string(&v.trailers.repro).unwrap();
    assert_eq!(json, r#"["run:j-000031/p-repo-03-r1","friction:f-000003"]"#);
    let back: Vec<ReproRef> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, v.trailers.repro);
    assert!(serde_json::from_str::<ReproRef>("\"friction:f-3\"").is_err());
    for bad in [
        "run:j-000031/q-repo-03-r1",
        "run:j-0031/p-repo-03-r1",
        "run:j-000031/p-repo-03",
        "friction:f-00003",
        "friction:p-000003",
    ] {
        let r = validate_message(&p, &with(ChangeType::Fix, |tr| set(tr, "Repro", bad)));
        assert_eq!(codes(&r), vec![codes::invalid("Repro")], "{bad}");
    }
}

#[test]
fn proposal_trailer_values_and_matrix() {
    let h = Home::new("proposal-msg");
    let p = parser(&h);
    let two = |t: ChangeType| with(t, |tr| tr.push(("Proposal", "p-000007, p-000008".into())));
    let v = validate_message(&p, &two(ChangeType::Feat)).unwrap();
    assert_eq!(v.trailers.proposals, vec!["p-000007", "p-000008"]);
    let json = to_sorted_json(&v.trailers);
    assert!(
        json.contains("\"proposals\":[\"p-000007\",\"p-000008\"]"),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Trailers>(&json).unwrap(), v.trailers);

    // Without the trailer the JSON is unchanged (no key), and JSON without the key still parses.
    let v0 = validate_message(&p, &with(ChangeType::Feat, |_| {})).unwrap();
    assert!(v0.trailers.proposals.is_empty());
    let json0 = to_sorted_json(&v0.trailers);
    assert!(!json0.contains("proposals"), "{json0}");
    assert_eq!(
        serde_json::from_str::<Trailers>(&json0).unwrap(),
        v0.trailers
    );

    // Optional on every type except upstream and revert.
    for t in ChangeType::ALL {
        let r = validate_message(&p, &two(t));
        if matches!(t, ChangeType::Upstream | ChangeType::Revert) {
            assert_eq!(codes(&r), vec![codes::forbidden("Proposal")], "{t:?}");
        } else {
            assert!(r.is_ok(), "{t:?}: {:?}", codes(&r));
        }
    }

    let dup = with(ChangeType::Feat, |tr| {
        tr.push(("Proposal", "p-000007".into()));
        tr.push(("Proposal", "p-000008".into()));
    });
    assert_eq!(codes(&validate_message(&p, &dup)), vec![codes::DUPLICATE]);
    for bad in [
        "p-7",
        "p-000001,p-000002",
        "p-000001, p-000002, p-000003, p-000004, p-000005",
        "proposal p-000001",
    ] {
        let r = validate_message(
            &p,
            &with(ChangeType::Feat, |tr| tr.push(("Proposal", bad.into()))),
        );
        assert_eq!(codes(&r), vec![codes::invalid("Proposal")], "{bad}");
    }
    let lower = with(ChangeType::Feat, |tr| {
        tr.push(("proposal", "p-000007".into()))
    });
    assert_eq!(
        codes(&validate_message(&p, &lower)),
        vec![codes::NONCANONICAL_KEY]
    );
}

#[test]
fn guard_must_name_a_weakness() {
    let h = Home::new("guard");
    let p = parser(&h);
    let r = validate_message(
        &p,
        &with(ChangeType::Guard, |tr| {
            set(tr, "Compensates-for", "none (just in case)")
        }),
    );
    assert_eq!(codes(&r), vec![codes::invalid("Compensates-for")]);
}

#[test]
fn value_grammar_errors() {
    let h = Home::new("values");
    let p = parser(&h);
    for (t, key, value) in [
        (ChangeType::Feat, "Why", "too short"),
        (ChangeType::Feat, "Why", "non-ASCII reason: café crashed"),
        (ChangeType::Feat, "Compensates-for", "Loop"),
        (ChangeType::Retire, "Retires", "thing/x"),
        (ChangeType::Upstream, "Upstream", "seed-v0.3 aaaa"),
        (ChangeType::Revert, "Reverts", "abc123"),
    ] {
        let r = validate_message(&p, &with(t, |tr| set(tr, key, value)));
        assert_eq!(codes(&r), vec![codes::invalid(key)], "{key}: {value}");
    }
}

#[test]
fn comment_line_in_block_is_loud() {
    let h = Home::new("comment");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |_| {}).replace(
        "Prefix-change: no\n",
        "# a comment git would drop\nPrefix-change: no\n",
    );
    assert_eq!(p.parse(&msg).unwrap().len(), 4, "git skips comment lines");
    assert_eq!(codes(&validate_message(&p, &msg)), vec![codes::BLOCK_MIXED]);
}

#[test]
fn unknown_keys_are_kept_sorted() {
    let h = Home::new("other");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| {
        tr.push(("Signed-off-by", "B <b@init.local>".into()));
        tr.push(("Co-authored-by", "A <a@init.local>".into()));
        tr.push(("Signed-off-by", "A <a@init.local>".into()));
    });
    let v = validate_message(&p, &msg).unwrap();
    let keys: Vec<&String> = v.trailers.other.keys().collect();
    assert_eq!(keys, vec!["Co-authored-by", "Signed-off-by"]);
    assert_eq!(
        v.trailers.other["Signed-off-by"],
        vec![
            "B <b@init.local>".to_string(),
            "A <a@init.local>".to_string()
        ]
    );
}

#[test]
fn capability_labels_from_message() {
    let h = Home::new("caps");
    let p = parser(&h);
    let pred = "pass_rate@label:pty > parent; pass_rate@label:git >= parent+5%; pass_rate@label:zzz >= parent; \
                cost_usd@label:x < parent; pass_rate@label:pty >= parent+1%";
    let v = validate_message(&p, &with(ChangeType::Feat, |tr| set(tr, "Predicts", pred))).unwrap();
    assert_eq!(
        v.capability_labels,
        vec!["git".to_string(), "pty".to_string()]
    );
}

#[test]
fn subject_errors_still_report_trailer_problems() {
    let h = Home::new("subject");
    let p = parser(&h);
    let msg = with(ChangeType::Feat, |tr| tr.retain(|(k, _)| *k != "Why"))
        .replace("feat: change", "feature: change");
    let r = validate_message(&p, &msg);
    // The type is unknown, so the matrix cannot run, but every other step still reports.
    assert_eq!(codes(&r), vec![codes::SUBJECT]);
    let msg = with(ChangeType::Feat, |tr| tr.retain(|(k, _)| *k != "Why")).replace(
        "feat: change something",
        &format!("feat: {}", "x".repeat(80)),
    );
    assert_eq!(
        codes(&validate_message(&p, &msg)),
        vec![codes::SUBJECT_TOO_LONG.to_string(), codes::missing("Why")]
    );
}

// ---- scratch lineage repos (GitCliRepo) ----

/// A scratch repo under a test home, built with an env-cleared git and the agent identity.
struct Scratch {
    home: Home,
    work: PathBuf,
    init: PathBuf,
    parser: GitTrailerParser,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let home = Home::new(name);
        let work = home.path().join("work");
        let init = home.path().join("init");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&init).unwrap();
        let parser = parser(&home);
        let s = Scratch {
            home,
            work,
            init,
            parser,
        };
        s.git(&["init", "-q"]);
        s
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new(GIT)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home.path())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "INIT Agent")
            .env("GIT_AUTHOR_EMAIL", "agent@init.local")
            .env("GIT_COMMITTER_NAME", "INIT Agent")
            .env("GIT_COMMITTER_EMAIL", "agent@init.local")
            .env("GIT_AUTHOR_DATE", "2026-10-20T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-10-20T00:00:00Z")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args([
                "-c",
                "user.useConfigOnly=true",
                "-c",
                "user.name=INIT Agent",
                "-c",
                "user.email=agent@init.local",
            ])
            .current_dir(&self.work)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.work.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn rm(&self, rel: &str) {
        fs::remove_file(self.work.join(rel)).unwrap();
    }

    fn msg_file(&self, msg: &str) -> String {
        let f = self.home.path().join("msg.txt");
        fs::write(&f, msg).unwrap();
        f.to_string_lossy().into_owned()
    }

    fn commit(&self, msg: &str) -> String {
        let f = self.msg_file(msg);
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "--allow-empty",
            "--cleanup=verbatim",
            "-F",
            &f,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn checkout(&self, rev: &str) {
        self.git(&["checkout", "-q", "--detach", rev]);
    }

    fn commit_tree(&self, tree_of: &str, parents: &[&str], msg: &str) -> String {
        let f = self.msg_file(msg);
        let tree = format!("{tree_of}^{{tree}}");
        let mut args = vec!["commit-tree", tree.as_str()];
        for p in parents {
            args.extend(["-p", p]);
        }
        args.extend(["-F", f.as_str()]);
        self.git(&args)
    }

    fn repo(&self) -> GitCliRepo {
        let mut r = GitCliRepo::new(GIT, &self.init);
        r.git_dir = self.work.join(".git");
        r
    }

    fn validate(&self, parent: &str, candidate: &str) -> Result<Validated, Vec<ValidationError>> {
        validate(&self.repo(), &self.parser, parent, candidate)
    }

    fn mkdir_state(&self, rel: &str) {
        fs::create_dir_all(self.init.join(rel)).unwrap();
    }

    /// A seed commit with source, skills, two extensions and a `self.json`.
    fn seeded(name: &str) -> (Scratch, String) {
        let s = Scratch::new(name);
        s.write("README.md", "seed\n");
        s.write(
            "src/lib.rs",
            &(1..=20)
                .map(|i| format!("// line {i}\n"))
                .collect::<String>(),
        );
        s.write("skills/a/SKILL.md", "a skill\n");
        s.write("SELF.md", "self\n");
        s.write(
            "ext/old/src/lib.rs",
            &(1..=40)
                .map(|i| format!("// old {i}\n"))
                .collect::<String>(),
        );
        s.write(
            "ext/thinme/src/lib.rs",
            &(1..=30)
                .map(|i| format!("// thin {i}\n"))
                .collect::<String>(),
        );
        s.write(
            "self.json",
            &self_json(&[
                ("ext/old", "none"),
                ("ext/thinme", "loop-detection"),
                ("tool/bash", "none"),
            ]),
        );
        let base = s.commit("upstream: seed v0.2.0\n");
        (s, base)
    }
}

fn self_json(components: &[(&str, &str)]) -> String {
    let comps: Vec<serde_json::Value> = components
        .iter()
        .map(|(id, tag)| serde_json::json!({"id": id, "compensates_for": tag, "core": false, "tokens": 10}))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({"self_v": 2, "components": comps})).unwrap()
        + "\n"
}

#[test]
fn scratch_two_agent_commits_are_not_single() {
    let (s, base) = Scratch::seeded("two-commits");
    s.write("src/a.rs", "a\n");
    let c1 = s.commit(&with(ChangeType::Feat, |_| {}));
    s.write("src/b.rs", "b\n");
    let c2 = s.commit(&with(ChangeType::Feat, |_| {}));
    let r = s.validate(&base, &c2);
    assert_eq!(codes(&r), vec![codes::NOT_SINGLE_COMMIT]);
    let v = s.validate(&base, &c1).expect("one commit is fine");
    assert_eq!(v.changed_paths, vec!["src/a.rs".to_string()]);
    // A merge of a non-upstream type is not single either.
    let m = s.commit_tree(&c1, &[&base, &c1], &with(ChangeType::Feat, |_| {}));
    assert_eq!(
        codes(&s.validate(&base, &m)),
        vec![codes::NOT_SINGLE_COMMIT]
    );
}

#[test]
fn scratch_upstream_shape_and_tags() {
    let (s, base) = Scratch::seeded("upstream");
    s.write("NEWS.md", "0.3.0\n");
    let release = s.commit("seed: release 0.3.0\n");
    s.checkout(&base);
    s.write("other.md", "x\n");
    let other = s.commit("other: not the release\n");
    s.checkout(&base);
    s.git(&["update-ref", "refs/upstream/seed-v0.3.0", &release]);
    let msg = |value: String| with(ChangeType::Upstream, |tr| set(tr, "Upstream", &value));

    let good = s.commit_tree(
        &base,
        &[&base, &release],
        &msg(format!("seed-v0.3.0 {release} declined")),
    );
    let v = s.validate(&base, &good).expect("a declined upstream merge");
    assert!(v.trailers.upstream.as_ref().unwrap().declined);
    assert!(v.changed_paths.is_empty());

    let wrong2 = s.commit_tree(
        &base,
        &[&base, &other],
        &msg(format!("seed-v0.3.0 {release}")),
    );
    assert_eq!(
        codes(&s.validate(&base, &wrong2)),
        vec![codes::UPSTREAM_SHAPE]
    );

    let wrong_sha = s.commit_tree(
        &base,
        &[&base, &release],
        &msg(format!("seed-v0.3.0 {other}")),
    );
    assert_eq!(
        codes(&s.validate(&base, &wrong_sha)),
        vec![codes::UPSTREAM_SHAPE]
    );

    let single = s.commit_tree(&base, &[&base], &msg(format!("seed-v0.3.0 {release}")));
    assert_eq!(
        codes(&s.validate(&base, &single)),
        vec![codes::UPSTREAM_SHAPE]
    );

    let missing = s.commit_tree(
        &base,
        &[&base, &release],
        &msg(format!("seed-v0.9.0 {release}")),
    );
    assert_eq!(
        codes(&s.validate(&base, &missing)),
        vec![codes::UPSTREAM_UNKNOWN_TAG]
    );
}

#[test]
fn scratch_skill_paths() {
    let (s, base) = Scratch::seeded("skill");
    s.write("skills/a/SKILL.md", "a better skill\n");
    s.write("src/lib.rs", "// changed\n");
    let bad = s.commit(&with(ChangeType::Skill, |_| {}));
    let r = s.validate(&base, &bad);
    assert_eq!(codes(&r), vec![codes::SKILL_PATHS]);
    assert!(r.unwrap_err()[0].detail.contains("src/lib.rs"));

    s.checkout(&base);
    s.write("skills/b/SKILL.md", "b\n");
    s.write("SELF.md", "self, revised\n");
    s.write("GENESIS.md", "genesis\n");
    let good = s.commit(&with(ChangeType::Skill, |_| {}));
    let v = s.validate(&base, &good).unwrap();
    assert_eq!(
        v.changed_paths,
        vec!["GENESIS.md", "SELF.md", "skills/b/SKILL.md"]
    );
}

#[test]
fn scratch_spec_example_end_to_end() {
    let (s, base) = Scratch::seeded("example");
    s.write("ext/pty/src/lib.rs", "// pty\n");
    s.write("ext/pty/Cargo.toml", "[package]\n");
    let c = s.commit(SPEC_EXAMPLE);
    assert_eq!(codes(&s.validate(&base, &c)), vec![codes::REPRO_NOT_FOUND]);
    s.mkdir_state("vault/gate/pool/2026-10-20-03");
    let v = s
        .validate(&base, &c)
        .expect("validates once the pool task exists");
    assert_eq!(v.sha, c);
    assert_eq!(v.parent, base);
    assert_eq!(
        v.changed_paths,
        vec!["ext/pty/Cargo.toml", "ext/pty/src/lib.rs"]
    );
    let json = v.to_sorted_json();
    assert!(
        json.starts_with(
            "{\"capability_labels\":[\"interactive\"],\"change_type\":\"feat\",\"changed_paths\":"
        ),
        "{json}"
    );
    assert!(
        json.contains("\"compensates_for\":{\"note\":\"new capability\",\"tag\":\"none\"}"),
        "{json}"
    );
    let back: Validated = serde_json::from_str(&json).unwrap();
    assert_eq!(back, v);
    // The parent may be given as a ref; it resolves to the same SHA.
    s.git(&["update-ref", "refs/heads/lineage-main", &base]);
    assert_eq!(
        s.validate("refs/heads/lineage-main", &c).unwrap().parent,
        base
    );
}

#[test]
fn scratch_repro_refs_exist() {
    let (s, base) = Scratch::seeded("repro");
    let hex = "e4f1a2b3c4d5e6f7";
    let trace = "0123456789abcdef";
    s.write("src/lib.rs", "// fixed\n");
    let refs = format!("task:t-9, event:sid-1#{hex}, trace:{trace}");
    let c = s.commit(&with(ChangeType::Fix, |tr| set(tr, "Repro", &refs)));
    s.mkdir_state("state/tasks/t-9");
    s.mkdir_state("state/sessions");
    fs::write(
        s.init.join("state/sessions/sid-1.jsonl"),
        format!("{{\"id\":\"0000000000000001\",\"type\":\"boot\"}}\nnot json\n{{\"id\":\"{hex}\",\"type\":\"tool_result\"}}\n"),
    )
    .unwrap();
    let r = s.validate(&base, &c);
    assert_eq!(
        codes(&r),
        vec![codes::REPRO_NOT_FOUND],
        "trace lookups default to not found"
    );
    assert!(r.unwrap_err()[0].detail.contains(trace));

    let repo = s.repo().with_trace_lookup(Box::new(move |id: &str| {
        Ok::<bool, std::io::Error>(id == trace)
    }));
    assert!(validate(&repo, &s.parser, &base, &c).is_ok());

    // A symlinked session log is not read (K-F2), and a missing task is not found.
    fs::rename(
        s.init.join("state/sessions/sid-1.jsonl"),
        s.init.join("state/sessions/real.jsonl"),
    )
    .unwrap();
    std::os::unix::fs::symlink("real.jsonl", s.init.join("state/sessions/sid-1.jsonl")).unwrap();
    fs::remove_dir(s.init.join("state/tasks/t-9")).unwrap();
    let repo = s.repo().with_trace_lookup(Box::new(move |id: &str| {
        Ok::<bool, std::io::Error>(id == trace)
    }));
    assert_eq!(
        codes(&validate(&repo, &s.parser, &base, &c)),
        vec![codes::REPRO_NOT_FOUND, codes::REPRO_NOT_FOUND]
    );
}

#[test]
fn scratch_retire_shape_and_components() {
    let (s, base) = Scratch::seeded("retire");
    let retire = |ids: &[&str]| {
        with(ChangeType::Retire, |tr| {
            tr.retain(|(k, _)| *k != "Retires");
            for id in ids {
                tr.push(("Retires", id.to_string()));
            }
        })
    };
    s.rm("ext/old/src/lib.rs");
    s.write(
        "self.json",
        &self_json(&[("ext/thinme", "loop-detection"), ("tool/bash", "none")]),
    );
    let good = s.commit(&retire(&["ext/old"]));
    let v = s.validate(&base, &good).expect("a deletion-shaped retire");
    assert_eq!(v.trailers.retires, vec!["ext/old".to_string()]);

    let unknown = s.commit_tree(&good, &[&base], &retire(&["ext/old", "ext/never"]));
    assert_eq!(
        codes(&s.validate(&base, &unknown)),
        vec![codes::RETIRE_UNKNOWN_COMPONENT]
    );

    s.checkout(&base);
    s.rm("ext/old/src/lib.rs");
    s.write("ext/new.rs", "// a new file\n");
    let new_file = s.commit(&retire(&["ext/old"]));
    assert_eq!(
        codes(&s.validate(&base, &new_file)),
        vec![codes::RETIRE_NOT_DELETION]
    );

    s.checkout(&base);
    s.rm("ext/old/src/lib.rs");
    s.write(
        "src/lib.rs",
        &(1..=40)
            .map(|i| format!("// grown {i}\n"))
            .collect::<String>(),
    );
    let grown = s.commit(&retire(&["ext/old"]));
    let r = s.validate(&base, &grown);
    assert_eq!(codes(&r), vec![codes::RETIRE_NOT_DELETION]);
    assert!(r.unwrap_err()[0].detail.contains("40 lines added"));

    // ADR-011 §6: a retire may move the cap to archive/ or quarantine/ (inert) instead of deleting it.
    s.checkout(&base);
    let old = fs::read_to_string(s.work.join("ext/old/src/lib.rs")).unwrap();
    s.rm("ext/old/src/lib.rs");
    s.write("archive/ext/old/src/lib.rs", &(old.clone() + &(1..=40).map(|i| format!("// kept {i}\n")).collect::<String>()));
    let moved = s.commit(&retire(&["ext/old"]));
    s.validate(&base, &moved).expect("a move to archive/ is deletion-shaped");
    s.checkout(&base);
    s.rm("ext/old/src/lib.rs");
    s.write("quarantine/ext/old/src/lib.rs", &old);
    let quarantined = s.commit(&retire(&["ext/old"]));
    s.validate(&base, &quarantined).expect("a move to quarantine/ is deletion-shaped");
    // Growing a live path is still refused.
    s.write("ext/other.rs", "// new\n");
    let mixed = s.commit_tree(&s.commit(&retire(&["ext/old"])), &[&base], &retire(&["ext/old"]));
    assert_eq!(codes(&s.validate(&base, &mixed)), vec![codes::RETIRE_NOT_DELETION]);
}

#[test]
fn scratch_thin_window_and_tag() {
    let (s, base) = Scratch::seeded("thin");
    s.rm("ext/thinme/src/lib.rs");
    let wrong_tag = s.commit(&with(ChangeType::Thin, |tr| {
        set(tr, "Thins", "ext/thinme");
        set(tr, "Compensates-for", "none");
    }));
    assert_eq!(
        codes(&s.validate(&base, &wrong_tag)),
        vec![codes::THIN_TAG_MISMATCH, codes::THIN_NO_WINDOW]
    );

    let good = s.commit_tree(
        &wrong_tag,
        &[&base],
        &with(ChangeType::Thin, |tr| set(tr, "Thins", "ext/thinme")),
    );
    assert_eq!(
        codes(&s.validate(&base, &good)),
        vec![codes::THIN_NO_WINDOW]
    );
    s.mkdir_state("state/kernel");
    fs::write(
        s.init.join("state/kernel/thinning.json"),
        "{\"state\":\"thinning_baseline\"}",
    )
    .unwrap();
    assert_eq!(
        codes(&s.validate(&base, &good)),
        vec![codes::THIN_NO_WINDOW]
    );
    fs::write(
        s.init.join("state/kernel/thinning.json"),
        "{\"candidates\":[],\"state\":\"thinning\"}",
    )
    .unwrap();
    assert!(s.validate(&base, &good).is_ok());
}

#[test]
fn scratch_revert_tree_check() {
    let (s, base) = Scratch::seeded("revert");
    s.write("src/lib.rs", "// rewritten by A\n");
    let a = s.commit("feat: A\n");
    s.write("docs.md", "later change\n");
    let parent = s.commit("feat: later\n");

    s.git(&["revert", "--no-commit", &a]);
    let good = s.commit(&with(ChangeType::Revert, |tr| set(tr, "Reverts", &a)));
    let v = s
        .validate(&parent, &good)
        .expect("the tree equals git revert");
    assert_eq!(v.trailers.reverts.as_deref(), Some(a.as_str()));
    assert!(
        !s.init.join("cache/revert-check").exists(),
        "the throwaway worktree is removed"
    );

    s.checkout(&parent);
    s.git(&["revert", "--no-commit", &a]);
    s.write("extra.md", "smuggled\n");
    let bad = s.commit(&with(ChangeType::Revert, |tr| set(tr, "Reverts", &a)));
    assert_eq!(
        codes(&s.validate(&parent, &bad)),
        vec![codes::REVERT_MISMATCH]
    );

    s.checkout(&base);
    s.write("side.md", "side\n");
    let side = s.commit("feat: side\n");
    let not_ancestor = s.commit_tree(
        &good,
        &[&parent],
        &with(ChangeType::Revert, |tr| set(tr, "Reverts", &side)),
    );
    assert_eq!(
        codes(&s.validate(&parent, &not_ancestor)),
        vec![codes::REVERT_MISMATCH]
    );
    let unknown = s.commit_tree(
        &good,
        &[&parent],
        &with(ChangeType::Revert, |tr| set(tr, "Reverts", SHA_B)),
    );
    assert_eq!(
        codes(&s.validate(&parent, &unknown)),
        vec![codes::REVERT_MISMATCH]
    );
    assert!(!s.init.join("cache/revert-check").exists());
}

#[test]
fn scratch_unknown_revisions_are_internal_errors() {
    let (s, base) = Scratch::seeded("unknown");
    let r = s.validate(&base, SHA_A);
    let errs = r.unwrap_err();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].is_internal(), "{errs:?}");
}

#[test]
fn scratch_run_and_friction_refs_exist() {
    let (s, base) = Scratch::seeded("engine-refs");
    s.write("src/lib.rs", "// fixed\n");
    let refs = "run:j-000031/p-repo-03-r1, friction:f-000003";
    let c = s.commit(&with(ChangeType::Fix, |tr| set(tr, "Repro", refs)));
    assert_eq!(
        codes(&s.validate(&base, &c)),
        vec![codes::REPRO_NOT_FOUND, codes::REPRO_NOT_FOUND]
    );

    s.mkdir_state("vault/gate/jobs/j-000031");
    s.mkdir_state("state/kernel/evo");
    let runs = s.init.join("vault/gate/jobs/j-000031/runs.jsonl");
    let frictions = s.init.join("state/kernel/evo/frictions.jsonl");
    // Other runs, a bad line, and the wanted run only as a torn last line: not found.
    fs::write(
        &runs,
        "{\"v\":2,\"run_id\":\"c-repo-03-r1\",\"score_permille\":0}\nnot json\n{\"v\":2,\"run_id\":\"p-repo-03-r1\",\"sco",
    )
    .unwrap();
    fs::write(&frictions, "{\"id\":\"f-000002\"}\n{\"id\":\"f-00000").unwrap();
    let r = s.validate(&base, &c);
    assert_eq!(
        codes(&r),
        vec![codes::REPRO_NOT_FOUND, codes::REPRO_NOT_FOUND]
    );
    let errs = r.unwrap_err();
    assert!(
        errs[0].detail.contains("run:j-000031/p-repo-03-r1"),
        "{errs:?}"
    );
    assert!(errs[1].detail.contains("friction:f-000003"), "{errs:?}");

    // Whole lines, followed by a torn one: found.
    fs::write(
        &runs,
        "{\"v\":2,\"run_id\":\"c-repo-03-r1\"}\n{\"v\":2,\"run_id\":\"p-repo-03-r1\",\"score_permille\":0}\n{\"v\":2,\"run_",
    )
    .unwrap();
    fs::write(
        &frictions,
        "{\"id\":\"f-000002\"}\n{\"id\":\"f-000003\",\"eligible\":true}\n{\"id\":",
    )
    .unwrap();
    let v = s.validate(&base, &c).expect("both refs exist");
    assert_eq!(v.trailers.repro.len(), 2);

    // The run must be in the named gate's runs.jsonl.
    s.checkout(&base);
    s.write("src/lib.rs", "// fixed again\n");
    let other_gate = s.commit(&with(ChangeType::Fix, |tr| {
        set(tr, "Repro", "run:j-000032/p-repo-03-r1, friction:f-000003")
    }));
    assert_eq!(
        codes(&s.validate(&base, &other_gate)),
        vec![codes::REPRO_NOT_FOUND]
    );

    // Symlinked index files are not read (K-F2).
    fs::rename(&runs, s.init.join("vault/gate/jobs/j-000031/real.jsonl")).unwrap();
    std::os::unix::fs::symlink("real.jsonl", &runs).unwrap();
    fs::rename(&frictions, s.init.join("state/kernel/evo/real.jsonl")).unwrap();
    std::os::unix::fs::symlink("real.jsonl", &frictions).unwrap();
    assert_eq!(
        codes(&s.validate(&base, &c)),
        vec![codes::REPRO_NOT_FOUND, codes::REPRO_NOT_FOUND]
    );
}

#[test]
fn scratch_proposal_must_be_screened_for_this_candidate() {
    let (s, base) = Scratch::seeded("proposal");
    s.write("src/lib.rs", "// engine-built\n");
    let c = s.commit(&with(ChangeType::Feat, |tr| {
        tr.push(("Proposal", "p-000007, p-000008".into()))
    }));
    let r = s.validate(&base, &c);
    assert_eq!(
        codes(&r),
        vec![codes::PROPOSAL_NOT_FOUND, codes::PROPOSAL_NOT_FOUND]
    );
    assert!(r.unwrap_err()[1].detail.contains("p-000008"));

    s.mkdir_state("state/kernel/evo/proposals");
    let path = |id: &str| s.init.join(format!("state/kernel/evo/proposals/{id}.json"));
    let write = |id: &str, state: &str, chosen: serde_json::Value| {
        let rec = serde_json::json!({"v": 1, "id": id, "scope": "inv", "state": state,
                                     "chosen": chosen, "k": 3, "est": null});
        fs::write(path(id), rec.to_string()).unwrap();
    };
    let chosen = |sha: &str| serde_json::json!({"k": 1, "sha": sha, "aim": "better"});
    write("p-000007", "screened", chosen(&c));
    write("p-000008", "variants", serde_json::Value::Null);
    let r = s.validate(&base, &c);
    assert_eq!(codes(&r), vec![codes::PROPOSAL_STATE]);
    let d = r.unwrap_err()[0].detail.clone();
    assert!(
        d.contains("p-000008") && d.contains("'variants'") && d.contains("no chosen variant"),
        "{d}"
    );

    write("p-000008", "screened", chosen(&base));
    let r = s.validate(&base, &c);
    assert_eq!(codes(&r), vec![codes::PROPOSAL_STATE]);
    let d = r.unwrap_err()[0].detail.clone();
    assert!(
        d.contains("not the candidate") && !d.contains("state"),
        "{d}"
    );

    write("p-000008", "screened", chosen("abc123"));
    let r = s.validate(&base, &c);
    assert!(
        r.unwrap_err()[0].detail.contains("not a 40-hex commit id"),
        "a short sha is named as such"
    );

    write("p-000008", "screened", chosen(&c.to_ascii_uppercase()));
    let v = s
        .validate(&base, &c)
        .expect("both proposals are screened with this candidate chosen");
    assert_eq!(v.trailers.proposals, vec!["p-000007", "p-000008"]);
    let json = v.to_sorted_json();
    assert!(
        json.contains("\"proposals\":[\"p-000007\",\"p-000008\"]"),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Validated>(&json).unwrap(), v);

    // A later state is not `screened` either.
    write("p-000007", "promoted", chosen(&c));
    let r = s.validate(&base, &c);
    assert_eq!(codes(&r), vec![codes::PROPOSAL_STATE]);
    assert!(r.unwrap_err()[0].detail.contains("'promoted'"));
    write("p-000007", "screened", chosen(&c));

    // A symlinked record is not read (K-F2): not found.
    fs::rename(path("p-000007"), path("real")).unwrap();
    std::os::unix::fs::symlink("real.json", path("p-000007")).unwrap();
    assert_eq!(
        codes(&s.validate(&base, &c)),
        vec![codes::PROPOSAL_NOT_FOUND]
    );
    fs::remove_file(path("p-000007")).unwrap();
    fs::rename(path("real"), path("p-000007")).unwrap();
    assert!(s.validate(&base, &c).is_ok());

    // A malformed or oversized record is the kernel's problem (evo.io), not the agent's.
    for content in [
        "{\"state\":".to_string(),
        "{\"chosen\":null}".to_string(),
        format!(
            "{{\"state\":\"screened\",\"pad\":\"{}\"}}",
            "x".repeat(1 << 20)
        ),
    ] {
        fs::write(path("p-000008"), content).unwrap();
        let errs = s.validate(&base, &c).unwrap_err();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].is_internal(), "{errs:?}");
    }
}

#[test]
fn proposal_lookup_defaults_to_none() {
    /// An [`EvoRepo`] that implements only the required methods.
    struct Bare;
    impl EvoRepo for Bare {
        fn resolve_commit(&self, _: &str) -> std::io::Result<Option<String>> {
            unreachable!()
        }
        fn parents(&self, _: &str) -> std::io::Result<Vec<String>> {
            unreachable!()
        }
        fn message(&self, _: &str) -> std::io::Result<Vec<u8>> {
            unreachable!()
        }
        fn changed_paths(&self, _: &str, _: &str) -> std::io::Result<Vec<String>> {
            unreachable!()
        }
        fn added_paths(&self, _: &str, _: &str) -> std::io::Result<Vec<String>> {
            unreachable!()
        }
        fn numstat(&self, _: &str, _: &str) -> std::io::Result<Vec<NumstatEntry>> {
            unreachable!()
        }
        fn upstream_tag_commit(&self, _: &str) -> std::io::Result<Option<String>> {
            unreachable!()
        }
        fn is_ancestor(&self, _: &str, _: &str) -> std::io::Result<bool> {
            unreachable!()
        }
        fn revert_tree_matches(&self, _: &str, _: &str, _: &str) -> std::io::Result<bool> {
            unreachable!()
        }
        fn parent_components(
            &self,
            _: &str,
        ) -> std::io::Result<Option<std::collections::BTreeMap<String, String>>> {
            unreachable!()
        }
        fn repro_exists(&self, _: &ReproRef) -> std::io::Result<bool> {
            unreachable!()
        }
        fn thinning_window_open(&self) -> std::io::Result<bool> {
            unreachable!()
        }
    }
    assert_eq!(Bare.proposal("p-000001").unwrap(), None);
    // GitCliRepo rejects a non-id without touching the file system.
    let h = Home::new("proposal-id");
    let repo = GitCliRepo::new(GIT, h.path());
    for bad in ["../x", "p-1", "p-000001.json"] {
        assert_eq!(repo.proposal(bad).unwrap(), None, "{bad}");
    }
}

// ---- ADR-011 X1: the free path's grammar (RF-06) ----

fn free(p: &GitTrailerParser, msg: &str) -> Result<ValidatedMessage, Vec<ValidationError>> {
    validate_message_in(p, msg, ExtendMode::Free)
}

/// RF-06 (golden parse table): `Cap:` and `Measure:` values, their errors, de-duplication, and the
/// records' JSON (empty lists are omitted, so v0.3 records keep their exact form).
#[test]
fn rf06_golden_parse_table() {
    let h = Home::new("rf06-golden");
    let p = parser(&h);
    type Row = (&'static [(&'static str, &'static str)], &'static [&'static str], &'static [&'static str], &'static [&'static str]);
    // (extra trailers on a minimal free feat, expected caps, expected measures, expected error codes)
    let table: &[Row] = &[
        (&[], &[], &[], &[]),
        (&[("Cap", "ext/pty")], &["ext/pty"], &[], &[]),
        (&[("Cap", "ext/pty"), ("Cap", "skill/pty-howto"), ("Cap", "ext/pty")], &["ext/pty", "skill/pty-howto"], &[], &[]),
        (&[("Cap", "agent/reviewer"), ("Cap", "prompt/SELF.md"), ("Cap", "mem/episodic"), ("Cap", "mod/src/plan.rs")],
            &["agent/reviewer", "prompt/SELF.md", "mem/episodic", "mod/src/plan.rs"], &[], &[]),
        (&[("Cap", "Ext/pty")], &[], &[], &["trailer.invalid:Cap"]),
        (&[("Cap", "widget/x")], &[], &[], &["trailer.invalid:Cap"]),
        (&[("Cap", "mod/../etc")], &[], &[], &["trailer.invalid:Cap"]),
        (&[("Measure", "g4")], &[], &["g4"], &[]),
        (&[("Measure", "g4 gate")], &[], &["g4 gate"], &[]),
        (&[("Measure", "gog")], &[], &["gog"], &[]),
        (&[("Measure", "exp:semver-sort"), ("Measure", "g4"), ("Measure", "exp:semver-sort")], &[], &["exp:semver-sort", "g4"], &[]),
        (&[("Measure", "g5")], &[], &[], &["trailer.invalid:Measure"]),
        (&[("Measure", "exp:a/b")], &[], &[], &["trailer.invalid:Measure"]),
        (&[("Measure", "G4")], &[], &[], &["trailer.invalid:Measure"]),
        (&[("Measure", "card:cost_turn"), ("Measure", "card:cache_read gate")], &[], &["card:cost_turn", "card:cache_read gate"], &[]),
        (&[("Measure", "card:Cost")], &[], &[], &["trailer.invalid:Measure"]),
        (&[("Measure", "g4"), ("Measure", "gog"), ("Measure", "g4 gate"), ("Measure", "exp:a"), ("Measure", "exp:b")], &[], &[], &["trailer.invalid:Measure"]),
    ];
    for (extra, caps, measures, errs) in table {
        let mut tr: Vec<(&'static str, String)> = vec![("Why", "a reason that is long enough".into()), ("Prefix-change", "no".into())];
        tr.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        let msg = render("feat(ext): add pty", &tr);
        for mode in [ExtendMode::Free, ExtendMode::Gated] {
            let r = validate_message_in(&p, &msg, mode);
            let got: Vec<String> = codes(&r).into_iter().filter(|c| !c.starts_with("trailer.missing:")).collect();
            assert_eq!(got, errs.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{mode:?} {extra:?}");
        }
        let r = free(&p, &msg);
        if errs.is_empty() {
            let v = r.unwrap();
            assert_eq!(v.trailers.caps, caps.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{extra:?}");
            let m: Vec<String> = v.trailers.measure.iter().map(|m| m.to_string()).collect();
            assert_eq!(m, measures.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{extra:?}");
            let j = serde_json::to_value(&v.trailers).unwrap();
            assert_eq!(j.get("caps").is_some(), !caps.is_empty(), "{j}");
            assert_eq!(j.get("measure").is_some(), !measures.is_empty(), "{j}");
            let back: Trailers = serde_json::from_value(j).unwrap();
            assert_eq!(back, v.trailers, "round trip");
        }
    }
    // Neither key is "unknown": they never land in `other`.
    let v = free(&p, &render("feat: x", &[("Why", "a reason that is long enough".into()), ("Prefix-change", "no".into()), ("Cap", "tool/pty".into())])).unwrap();
    assert!(v.trailers.other.is_empty());
    // ExtendMode spelling: only "free" is free.
    assert_eq!((ExtendMode::parse(Some("free")), ExtendMode::parse(Some("gated")), ExtendMode::parse(None), ExtendMode::parse(Some("FREE"))),
        (ExtendMode::Free, ExtendMode::Gated, ExtendMode::Gated, ExtendMode::Gated));
}

/// RF-06: every former mandatory-trailer case passes on `free` and fails on `gated`.
#[test]
fn rf06_former_mandatory_cases_pass_free_fail_gated() {
    let h = Home::new("rf06-cases");
    let p = parser(&h);
    let mut cases: Vec<(String, String, String)> = Vec::new(); // (name, message, gated code)
    for t in ChangeType::ALL {
        for key in ["Predicts", "Compensates-for", "Repro"] {
            if need(t, key) != Need::Mandatory {
                continue;
            }
            let base = minimal(t);
            let without: Vec<_> = base.iter().filter(|(k, _)| *k != key).cloned().collect();
            cases.push((format!("{t:?} without {key}"), render(&subject(t), &without), codes::missing(key)));
        }
        // The smallest free commit: Why, Prefix-change and the type's identity key.
        let bare: Vec<_> = minimal(t).into_iter().filter(|(k, _)| !["Predicts", "Compensates-for", "Repro"].contains(k)).collect();
        cases.push((format!("{t:?} bare"), render(&subject(t), &bare), codes::missing("Predicts")));
    }
    // The prefix rule (C¹): Prefix-change: yes with no Repro.
    for t in [ChangeType::Feat, ChangeType::Perf, ChangeType::Refactor, ChangeType::Skill] {
        let msg = with(t, |tr| set(tr, "Prefix-change", "yes"));
        cases.push((format!("{t:?} prefix without Repro"), msg, codes::REPRO_REQUIRED_FOR_PREFIX.into()));
    }
    // A baseline-only (trivial) Predicts on the types that need a real one.
    for t in [ChangeType::Feat, ChangeType::Fix, ChangeType::Guard] {
        let msg = with(t, |tr| set(tr, "Predicts", "pass_rate >= parent"));
        cases.push((format!("{t:?} trivial Predicts"), msg, codes::PREDICTS_TRIVIAL_ONLY.into()));
    }
    cases.push(("Perf without a strict clause".into(), with(ChangeType::Perf, |tr| set(tr, "Predicts", "pass_rate > parent")), codes::PREDICTS_TRIVIAL_ONLY.into()));
    // A guard that names no weakness.
    cases.push(("Guard Compensates-for none".into(), with(ChangeType::Guard, |tr| set(tr, "Compensates-for", "none")), codes::invalid("Compensates-for")));
    assert!(cases.len() >= 20, "{}", cases.len());
    for (name, msg, code) in &cases {
        let g = validate_message(&p, msg);
        assert!(has(&g, code), "{name}: gated must fail with {code}, got {:?}", codes(&g));
        let f = free(&p, msg);
        assert!(f.is_ok(), "{name}: free must pass, got {:?}", codes(&f));
    }
    // What stays mandatory on both paths: Why and Prefix-change; and grammar of present values.
    for key in ["Why", "Prefix-change"] {
        let msg = render("feat: x", &minimal(ChangeType::Feat).into_iter().filter(|(k, _)| *k != key).collect::<Vec<_>>());
        for mode in [ExtendMode::Free, ExtendMode::Gated] {
            assert!(has(&validate_message_in(&p, &msg, mode), &codes::missing(key)), "{mode:?} without {key}");
        }
    }
    let bad_predicts = with(ChangeType::Feat, |tr| set(tr, "Predicts", "pass_rate >> parent"));
    assert!(has(&free(&p, &bad_predicts), codes::PREDICTS_PARSE));
    let bad_repro = with(ChangeType::Fix, |tr| set(tr, "Repro", "task:a/b"));
    assert!(has(&free(&p, &bad_repro), &codes::invalid("Repro")));
    // A forbidden cell stays forbidden: Repro on retire.
    let retire_repro = with(ChangeType::Retire, |tr| tr.push(("Repro", "task:t-1".into())));
    assert!(has(&free(&p, &retire_repro), &codes::forbidden("Repro")));
    // The free matrix, cell by cell.
    for t in ChangeType::ALL {
        for key in KEYS {
            let (g, f) = (need(t, key), need_in(ExtendMode::Free, t, key));
            let want = match (key, g) {
                ("Predicts" | "Compensates-for" | "Repro", Need::Mandatory | Need::Conditional) => Need::Optional,
                _ => g,
            };
            assert_eq!(f, want, "{t:?} {key}");
        }
        assert_eq!(need(t, "Cap"), Need::Optional);
        assert_eq!(need(t, "Measure"), Need::Optional);
    }
}
