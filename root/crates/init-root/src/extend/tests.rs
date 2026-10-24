//! X5 unit tests ($0): cap ids, the call heuristic, `x-egress-caps`, the usage ledger and its digests, the cap
//! table fold, and the Root-evidence identity of a `t0` tar. The tap's RF-19, RF-20 and RF-21 are in
//! `egress/tap/calls.rs`, beside the reader they drive.

use super::caps::{Table, t0_caps, tar_files};
use super::usage::{self, Day};
use super::*;
use std::path::PathBuf;

fn home(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/x5").join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap(); // exact path, made by an earlier run of this test
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn turn(seq: u64, end: i64, caps: &[(&str, u32, &str)], harmed: bool) -> Turn {
    Turn { seq, kind: "owner".into(), start: end - 1000, end, correction: harmed, wall_ms: 1000, requests: 2,
        caps: caps.iter().map(|(id, n, ev)| (id.to_string(), (*n, ev.to_string()))).collect(), groups: vec![3], ..Turn::default() }
}

#[test]
fn cap_ids_follow_the_grammar() {
    for ok in ["skill/release-notes", "ext/semver-sort", "tool/bash", "mod/context/assemble.rs", "mem/vec_index", "dep/serde",
        "prompt/GENESIS.md", "prompt/SELF.md", "kernel/kernel-gate", "loop/plan2", "agent/reviewer"] {
        assert!(valid_id(ok), "{ok}");
    }
    let long = format!("skill/{}", "a".repeat(64));
    for bad in ["skill/", "skill/Foo", "skill/-x", "skill/a//b", "skill/a/../b", "skill/a/", "prompt/other.md", "hook/x", "tool",
        "ext/a@b.com", "skill//etc", long.as_str()] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[test]
fn calls_name_caps_at_component_boundaries() {
    assert_eq!(call_caps("ext", Some("semver-sort")), ["ext/semver-sort", "tool/ext"]);
    assert_eq!(call_caps("ext", Some("Bad Name")), ["tool/ext"]);
    assert_eq!(call_caps("read", Some("/h/.init/body/skills/a/SKILL.md")), ["skill/a", "tool/read"]);
    assert_eq!(call_caps("bash", Some("cat skills/a/x; ls ./agents/r/ \"skills/b/\" myskills/c/ skills/d skills/$X/ skills/a/y")),
        ["agent/r", "skill/a", "skill/b", "tool/bash"]);
    assert_eq!(call_caps("edit", Some("work/wt-1/skills/a/SKILL.md")), ["skill/a", "tool/edit"], "an evolution worktree counts");
    assert_eq!(call_caps("grep", Some("skills/a/")), ["tool/grep"], "only the five tools' arguments are read");
    assert_eq!(call_caps("Bash", None), Vec::<String>::new(), "an invalid tool name names nothing");
    assert_eq!(call_caps("read", None), ["tool/read"]);
}

#[test]
fn zone_header_is_bounded_and_root_wins() {
    assert_eq!(zone_caps(Some("mem/vec, loop/plan2,mem/vec,BAD,skill/x")), ["loop/plan2", "mem/vec", "skill/x"]);
    let many: Vec<String> = (0..17).map(|i| format!("mem/m{i}")).collect();
    assert!(zone_caps(Some(&many.join(","))).is_empty(), "over 16 ids: absent");
    assert_eq!(zone_caps(Some(&many[..16].join(","))).len(), 16);
    assert!(zone_caps(Some(&format!("mem/{}", "a,".repeat(600)))).is_empty(), "over 1 KiB: absent");
    assert!(zone_caps(None).is_empty());
    let calls: BTreeMap<String, u32> = [("tool/bash".to_string(), 2), ("skill/x".to_string(), 1)].into();
    assert_eq!(request_caps(calls, Some("skill/x,mem/vec")),
        [("mem/vec".to_string(), 1, "zone"), ("skill/x".to_string(), 1, "root"), ("tool/bash".to_string(), 2, "root")]);
}

#[test]
fn usage_lines_days_digests_and_restart() {
    let rh = home("usage");
    let e = Extend::default();
    let d0 = 1_792_000_000_000i64; // 2026-10-15T...Z
    assert!(e.turn(&rh, &turn(1, d0, &[("skill/a", 2, "root"), ("mem/v", 1, "zone")], false), "2026-10-15").unwrap().is_empty());
    assert!(e.turn(&rh, &turn(2, d0 + 1000, &[("skill/a", 1, "root")], true), "2026-10-15").unwrap().is_empty());
    let dir = rh.join("extend/usage");
    let file = dir.join(format!("{}.jsonl", &rfc3339(d0)[..10]));
    let lines: Vec<Value> = std::fs::read_to_string(&file).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[1], json!({"caps": [{"evidence": "root", "id": "skill/a", "uses": 1}], "cost_musd": 0, "day": "2026-10-15",
        "end": rfc3339(d0 + 1000), "groups": [3], "harm": ["correction"], "harmed": true, "kind": "owner", "req_bytes": 0, "requests": 2,
        "start": rfc3339(d0), "turn": 2, "v": 1, "wall_ms": 1000}));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(rh.join("extend")).unwrap().permissions().mode() & 0o777, 0o700);
    // a restart: a fresh state reads the undigested day back from the file, then the next day digests it
    let e2 = Extend::default();
    let ds = e2.turn(&rh, &turn(3, d0 + 86_400_000, &[("tool/bash", 1, "root")], false), "2026-10-16").unwrap();
    assert_eq!(ds, [json!({"caps": [{"evidence": "zone", "harm_turns": 0, "id": "mem/v", "last_turn_seq": 1, "turns": 1, "uses": 1},
        {"evidence": "root", "harm_turns": 1, "id": "skill/a", "last_turn_seq": 2, "turns": 2, "uses": 3}], "day": "2026-10-15", "part": 0})]);
    // the appended digest folds into the table; today's counters stay on top until digested
    e2.apply(&json!({"by": "root", "kind": "cap_use", "data": ds[0]}));
    let rows = e2.table();
    let a = rows.iter().find(|r| r.id == "skill/a").unwrap();
    assert_eq!((a.turns, a.uses, a.harm_turns, a.last_turn_seq, a.last_day.as_deref(), a.state.as_str()), (2, 3, 1, Some(2), Some("2026-10-15"), "observed"));
    assert_eq!(rows.iter().find(|r| r.id == "tool/bash").map(|r| (r.turns, r.last_turn_seq)), Some((1, Some(3))));
    // a third state after restart skips the digested day (no double count)
    let e3 = Extend::default();
    e3.apply(&json!({"by": "root", "kind": "cap_use", "data": ds[0]}));
    assert!(e3.turn(&rh, &turn(4, d0 + 86_401_000, &[], false), "2026-10-16").unwrap().is_empty());
    assert_eq!(e3.table().iter().find(|r| r.id == "skill/a").map(|r| r.uses), Some(3));
    // a refused digest goes back and is retried by the next day's turn
    e3.undigest(&json!({"caps": [{"evidence": "root", "harm_turns": 0, "id": "skill/z", "last_turn_seq": 9, "turns": 1, "uses": 4}], "day": "2026-10-16"}));
    let ds = e3.turn(&rh, &turn(5, d0 + 2 * 86_400_000, &[], false), "2026-10-17").unwrap();
    assert!(ds[0]["caps"].as_array().unwrap().iter().any(|c| c["id"] == "skill/z" && c["uses"] == 4), "{ds:?}");
}

#[test]
fn digests_keep_256_recent_caps_in_16k_parts() {
    let mut day = Day::new();
    for i in 0..300u64 {
        let l = json!({"caps": [{"evidence": "root", "id": format!("skill/a-rather-long-capability-name-for-size-{i:03}"), "uses": 1}], "harmed": false, "turn": i});
        usage::add(&mut day, &l);
    }
    let parts = usage::digest("2026-10-15", &day);
    let caps: Vec<&Value> = parts.iter().flat_map(|p| p["caps"].as_array().unwrap()).collect();
    assert_eq!(caps.len(), 256);
    assert!(parts.len() >= 2 && parts.iter().all(|p| icj(p).unwrap().len() <= 16 * 1024), "{}", parts.len());
    assert!(caps.iter().all(|c| c["last_turn_seq"].as_u64().unwrap() >= 44), "the most recently used are kept");
    assert!(caps.windows(2).all(|w| w[0]["id"].as_str() < w[1]["id"].as_str()), "sorted by id");
    assert_eq!(parts.iter().map(|p| p["part"].as_u64().unwrap()).collect::<Vec<_>>(), (0..parts.len() as u64).collect::<Vec<_>>());
}

#[test]
fn retention_removes_old_files_by_exact_path() {
    let rh = home("sweep");
    let dir = rh.join("extend/usage");
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["2026-07-01.jsonl", "2026-07-23.jsonl", "2026-07-24.jsonl", "2026-10-01.jsonl", "notes.txt"] {
        std::fs::write(dir.join(n), "{}\n").unwrap();
    }
    std::fs::create_dir(dir.join("2026-01-01.jsonl")).unwrap(); // not a file: kept
    usage::sweep(&dir, crate::util::parse_rfc3339("2026-10-22T12:00:00.000Z").unwrap());
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["2026-01-01.jsonl", "2026-07-24.jsonl", "2026-10-01.jsonl", "notes.txt"], "90 days before 2026-10-22 is 2026-07-24");
}

fn rec(kind: &str, data: Value) -> Value {
    json!({"by": "root", "data": data, "kind": kind})
}

#[test]
fn the_table_folds_changes_rollbacks_states_and_uses() {
    let e = Extend::default();
    let open = |g: u64, caps: Value| rec("change_open", json!({"caps": caps, "group": g}));
    e.apply(&open(1, json!([{"evidence": "root", "id": "skill/a", "op": "add", "prev_ver": null, "ver": "v1"}])));
    e.apply(&rec("change_close", json!({"group": 1, "outcome": "clean"})));
    e.apply(&open(2, json!([{"evidence": "root", "id": "skill/a", "op": "modify", "prev_ver": "v1", "ver": "v2"},
        {"evidence": "zone", "id": "mem/v", "op": "add", "prev_ver": null, "ver": "d1"}])));
    e.apply(&rec("cap_use", json!({"caps": [{"evidence": "root", "harm_turns": 1, "id": "skill/a", "last_turn_seq": 7, "turns": 2, "uses": 5}], "day": "2026-10-15"})));
    let row = |id: &str| e.table().into_iter().find(|r| r.id == id).unwrap();
    assert_eq!((row("skill/a").ver.as_deref(), row("skill/a").group, row("mem/v").registered), (Some("v2"), Some(2), true));
    // group 2 rolled back: skill/a returns to v1 (keeping its counters), mem/v (added by the group) is removed
    e.apply(&rec("change_close", json!({"group": 2, "outcome": "rolled_back"})));
    let a = row("skill/a");
    assert_eq!((a.ver.as_deref(), a.state.as_str(), a.group, a.uses, a.harm_turns), (Some("v1"), "active", None, 5, 1));
    assert_eq!((row("mem/v").state.as_str(), row("mem/v").ver.clone()), ("removed", None));
    e.apply(&rec("cap_state", json!({"cap": "skill/a", "from": "active", "reason": "correlation", "to": "quarantined", "ver": "v1"})));
    assert_eq!(row("skill/a").state, "quarantined");
    e.apply(&open(3, json!([{"evidence": "root", "id": "skill/a", "op": "remove", "prev_ver": "v1", "ver": null}])));
    assert_eq!(row("skill/a").state, "removed");
    // records the kernel reports never reach the table
    e.apply(&json!({"by": "kernel:abc", "kind": "cap_state", "data": {"cap": "skill/a", "to": "active"}}));
    assert_eq!(row("skill/a").state, "removed");
    let mut t = Table::default();
    t.apply("cap_state", &json!({"cap": "../x", "to": "active"}));
    assert!(t.rows(&BTreeMap::new()).is_empty(), "invalid ids are ignored");
}

/// A pax tar made by the system tar from a fixture tree (the kernel's `t0` blob format, root.md §3.7.1).
fn tar_of(dir: &Path, out: &Path) -> Vec<u8> {
    let ok = std::process::Command::new("/usr/bin/tar").arg("--format").arg("pax").arg("-cf").arg(out).arg("-C").arg(dir)
        .args(["GENESIS.md", "SELF.md", "skills", "agents"]).status().unwrap().success();
    assert!(ok);
    std::fs::read(out).unwrap()
}

#[test]
fn t0_identity_from_the_tar() {
    let h = home("t0");
    let mk = |root: &Path, files: &[(&str, &str)]| {
        for (p, c) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        }
    };
    let (a, b) = (h.join("a"), h.join("b"));
    let long = format!("skills/s/{}/deep.md", "d".repeat(120)); // over ustar's 100 bytes: a pax path record
    mk(&a, &[("GENESIS.md", "g"), ("SELF.md", "s1"), ("skills/s/SKILL.md", "x"), (long.as_str(), "y"), ("agents/r/AGENT.md", "role"), ("skills/t/SKILL.md", "t")]);
    mk(&b, &[("GENESIS.md", "g"), ("SELF.md", "s2"), ("skills/s/SKILL.md", "x"), (long.as_str(), "y"), ("agents/r/AGENT.md", "role"), ("skills/u/SKILL.md", "u")]);
    let (ta, tb) = (tar_of(&a, &h.join("a.tar")), tar_of(&b, &h.join("b.tar")));
    let fa = tar_files(&ta).unwrap();
    assert!(fa.contains_key(&long) && fa.contains_key("skills/s/SKILL.md"), "{:?}", fa.keys().collect::<Vec<_>>());
    let (ca, cb) = (t0_caps(&fa), t0_caps(&tar_files(&tb).unwrap()));
    assert_eq!(ca.keys().collect::<Vec<_>>(), ["agent/r", "prompt/GENESIS.md", "prompt/SELF.md", "skill/s", "skill/t"]);
    assert_eq!(ca["prompt/GENESIS.md"], crate::util::sha256_hex(b"g"));
    assert_eq!((ca["skill/s"] == cb["skill/s"], ca["agent/r"] == cb["agent/r"], ca["prompt/SELF.md"] == cb["prompt/SELF.md"]), (true, true, false),
        "a directory's version depends on its files only, not on tar metadata");
    let mut out = Vec::new();
    super::caps::diff(&cb, &ca, &mut out);
    let ops: Vec<(String, String)> = out.iter().map(|v| (v["id"].as_str().unwrap().into(), v["op"].as_str().unwrap().into())).collect();
    assert_eq!(ops, [("prompt/SELF.md".into(), "modify".into()), ("skill/u".into(), "add".into()), ("skill/t".into(), "remove".into())]);
    // LINUX: GNU tar pads to a 10240-byte record, so a cut 1 KiB before the end can still hold the zero block; cut
    // inside the last member's data instead (just before its last non-zero byte), wherever the archive's padding ends.
    let last = ta.iter().rposition(|b| *b != 0).unwrap();
    for cut in [700, 1024, last] {
        assert!(tar_files(&ta[..cut]).is_err(), "a tar cut at {cut} is refused");
    }
}
