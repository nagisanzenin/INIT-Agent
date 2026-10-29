//! `init-kernel install --from-bundle <kernel/seed/<tag>.bundle> [--seed-tag <tag>]`
//! (supervise.md §4.8, §3.11.4; root/root.md §4.3 step 8): the Root spawns the kernel in install
//! mode (`reason:"install"`, fds 3 and 4). The kernel
//! 1. creates the zone's part of the layout;
//! 2. imports the bundle into the `lineage/seed.git` mirror (`git bundle verify` first) and
//!    picks a **tag** (`--seed-tag`, default the highest-semver `seed-v*`);
//! 3. creates `lineage/body.git` (with the export-neutral `info/attributes`), copies the tag in
//!    as `refs/upstream/<tag>`, and sets `main` and `lkg` to it (create-only);
//! 4. clones `body/` (origin = lineage, upstream = the seed mirror) before any agent code runs;
//! 5. builds the seed in the build slot through `spawn{kind:"build"}`, installs it as
//!    `bin/init-body` and `bin/init-body.lkg`. With a prebuilt body (a release bundle, docs/ux-v1.md
//!    §3.5.1) the Root has put `kernel/prebuilt/init-body` and `init-body.json` (`{v, sha256, commit}`)
//!    there instead: the commit must be the seed tag's, the copy's sha256 the recorded one, and no build
//!    runs, so the install needs no Rust toolchain;
//! 6. `store.put`s the binary and reports `promotion{lkg:true}` (waiting for the Root's `seq`),
//!    from which the Root forms the genesis tuple; writes `lkg.json` (`reason:"seed"`) and exports
//!    `lineage/trees/<sha12>`;
//! 7. exits 0 (1 on any failure, with the reason on stderr and in an `alert`).
//!
//! It writes nothing under `kernel/` (Root-written) and no service file.

use crate::build::BuildSlot;
use crate::config;
use crate::fsx;
use crate::git::{Git, GIT_TIMEOUT};
use crate::link::{Inbound, RootLink};
use crate::outbox::Outbox;
use init_proto::files::{self as pf, LkgFile};
use init_proto::icj::V1;
use init_proto::root_link::{AlertArgs, AlertLevel, BootBlock, HelloArgs, KernelRequest, StoreKind, StorePutArgs};
use serde_json::{json, Value};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Parse `install …` arguments and run with fds 3 and 4.
pub fn main_install(args: Vec<String>) -> i32 {
    let mut bundle = None;
    let mut tag = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from-bundle" => bundle = it.next().map(PathBuf::from),
            "--seed-tag" => tag = it.next(),
            other => {
                eprintln!("init-kernel install: unknown argument {other}");
                return 64;
            }
        }
    }
    let Some(bundle) = bundle else {
        eprintln!("usage: init-kernel install --from-bundle <kernel/seed/<tag>.bundle> [--seed-tag <tag>]");
        return 64;
    };
    let Some(home) = std::env::var_os("INIT_HOME").map(PathBuf::from) else {
        eprintln!("init-kernel install: INIT_HOME is not set");
        return 64;
    };
    if !crate::sys::fd_is_open(3) || !crate::sys::fd_is_open(4) {
        eprintln!("init-kernel install: control link fds 3 and 4 missing (only the Root runs install)");
        return 64;
    }
    let _ = crate::sys::set_cloexec(3);
    let _ = crate::sys::set_cloexec(4);
    let (Ok(ctl), Ok(pass)) = (crate::sys::own_inherited_fd(3), crate::sys::own_inherited_fd(4)) else { return 64 };
    run(&home, ctl, pass, &bundle, tag.as_deref())
}

/// Install mode on the control link. Returns the exit code.
pub fn run(home: &Path, ctl: OwnedFd, pass: OwnedFd, bundle: &Path, seed_tag: Option<&str>) -> i32 {
    let (ltx, _lrx) = mpsc::channel::<Inbound>();
    let link = match RootLink::start(ctl, pass, ltx, Duration::from_millis(2000)) {
        Ok(l) => l,
        Err(_) => return 69,
    };
    let hello = KernelRequest::Hello(HelloArgs { kernel_version: crate::supervisor::KERNEL_VERSION.into(), proto: init_proto::ROOT_PROTO });
    let boot: BootBlock = match link.call(&hello, Duration::from_secs(30)).ok().and_then(|v| serde_json::from_value(v).ok()) {
        Some(b) => b,
        None => return 69,
    };
    let cfg = match config::load(&home.join("kernel/config.toml")) {
        Ok(c) => c,
        Err(errs) => {
            for e in errs {
                eprintln!("config: {e}");
            }
            return 78;
        }
    };
    // `ready` early, before the seed build: the Root bounds the whole install at `INSTALL_TIMEOUT_S`
    // (4,500 s, over the cold build's 3,600 s), and hb keeps flowing while it builds.
    let _ = link.call(&KernelRequest::Ready, link.request_timeout);
    crate::supervisor::start_hb(link.clone(), Duration::from_secs(boot.limits.hb_interval_s.max(1)), "install");
    match install(home, &link, &cfg, bundle, seed_tag) {
        Ok(sha) => {
            crate::klog::log("install", &format!("installed seed {sha}"));
            0
        }
        Err(e) => {
            eprintln!("init-kernel install: {e}");
            let _ = link.call(&KernelRequest::Alert(AlertArgs { code: "install_failed".into(), level: AlertLevel::Alert, message: fsx::tail_str(&e, 500) }), link.request_timeout);
            1
        }
    }
}

fn semver(tag: &str) -> Option<(u64, u64, u64)> {
    let v = tag.strip_prefix("seed-v")?;
    let mut p = v.split('.').map(|x| x.parse::<u64>().ok());
    Some((p.next()??, p.next()??, p.next()??))
}

fn install(home: &Path, link: &std::sync::Arc<RootLink>, cfg: &config::KernelCfg, bundle: &Path, seed_tag: Option<&str>) -> Result<String, String> {
    fn e(what: &str) -> impl Fn(crate::git::GitError) -> String + '_ {
        move |x| format!("{what}: {x}")
    }
    // 1. layout (zone part; kernel/ and root/ are the Root's).
    for d in ["bin", "body", "work/tmp", "work/home", "work/cargo-home", "state/kernel/logs", "state/run", "state/sessions", "lineage/trees", "cache/git-home", "vault", "probe", "run"] {
        std::fs::create_dir_all(home.join(d)).map_err(|x| format!("mkdir {d}: {x}"))?;
    }
    for d in ["vault", "cache", "run", "probe"] {
        let _ = fsx::mkdir_mode(&home.join(d), 0o700);
    }
    let git_bin = &cfg.supervise.git;
    crate::git::version_ok(git_bin)?;
    // 2. the seed mirror.
    let seed_dir = home.join("lineage/seed.git");
    let seed = Git::with_dir(git_bin, home, &seed_dir);
    if !seed_dir.is_dir() {
        seed.init_bare(&seed_dir).map_err(e("init lineage/seed.git"))?;
    }
    let b = bundle.to_string_lossy().into_owned();
    seed.run(&["bundle", "verify", "--quiet", &b], None, GIT_TIMEOUT).map_err(e("git bundle verify"))?;
    seed.run(&["fetch", "--no-tags", "--no-write-fetch-head", &b, "+refs/heads/seed:refs/heads/seed", "refs/tags/seed-v*:refs/tags/seed-v*"], None, GIT_TIMEOUT)
        .map_err(e("import the seed bundle"))?;
    let tags = seed.run(&["for-each-ref", "--format=%(refname:short)", "refs/tags/seed-v*"], None, GIT_TIMEOUT).map_err(e("list seed tags"))?.text();
    let tag = match seed_tag {
        Some(t) => t.to_string(),
        None => tags.lines().filter(|t| semver(t).is_some()).max_by_key(|t| semver(t)).ok_or("the bundle has no seed-v* tag")?.to_string(),
    };
    if !tags.lines().any(|t| t == tag) {
        return Err(format!("seed tag {tag} is not in the bundle"));
    }
    // 3. lineage/body.git at the tag.
    let body_dir = home.join("lineage/body.git");
    let g = Git::body(git_bin, home);
    if !body_dir.is_dir() {
        g.init_bare(&body_dir).map_err(e("init lineage/body.git"))?;
    }
    let sd = seed_dir.to_string_lossy().into_owned();
    g.run(&["fetch", "--no-tags", "--no-write-fetch-head", &sd, &format!("refs/tags/{tag}:refs/upstream/{tag}")], None, GIT_TIMEOUT).map_err(e("copy the seed tag"))?;
    let s = g.rev_parse_commit(&format!("refs/upstream/{tag}")).ok_or("the seed tag names no commit")?;
    for r in ["refs/heads/main", "refs/heads/lkg"] {
        match g.rev_parse_commit(r) {
            Some(cur) if cur == s => {}
            Some(cur) => return Err(format!("{r} already points at {cur}; install runs once")),
            None => g.update_ref(r, &s, Some(""), Some(&format!("init: install {tag}"))).map_err(e(r))?,
        }
    }
    // 4. the agent's clone (before any agent code exists).
    let body = home.join("body");
    if !body.join(".git").exists() {
        let ld = body_dir.to_string_lossy().into_owned();
        let bd = body.to_string_lossy().into_owned();
        let run = |args: &[&str]| crate::git::exec(g.plain(args), None, GIT_TIMEOUT).map(|_| ());
        run(&["clone", "-q", "--no-local", "--origin", "origin", &ld, &bd]).map_err(e("clone body/"))?;
        let gd = format!("--git-dir={bd}/.git");
        run(&[&gd, "remote", "add", "upstream", &sd]).map_err(e("body/ upstream"))?;
        run(&[&gd, "config", "--add", "remote.origin.fetch", "+refs/notes/*:refs/notes/*"]).map_err(e("body/ notes refspec"))?;
        run(&[&gd, "config", "--add", "remote.origin.fetch", "+refs/init/*:refs/remotes/origin/init/*"]).map_err(e("body/ init refspec"))?;
    }
    // 5. build the seed, or take the release's prebuilt body (Root-written, read-only to the zone).
    let sha256 = match prebuilt(home)? {
        Some((src, want, commit)) => {
            if commit.as_deref().is_some_and(|c| c != s) {
                return Err(format!("the prebuilt body was built from {}, not the seed tag's commit {s}", commit.unwrap_or_default()));
            }
            let got = stage_prebuilt(home, &src).map_err(|x| format!("stage the prebuilt body: {x}"))?;
            if got != want {
                let _ = std::fs::remove_file(home.join("bin/init-body.staged"));
                return Err(format!("the prebuilt body's sha256 is {got}, not the recorded {want}"));
            }
            crate::klog::log("install", &format!("prebuilt body {got} (no build)"));
            got
        }
        None => {
            let slot = BuildSlot::configured(home, &cfg.supervise);
            slot.reset().map_err(|x| format!("build slot: {x}"))?;
            slot.export(&g, &s).map_err(e("export the seed"))?;
            let ct = g.commit_time(&s).map_err(e("commit time"))?;
            let req = slot.build_request(ct, &cfg.sandbox);
            let out = slot.run_build(link.as_ref(), &req, false, &home.join("state/kernel/logs/install-build.log"));
            if !out.ok {
                return Err(format!("seed build: {} ({})", out.reason.unwrap_or("build_failed"), out.message));
            }
            slot.stage_binary().map_err(|x| format!("stage: {x}"))?
        }
    };
    let bin = home.join("bin");
    std::fs::rename(bin.join("init-body.staged"), bin.join("init-body")).map_err(|x| format!("install bin/init-body: {x}"))?;
    crate::binstore::install_lkg(&bin).map_err(|x| format!("bin/init-body.lkg: {x}"))?;
    // 6. store, report, record.
    let f = std::fs::File::open(bin.join("init-body")).map_err(|x| x.to_string())?;
    let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
    let put = KernelRequest::StorePut(StorePutArgs { kind: StoreKind::InitBody, name: None, sha256: sha256.clone(), bytes, job: None });
    link.call_with_fd(&put, f.as_fd(), crate::link::STORE_TIMEOUT).map_err(|x| format!("store.put: {x}"))?;
    let outbox = Outbox::open(home, &crate::sys::random_hex(8));
    outbox.attach(link.clone());
    let data = json!({"sha": s, "from_sha": Value::Null, "gate_job": Value::Null, "scope": "seed", "lkg": true, "body_binary": sha256, "ext_set": Value::Null, "components": []});
    let r = outbox.post_wait("promotion", data).map_err(|x| format!("promotion{{lkg:true}}: {x}"))?.seq;
    let sha12: String = s.chars().take(12).collect();
    let l = LkgFile {
        v: V1,
        sha: s.clone(),
        binary: "bin/init-body.lkg".into(),
        binary_sha256: sha256,
        tree: format!("lineage/trees/{sha12}"),
        reason: pf::LkgReason::Seed,
        lineage_seq: Some(r),
        seq: 1,
        prev_sha: None,
        gate_job: None,
        committed_at: crate::clock::now_rfc3339(),
        model: None,
        ext_set: None,
    };
    fsx::write_json(&home.join(pf::paths::LKG), &l).map_err(|x| format!("lkg.json: {x}"))?;
    let tree = home.join("lineage/trees").join(&sha12);
    if !tree.exists() {
        g.export_tree(&s, &tree).map_err(e("export the LKG tree"))?;
    }
    Ok(s)
}


/// `kernel/prebuilt/init-body` and its record `init-body.json` (`{"v":1,"sha256":<hex>,"commit":<sha>|null}`),
/// written by the Root from a release bundle. None when there is no record (build from source).
fn prebuilt(home: &Path) -> Result<Option<(PathBuf, String, Option<String>)>, String> {
    let dir = home.join("kernel/prebuilt");
    let rec = dir.join("init-body.json");
    let text = match std::fs::read_to_string(&rec) {
        Ok(t) => t,
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(x) => return Err(format!("{}: {x}", rec.display())),
    };
    let v: Value = serde_json::from_str(&text).map_err(|x| format!("{}: {x}", rec.display()))?;
    let sha = v["sha256"].as_str().filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let sha = sha.ok_or_else(|| format!("{}: sha256 must be 64 lowercase hex digits", rec.display()))?;
    let commit = v["commit"].as_str().map(String::from);
    Ok(Some((dir.join("init-body"), sha.to_string(), commit)))
}

/// Copies the prebuilt body to `bin/init-body.staged` as a new inode (as `BuildSlot::stage_binary` does with a
/// built one) and returns the copy's sha256.
fn stage_prebuilt(home: &Path, src: &Path) -> std::io::Result<String> {
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let bin = home.join("bin");
    fs::create_dir_all(&bin)?;
    let mut from = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(src)?;
    if !from.metadata()?.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the prebuilt init-body is not a regular file"));
    }
    let tmp = bin.join(".init-body.staged.tmp");
    match fs::remove_file(&tmp) {
        Err(x) if x.kind() != std::io::ErrorKind::NotFound => return Err(x),
        _ => {}
    }
    let res = (|| {
        let mut dst = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o755).open(&tmp)?;
        std::io::copy(&mut from, &mut dst)?;
        dst.set_permissions(fs::Permissions::from_mode(0o755))?;
        dst.sync_all()?;
        drop(dst);
        let sha = fsx::sha256_file(&tmp)?;
        fs::rename(&tmp, bin.join("init-body.staged"))?;
        fsx::sync_dir(&bin)?;
        Ok(sha)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod prebuilt_tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let h = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/prebuilt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join("kernel/prebuilt")).unwrap();
        h
    }

    #[test]
    fn no_record_means_build_from_source() {
        let h = home("none");
        assert_eq!(prebuilt(&h).unwrap(), None);
        std::fs::remove_dir_all(&h).unwrap(); // exact path, this test's own
    }

    #[test]
    fn a_prebuilt_body_is_staged_as_a_new_inode_with_its_sha256() {
        let h = home("ok");
        std::fs::write(h.join("kernel/prebuilt/init-body"), b"#!body\n").unwrap();
        let want = fsx::sha256_bytes(b"#!body\n");
        std::fs::write(h.join("kernel/prebuilt/init-body.json"), json!({"v": 1, "sha256": want, "commit": "c0ffee"}).to_string()).unwrap();
        let (src, sha, commit) = prebuilt(&h).unwrap().unwrap();
        assert_eq!((sha.as_str(), commit.as_deref()), (want.as_str(), Some("c0ffee")));
        assert_eq!(stage_prebuilt(&h, &src).unwrap(), want);
        assert_eq!(std::fs::read(h.join("bin/init-body.staged")).unwrap(), b"#!body\n");
        assert!(!h.join("bin/.init-body.staged.tmp").exists());
        std::fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn a_bad_record_or_a_symlinked_body_is_refused() {
        let h = home("bad");
        std::fs::write(h.join("kernel/prebuilt/init-body.json"), r#"{"v":1,"sha256":"ABC"}"#).unwrap();
        assert!(prebuilt(&h).unwrap_err().contains("64 lowercase hex"));
        std::fs::write(h.join("outside"), b"x").unwrap();
        std::os::unix::fs::symlink(h.join("outside"), h.join("kernel/prebuilt/init-body")).unwrap();
        assert!(stage_prebuilt(&h, &h.join("kernel/prebuilt/init-body")).is_err(), "O_NOFOLLOW");
        std::fs::remove_dir_all(&h).unwrap();
    }
}
