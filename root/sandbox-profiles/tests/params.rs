//! V1–V5 and the class rules against a real directory tree (root/root.md RR-4, RR-4b).
//! The tree lives under root/target/test-homes/wp6a/unit-<id>/ and is removed by its exact path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use init_sandbox_profiles::ProfileId;
use init_sandbox_profiles::params::*;

struct Tree {
    top: PathBuf,
    ctx: ValidationCtx,
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.top); // exact path created by this test
    }
}

fn tree(tag: &str) -> Tree {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/test-homes/wp6a");
    fs::create_dir_all(&base).unwrap();
    let base = fs::canonicalize(base).unwrap();
    let top = base.join(format!("unit-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&top);
    for d in [
        "i/body",
        "i/work",
        "i/state/kernel",
        "i/kernel",
        "i/bin",
        "i/lineage",
        "i/vault",
        "i/cache/build/slot/src",
        "i/cache/build/slot/out",
        "i/root",
        "i/run/fleet/b1",
        "i/probe/p1/body",
        "i/probe/p1/work",
        "i/probe/p1/state/kernel",
        "i/probe/p1/kernel",
        "i/probe/p1/bin",
        "i/probe/p1/run",
        "i/probe/p2/body",
        "i/probe/p2/work",
        "i/probe/p2/state/kernel",
        "i/probe/p2/bin",
        "i/probe/p2/run",
        "i/fleet/b1/body",
        "i/fleet/b1/work",
        "i/fleet/b1/state/kernel",
        "i/fleet/b1/kernel",
        "i/fleet/b1/bin",
        "tc",
        "dev",
        "home",
        "extra",
        "real",
    ] {
        fs::create_dir_all(top.join(d)).unwrap();
    }
    std::os::unix::fs::symlink(top.join("i/kernel"), top.join("i/probe/p2/kernel")).unwrap();
    std::os::unix::fs::symlink(top.join("real"), top.join("i/work/linked")).unwrap();
    let ctx = ValidationCtx {
        init_home: top.join("i"),
        toolchain: top.join("tc"),
        devdir: top.join("dev"),
        home_real: top.join("home"),
        extra_read: vec![top.join("extra")],
    };
    Tree { top, ctx }
}

fn m(pairs: &[(&str, PathBuf)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), v.to_string_lossy().into_owned())).collect()
}

fn prelude(t: &Tree) -> Vec<(&'static str, PathBuf)> {
    vec![("TOOLCHAIN", t.ctx.toolchain.clone()), ("DEVDIR", t.ctx.devdir.clone()), ("HOME_REAL", t.ctx.home_real.clone())]
}

fn body(t: &Tree, r: &Path, run: &Path, kernel: &Path, bin: &Path, link_kernel: &Path) -> BTreeMap<String, String> {
    let i = &t.ctx.init_home;
    let mut v = prelude(t);
    v.extend([
        ("INIT_HOME", r.to_path_buf()),
        ("BODY", r.join("body")),
        ("WORK", r.join("work")),
        ("STATE", r.join("state")),
        ("STATE_KERNEL", r.join("state/kernel")),
        ("KERNEL", kernel.to_path_buf()),
        ("BIN", bin.to_path_buf()),
        ("LINEAGE", i.join("lineage")),
        ("RUN", run.to_path_buf()),
        ("KERNEL_SOCK", run.join("kernel.sock")),
        ("PROXY_SOCK", run.join("proxy.sock")),
        ("VAULT", i.join("vault")),
        ("CACHE", i.join("cache")),
        ("PUB", i.join("root")),
        ("LINK_KERNEL", link_kernel.to_path_buf()),
        ("LINK_LINEAGE", i.join("lineage")),
        ("LINK_BIN", bin.to_path_buf()),
    ]);
    m(&v)
}

fn live(t: &Tree) -> BTreeMap<String, String> {
    let i = &t.ctx.init_home;
    body(t, i, &i.join("run"), &i.join("kernel"), &i.join("bin"), &i.join("kernel"))
}

fn build(t: &Tree) -> BTreeMap<String, String> {
    let i = &t.ctx.init_home;
    let mut v = prelude(t);
    v.extend([
        ("INIT_HOME", i.clone()),
        ("SRC", i.join("cache/build/slot/src")),
        ("OUT", i.join("cache/build/slot/out")),
        ("KERNEL", i.join("kernel")),
        ("VAULT", i.join("vault")),
    ]);
    m(&v)
}

fn rule_of(r: Result<(), ArgsError>) -> &'static str {
    r.expect_err("must be refused").rule
}

#[test]
fn valid_maps_pass() {
    let t = tree("ok");
    let i = t.ctx.init_home.clone();
    validate_spawn_params("body", ProfileId::Body, &live(&t), &t.ctx).unwrap();
    let p1 = i.join("probe/p1");
    let probe = body(&t, &p1, &p1.join("run"), &p1.join("kernel"), &p1.join("bin"), &p1.join("kernel"));
    validate_spawn_params("probe", ProfileId::Body, &probe, &t.ctx).unwrap();
    let b1 = i.join("fleet/b1");
    let member = body(&t, &b1, &i.join("run/fleet/b1"), &b1.join("kernel"), &b1.join("bin"), &b1.join("kernel"));
    validate_spawn_params("body", ProfileId::Body, &member, &t.ctx).unwrap();
    validate_spawn_params("build", ProfileId::Build, &build(&t), &t.ctx).unwrap();
    validate_spawn_params("check", ProfileId::Check, &build(&t), &t.ctx).unwrap();
    let mut g = prelude(&t);
    g.extend([("INIT_HOME", i.clone()), ("BODY", p1.join("body")), ("WORK", p1.join("work")), ("VAULT", i.join("vault")), ("CACHE", i.join("cache"))]);
    validate_spawn_params("git_read", ProfileId::GitRead, &m(&g), &t.ctx).unwrap();
}

/// RR-4b: a LINK_* naming a real symlink inside a probe root (D8) is accepted.
#[test]
fn link_symlink_in_probe_root_is_accepted() {
    let t = tree("link");
    let i = t.ctx.init_home.clone();
    let p2 = i.join("probe/p2");
    let probe = body(&t, &p2, &p2.join("run"), &i.join("kernel"), &p2.join("bin"), &p2.join("kernel"));
    validate_spawn_params("probe", ProfileId::Body, &probe, &t.ctx).unwrap();
    // The same symlink as an ordinary (non-LINK_*) value is not canonical.
    let mut bad = probe.clone();
    bad.insert("KERNEL".into(), p2.join("kernel").to_string_lossy().into_owned());
    assert_eq!(rule_of(validate_spawn_params("probe", ProfileId::Body, &bad, &t.ctx)), "V2");
    // In the live root a LINK_* must equal its target.
    let mut live_link = live(&t);
    live_link.insert("LINK_KERNEL".into(), i.join("work/linked").to_string_lossy().into_owned());
    assert!(validate_spawn_params("body", ProfileId::Body, &live_link, &t.ctx).is_err());
    // Inside a probe root, a LINK_* naming a real directory other than its target is refused.
    let mut real_dir = probe;
    real_dir.insert("LINK_KERNEL".into(), p2.join("body").to_string_lossy().into_owned());
    assert_eq!(rule_of(validate_spawn_params("probe", ProfileId::Body, &real_dir, &t.ctx)), "class");
}

/// RR-4: fuzzed spawn arguments are refused with root.args; nothing is spawned.
#[test]
fn refused_arguments() {
    let t = tree("bad");
    let i = t.ctx.init_home.clone();
    let ok = live(&t);
    let with = |k: &str, v: &str| {
        let mut x = ok.clone();
        x.insert(k.into(), v.into());
        x
    };
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let cases: Vec<(&str, BTreeMap<String, String>, &str)> = vec![
        ("Z_ name", with("Z_ROOT_HOME", "/"), "V1"),
        ("MARK", with("MARK", &s(i.join("run/mark/x"))), "V1"),
        ("lowercase name", with("body", &s(i.join("body"))), "V1"),
        ("unused name", with("SRC", &s(i.join("cache/build/slot/src"))), "V1"),
        ("relative", with("BODY", "i/body"), "V2"),
        ("dot-dot", with("BODY", &format!("{}/../i/body", s(i.join("work")))), "V2"),
        ("NUL", with("BODY", &format!("{}\0x", s(i.join("body")))), "V2"),
        ("symlinked value", with("WORK", &s(i.join("work/linked"))), "V2"),
        ("LINK_* with a non-canonical parent", with("LINK_BIN", &s(i.join("work/linked/x"))), "V2"),
        ("outside V3 roots", with("BODY", "/private/etc"), "V3"),
        ("/etc is a symlink", with("BODY", "/etc"), "V2"),
        ("BODY at vault/", with("BODY", &s(i.join("vault"))), "class"),
        ("another lane's socket", with("KERNEL_SOCK", &s(i.join("probe/p1/run/kernel.sock"))), "class"),
        ("PUB elsewhere", with("PUB", &s(i.join("lineage"))), "class"),
        ("foreign toolchain", with("TOOLCHAIN", &s(t.ctx.devdir.clone())), "class"),
        ("extra_read as BODY", with("BODY", &s(t.top.join("extra"))), "class"),
    ];
    for (what, params, want) in cases {
        let got = validate_spawn_params("body", ProfileId::Body, &params, &t.ctx);
        assert_eq!(rule_of(got), want, "{what}");
    }
    let mut missing = ok.clone();
    missing.remove("VAULT");
    assert_eq!(rule_of(validate_spawn_params("body", ProfileId::Body, &missing, &t.ctx)), "V1");
    let mut many = ok.clone();
    for n in 0..20 {
        many.insert(format!("X{n}"), s(i.join("work")));
    }
    assert_eq!(rule_of(validate_spawn_params("body", ProfileId::Body, &many, &t.ctx)), "V1");
    // V5: the kernel profile and the sealed kind are never available to the kernel.
    assert_eq!(rule_of(validate_spawn_params("body", ProfileId::Kernel, &ok, &t.ctx)), "V5");
    assert_eq!(rule_of(validate_spawn_params("sealed", ProfileId::Body, &ok, &t.ctx)), "V5");
    assert_eq!(rule_of(validate_spawn_params("build", ProfileId::Body, &ok, &t.ctx)), "V5");
    // build/check: SRC and OUT under cache/ only.
    let mut b = build(&t);
    b.insert("SRC".into(), s(i.join("body")));
    assert_eq!(rule_of(validate_spawn_params("build", ProfileId::Build, &b, &t.ctx)), "class");
    let mut b = build(&t);
    b.insert("OUT".into(), s(i.join("cache")));
    assert_eq!(rule_of(validate_spawn_params("build", ProfileId::Build, &b, &t.ctx)), "class");
    // OUT is also the only place a build may bind and connect Unix sockets (sandbox.md §3.4 section 6),
    // so it can never name a directory that holds lane sockets or the agent's own sockets.
    for (kind, profile) in [("build", ProfileId::Build), ("check", ProfileId::Check)] {
        for out in ["run", "run/fleet/b1", "probe/p1/run", "work", "body", "vault"] {
            let mut b = build(&t);
            b.insert("OUT".into(), s(i.join(out)));
            assert_eq!(rule_of(validate_spawn_params(kind, profile, &b, &t.ctx)), "class", "{kind} OUT={out}");
        }
    }
}

#[test]
fn v4_spawn_misc() {
    let t = tree("v4");
    let i = t.ctx.init_home.clone();
    let cwd = i.join("work").to_string_lossy().into_owned();
    let env: BTreeMap<String, String> = [("HOME".to_owned(), cwd.clone())].into();
    assert!(check_spawn_misc("/bin/true", &cwd, &env, &[], &t.ctx).is_ok());
    assert!(check_spawn_misc("true", &cwd, &env, &[], &t.ctx).is_err());
    assert!(check_spawn_misc("/bin/true", "/tmp", &env, &[], &t.ctx).is_err());
    let mut bad = env.clone();
    bad.insert("INIT_ROOT_FD".into(), "3".into());
    assert!(check_spawn_misc("/bin/true", &cwd, &bad, &[], &t.ctx).is_err());
    let big: Vec<String> = vec!["x".repeat(MAX_ITEM_BYTES + 1)];
    assert!(check_spawn_misc("/bin/true", &cwd, &env, &big, &t.ctx).is_err());
    let many: Vec<String> = vec![String::new(); MAX_ARGS + 1];
    assert!(check_spawn_misc("/bin/true", &cwd, &env, &many, &t.ctx).is_err());
}
