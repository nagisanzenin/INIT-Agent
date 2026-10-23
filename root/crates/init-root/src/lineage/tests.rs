//! R4 unit tests (root/root.md §8 RR-16…RR-19, D7) and `r4.verify` (testing.md §3.13: the chain verifies
//! at the end of every scenario). Homes live under `root/target/test-homes/wp23/` (or `$WP23_HOMES`).
//! Every git command here runs with a cleared environment and no global config; mirrors are local
//! `file://` bare repositories, so nothing touches the network or the owner's files.

use super::verify::{audit_verify, chain, verify};
use super::*;
use crate::install::{Paths, RH_DIRS, make_dirs};
use std::process::Command;
use std::time::Instant;

fn base() -> PathBuf {
    std::env::var_os("WP23_HOMES").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/wp23"))
}

/// A fresh home `<base>/u-<name>` with ROOT_HOME `rh`, INIT_HOME `ih` (with its `root/` view dir).
fn home(name: &str) -> Paths {
    fs::create_dir_all(base()).unwrap();
    let dir = fs::canonicalize(base()).unwrap().join(format!("u-{name}"));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap(); // exact path, made by an earlier run of this test
    }
    let p = Paths { rh: dir.join("rh"), ih: dir.join("ih"), home: dir.clone() };
    make_dirs(&p.rh, &RH_DIRS).unwrap();
    make_dirs(&p.ih, &["", "root", "lineage", "kernel", "kernel/src", "work"]).unwrap();
    p
}

fn genesis(p: &Paths) -> Lineage {
    let vk = ensure_key(&p.rh).unwrap();
    let mut l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    l.append("root", "genesis", json!({"install_id": "00", "pubkey": hex(&vk.to_bytes()), "root_version": "0.1.0"})).unwrap();
    l
}

fn text(p: &Paths) -> String {
    fs::read_to_string(p.r("lineage/lineage.jsonl")).unwrap()
}

/// `r4.verify`: the whole chain, `head.json` and (when configured) the mirror; and the published copy.
fn r4_verify(p: &Paths) {
    if !p.i("root/lineage.pub").exists() {
        fs::copy(p.r("keys/lineage.pub"), p.i("root/lineage.pub")).unwrap(); // what install::publish does
    }
    assert_eq!(audit_verify(p, true, false), 0, "r4.verify (the Root's chain)");
    assert_eq!(audit_verify(p, true, true), 0, "r4.verify (the published view)");
    assert!(fs::read(p.i("root/lineage.jsonl")).unwrap() == fs::read(p.r("lineage/lineage.jsonl")).unwrap(), "the view is a byte copy");
}

fn no_relay(_: &str) -> Option<String> {
    None
}

fn kctx<'a>(deny: &'a [String], relayed: &'a dyn Fn(&str) -> Option<String>) -> KernelCtx<'a> {
    KernelCtx { build: "0123456789ab", deny, relayed }
}

/// A test-made git command: no global or system config, no owner identity.
fn tgit(p: &Paths, git_dir: &Path, args: &[&str]) -> String {
    let o = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &p.home)
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").env("TZ", "UTC")
        .args(["-c", "user.name=wp23-test", "-c", "user.email=test@example.invalid", "-c", "init.defaultBranch=main"])
        .arg(format!("--git-dir={}", git_dir.display())).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn rr16_ten_thousand_records_verify_and_every_tampering_is_located() {
    let p = home("rr16");
    let mut l = genesis(&p);
    let t0 = Instant::now();
    let mut lines = text(&p);
    for n in 1..10_000u64 {
        let (_, line, head) = l.seal(1_760_947_262_345 + n as i64, "root", "stop", json!({"n": n})).unwrap();
        lines += &line;
        l.head = Some(head);
    }
    let sign_ms = t0.elapsed().as_millis();
    fs::write(p.r("lineage/lineage.jsonl"), &lines).unwrap();
    let vk = l.sk.verifying_key();
    let t1 = Instant::now();
    let head = verify(&lines, Some(&vk)).unwrap().unwrap();
    let verify_ms = t1.elapsed().as_millis();
    eprintln!("RR-16 (debug build): sealed 10,000 records in {sign_ms} ms; full verification {verify_ms} ms");
    assert_eq!((head.seq, head.hash.len()), (9_999, 71));
    let ls: Vec<&str> = lines.lines().collect();
    let join = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
    let mut v: Vec<String> = ls.iter().map(|s| s.to_string()).collect();
    // flip a byte of the data
    v[60] = v[60].replacen("\"n\":60", "\"n\":61", 1);
    assert_eq!(verify(&join(&v), Some(&vk)).unwrap_err(), (60, "4: hash does not recompute"));
    v[60] = ls[60].to_string();
    // flip a byte of a signature
    let s = v[70].find("\"sig\":\"ed25519:").unwrap() + 16;
    let flip = if &v[70][s..s + 1] == "0" { "1" } else { "0" };
    v[70].replace_range(s..s + 1, flip);
    assert_eq!(verify(&join(&v), Some(&vk)).unwrap_err(), (70, "5: signature does not verify"));
    v[70] = ls[70].to_string();
    // delete a line; swap two lines
    let del: Vec<String> = v.iter().enumerate().filter(|(i, _)| *i != 80).map(|(_, l)| l.clone()).collect();
    assert_eq!(verify(&join(&del), Some(&vk)).unwrap_err(), (80, "2: seq gap"));
    v.swap(90, 91);
    assert_eq!(verify(&join(&v), Some(&vk)).unwrap_err(), (90, "2: seq gap"));
    v.swap(90, 91);
    // a non-canonical line (same content, a space added)
    v[95] = v[95].replacen("{\"at\"", "{ \"at\"", 1);
    assert_eq!(verify(&join(&v), Some(&vk)).unwrap_err(), (95, "1: not a canonical ICJ v1 record"));
    v[95] = ls[95].to_string();
    // a wrong key, and a torn tail
    let other = SigningKey::from_bytes(&[7u8; 32]).verifying_key();
    assert_eq!(verify(&lines, Some(&other)).unwrap_err().0, 0);
    assert_eq!(verify(&lines[..lines.len() - 1], Some(&vk)).unwrap_err().0, 9_999);
    // the tail check at open finds a late flip only within the signed tail; the full check finds it anywhere
    let mut late = v.clone();
    let s = late[9_990].find("\"sig\":\"ed25519:").unwrap() + 16;
    let flip = if &late[9_990][s..s + 1] == "0" { "1" } else { "0" };
    late[9_990].replace_range(s..s + 1, flip);
    assert_eq!(chain(&join(&late), Some(&vk), 10_000 - TAIL, &HashSet::new()).err().map(|e| e.0), Some(9_990));
    assert!(chain(&join(&late), Some(&vk), 10_000, &HashSet::new()).is_ok());
    // head.json is still the genesis head (the lines were written behind its back): rule 7
    assert_eq!(audit_verify(&p, false, false), 1);
    l.write_head().unwrap();
    let t2 = Instant::now();
    assert_eq!(audit_verify(&p, false, false), 0);
    eprintln!("RR-16: audit verify (signatures over the last {TAIL}) {} ms", t2.elapsed().as_millis());
    drop(l);
    let l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    assert_eq!(l.head.as_ref().map(|h| h.seq), Some(9_999));
    r4_verify(&p);
}

#[test]
#[cfg(feature = "fault-injection")] // INIT_FAULT points
fn torn_tail_is_copied_and_truncated_and_head_json_is_repaired() {
    let p = home("torn");
    let mut l = genesis(&p);
    l.append("root", "charter", json!({"n": 1})).unwrap();
    let before = text(&p);
    // RR-17: `root.lineage.mid_append` leaves half a line; `enospc` stands in for `abort` in-process
    l.fault = "root.lineage.mid_append:enospc".into();
    assert!(l.append("root", "charter", json!({"n": 2})).is_err());
    drop(l);
    assert!(text(&p).len() > before.len() && !text(&p).ends_with('\n'));
    assert_eq!(audit_verify(&p, true, false), 1, "a torn tail is reported until the Root repairs it");
    let l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    assert_eq!(text(&p), before);
    let torn: Vec<_> = fs::read_dir(p.r("lineage")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("torn-")).collect();
    assert_eq!(torn.len(), 1);
    assert!(fs::read_to_string(torn[0].path()).unwrap().starts_with("{\"at\""));
    assert_eq!(l.head.as_ref().map(|h| h.seq), Some(1));
    // `root.lineage.after_append`: the line is synced, head.json is one behind → repaired at open
    let mut l = l;
    l.fault = "root.lineage.after_append:enospc".into();
    assert!(l.append("root", "charter", json!({"n": 3})).is_err());
    drop(l);
    let hj: Value = serde_json::from_str(&fs::read_to_string(p.r("lineage/head.json")).unwrap()).unwrap();
    assert_eq!(hj["seq"], 1);
    assert_eq!(audit_verify(&p, true, false), 0, "one record behind is an append in progress, not an error");
    let l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    let hj: Value = serde_json::from_str(&fs::read_to_string(p.r("lineage/head.json")).unwrap()).unwrap();
    assert_eq!((hj["seq"].as_u64(), l.head.as_ref().map(|h| h.seq)), (Some(2), Some(2)));
    drop(l);
    r4_verify(&p);
    // deleting the newest line is truncation: head.json is ahead → rule 7, and open refuses (Halted{lineage})
    let t = text(&p);
    let cut = t[..t.len() - 1].rfind('\n').unwrap() + 1;
    fs::write(p.r("lineage/lineage.jsonl"), &t[..cut]).unwrap();
    assert!(Lineage::open(&p.rh, None).err().unwrap().to_string().contains("7:"));
    assert_eq!(audit_verify(&p, true, false), 1);
}

#[test]
fn rr18_kernel_records_kinds_limits_privacy_and_rate() {
    let p = home("rr18");
    let mut l = genesis(&p);
    let deny = [p.home.display().to_string(), p.rh.display().to_string(), p.ih.display().to_string()];
    let k = kctx(&deny, &no_relay);
    let mut n = 0;
    let mut send = |l: &mut Lineage, kind: &str, data: Value| {
        n += 1;
        l.append_kernel(&k, kind, data, &format!("b00t:{n}")).map(|r| r.0).map_err(|e| e.0)
    };
    assert_eq!(send(&mut l, "nonsense", json!({})), Err("root.lineage"));
    assert_eq!(send(&mut l, "ladder", json!({"from": 0, "to": 4})), Err("root.lineage"), "a Root kind from the kernel");
    assert_eq!(send(&mut l, "x_my_kind", json!({"a": 1})).unwrap()["dup"], false);
    assert_eq!(send(&mut l, "x_", json!({})), Err("root.lineage"));
    assert_eq!(send(&mut l, "x_Bad", json!({})), Err("root.lineage"));
    assert_eq!(send(&mut l, "fleet_campaign", json!({"x": [1.5]})), Err("root.lineage"), "a float");
    assert_eq!(send(&mut l, "fleet_campaign", json!({"x": "y".repeat(17 * 1024)})), Err("root.lineage"), "17 KiB");
    let deep = (0..9).fold(json!(1), |v, _| json!([v]));
    assert_eq!(send(&mut l, "fleet_campaign", json!({"d": deep})), Err("root.lineage"), "depth 10");
    let path = format!("{}/x", p.home.display());
    assert_eq!(send(&mut l, "task_suspended", json!({"task_id": path, "reason": "x"})), Err("root.lineage"), "a home path");
    assert_eq!(send(&mut l, "fetch", json!({"remote": "git@github.com:someone/repo.git"})), Err("root.lineage"), "an address");
    assert_eq!(send(&mut l, "fetch", json!({"owner@example.com": 1})), Err("root.lineage"), "an address in a key");
    assert_eq!(send(&mut l, "fetch", json!({"author": "INIT Agent <agent@init.local>", "ref": "HEAD@{1}"})).unwrap()["dup"], false);
    assert_eq!(send(&mut l, "promotion", json!({"sha": "a"})), Err("root.lineage"), "required fields");
    let promo = json!({"sha": "a", "from_sha": "b", "gate_job": null, "scope": null, "lkg": true, "body_binary": "c", "ext_set": null, "components": []});
    assert_eq!(send(&mut l, "promotion", promo.clone()).unwrap()["dup"], false);
    let mut bad = promo.clone();
    bad["lkg"] = json!("yes");
    assert_eq!(send(&mut l, "promotion", bad), Err("root.lineage"));
    for kind in ["task_suspended", "fleet_campaign", "gate", "body_safe", "kernel_start", "owner_signal"] {
        let d = match kind {
            "task_suspended" => json!({"task_id": "t-1", "reason": "task_flaky"}),
            "gate" => json!({"gate_id": "j-1", "gate_kind": "g4", "sha": "a", "verdict": "reject", "reason": "x"}),
            "body_safe" => json!({"on": true, "sha": "a", "crashes": []}),
            _ => json!({"campaign": "c-1"}),
        };
        assert_eq!(send(&mut l, kind, d).map(|v| v["dup"].clone()), Ok(json!(false)), "{kind}");
    }
    let r = send(&mut l, "x_count", json!({})).unwrap();
    let rec: Value = serde_json::from_str(text(&p).lines().last().unwrap()).unwrap();
    assert_eq!((&rec["by"], &rec["seq"], &rec["hash"]), (&json!("kernel:0123456789ab"), &r["seq"], &r["hash"]));
    assert!(!text(&p).contains("b00t:"), "idem keys stay out of the chain");
    // RR-18: 601 records in an hour → the 601st is root.rate (the window counts accepted records)
    let have = l.kernel_q.len();
    for _ in have..600 {
        assert!(send(&mut l, "x_flood", json!({})).is_ok());
    }
    assert_eq!(send(&mut l, "x_flood", json!({})), Err("root.rate"));
    drop(l);
    r4_verify(&p);
}

#[test]
#[cfg(feature = "fault-injection")] // INIT_FAULT points
fn rr18b_idem_dedup_survives_crashes_at_every_point() {
    let p = home("idem");
    let mut l = genesis(&p);
    let deny: [String; 0] = [];
    let k = kctx(&deny, &no_relay);
    let a = l.append_kernel(&k, "kernel_start", json!({"n": 1}), "kb:1").unwrap().0;
    let b = l.append_kernel(&k, "kernel_start", json!({"n": 1}), "kb:1").unwrap().0;
    assert_eq!((a["dup"].clone(), b["dup"].clone(), &a["seq"], &a["hash"]), (json!(false), json!(true), &b["seq"], &b["hash"]));
    // `root.lineage.after_idem`: key indexed, record never written → the resend appends exactly once
    l.fault = "root.lineage.after_idem:enospc".into();
    assert_eq!(l.append_kernel(&k, "kernel_start", json!({"n": 2}), "kb:2").unwrap_err().0, "root.internal");
    drop(l);
    let mut l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    let c = l.append_kernel(&k, "kernel_start", json!({"n": 2}), "kb:2").unwrap().0;
    assert_eq!((c["dup"].clone(), c["seq"].as_u64()), (json!(false), Some(2)));
    // `root.lineage.after_append`: record written, answer lost → the resend gets the original seq, dup:true
    l.fault = "root.lineage.after_append:enospc".into();
    assert!(l.append_kernel(&k, "kernel_start", json!({"n": 3}), "kb:3").is_err());
    drop(l);
    let mut l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    let d = l.append_kernel(&k, "kernel_start", json!({"n": 3}), "kb:3").unwrap().0;
    assert_eq!((d["dup"].clone(), d["seq"].as_u64()), (json!(true), Some(3)));
    assert_eq!(l.append_kernel(&k, "kernel_start", json!({"n": 1}), "kb:1").unwrap().0["seq"], a["seq"]);
    assert_eq!(text(&p).lines().filter(|x| x.contains("\"kind\":\"kernel_start\"")).count(), 3, "one record per key");
    // keys older than 7 days are dropped at open; a key with a stale hash is never trusted
    let aged: String = fs::read_to_string(p.r("lineage/idem.jsonl")).unwrap().lines().map(|x| {
        let mut e: Value = serde_json::from_str(x).unwrap();
        if e["idem"] == "kb:1" {
            e["at"] = json!("2000-01-01T00:00:00.000Z");
        }
        icj(&e).unwrap() + "\n"
    }).collect();
    fs::write(p.r("lineage/idem.jsonl"), aged).unwrap();
    drop(l);
    let mut l = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    assert!(!fs::read_to_string(p.r("lineage/idem.jsonl")).unwrap().contains("\"kb:1\""));
    assert_eq!(l.append_kernel(&k, "kernel_start", json!({"n": 1}), "kb:1").unwrap().0["dup"], false);
    assert_eq!(l.append_kernel(&k, "x_y", json!({}), "").unwrap_err().0, "root.args");
    drop(l);
    r4_verify(&p);
}

#[test]
fn rr18c_owner_records_need_a_relayed_qid_with_that_signal() {
    let p = home("owner");
    let mut l = genesis(&p);
    let deny: [String; 0] = [];
    let relayed = |q: &str| match q {
        "q-1" => Some("/bless j-7".to_string()),
        "q-2" => Some("/accept t-3".to_string()),
        _ => None,
    };
    let k = kctx(&deny, &relayed);
    let owner = |sig: &str, qid: &str| json!({"signal": sig, "qid": qid, "target": "j-7"});
    assert_eq!(l.append_kernel(&k, "owner", owner("bless", "q-9"), "o:1").unwrap_err().0, "root.lineage", "never relayed");
    assert_eq!(l.append_kernel(&k, "owner", owner("bless", "q-2"), "o:2").unwrap_err().0, "root.lineage", "another first word");
    assert_eq!(l.append_kernel(&k, "owner", owner("approve", "q-1"), "o:3").unwrap_err().0, "root.lineage", "not a signal");
    assert_eq!(l.append_kernel(&k, "owner", owner("bless", "q-1"), "o:4").unwrap().0["dup"], false);
    drop(l);
    r4_verify(&p);
}

#[test]
fn root_kinds_ceiling_hit_cap_and_the_view() {
    let p = home("root");
    let mut l = genesis(&p);
    assert!(l.append("root", "promotion", json!({})).is_err(), "a kernel kind is not a Root kind");
    assert!(l.append("root", "x_thing", json!({})).is_err());
    for kind in ["egress_action", "family_eval", "audit", "mutation", "zone_start", "ladder"] {
        l.append("root", kind, json!({"k": kind})).unwrap();
    }
    l.per_hour = 3;
    for i in 0..3 {
        l.append("root", "ceiling_hit", json!({"demotes": true, "i": i})).unwrap();
    }
    assert!(l.append("root", "ceiling_hit", json!({"demotes": true})).unwrap_err().to_string().starts_with("root.rate"));
    l.append("root", "ceiling_hit", json!({"demotes": false})).unwrap();
    assert!(l.append("root", "stop", json!({"x": 1.5})).is_err());
    // the view: a byte copy kept by appends; rebuilt at open when it drifted, also when it is gone
    assert!(fs::read(p.i("root/lineage.jsonl")).unwrap() == fs::read(p.r("lineage/lineage.jsonl")).unwrap());
    fs::write(p.i("root/lineage.jsonl"), "{}\n").unwrap();
    l.append("root", "stop", json!({})).unwrap();
    assert!(fs::read(p.i("root/lineage.jsonl")).unwrap() == fs::read(p.r("lineage/lineage.jsonl")).unwrap(), "rebuilt on a length mismatch");
    drop(l);
    fs::write(p.i("root/lineage.jsonl"), text(&p).replace("\"k\":\"audit\"", "\"k\":\"AUDIT\"")).unwrap();
    let _ = Lineage::open(&p.rh, Some(p.i("root/lineage.jsonl"))).unwrap();
    fs::copy(p.r("keys/lineage.pub"), p.i("root/lineage.pub")).unwrap();
    r4_verify(&p);
    // the agent's own check of a tampered copy fails, at the record
    fs::write(p.i("root/lineage.jsonl"), text(&p).replace("\"k\":\"audit\"", "\"k\":\"AUDIT\"")).unwrap();
    assert_eq!(audit_verify(&p, true, true), 1);
}

fn write_charter(p: &Paths, extra: &str) {
    let src = format!("schema_version = 1\n[purpose]\ntext = \"WP23 test home.\"\n[autonomy]\nmax_level = 4\n[quotas]\n\
        disk_min_free_gib = 0\n[root]\ntest_mode = true\n{extra}\n");
    fs::write(p.r("state/charter.active.toml"), src).unwrap();
}

#[test]
fn rr19_mirror_to_a_local_bare_repo() {
    let p = home("mirror");
    let remote = p.home.join("remote.git");
    tgit(&p, &remote, &["init", "--bare", "-q"]);
    // a planted owner identity, wherever git could look for it: none of it may reach the mirror
    for d in [p.home.clone(), p.r("cache/git-home")] {
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(".gitconfig"), "[user]\n\tname = Owner Name\n\temail = owner@example.invalid\n").unwrap();
    }
    let url = format!("file://{}", remote.display().to_string().replace(' ', "%20"));
    write_charter(&p, &format!("[mirror]\nurl = \"{url}\"\npush_interval_s = 1\n"));
    let ch = crate::charter::load(&p.r("state/charter.active.toml"), &p.ctx()).unwrap();
    let c = mirror::Cfg::of(&p, &ch.ch).expect("a file:// mirror in test mode");
    // off by default, and never a network URL in test mode
    let mut off = ch.ch.clone();
    off.mirror.url = String::new();
    assert!(mirror::Cfg::of(&p, &off).is_none());
    off.mirror.url = "git@github.com:nagisanzenin/INIT-Lineage.git".into();
    assert!(mirror::Cfg::of(&p, &off).is_none());
    off.root.test_mode = false;
    off.mirror.url = url.clone();
    assert!(mirror::Cfg::of(&p, &off).is_none());
    let mut l = genesis(&p);
    l.append("root", "charter", json!({"n": 1})).unwrap();
    let (seq, c1) = mirror::push(&p, &c).ok().unwrap();
    assert_eq!(seq, 1);
    assert_eq!(tgit(&p, &remote, &["rev-parse", "refs/heads/lineage"]), c1);
    assert_eq!(tgit(&p, &remote, &["show", "lineage:lineage-000000.jsonl"]) + "\n", text(&p));
    assert_eq!(tgit(&p, &remote, &["show", "lineage:lineage.pub"]) + "\n", fs::read_to_string(p.r("keys/lineage.pub")).unwrap());
    let hj: Value = serde_json::from_str(&tgit(&p, &remote, &["show", "lineage:HEAD.json"])).unwrap();
    assert_eq!((hj["seq"].as_u64(), hj["hash"].as_str()), (Some(1), l.head.as_ref().map(|h| h.hash.as_str())));
    assert_eq!(mirror::check_prefix(&p, &c, &text(&p)).ok(), Some(Some(1)));
    assert_eq!(audit_verify(&p, true, false), 0);
    l.append("root", "stop", json!({})).unwrap();
    let (_, c2) = mirror::push(&p, &c).ok().unwrap();
    assert_eq!(tgit(&p, &remote, &["rev-parse", "lineage^"]), c1, "a fast-forward on the previous commit");
    let (_, c3) = mirror::push(&p, &c).ok().unwrap();
    assert_eq!(c3, c2, "nothing new: no new commit");
    let log = tgit(&p, &remote, &["log", "--format=%an <%ae> %ad|%cn <%ce> %cd|%s", "--date=raw", "lineage"]);
    for line in log.lines() {
        assert!(line.starts_with("init-root <root@init.invalid> ") && line.contains(" +0000|init-root <root@init.invalid> "), "{line}");
    }
    let all = tgit(&p, &remote, &["cat-file", "--batch-all-objects", "--batch"]);
    assert!(!all.contains("owner@example.invalid") && !all.contains("Owner Name") && !all.contains(&p.home.display().to_string()));
    // the mirror-ref panic channel sees a panic branch
    assert_eq!(mirror::panic_ref(&p, &c), Ok(None));
    tgit(&p, &remote, &["update-ref", "refs/heads/panic", &c1]);
    assert_eq!(mirror::panic_ref(&p, &c), Ok(Some(c1.clone())));
    // the remote history is rewritten: rule 8 fails, and the next push is refused as diverged, never forced
    let blob = |s: &str| {
        let f = p.home.join("blob.tmp");
        fs::write(&f, s).unwrap();
        tgit(&p, &remote, &["hash-object", "-w", f.to_str().unwrap()])
    };
    let fake = format!("{{\"at\":\"2026-01-01T00:00:00.000Z\",\"hash\":\"sha256:{}\",\"seq\":0}}\n", "1".repeat(64));
    let tree = format!("100644 blob {}\tHEAD.json\n", blob(&fake));
    let t = Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg(format!("--git-dir={}", remote.display())).arg("mktree").stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut t = t;
    t.stdin.take().unwrap().write_all(tree.as_bytes()).unwrap();
    let t = String::from_utf8(t.wait_with_output().unwrap().stdout).unwrap().trim().to_string();
    let rewritten = tgit(&p, &remote, &["commit-tree", &t, "-m", "rewritten"]);
    tgit(&p, &remote, &["update-ref", "refs/heads/lineage", &rewritten]);
    assert!(matches!(mirror::check_prefix(&p, &c, &text(&p)), Err(mirror::Prefix::Bad(0, _))));
    assert_eq!(audit_verify(&p, true, false), 1, "rule 8");
    l.append("root", "stop", json!({})).unwrap();
    assert!(matches!(mirror::push(&p, &c), Err(mirror::Fail::Diverged(_))));
    assert_eq!(tgit(&p, &remote, &["rev-parse", "refs/heads/lineage"]), rewritten, "never forced");
}

#[test]
fn mirror_chunks_reuse_full_blobs() {
    let p = home("chunks");
    let remote = p.home.join("remote.git");
    tgit(&p, &remote, &["init", "--bare", "-q"]);
    write_charter(&p, &format!("[mirror]\nurl = \"file://{}\"\n", remote.display().to_string().replace(' ', "%20")));
    let ch = crate::charter::load(&p.r("state/charter.active.toml"), &p.ctx()).unwrap();
    let c = mirror::Cfg::of(&p, &ch.ch).unwrap();
    let mut l = genesis(&p);
    let mut lines = text(&p);
    for n in 1..10_003u64 {
        let (_, line, head) = l.seal(1_760_947_262_345, "root", "stop", json!({"n": n})).unwrap();
        lines += &line;
        l.head = Some(head);
    }
    fs::write(p.r("lineage/lineage.jsonl"), &lines).unwrap();
    l.write_head().unwrap();
    mirror::push(&p, &c).ok().unwrap();
    let ls = |r: &str| tgit(&p, &remote, &["ls-tree", r]);
    let before = ls("lineage");
    assert_eq!(before.lines().count(), 4, "HEAD.json, two chunks, lineage.pub: {before}");
    assert_eq!(tgit(&p, &remote, &["cat-file", "-p", "lineage:lineage-000001.jsonl"]).lines().count(), 3);
    l.len = lines.len() as u64;
    l.append("root", "stop", json!({})).unwrap();
    mirror::push(&p, &c).ok().unwrap();
    let after = ls("lineage");
    let id = |t: &str, n: &str| t.lines().find(|x| x.ends_with(n)).unwrap().split_whitespace().nth(2).unwrap().to_string();
    assert_eq!(id(&before, "lineage-000000.jsonl"), id(&after, "lineage-000000.jsonl"));
    assert_ne!(id(&before, "lineage-000001.jsonl"), id(&after, "lineage-000001.jsonl"));
    r4_verify(&p);
}

#[test]
fn kernel_git_bundles_refs_and_agent_sources() {
    let p = home("kgit");
    let src = p.home.join("src.git");
    tgit(&p, &src, &["init", "--bare", "-q"]);
    let commit = |parent: Option<&str>, msg: &str| {
        let e = tgit(&p, &src, &["hash-object", "-w", "-t", "tree", "/dev/null"]);
        let mut a = vec!["commit-tree", e.as_str(), "-m", msg];
        a.extend(parent.iter().flat_map(|x| ["-p", *x]));
        tgit(&p, &src, &a)
    };
    let c1 = commit(None, "kernel 0.1.0");
    tgit(&p, &src, &["update-ref", "refs/heads/release", &c1]);
    let bundle = p.i("kernel/src/0.1.0.bundle");
    tgit(&p, &src, &["bundle", "create", bundle.to_str().unwrap(), "refs/heads/release"]);
    let git = "/usr/bin/git";
    tgit(&p, &p.i("lineage/kernel.git"), &["init", "--bare", "-q"]);
    assert_eq!(kgit::import_release(&p, git).unwrap(), vec![("0.1.0".to_string(), c1.clone())]);
    assert_eq!(kgit::get(&p, git, "refs/heads/lkg").as_deref(), Some(c1.as_str()), "§4.3 step 6: lkg names the release too (§5 L4)");
    assert_eq!(kgit::get(&p, git, "refs/upstream/0.1.0").as_deref(), Some(c1.as_str()));
    assert_eq!(kgit::get(&p, git, "refs/heads/main").as_deref(), Some(c1.as_str()));
    assert_eq!(kgit::import(&p, git, &bundle, "0.1.0").unwrap(), c1, "the same commit again: a no-op");
    let c1b = commit(None, "another 0.1.0");
    tgit(&p, &src, &["update-ref", "refs/heads/release", &c1b]);
    let b2 = p.home.join("other.bundle");
    tgit(&p, &src, &["bundle", "create", b2.to_str().unwrap(), "refs/heads/release"]);
    assert!(kgit::import(&p, git, &b2, "0.1.0").unwrap_err().contains("refused"));
    assert!(kgit::import(&p, git, &b2, "../x").is_err());
    // an agent source: fetched by id through the given upload-pack, archived create-only, must descend from main
    let agent = p.i("work/kcand.git");
    tgit(&p, &agent, &["init", "--bare", "-q"]);
    tgit(&p, &agent, &["fetch", "-q", src.to_str().unwrap(), &format!("{c1}:refs/heads/base")]);
    let c2 = {
        let e = tgit(&p, &agent, &["hash-object", "-w", "-t", "tree", "/dev/null"]);
        tgit(&p, &agent, &["commit-tree", &e, "-p", &c1, "-m", "agent kernel"])
    };
    tgit(&p, &agent, &["update-ref", "refs/heads/kevo", &c2]);
    let up = "git -c uploadpack.allowAnySHA1InWant=true upload-pack";
    kgit::fetch_agent(&p, git, up, &agent, &c2).unwrap();
    assert_eq!(kgit::get(&p, git, &format!("refs/init/karchive/{c2}")).as_deref(), Some(c2.as_str()));
    assert!(kgit::fetch_agent(&p, git, up, &agent, &c2).unwrap_err().contains("create-only"));
    assert!(kgit::fetch_agent(&p, git, up, &p.home.join("src.git"), &c1b).is_err(), "only repos under INIT_HOME");
    tgit(&p, &agent, &["fetch", "-q", src.to_str().unwrap(), &format!("{c1b}:refs/heads/other")]);
    assert!(kgit::fetch_agent(&p, git, up, &agent, &c1b).unwrap_err().contains("does not descend"));
    // install moves main (CAS); a stale CAS fails; rollback keeps the left source
    kgit::update(&p, git, "refs/heads/main", &c2, Some(&c1)).unwrap();
    assert!(kgit::update(&p, git, "refs/heads/main", &c1, Some(&c1)).is_err());
    kgit::update(&p, git, "refs/heads/lkg", &c2, Some(&c1)).unwrap(); // install created lkg at the release (§5 L4)
    kgit::rollback(&p, git, &c1).unwrap();
    assert_eq!(kgit::get(&p, git, "refs/heads/main").as_deref(), Some(c1.as_str()));
    assert_eq!(kgit::get(&p, git, &format!("refs/init/rolled-back/{c2}")).as_deref(), Some(c2.as_str()));
}
