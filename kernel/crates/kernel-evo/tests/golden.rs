//! EV-01, EV-02 (evolution-v2 §8): the miner on the 6 S-REAL sessions with failures
//! (fixtures copied from `build/real-artifacts/real/`, the longest copy of each session) and on
//! E2E-2's silent write.
//!
//! Golden census (§3.1 W2): 12 failure events = 6 path errors in 4 sessions (2 `cd work/a1-calc`
//! from `work/`, 2 `body/skills/…` reads from `work/`, 1 ext `/work/work/…`, 1 guessed
//! `/Users/init/…`), 2 validator refusals (one returned with exit 0), 3 kernel/seed defects since
//! fixed, 1 expected red test (excluded).

use kernel_evo::classify::PathCtx;
use kernel_evo::mine::{self, MineCfg};
use kernel_evo::session;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const S_REAL_HOME: &str = "/Users/quanduong/Documents/INIT Agent/target/real-homes/r/ih";

fn fixtures() -> Vec<PathBuf> {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s-real");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

fn cx() -> PathCtx {
    PathCtx { cwd_rel: "work".into(), init_home: Some(PathBuf::from(S_REAL_HOME)), check_fs: false }
}

fn test_home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/EVO-LITE").join(format!("{name}-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn remove_home(d: &Path) {
    assert!(d.to_string_lossy().contains("/target/test-homes/EVO-LITE/"));
    let _ = std::fs::remove_dir_all(d);
}

fn category(signal: &str) -> &'static str {
    if signal.starts_with("path.") {
        "path"
    } else if signal.starts_with("initctl.refused:") {
        "refusal"
    } else if signal.starts_with("initctl.failed:") {
        "defect"
    } else if signal.starts_with("bash.") || signal.starts_with("tool.") || signal == "owner.interrupt" {
        "other_failure"
    } else {
        "run_level"
    }
}

#[test]
fn ev01_s_real_census() {
    let files = fixtures();
    assert_eq!(files.len(), 6, "the 6 S-REAL sessions with failures");
    let mut cats: BTreeMap<&str, Vec<(String, String, String)>> = BTreeMap::new();
    let mut expected = Vec::new();
    for f in &files {
        let p = session::parse(&std::fs::read(f).unwrap(), "x", &cx(), None);
        expected.extend(p.expected.clone());
        for i in p.instances.iter().filter(|i| i.failure) {
            cats.entry(category(&i.signal)).or_default().push((i.session.clone(), i.signal.clone(), i.reference.clone()));
        }
    }
    let path = &cats["path"];
    assert_eq!(path.len(), 6, "{path:#?}");
    let sessions: std::collections::BTreeSet<&String> = path.iter().map(|x| &x.0).collect();
    assert_eq!(sessions.len(), 4);
    let n = |s: &str| path.iter().filter(|x| x.1 == s).count();
    assert_eq!((n("path.doubled_prefix"), n("path.home_relative"), n("path.guessed_absolute")), (3, 2, 1), "{path:#?}");
    for id in ["bfa14a0cb6a6ddb9", "a78baa4a82213c46", "e5470011661ec349", "be230897960d0477", "c497bb8961a8f579", "50368e6371b4c8c1"] {
        assert!(path.iter().any(|x| x.2.ends_with(id)), "path event {id}: {path:#?}");
    }
    let refusals = &cats["refusal"];
    assert_eq!(refusals.len(), 2, "{refusals:#?}");
    assert!(refusals.iter().any(|x| x.2.ends_with("7ebd9d8c153b0a15") && x.1 == "initctl.refused:trailer.subject_too_long"));
    // The refusal that came back with exit 0 (is_error false): invisible to an is_error check.
    assert!(refusals.iter().any(|x| x.2.ends_with("ee1e29efbd417748") && x.1 == "initctl.refused:trailer.subject"));
    let defects = &cats["defect"];
    assert_eq!(defects.len(), 3, "{defects:#?}");
    let d: Vec<&str> = defects.iter().map(|x| x.1.as_str()).collect();
    assert!(d.contains(&"initctl.failed:E_FETCH") && d.contains(&"initctl.failed:gate.g0.forbid_missing") && d.contains(&"initctl.failed:gate.g0.test"), "{d:?}");
    assert_eq!(expected.len(), 1, "{expected:?}");
    assert!(expected[0].starts_with("20261020-063736-939a9b/"));
    assert!(!cats.contains_key("other_failure"), "{:#?}", cats.get("other_failure"));
    assert_eq!(path.len() + refusals.len() + defects.len() + expected.len(), 12);
}

#[test]
fn ev01_mine_index_on_fixtures() {
    let home = test_home("ev01");
    let cfg = MineCfg { path: cx(), imports: fixtures(), live_sessions: false, traces: false, pool: false };
    let out = mine::mine(&home, &cfg).unwrap();
    let idx = mine::read_index(&home);
    assert_eq!(idx, out.clusters);
    let c = idx.iter().find(|c| c.signature == "path.doubled_prefix").unwrap();
    assert_eq!((c.counts.instances, c.counts.sessions), (3, 3));
    assert!(c.id.starts_with("f-") && !c.eligible, "event: refs are supplementary only");
    assert!(c.refs.iter().all(|r| r.starts_with("event:")));
    let digest = mine::digest(&idx);
    assert!(digest.len() <= mine::DIGEST_MAX && digest.contains("path.doubled_prefix"), "{digest}");
    // A second mine changes nothing (cursors, stable ids).
    let again = mine::mine(&home, &cfg).unwrap();
    assert_eq!(again.parsed_sources, 0);
    assert_eq!(mine::read_index(&home), idx);
    remove_home(&home);
}

#[test]
fn ev02_silent_doubled_write() {
    let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/20261020-064859-a31ee0.jsonl");
    let p = session::parse(&std::fs::read(f).unwrap(), "x", &cx(), None);
    let d: Vec<&session::Instance> = p.instances.iter().filter(|i| i.signal == "path.doubled_prefix").collect();
    assert!(!d.is_empty(), "{:#?}", p.instances);
    assert!(d.iter().all(|i| !i.failure), "a silent write: no error event");
    assert!(d.iter().any(|i| i.detail.contains("work/e2e")), "{d:#?}");
}
