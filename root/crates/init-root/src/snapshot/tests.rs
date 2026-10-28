//! Tests of `snapshot` (lane X4, ADR-011 RF-15, RF-16, RF-17; retention and `settled`). Test-only, outside the
//! Root's size budget. Every home is under `target/test-homes/x4/`, removed by its exact path; the only processes
//! signalled are the ones these tests spawn (by handle).

use super::*;
use std::os::unix::fs::symlink;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

fn base(tag: &str) -> PathBuf {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    fs::create_dir_all(&target).unwrap(); // CARGO_TARGET_DIR may point outside the workspace.
    let t = fs::canonicalize(target).unwrap();
    let b = t.join(format!("test-homes/x4/{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&b); // exact path, created by this test
    fs::create_dir_all(&b).unwrap();
    b
}

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn r(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

const SID: &str = "20261022-120000-abcdef";

#[test]
fn system_snapshot_filesystem_support_is_explicit_and_keeps_macos_apfs_contract() {
    assert!(snapshot_filesystem_supported("macos","apfs"));
    for filesystem in ["ext2/3/4","btrfs","xfs","overlayfs","tmpfs"] {
        assert!(snapshot_filesystem_supported("linux",filesystem));
        assert!(!snapshot_filesystem_supported("macos",filesystem));
    }
    for filesystem in ["unsupported","nfs","fuse","linux:0x1234","apfs"] {
        assert!(!snapshot_filesystem_supported("linux",filesystem));
    }
    assert!(!snapshot_filesystem_supported("windows","ntfs"));
}

#[cfg(target_os="linux")]
#[test]
fn linux_system_snapshot_quota_excludes_conversation_and_refuses_before_large_file_copy() {
    let b=base("linux-quota"); let (ih,rh)=fixture(&b);
    // The bytes under sessions are never part of the snapshot quota.
    fs::File::create(ih.join("state/sessions/large")).unwrap().set_len(1<<30).unwrap();
    let result=take_at(&rh,&ih,"small","sys",None,4096).unwrap();
    assert!(result["logical_bytes"].as_u64().unwrap()<4096);
    assert!(!dir(&rh).join("small/state/sessions").exists());
    fs::File::create(ih.join("body/large")).unwrap().set_len(1<<30).unwrap();
    let error=take_at(&rh,&ih,"over","sys",None,4096).unwrap_err();
    assert_eq!(error.0,"extend.snapshot_too_large");
    assert!(!dir(&rh).join("over").exists());
    assert!(!dir(&rh).join("over.json").exists());
    assert_eq!(r(&b.join("outside/secret")),"secret");
    fs::remove_dir_all(b).unwrap();
}

/// A fixture INIT_HOME (`ih`) and ROOT_HOME (`rh`), and `outside/secret`, which links in `ih` point at.
fn fixture(b: &Path) -> (PathBuf, PathBuf) {
    let (ih, rh) = (b.join("ih"), b.join("rh"));
    for (f, s) in [("body/GENESIS.md", "g1"), ("body/SELF.md", "s1"), ("body/skills/x/SKILL.md", "k1"), ("state/sessions/S.jsonl", "line1\n"),
        ("state/spill/s1/a", "spill"), ("state/mem/data", "v1"), ("state/session", SID), ("state/run/ready", "{\"boot_id\":\"live\"}"),
        ("vault/traces/v1/t", "trace"), ("vault/gate/noise.json", "{}"), ("kernel/CONSTITUTION.md", "c")] {
        w(&ih.join(f), s);
    }
    fs::create_dir_all(rh.join("sealed/run")).unwrap();
    fs::create_dir_all(rh.join("store/blobs")).unwrap();
    w(&b.join("outside/secret"), "secret");
    symlink(b.join("outside"), ih.join("body/outlink")).unwrap();
    (ih, rh)
}

/// RF-15: a snapshot and restore round trip on a fixture INIT_HOME: the excluded trees are neither snapshotted nor
/// touched by the restore, links are cloned and removed as links (never followed), the replaced state is saved to
/// `pre-restore-<n>` first and restoring that undoes the restore; a tree swapped for a link and an oversized state
/// are refused, leaving nothing behind; retention removes what nothing pins, by exact path.
#[test]
fn rf15_snapshot_restore_round_trip() {
    let b = base("rf15");
    let (ih, rh) = fixture(&b);
    let big = vec![7u8; 64 << 20];
    fs::write(ih.join("state/mem/big"), &big).unwrap();
    let free0 = crate::sys::free_bytes(&ih).unwrap();
    let m = take_at(&rh, &ih, "1", "sys", Some(1), 1 << 30).unwrap();
    let free1 = crate::sys::free_bytes(&ih).unwrap();
    eprintln!("X4 snapshot cost: {} KiB of free disk for {} KiB logical (platform clone/copy; other lanes write concurrently)",
        (free0 as i64 - free1 as i64) / 1024, m["logical_bytes"].as_u64().unwrap() / 1024);
    assert_eq!((m["id"].as_str(), m["kind"].as_str(), m["group"].as_u64()), (Some("1"), Some("sys"), Some(1)));
    assert!(m["logical_bytes"].as_u64().unwrap() >= 64 << 20);
    let s = dir(&rh).join("1");
    assert_eq!((r(&s.join("body/GENESIS.md")), r(&s.join("state/mem/data")), r(&s.join("vault/gate/noise.json"))), ("g1".into(), "v1".into(), "{}".into()));
    assert!(is_link(&s.join("body/outlink")), "a link is cloned as a link");
    for x in ["state/sessions", "state/spill", "vault/traces"] {
        assert!(fs::symlink_metadata(s.join(x)).is_err(), "{x} is never snapshotted");
    }
    fs::remove_file(ih.join("state/mem/big")).unwrap();

    // the agent changes its state; a tree's entry becomes a link to outside
    w(&ih.join("state/new"), "n");
    w(&ih.join("body/SELF.md"), "s2");
    fs::remove_file(ih.join("body/GENESIS.md")).unwrap();
    std::io::Write::write_all(&mut fs::OpenOptions::new().append(true).open(ih.join("state/sessions/S.jsonl")).unwrap(), b"line2\n").unwrap();
    w(&ih.join("state/spill/s1/b"), "spill2");
    w(&ih.join("vault/traces/v1/t2"), "trace2");
    fs::remove_dir_all(ih.join("state/mem")).unwrap();
    symlink(b.join("outside"), ih.join("state/mem")).unwrap();

    let out = restore_at(&rh, &ih, "1", &|| false).unwrap();
    assert_eq!((out["restored"].as_str(), out["saved"].as_str()), (Some("1"), Some("pre-restore-1")));
    assert_eq!((r(&ih.join("body/GENESIS.md")), r(&ih.join("body/SELF.md")), r(&ih.join("state/mem/data"))), ("g1".into(), "s1".into(), "v1".into()));
    assert!(!is_link(&ih.join("state/mem")) && !ih.join("state/new").exists());
    assert!(is_link(&ih.join("body/outlink")));
    assert_eq!(r(&b.join("outside/secret")), "secret", "the link was removed, never followed");
    assert_eq!(r(&ih.join("state/sessions/S.jsonl")), "line1\nline2\n", "the conversation is kept");
    assert_eq!((r(&ih.join("state/spill/s1/b")), r(&ih.join("vault/traces/v1/t2"))), ("spill2".into(), "trace2".into()));
    let pre = dir(&rh).join("pre-restore-1");
    assert!(is_link(&pre.join("state/mem")) && r(&pre.join("state/new")) == "n" && r(&pre.join("body/SELF.md")) == "s2");

    // the restore itself can be undone
    let back = restore_at(&rh, &ih, "pre-restore-1", &|| false).unwrap();
    assert_eq!(back["saved"].as_str(), Some("pre-restore-2"));
    assert_eq!((r(&ih.join("state/new")), r(&ih.join("body/SELF.md"))), ("n".into(), "s2".into()));
    assert!(is_link(&ih.join("state/mem")) && !ih.join("body/GENESIS.md").exists());
    assert_eq!(r(&b.join("outside/secret")), "secret");

    // a tree swapped for a link is refused, and nothing is left; so is an oversized state
    fs::rename(ih.join("vault"), b.join("vault-real")).unwrap();
    symlink(b.join("outside"), ih.join("vault")).unwrap();
    let e = take_at(&rh, &ih, "2", "sys", Some(2), 1 << 30).unwrap_err();
    assert!(e.0 == "root.internal" && e.1.contains("INIT_HOME/vault is not a directory"), "{e:?}");
    assert!(!dir(&rh).join("2").exists() && !dir(&rh).join("2.json").exists());
    fs::remove_file(ih.join("vault")).unwrap();
    fs::rename(b.join("vault-real"), ih.join("vault")).unwrap();
    let e = take_at(&rh, &ih, "3", "sys", Some(3), 1).unwrap_err();
    assert_eq!(e.0, "extend.snapshot_too_large", "{e:?}");
    assert!(!dir(&rh).join("3").exists());
    assert_eq!(restore_at(&rh, &ih, "../1", &|| false).unwrap_err().0, "root.not_found");

    // retention: nothing pins "1" (no open group, no pointer); the newest pre-restore stays
    let gone = prune_at(&rh, &[], now_ms() + FRESH_MS + 1);
    assert_eq!(gone, vec!["1".to_string(), "pre-restore-1".to_string()]);
    assert!(!dir(&rh).join("1").exists() && !dir(&rh).join("1.json").exists() && dir(&rh).join("pre-restore-2").is_dir());
    fs::remove_dir_all(&b).unwrap();
}

/// RF-17: a restore is refused while a zone process lives (here: a child this test spawned and holds), before it
/// changes anything; a zone process appearing mid-restore stops it with the state saved; once the process is gone
/// the restore runs.
#[test]
fn rf17_restore_refused_while_a_zone_process_lives() {
    let b = base("rf17");
    let (ih, rh) = fixture(&b);
    take_at(&rh, &ih, "1", "sys", Some(1), 1 << 30).unwrap();
    w(&ih.join("state/mem/data"), "v2");
    let child: Mutex<Child> = Mutex::new(Command::new("/bin/sleep").arg("30").stdin(Stdio::null()).spawn().unwrap());
    let live = || lock(&child).try_wait().is_ok_and(|s| s.is_none());
    let e = restore_at(&rh, &ih, "1", &live).unwrap_err();
    assert_eq!((e.0, e.2["reason"].as_str()), ("root.busy", Some("zone_live")), "{e:?}");
    assert_eq!(r(&ih.join("state/mem/data")), "v2", "nothing changed");
    assert!(!dir(&rh).join("pre-restore-1").exists(), "not even the pre-restore save");

    // a zone process that starts during the restore (the 3rd liveness check, before state/)
    let n = AtomicU32::new(0);
    let late = || n.fetch_add(1, Ordering::SeqCst) >= 2;
    let e = restore_at(&rh, &ih, "1", &late).unwrap_err();
    assert!(e.0 == "root.busy" && e.1.contains("before state/") && e.1.contains("pre-restore-1"), "{e:?}");
    assert_eq!(r(&dir(&rh).join("pre-restore-1/state/mem/data")), "v2", "the state before it was saved");

    let mut c = child.into_inner().unwrap();
    c.kill().unwrap(); // our own child, by handle
    c.wait().unwrap();
    let out = restore_at(&rh, &ih, "1", &|| false).unwrap();
    assert_eq!(out["saved"].as_str(), Some("pre-restore-2"));
    assert_eq!(r(&ih.join("state/mem/data")), "v1");
    fs::remove_dir_all(&b).unwrap();
}

/// §4.2 retention, pure: open groups (by `change_open.snapshot` or by the snapshot's group), the newest snapshot at
/// or before each pointer move, the newest pre-restore, and fresh snapshots are kept.
#[test]
fn retention_keeps_open_groups_pointer_snapshots_and_the_newest_pre_restore() {
    let meta = |id: &str, at: i64, kind: &str, g: Option<u64>| json!({"at_ms": at, "group": g, "id": id, "kind": kind});
    let metas = vec![meta("1", 1000, "sys", Some(1)), meta("2", 2000, "sys", Some(2)), meta("pre-restore-1", 2500, "pre_restore", None),
        meta("3", 3000, "sys", Some(3)), meta("pre-restore-2", 3500, "pre_restore", None), meta("4", 4000, "sys", Some(4))];
    let rec = |kind: &str, at: i64, d: Value| json!({"at": rfc3339(at), "data": d, "kind": kind});
    let mut recs = vec![rec("change_open", 1000, json!({"group": 1, "snapshot": "1"})), rec("change_close", 1500, json!({"group": 1})),
        rec("lkg", 2600, json!({"tuple": "tB"})), rec("change_open", 3000, json!({"group": 3, "snapshot": "3"})),
        rec("change_open", 4000, json!({"group": 4}))];
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<String>>();
    let far = 10_000_000_000;
    assert_eq!(keep_set(&metas, &recs, far), set(&["2", "3", "4", "pre-restore-2"]));
    recs.push(rec("settled", 1500, json!({"tuple": "tA"})));
    recs.push(rec("change_close", 4500, json!({"group": 3})));
    assert_eq!(keep_set(&metas, &recs, far), set(&["1", "2", "4", "pre-restore-2"]));
    assert_eq!(keep_set(&metas, &recs, 4001).len(), 6, "a snapshot younger than 10 minutes is never pruned");
}

/// §4.1 `settled`: the newest tuple `lkg` for ≥ 7 days with no direct rollback of its groups and no quarantine of a
/// cap version its groups added or modified.
#[test]
fn settled_moves_after_seven_days_and_skips_harmed_tuples() {
    let day = 86_400_000i64;
    let t0 = 1_800_000_000_000i64;
    let rec = |kind: &str, at: i64, d: Value| json!({"at": rfc3339(at), "data": d, "kind": kind});
    let mut recs = vec![rec("lkg", t0, json!({"tuple": "tA"})),
        rec("change_open", t0 + day / 2, json!({"group": 1, "tuple": "tB", "caps": [{"id": "skill/x", "ver": "v1", "op": "add"}]})),
        rec("change_close", t0 + day, json!({"group": 1, "outcome": "clean", "attribution": null})), rec("lkg", t0 + day, json!({"tuple": "tB"})),
        rec("change_open", t0 + day + 1, json!({"group": 2, "tuple": "tC", "caps": []})), rec("lkg", t0 + 2 * day, json!({"tuple": "tC"}))];
    assert_eq!(settled_of(&recs, t0 + 5 * day), None);
    assert_eq!(settled_of(&recs, t0 + 7 * day).as_deref(), Some("tA"));
    assert_eq!(settled_of(&recs, t0 + 8 * day + 1).as_deref(), Some("tB"));
    assert_eq!(settled_of(&recs, t0 + 9 * day).as_deref(), Some("tC"));
    recs.push(rec("cap_state", t0 + 9 * day, json!({"cap": "skill/x", "ver": "v1", "from": "active", "to": "quarantined"})));
    recs.push(rec("change_close", t0 + 9 * day, json!({"group": 2, "outcome": "rolled_back", "attribution": "direct"})));
    assert_eq!(settled_of(&recs, t0 + 10 * day).as_deref(), Some("tA"), "tB's cap was quarantined, tC's group rolled back direct");
    recs.push(rec("cap_state", t0 + 9 * day, json!({"cap": "skill/x", "ver": "v2", "to": "quarantined"})));
    assert_eq!(settled_of(&recs, t0 + 10 * day).as_deref(), Some("tA"));
}

/// v0.5.1: an owner upgrade's commit (`lkg{upgrade}`) starts the window again. A tuple of the kernel from before the
/// upgrade never becomes `settled` afterwards (the commit re-anchored `settled` onto the new kernel, tuple::reanchor);
/// the upgraded tuple settles 7 days after the commit.
#[test]
fn no_tuple_from_before_an_upgrade_settles_after_its_commit() {
    let day = 86_400_000i64;
    let t0 = 1_800_000_000_000i64;
    let rec = |kind: &str, at: i64, d: Value| json!({"at": rfc3339(at), "data": d, "kind": kind});
    let recs = vec![rec("lkg", t0, json!({"tuple": "tOld"})), rec("lkg", t0 + 3 * day, json!({"tuple": "tNew", "upgrade": "u-1"}))];
    assert_eq!(settled_of(&recs, t0 + 2 * day), None, "nothing is 7 days old");
    assert_eq!(settled_of(&recs, t0 + 8 * day), None, "tOld is 8 days lkg, but from before the upgrade");
    assert_eq!(settled_of(&recs, t0 + 10 * day).as_deref(), Some("tNew"));
    assert_eq!(settled_of(&recs[..1], t0 + 8 * day).as_deref(), Some("tOld"), "without the upgrade it would settle");
}

/// A fixture body for A4: a `/bin/sh` script; `@CHECKS@` is replaced by the common checks of mode `check`.
fn body(rh: &Path, script: &str, live_ih: &Path) -> String {
    let checks = format!(r#"[ "$INIT_MODE" = check ] || exit 3
[ -f "$INIT_HOME/body/GENESIS.md" ] || exit 4
[ "$INIT_SESSION" = {SID} ] || exit 5
grep -q '"mode":"check"' "$INIT_HOME/state/kernel/boot.json" || exit 6
cat '{}/state/mem/data' >/dev/null 2>&1 && exit 7
[ -e "$INIT_HOME/state/spill" ] && exit 8
ready() {{ printf '{{"boot_id":"%s","mode":"%s","v":1}}\n' "$INIT_BOOT_ID" "$INIT_MODE" > "$INIT_HOME/state/run/ready"; }}"#, live_ih.display());
    let text = format!("#!/bin/sh\n{}\n", script.replace("@CHECKS@", &checks));
    crate::tuple::put(rh, &mut text.as_bytes(), None, 1 << 30).unwrap()
}

/// RF-16: a candidate whose state the parent cannot read is refused (`extend.rollback_incompatible`, arm `parent`);
/// a compatible candidate passes; a candidate that writes no `ready`, or runs past the wall, is refused (arm
/// `candidate`). Both arms run sandboxed (the sealed profile: the live INIT_HOME is unreadable, exit 7 otherwise) on a
/// clone of the live `state/` without `spill/`; the live state is never written; probe roots are removed.
#[cfg(target_os = "macos")]
#[test]
fn rf16_a4_refuses_a_candidate_whose_state_the_parent_cannot_read() {
    let b = base("rf16");
    let (ih, rh) = fixture(&b);
    let v1 = body(&rh, r#"@CHECKS@
f="$INIT_HOME/state/mem/format"
if [ -e "$f" ] && [ "$(cat "$f")" != 1 ]; then echo "memory format $(cat "$f") unknown" >&2; exit 1; fi
printf 1 > "$f"; ready; exit 0"#, &ih);
    let v2 = body(&rh, "@CHECKS@\nprintf 2 > \"$INIT_HOME/state/mem/format\"; ready; exit 0", &ih);
    let silent = body(&rh, "@CHECKS@\nexit 0", &ih);
    let slow = body(&rh, "@CHECKS@\nsleep 30; ready; exit 0", &ih);
    // the parent's T0 from the exported tree, the candidates' from a t0 blob
    let (psha, csha) = ("a".repeat(40), "c".repeat(40));
    for f in ["GENESIS.md", "SELF.md"] {
        w(&ih.join("lineage/trees").join(&psha[..12]).join(f), "t0");
        w(&b.join("t0src").join(f), "t0");
    }
    let tar = b.join("t0.tar");
    assert!(Command::new("/usr/bin/tar").arg("-cf").arg(&tar).arg("-C").arg(b.join("t0src")).args(["GENESIS.md", "SELF.md"]).status().unwrap().success());
    let t0 = crate::tuple::put(&rh, &mut fs::File::open(&tar).unwrap(), None, 1 << 30).unwrap();
    let ext = crate::tuple::put(&rh, &mut &b"\0asm"[..], None, 1 << 30).unwrap();
    let tup = |bin: &str, sha: &str, t0: Option<&str>| json!({"body": {"binary": bin, "sha": sha, "t0": t0},
        "ext": {"components": [{"name": "count-lines", "sha256": ext}], "set": "s1"}, "kernel": {}, "v": 1});
    let parent = tup(&v1, &psha, None);
    let cx = crate::charter::Ctx { root_home: &rh, init_home: &ih, home: &b };
    let mut ch = crate::charter::parse(&crate::charter::template().replace("REPLACE ME: what INIT is for, in your words.", "X4 test."), &cx).map(|x| x.0).unwrap_or_else(|e| panic!("{e:?}"));
    ch.sandbox.disk_floor_gib = 0; // the floor is off in this test (the machine may sit near it)
    let env = Env { rh: rh.clone(), ih: ih.clone(), home: b.clone(), info: json!({}), ch, level: 0, t0_sha: None };
    let wall = Duration::from_secs(20);

    let ok = probe(&env, &tup(&v1, &csha, Some(&t0)), &parent, wall).unwrap();
    assert_eq!((ok["candidate"]["exit"].as_u64(), ok["parent"]["exit"].as_u64()), (Some(0), Some(0)), "{ok}");

    let e = probe(&env, &tup(&v2, &csha, Some(&t0)), &parent, wall).unwrap_err();
    assert_eq!((e.0, e.2["arm"].as_str(), e.2["exit"].as_i64()), ("extend.rollback_incompatible", Some("parent"), Some(1)), "{e:?}");
    assert!(e.1.contains("the parent body cannot boot on the state the candidate leaves"), "{}", e.1);

    let e = probe(&env, &tup(&silent, &csha, Some(&t0)), &parent, wall).unwrap_err();
    assert_eq!((e.0, e.2["arm"].as_str(), e.2["ready"].as_bool()), ("extend.rollback_incompatible", Some("candidate"), Some(false)), "{e:?}");
    let e = probe(&env, &tup(&slow, &csha, Some(&t0)), &parent, Duration::from_secs(2)).unwrap_err();
    assert_eq!((e.0, e.2["arm"].as_str(), e.2["why"].as_str()), ("extend.rollback_incompatible", Some("candidate"), Some("timeout")), "{e:?}");

    // a missing blob or T0 source is a store refusal, not an incompatibility
    let e = probe(&env, &tup(&"0".repeat(64), &csha, Some(&t0)), &parent, wall).unwrap_err();
    assert_eq!(e.0, "root.store", "{e:?}");
    let e = probe(&env, &tup(&v1, &csha, None), &parent, wall).unwrap_err();
    assert!(e.0 == "root.store" && e.1.contains("no T0"), "{e:?}");
    assert_eq!(probe(&env, &json!({"body": {"binary": ""}}), &parent, wall).unwrap_err().0, "root.args");

    assert!(!ih.join("state/mem/format").exists(), "the live state is never written");
    assert_eq!(r(&ih.join("state/run/ready")), "{\"boot_id\":\"live\"}");
    assert_eq!(fs::read_dir(rh.join("sealed/run")).unwrap().count(), 0, "every probe root is removed");
    fs::remove_dir_all(&b).unwrap();
}

/// The Root wrappers on an installed test home (in process, no daemon, no kernel): `admit` takes the group's
/// snapshot (`snapshot` record) when a change opens a group and none when it joins one; `t0`/`t1` skip A4; a `t2`
/// change whose parent tuple is not stored is refused before any probe; `restore` with no zone instance restores and
/// records `restore` plus the `pre_restore` snapshot; the hooks are registered once. (A successful A4 through
/// `admit` would take the process-wide sealed lock, whose `cancel_all` kills the sealed instances that concurrently
/// running unit tests hold; RF-16 covers the probe itself.)
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn root_wrappers_admit_take_and_restore_record_their_lineage() {
    let t = fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
    let dir = t.join(format!("test-homes/x4r-{}", std::process::id())); // ROOT_HOME ≤ 80 bytes in test builds
    let _ = fs::remove_dir_all(&dir); // exact path, created by this test
    let p = crate::install::Paths { rh: dir.join("rh"), ih: dir.join("ih"), home: dir.clone() };
    fs::create_dir_all(&p.rh).unwrap();
    w(&p.rh.join("charter.toml"), "schema_version = 1\n[purpose]\ntext = \"X4 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
        disk_min_free_gib = 0\n[sandbox]\ndisk_floor_gib = 1\n[root]\ntest_mode = true\n");
    let o = crate::install::Opts { release: None, charter: None, git: "/usr/bin/git".into(), with_testkit: false, toolchain: None,
        seed_source: None, seed_tag: None, no_service: true, bin_dir: None };
    crate::install::install(&p, &o).unwrap_or_else(|e| panic!("install: {e:?}"));
    let root = crate::link::Root::boot(p.clone()).unwrap();
    let (ih, rh) = (p.ih.clone(), p.rh.clone());
    w(&ih.join("state/mem/data"), "v1");
    w(&ih.join("state/session"), SID);
    let v1 = body(&rh, "@CHECKS@\nprintf 1 > \"$INIT_HOME/state/mem/format\"; ready; exit 0", &ih);
    let sha = "b".repeat(40);
    for f in ["GENESIS.md", "SELF.md"] {
        w(&ih.join("lineage/trees").join(&sha[..12]).join(f), "t0");
    }
    let a = json!({"scope": "t2", "group": 5, "parent_tuple": "t000000000000", "candidate": {"body": {"binary": v1, "sha": sha, "t0": null}}});
    let e = admit(&root, &a, true).unwrap_err();
    assert!(e.0 == "root.store" && e.1.contains("t000000000000 is not in the store"), "{e:?}");
    let out = admit(&root, &json!({"scope": "t1", "group": 5, "candidate": a["candidate"]}), true).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(out, json!({"A4": "skip", "snapshot": "5"}));
    assert_eq!(admit(&root, &json!({"scope": "t0", "group": 5}), false).unwrap(), json!({"A4": "skip", "snapshot": null}));
    assert!(dir_of(&rh, "5").join("state/mem/data").is_file());

    w(&ih.join("state/mem/data"), "v2");
    let r2 = restore(&root, "5", "test").unwrap();
    assert_eq!((r2["restored"].as_str(), r2["saved"].as_str()), (Some("5"), Some("pre-restore-1")));
    assert_eq!(r(&ih.join("state/mem/data")), "v1");
    let recs = crate::lineage::records(&rh).unwrap();
    let kinds: Vec<(String, Value)> = recs.iter().filter(|x| ["snapshot", "restore"].contains(&x["kind"].as_str().unwrap_or("")))
        .map(|x| (x["kind"].as_str().unwrap().to_string(), x["data"].clone())).collect();
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    assert_eq!((kinds[0].1["id"].as_str(), kinds[0].1["kind"].as_str(), kinds[0].1["group"].as_u64()), (Some("5"), Some("sys"), Some(5)));
    assert_eq!((kinds[1].1["id"].as_str(), kinds[1].1["kind"].as_str()), (Some("pre-restore-1"), Some("pre_restore")));
    assert_eq!(kinds[2], ("restore".into(), json!({"kept": ["sessions", "spill", "traces"], "reason": "test", "saved": "pre-restore-1", "snapshot": "5"})));
    register();
    register();
    let n = lock(crate::sentinel::hooks()).admit.len();
    assert!(n >= 1 && lock(crate::sentinel::hooks()).restore.is_some());
    register();
    assert_eq!(lock(crate::sentinel::hooks()).admit.len(), n, "registered once");
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

fn dir_of(rh: &Path, id: &str) -> PathBuf {
    dir(rh).join(id)
}
