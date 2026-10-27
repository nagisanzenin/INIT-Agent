//! X7 grant tests ($0; RF-27): a request changes no value, only the owner's confirmed `/grant` writes the overlay, the
//! overlay expires, a Root-mechanism kind is refused, and the effective charter is re-validated.

use super::*;
use init_sandbox_profiles::access as pa_grant;
use std::fs;
use std::path::Path;

const NOW: i64 = 1_893_456_000_000; // 2030-01-01T00:00:00Z: later than any clock these unit tests run on

pub(crate) fn test_home(tag: &str, extra: &str) -> (PathBuf, std::sync::Arc<Root>) {
    // Tests may build with CARGO_TARGET_DIR outside the source tree. Only create
    // the fixture parent here; do not assume Cargo created this directory.
    let fixture_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes");
    fs::create_dir_all(&fixture_parent).unwrap();
    let t = fs::canonicalize(fixture_parent).unwrap();
    let dir = t.join(format!("x7{tag}-{}", std::process::id())); // ROOT_HOME ≤ 80 bytes in test builds
    let _ = fs::remove_dir_all(&dir); // exact path, created by this test
    let p = crate::install::Paths { rh: dir.join("rh"), ih: dir.join("ih"), home: dir.clone() };
    fs::create_dir_all(&p.rh).unwrap();
    fs::write(p.rh.join("charter.toml"), format!("schema_version = 1\n[purpose]\ntext = \"X7 test home.\"\n[autonomy]\nmax_level = 4\n\
        [quotas]\ndisk_min_free_gib = 0\n[sandbox]\ndisk_floor_gib = 1\n[root]\ntest_mode = true\n{extra}")).unwrap();
    let o = crate::install::Opts { release: None, charter: None, git: "/usr/bin/git".into(), with_testkit: false, toolchain: None,
        seed_source: None, seed_tag: None, no_service: true, bin_dir: None };
    crate::install::install(&p, &o).unwrap_or_else(|e| panic!("install: {e:?}"));
    (dir, Root::boot(p).unwrap())
}

fn ch() -> Charter {
    let (r, i, h) = (PathBuf::from("/Users/o/.init-root"), PathBuf::from("/Users/o/.init"), PathBuf::from("/Users/o"));
    let src = "schema_version = 1\n[purpose]\ntext = \"p\"\n[autonomy]\nmax_level = 2\n[actions.git-push]\nkind = \"git-push\"\nmin_level = 3\n";
    charter::parse(src, &charter::Ctx { root_home: &r, init_home: &i, home: &h }).map(|x| x.0).unwrap_or_else(|_| {
        charter::parse("schema_version = 1\n[purpose]\ntext = \"p\"\n[autonomy]\nmax_level = 2\n", &charter::Ctx { root_home: &r, init_home: &i, home: &h }).unwrap().0
    })
}

fn cx_paths() -> (PathBuf, PathBuf, PathBuf) {
    (PathBuf::from("/Users/o/.init-root"), PathBuf::from("/Users/o/.init"), PathBuf::from("/Users/o"))
}

/// RF-27: only the §7.4 kinds, with checked values; anything naming a Root mechanism is refused.
#[test]
fn rf27_only_grant_kinds_with_checked_values() {
    let c = ch();
    assert_eq!(validate("money.day_usd", &json!("8.00"), &c), Ok(json!("8.00")));
    assert_eq!(validate("extend.open_groups", &json!(8), &c), Ok(json!(8)));
    assert_eq!(validate("autonomy.max_level", &json!(3), &c), Ok(json!(3)));
    let dep = json!([{"name": "regex", "version": "1.11.1", "checksum": "a".repeat(64)}]);
    assert_eq!(validate("dep", &dep, &c), Ok(dep.clone()));
    for (k, v) in [("money.day_usd", json!(8)), ("money.day_usd", json!("8.0000001")), ("extend.open_groups", json!(0)), ("extend.bloat_factor", json!(900)),
        ("autonomy.max_level", json!(5)), ("quotas.parallel_bodies", json!(0)), ("dep", json!([{"name": "regex", "version": "^1"}])),
        ("extend.snap_max_gib", json!(257)), ("actions.nope", json!(1))] {
        assert!(validate(k, &v, &c).is_err(), "{k} = {v}");
    }
    for k in ["extend.win_turns", "extend.mode", "sentinel.alpha", "sandbox.read_deny", "keys.deepseek", "extend.twin.usd_day", "root.test_mode",
        "extend.cold_days", "ladder.l1", "snapshots", "profiles.body"] {
        assert!(validate(k, &json!(1), &c).unwrap_err().contains("not a grant kind"), "{k}");
    }
    if c.actions.contains_key("git-push") {
        assert_eq!(validate("actions.git-push", &json!(1), &c), Ok(json!(1)), "below the class's min_level");
        assert!(validate("actions.git-push", &json!(3), &c).is_err(), "not below it");
    }
}

/// The overlay on top of a charter: per-level budgets are raised, scalars replaced, expired entries ignored, and an
/// entry that would fail C1–C17 is skipped.
#[test]
fn merge_applies_live_entries_and_skips_invalid_ones() {
    let (r, i, h) = cx_paths();
    let cx = charter::Ctx { root_home: &r, init_home: &i, home: &h };
    let e = |id: &str, kind: &str, value: Value, until: i64| Entry { id: id.into(), kind: kind.into(), value, until: rfc3339(until) };
    let base = ch();
    let entries = [e("g00000001", "extend.open_groups", json!(10), NOW + HOUR), e("g00000002", "extend.snap_max_gib", json!(0), NOW + HOUR),
        e("g00000003", "extend.snap_max_gib", json!(64), NOW - 1), e("g00000004", "quotas.parallel_bodies", json!(6), NOW + HOUR),
        e("g00000005", "money.total_usd", json!("40.00"), NOW + HOUR), e("g00000006", "money.day_usd", json!("30.00"), NOW + HOUR)];
    let (m, skipped) = merge(&base, &entries, &cx, NOW);
    assert_eq!(m.extend.open_groups, vec![10, 10, 10, 12, 12]);
    assert_eq!(m.extend.snap_max_gib, 16, "expired");
    assert_eq!(m.quotas.parallel_bodies, 6);
    assert_eq!((m.money.day_usd.as_str(), m.money.total_usd.as_str()), ("30.00", "40.00"), "valid once the total grant is on top");
    assert_eq!(skipped.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["g00000002"], "snap_max_gib = 0 fails C17 (money values are metadata since the 2026-10-23 policy)");
    assert_eq!(m.extend.budget(0).open_groups, 10);
    assert_eq!(parse_dur("30d"), Some(720));
    assert_eq!(parse_dur("12h"), Some(12));
    assert_eq!((parse_dur("0d"), parse_dur("3w"), parse_dur("")), (None, None, None));
}

/// RF-27 on an installed test home (in process): a request is stored and alerted and changes nothing; the owner's
/// `/grant` needs the typed confirmation, writes the 0600 overlay and the `grant` record, and the effective charter
/// takes the value (the digest stays the file's); a denial is recorded; the overlay expires; at most 3 requests a day.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn rf27_request_changes_nothing_and_the_owner_grant_expires() {
    let (dir, root) = test_home("g", "");
    let eff = |root: &Root| lock(&root.st).charter.as_ref().map(|l| (l.ch.money.day_usd.clone(), l.digest.clone())).unwrap();
    let (day0, digest) = eff(&root);
    assert_eq!(day0, "5.00");
    let e = request(&root, &json!({"kind": "sentinel.alpha", "value": 1, "why": "x"})).unwrap_err();
    assert_eq!(e.0, "root.args", "a Root mechanism is refused");
    let r = request(&root, &json!({"kind": "money.day_usd", "value": "8.00", "duration_h": 48, "why": "a long migration needs a bigger day"})).unwrap();
    let id = r["id"].as_str().unwrap().to_string();
    assert_eq!(eff(&root).0, "5.00", "a request changes no value");
    let recs = crate::lineage::records(&root.p.rh).unwrap();
    let gr = recs.iter().find(|x| x["kind"] == "grant_request").unwrap();
    assert_eq!(gr["data"]["why_sha256"], sha256_hex(b"a long migration needs a bigger day"));
    assert!(!fs::read_to_string(root.p.rh.join("lineage/lineage.jsonl")).unwrap().contains("migration"), "the why text stays off the lineage");
    assert!(fs::read_to_string(root.p.rh.join(format!("state/grants/{id}.json"))).unwrap().contains("migration"));
    assert!(fs::read_to_string(root.p.rh.join("alerts.jsonl")).unwrap().contains("grant_request"));

    let e = decide_at(&root, &id, true, None, None, NOW).unwrap_err();
    assert_eq!((e.0, e.2["phrase"].as_str()), ("root.confirm", Some(crate::access::PHRASE)));
    let ok = decide_at(&root, &id, true, Some(1), Some(crate::access::PHRASE), NOW).unwrap();
    assert_eq!(ok["until"], rfc3339(NOW + HOUR), "the owner's `for 1h` beats the request's 48 h");
    assert_eq!(eff(&root), ("8.00".into(), digest.clone()), "effective value, file digest");
    let mode = fs::metadata(root.p.rh.join("state/grants.toml")).unwrap().permissions();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);
    assert_eq!(decide_at(&root, &id, true, None, Some("confirm"), NOW).unwrap_err().0, "root.args", "decided once");

    let r2 = request(&root, &json!({"kind": "extend.open_groups", "value": 6, "why": "w"})).unwrap();
    let id2 = r2["id"].as_str().unwrap();
    assert!(line(&root, &format!("/deny {id2}")));
    assert_eq!(lock(&root.st).charter.as_ref().unwrap().ch.extend.open_groups[0], 4);
    request(&root, &json!({"kind": "extend.changes_day", "value": 60, "why": "w"})).unwrap();
    assert_eq!(request(&root, &json!({"kind": "extend.changes_day", "value": 70, "why": "w"})).unwrap_err().0, "root.rate");

    tick_at(&root, NOW + 30 * 60_000);
    assert_eq!(eff(&root).0, "8.00", "not yet");
    tick_at(&root, NOW + 2 * HOUR);
    assert_eq!(eff(&root).0, "5.00", "expired: back to the charter's value");
    assert!(read(&root.p).is_empty());
    let grants: Vec<Value> = crate::lineage::records(&root.p.rh).unwrap().into_iter().filter(|x| x["kind"] == "grant").map(|x| x["data"].clone()).collect();
    let decisions: Vec<&str> = grants.iter().map(|g| g["decision"].as_str().unwrap()).collect();
    assert_eq!(decisions, ["approve", "deny", "expired"]);
    assert_eq!((grants[2]["id"].as_str(), grants[2]["kind"].as_str()), (Some(id.as_str()), Some("money.day_usd")));
    assert!(line(&root, "/grants") && !line(&root, "/grantx"));
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn request_ids_are_checked_before_any_path_is_built() {
    let ok = |id: &str| id.len() == 9 && id.starts_with('g') && id[1..].bytes().all(|b| b.is_ascii_hexdigit());
    assert!(ok("g0a1b2c3d"));
    for bad in ["g../../x", "../g00000", "g0000000z", ""] {
        assert!(!ok(bad), "{bad}");
    }
    let _ = Path::new("x");
}

/// INTEG-A (ADR-011 §4.5, §7.4): fleet members read the home only; a live `fleet.workspace_write` grant of N lets at
/// most N running members write the workspace (first come; a failed spawn gives its slot back); the live body and
/// probes are unchanged; no workspace, nothing to write.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "test-fixtures"), ignore = "a test home needs the test-fixtures ROOT_HOME bound (80 bytes)")]
#[test]
fn fleet_workspace_write_grants_members_the_workspace() {
    let (dir, root) = test_home("w", "");
    let member = |id: &str| crate::access::spawn_grant(&root, id, "body", false);
    let read_only = |g: &pa_grant::SpawnGrant| g.access == "read_home" && g.grant.write.is_none() && g.workspace.is_none();
    assert!(read_only(&member("m0")), "no workspace, no grant");
    fs::create_dir_all(dir.join("ws")).unwrap();
    // ADR-012: members' workspace writes are a workspace-mode grant (`init-agent --workspace`; full access gives the live body
    // its own full grant)
    let r = crate::access::workspace_set(&root, &json!({"cwd": dir.join("ws").to_str().unwrap(), "mode": "workspace"})).unwrap();
    let ws = r["access"]["workspace"].as_str().map(std::path::PathBuf::from).unwrap_or_else(|| panic!("{r}"));
    assert!(read_only(&member("m1")), "a workspace but no grant: unchanged");
    let live = crate::access::spawn_grant(&root, "b0", "body", true);
    assert_eq!((live.access, live.grant.write.as_deref()), ("workspace", Some(ws.as_path())));
    let id = request(&root, &json!({"kind": "fleet.workspace_write", "value": 1, "why": "two sub-agents edit one tree"})).unwrap()["id"]
        .as_str().unwrap().to_string();
    assert!(read_only(&member("m2")), "a pending request grants nothing");
    decide(&root, &id, true, None, Some(crate::access::PHRASE)).unwrap();
    assert_eq!(workspace_writers(&root), 1);
    let w = member("m3");
    assert_eq!((w.access, w.grant.write.as_deref(), w.workspace.as_deref(), w.grant.read_home), ("workspace", Some(ws.as_path()), Some(ws.as_path()), true));
    assert!(read_only(&member("m4")), "one writer at a time: m3's slot is held while its spawn is pending");
    crate::access::unspawned(&root, "m3");
    assert_eq!(member("m5").grant.write.as_deref(), Some(ws.as_path()), "a failed spawn gives the slot back");
    let probe = crate::access::spawn_grant(&root, "p0", "probe", false);
    assert!(probe.grant.write.is_none() && probe.access == "none", "probes stay hermetic");
    drop(root);
    fs::remove_dir_all(&dir).unwrap();
}
