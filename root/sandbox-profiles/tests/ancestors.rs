//! v0.5.5 (REL-054 §7.5, should-fix 1): the folders above a protected path cannot be moved, removed or replaced, so renaming
//! a parent away no longer defeats a deny that matches by path: in full access (ADR-012, the full tail's Z8) and in workspace
//! mode (the zone tail's Z2 and Z3a). The real live-body profile, composed as the Root composes it (zone.rs `launcher`: the
//! access lists, `seatbelt::compose`), runs the crate's probe under `/usr/bin/sandbox-exec` over a test home under
//! /private/tmp laid out like an owner's install (`~/.local/bin` launchers, `~/.config/init-agent` keys,
//! `~/Library/LaunchAgents/dev.init.root.plist`, `~/.init-root`, `~/.init`). Each scenario has its own fixture, removed by
//! its exact path.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use init_sandbox_profiles::ProfileId;
use init_sandbox_profiles::access::{self, Fixed, Grant};
use init_sandbox_profiles::params::kernel_params;
use init_sandbox_profiles::seatbelt::{self, Rendered, ZoneLayout, create_mark};

const LAUNCHER: &str = "#!/bin/sh\n# INIT launcher (test copy)\n";
const PLANTED: &str = "#!/bin/sh\n# planted by the agent\n";
const PLIST: &str = "<plist><!-- the Root's service (test copy) --></plist>\n";
const KGIT_MAIN: &str = "0123456789abcdef0123456789abcdef01234567\n";

struct Fx {
    top: PathBuf,
    home: PathBuf,
    rh: PathBuf,
    ih: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.top); // exact path created by this test
    }
}

/// A test home with an owner's layout. `nested`: ROOT_HOME and INIT_HOME one folder down (`~/inst/`), as a test install
/// puts them, instead of the defaults `~/.init-root` and `~/.init`.
fn fixture(tag: &str, nested: bool) -> Fx {
    let top = PathBuf::from(format!("/private/tmp/init-anc-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&top);
    let home = top.join("home");
    let (rh, ih) = if nested { (home.join("inst/rh"), home.join("inst/ih")) } else { (home.join(".init-root"), home.join(".init")) };
    for d in ["body", "work", "state/kernel", "kernel", "bin", "lineage", "run/mark", "root", "vault", "cache"] {
        fs::create_dir_all(ih.join(d)).unwrap();
    }
    for d in ["keys/providers", "run"] {
        fs::create_dir_all(rh.join(d)).unwrap();
    }
    for d in [
        ".local/bin",
        ".config/init-agent",
        ".config/other",
        "Library/LaunchAgents",
        "Library/Caches",
        "Documents/tax",
        "Documents/notes",
        "ws",
        "prepared",
        "prepared-lib/LaunchAgents",
        "Library/Application Support/Google/Chrome/Default",
        "Library/Application Support/SomeApp",
        "Library/Application Support/prep",
        "proj/secrets",
        "proj/src",
        ".config/systemd/user",
        "Documents/Café/tax",
        "Documents/empty",
        "prepared-empty",
        "agentdir",
    ] {
        fs::create_dir_all(home.join(d)).unwrap();
    }
    for n in access::LAUNCHERS {
        fs::write(home.join(".local/bin").join(n), LAUNCHER).unwrap();
        fs::write(home.join("prepared").join(n), PLANTED).unwrap();
    }
    for (f, body) in [
        (".config/init-agent/deepseek.key", "sk-anc-fake-not-a-key\n"),
        (".config/other/x", "another app's config\n"),
        ("Library/LaunchAgents/dev.init.root.plist", PLIST),
        ("prepared-lib/LaunchAgents/dev.init.root.plist", PLANTED),
        ("Documents/tax/2025.txt", "a charter read_deny entry\n"),
        ("ws/a.txt", "workspace file\n"),
        ("Library/Application Support/Google/Chrome/Default/Cookies", "fake cookie store\n"),
        ("Library/Application Support/SomeApp/x", "app data\n"),
        ("proj/secrets/prod.env", "TOKEN=fake-not-a-token\n"),
        ("proj/secrets/other.txt", "not denied\n"),
        ("proj/src/main.rs", "fn main() {}\n"),
    ] {
        fs::write(home.join(f), body).unwrap();
    }
    fs::write(rh.join("keys/providers/fake"), "sk-anc-root-fake-not-a-key\n").unwrap();
    for d in ["lineage/kernel.git/refs/heads", "lineage/body.git", "probe", "cache/build/slot/out"] {
        fs::create_dir_all(ih.join(d)).unwrap();
    }
    fs::write(ih.join("lineage/kernel.git/refs/heads/main"), KGIT_MAIN).unwrap();
    create_mark(&ih.join("run/zone-mark")).unwrap();
    create_mark(&ih.join("run/mark/abc")).unwrap();
    Fx { top, home, rh, ih }
}

impl Fx {
    fn h(&self, rel: &str) -> String {
        self.home.join(rel).display().to_string()
    }

    /// The live body's profile for `grant`, with the Root's lists: the charter's `sandbox.read_deny` is `~/Documents/tax`, and
    /// `install.json`'s `bin_dir` is `~/.local/bin`.
    fn profile(&self, grant: &Grant) -> Rendered {
        self.profile_with(grant, &["Documents/tax"])
    }

    /// As [`Fx::profile`], with the charter's `sandbox.read_deny` = `charter` (relative to the home). A workspace grant's
    /// workspace must pass the Root's validation (`validate_workspace`, against both deny lists, as access.rs `ws_deny`).
    fn profile_with(&self, grant: &Grant, charter: &[&str]) -> Rendered {
        let layout = self.layout(grant, charter);
        let (i, home) = (&self.ih, &self.home);
        let dir = |k: &str| -> PathBuf {
            match k {
                "INIT_HOME" => i.clone(),
                "STATE_KERNEL" => i.join("state/kernel"),
                "KERNEL_SOCK" => i.join("run/kernel.sock"),
                "PROXY_SOCK" => i.join("run/proxy.sock"),
                "PUB" => i.join("root"),
                "LINK_KERNEL" => i.join("kernel"),
                "LINK_BIN" => i.join("bin"),
                "LINK_LINEAGE" => i.join("lineage"),
                "TOOLCHAIN" | "DEVDIR" => PathBuf::from("/usr"),
                "HOME_REAL" => home.clone(),
                k => i.join(k.to_lowercase()),
            }
        };
        let given: BTreeMap<String, String> = kernel_params(ProfileId::Body).into_iter().map(|k| (k.to_owned(), dir(k).display().to_string())).collect();
        seatbelt::compose(ProfileId::Body, &given, &layout, &i.join("run/mark/abc"), grant).unwrap()
    }

    /// The kernel's profile (`kernel` SECTION + the zone tail), with the Root's parameters (`kernel_profile_params`).
    fn kernel_profile(&self) -> Rendered {
        let layout = self.layout(&Grant::default(), &["Documents/tax"]);
        let given = layout.kernel_profile_params(Path::new("/usr"), Path::new("/usr"), &self.home);
        seatbelt::compose(ProfileId::Kernel, &given, &layout, &self.ih.join("run/mark/abc"), &Grant::default()).unwrap()
    }

    /// The zone layout for `grant` (the full lists for a full grant, as zone.rs `launcher` picks them).
    fn layout(&self, grant: &Grant, charter: &[&str]) -> ZoneLayout {
        let (home, bin) = (&self.home, self.home.join(".local/bin"));
        let charter: Vec<PathBuf> = charter.iter().map(|c| home.join(c)).collect();
        let others = access::other_installs(home, &self.rh, &self.ih);
        let svc = home.join("Library/LaunchAgents/dev.init.root.plist");
        let (read_deny, write_deny) = if grant.full {
            (access::full_read_deny(home, &[others, charter].concat()), access::full_write_deny(home, Some(&bin)))
        } else {
            (access::read_deny(home, &[others, charter].concat()), access::write_deny(home, &[bin]))
        };
        if let Some(ws) = &grant.write {
            let fixed = Fixed { home, root_home: &self.rh, init_home: &self.ih, root_svc_dir: &home.join("Library/LaunchAgents") };
            let valid = access::validate_workspace(&ws.display().to_string(), &fixed, &[read_deny.clone(), write_deny.clone()].concat());
            assert_eq!(valid.as_ref(), Ok(ws), "the workspace is one the Root accepts");
        }
        ZoneLayout { root_home: self.rh.clone(), init_home: self.ih.clone(), root_svc: svc, extra_read: vec![], home: home.clone(), read_deny, write_deny }
    }

    /// One probe op under `r`: Ok(()) when it succeeded, else its errno.
    fn run(&self, r: &Rendered, op: &[&str]) -> Result<(), i64> {
        let argv = r.sandbox_exec_argv();
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_init-sbx-selftest"))
            .arg("inner")
            .args(op)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&out.stdout);
        let v: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|_| panic!("{op:?}: no probe line: {line} {}", String::from_utf8_lossy(&out.stderr)));
        if v["ok"] == true { Ok(()) } else { Err(v["errno"].as_i64().unwrap_or(-1)) }
    }
}

const EPERM: i64 = 1;
const EEXIST: i64 = 17;

fn full() -> Grant {
    Grant { full: true, ..Grant::default() }
}

/// Every row's outcome, so one run shows the whole table; the test fails on any row that is not as expected.
#[derive(Default)]
struct Table(Vec<String>, usize);

impl Table {
    fn row(&mut self, what: &str, got: Result<(), i64>, want: Result<(), i64>) {
        let ok = got == want;
        self.1 += usize::from(!ok);
        self.0.push(format!("{} {what}: got {got:?}, want {want:?}", if ok { "ok  " } else { "FAIL" }));
    }
    fn holds(&mut self, what: &str, held: bool) {
        self.1 += usize::from(!held);
        self.0.push(format!("{} {what}", if held { "ok  " } else { "FAIL" }));
    }
    fn done(self) {
        let t = self.0.join("\n");
        eprintln!("{t}");
        assert_eq!(self.1, 0, "\n{t}");
    }
}

/// Renaming or swapping each folder above a protected path is refused (EPERM), from the path's parent up to the home: the bin
/// directory and its parent (the launchers), `~/.config` (the key directory), `~/Library` (the service directory), the
/// charter's entry's parent, and a nested ROOT_HOME's and INIT_HOME's parent.
#[test]
fn full_ancestors_cannot_be_renamed() {
    let mut t = Table::default();
    for (rel, to) in [(".local/bin", ".local/bin.old"), (".local", ".local2"), (".config", ".cfg"), ("Library", "Lib2"), ("Documents", "Docs2")] {
        let fx = fixture("rename", false);
        let r = fx.profile(&full());
        t.row(&format!("full: rename ~/{rel} away"), fx.run(&r, &["rename", &fx.h(rel), &fx.h(to)]), Err(EPERM));
        t.row(&format!("full: swap ~/{rel} with ~/prepared (RENAME_SWAP)"), fx.run(&r, &["swap", &fx.h(rel), &fx.h("prepared")]), Err(EPERM));
    }
    let fx = fixture("nested", true);
    let r = fx.profile(&full());
    t.row("full: rename ~/inst away (a nested ROOT_HOME's and INIT_HOME's parent)", fx.run(&r, &["rename", &fx.h("inst"), &fx.h("inst2")]), Err(EPERM));
    t.row("full: read the nested ROOT_HOME's keys after that rename", fx.run(&r, &["read", &fx.h("inst2/rh/keys/providers/fake")]), Err(2));
    t.done();
}

/// The two tricks of the finding fail: replacing INIT's launchers by swapping the bin directory, and reading a denied file
/// by renaming a parent folder; and the same swap of `~/Library` for the Root's service file.
#[test]
fn full_rename_tricks_fail() {
    let mut t = Table::default();
    let fx = fixture("launchers", false);
    let r = fx.profile(&full());
    t.row("full: mv ~/.local/bin ~/.local/bin.old", fx.run(&r, &["rename", &fx.h(".local/bin"), &fx.h(".local/bin.old")]), Err(EPERM));
    let _ = fx.run(&r, &["rename", &fx.h("prepared"), &fx.h(".local/bin")]);
    t.row("full: swap ~/prepared and ~/.local/bin in one step (RENAME_SWAP)", fx.run(&r, &["swap", &fx.h("prepared"), &fx.h(".local/bin")]), Err(EPERM));
    for n in access::LAUNCHERS {
        let now = fs::read_to_string(fx.home.join(".local/bin").join(n)).unwrap_or_default();
        t.holds(&format!("full: ~/.local/bin/{n} is still INIT's launcher after both"), now == LAUNCHER);
    }
    let fx = fixture("keys", false);
    let r = fx.profile(&full());
    t.row("full: mv ~/.config ~/.cfg", fx.run(&r, &["rename", &fx.h(".config"), &fx.h(".cfg")]), Err(EPERM));
    t.row("full: cat ~/.cfg/init-agent/deepseek.key", fx.run(&r, &["read", &fx.h(".cfg/init-agent/deepseek.key")]), Err(2));
    t.row("full: cat ~/.config/init-agent/deepseek.key (Z2, unchanged)", fx.run(&r, &["read", &fx.h(".config/init-agent/deepseek.key")]), Err(EPERM));
    t.row("full: mv ~/Documents ~/Docs2 (the charter entry's parent)", fx.run(&r, &["rename", &fx.h("Documents"), &fx.h("Docs2")]), Err(EPERM));
    t.row("full: cat ~/Docs2/tax/2025.txt", fx.run(&r, &["read", &fx.h("Docs2/tax/2025.txt")]), Err(2));
    let fx = fixture("service", false);
    let r = fx.profile(&full());
    t.row("full: mv ~/Library ~/Lib2", fx.run(&r, &["rename", &fx.h("Library"), &fx.h("Lib2")]), Err(EPERM));
    let _ = fx.run(&r, &["rename", &fx.h("prepared-lib"), &fx.h("Library")]);
    let now = fs::read_to_string(fx.home.join("Library/LaunchAgents/dev.init.root.plist")).unwrap_or_default();
    t.holds("full: the Root's service file is unchanged after mv ~/prepared-lib ~/Library", now == PLIST);
    t.done();
}

/// Ordinary work next to the protected paths still works: new files in the bin directory and in `~/Library`, another app's
/// config under `~/.config` (written, renamed, a new folder beside it), a sibling of the charter entry renamed, listing the
/// pinned folders; `mkdir` of a pinned folder that exists still answers EEXIST, so `mkdir -p` keeps working, and a missing
/// one can still be made.
#[test]
fn full_ordinary_work_beside_them() {
    let mut t = Table::default();
    let fx = fixture("ordinary", false);
    let r = fx.profile(&full());
    for (what, op) in [
        ("full: add a command to ~/.local/bin", vec!["write", &fx.h(".local/bin/newtool")]),
        ("full: edit ~/.config/other/x", vec!["write", &fx.h(".config/other/x")]),
        ("full: rename ~/.config/other", vec!["rename", &fx.h(".config/other"), &fx.h(".config/other2")]),
        ("full: create ~/.config/new-app", vec!["mkdir", &fx.h(".config/new-app")]),
        ("full: write ~/Library/Caches/x", vec!["write", &fx.h("Library/Caches/x")]),
        ("full: rename ~/Documents/notes (beside the charter entry)", vec!["rename", &fx.h("Documents/notes"), &fx.h("Documents/notes2")]),
        ("full: list ~/.config", vec!["list", &fx.h(".config")]),
        ("full: list ~/.local/bin", vec!["list", &fx.h(".local/bin")]),
        ("full: stat ~/Library", vec!["stat", &fx.h("Library")]),
        ("full: rename the workspace's file", vec!["rename", &fx.h("ws/a.txt"), &fx.h("ws/b.txt")]),
        (
            "full: swap ~/.config/other2 and ~/Documents/notes2 (neither is above a protected path)",
            vec!["swap", &fx.h(".config/other2"), &fx.h("Documents/notes2")],
        ),
    ] {
        t.row(what, fx.run(&r, &op), Ok(()));
    }
    for rel in [".config", ".local", ".local/bin", "Library"] {
        t.row(&format!("full: mkdir ~/{rel}, which exists"), fx.run(&r, &["mkdir", &fx.h(rel)]), Err(EEXIST));
    }
    // a missing folder above a protected path (~/.local/share/systemd/user) can still be made, then not moved
    t.row("full: mkdir ~/.local/share, missing", fx.run(&r, &["mkdir", &fx.h(".local/share")]), Ok(()));
    t.row("full: mkdir ~/.local/share/uv", fx.run(&r, &["mkdir", &fx.h(".local/share/uv")]), Ok(()));
    t.row("full: rename the new ~/.local/share away", fx.run(&r, &["rename", &fx.h(".local/share"), &fx.h(".local/s2")]), Err(EPERM));
    t.done();
}

/// v0.5.5: workspace mode pins the same folders (zone tail Z2 and Z3a). Its writes are confined to the workspace (Z3), so
/// what it adds is the folders above a deny entry that lie inside the workspace: a workspace that holds a deny entry two or
/// more levels down (`~/Library/Application Support` and the built-in browser profiles; a charter entry deep in a project).
/// Ordinary work in the workspace is unchanged.
#[test]
fn workspace_ancestors_cannot_be_renamed() {
    let mut t = Table::default();
    let fx = fixture("workspace", false);
    let support = fx.home.join("Library/Application Support");
    let r = fx.profile(&Grant { read_home: true, write: Some(support.clone()), full: false });
    let a = |rel: &str| support.join(rel).display().to_string();
    t.row("workspace ~/Library/Application Support: read Google/Chrome (Z2)", fx.run(&r, &["read", &a("Google/Chrome/Default/Cookies")]), Err(EPERM));
    t.row("workspace ~/Library/Application Support: mv Google G2", fx.run(&r, &["rename", &a("Google"), &a("G2")]), Err(EPERM));
    t.row("workspace ~/Library/Application Support: cat G2/Chrome/Default/Cookies", fx.run(&r, &["read", &a("G2/Chrome/Default/Cookies")]), Err(2));
    t.row("workspace ~/Library/Application Support: swap Google and prep", fx.run(&r, &["swap", &a("Google"), &a("prep")]), Err(EPERM));
    t.row("workspace ~/Library/Application Support: write SomeApp/x", fx.run(&r, &["write", &a("SomeApp/x")]), Ok(()));
    t.row("workspace ~/Library/Application Support: rename SomeApp", fx.run(&r, &["rename", &a("SomeApp"), &a("SomeApp2")]), Ok(()));
    t.row("workspace ~/Library/Application Support: mkdir NewApp", fx.run(&r, &["mkdir", &a("NewApp")]), Ok(()));
    let fx = fixture("project", false);
    let r = fx.profile_with(&Grant { read_home: true, write: Some(fx.home.join("proj")), full: false }, &["proj/secrets/prod.env"]);
    t.row("workspace ~/proj, read_deny ~/proj/secrets/prod.env: cat it (Z2)", fx.run(&r, &["read", &fx.h("proj/secrets/prod.env")]), Err(EPERM));
    t.row("workspace ~/proj: mv ~/proj/secrets ~/proj/s2", fx.run(&r, &["rename", &fx.h("proj/secrets"), &fx.h("proj/s2")]), Err(EPERM));
    t.row("workspace ~/proj: cat ~/proj/s2/prod.env", fx.run(&r, &["read", &fx.h("proj/s2/prod.env")]), Err(2));
    t.row("workspace ~/proj: swap ~/proj/secrets and ~/proj/src", fx.run(&r, &["swap", &fx.h("proj/secrets"), &fx.h("proj/src")]), Err(EPERM));
    for (what, op) in [
        ("workspace ~/proj: edit src/main.rs", vec!["write", &fx.h("proj/src/main.rs")]),
        ("workspace ~/proj: edit secrets/other.txt (beside the entry)", vec!["write", &fx.h("proj/secrets/other.txt")]),
        ("workspace ~/proj: remove secrets/other.txt", vec!["unlink", &fx.h("proj/secrets/other.txt")]),
        ("workspace ~/proj: mkdir secrets/new", vec!["mkdir", &fx.h("proj/secrets/new")]),
        ("workspace ~/proj: rename src", vec!["rename", &fx.h("proj/src"), &fx.h("proj/src2")]),
        ("workspace ~/proj: read ~/.config/other/x (the home read)", vec!["read", &fx.h(".config/other/x")]),
    ] {
        t.row(what, fx.run(&r, &op), Ok(()));
    }
    t.row("workspace ~/proj: mkdir secrets, which exists", fx.run(&r, &["mkdir", &fx.h("proj/secrets")]), Err(EEXIST));
    t.row(
        "workspace ~/proj: rename ~/.config away (outside the workspace: Z3, unchanged)",
        fx.run(&r, &["rename", &fx.h(".config"), &fx.h(".cfg")]),
        Err(EPERM),
    );
    // a write deny entry (Z3a) two levels below the workspace: ~/.config/systemd/user in a workspace ~/.config
    let fx = fixture("config", false);
    let r = fx.profile(&Grant { read_home: true, write: Some(fx.home.join(".config")), full: false });
    t.row(
        "workspace ~/.config: mv ~/.config/systemd ~/.config/sd2 (above Z3a's ~/.config/systemd/user)",
        fx.run(&r, &["rename", &fx.h(".config/systemd"), &fx.h(".config/sd2")]),
        Err(EPERM),
    );
    t.row("workspace ~/.config: rename ~/.config/other", fx.run(&r, &["rename", &fx.h(".config/other"), &fx.h(".config/other2")]), Ok(()));
    t.done();
}

/// v0.5.5 (REL-055 review B1): the kernel profile writes all of INIT_HOME, and Z4 denied lineage/kernel.git but not its
/// parent, so `mv lineage lin.x`, a write into `lin.x/kernel.git` and `mv lin.x lineage` put the zone's own refs where
/// the Root reads them. Z4 now pins the folders above lineage/kernel.git; the kernel's work in lineage/ is unchanged.
#[test]
fn kernel_cannot_move_lineage() {
    let mut t = Table::default();
    let fx = fixture("kernel", false);
    let r = fx.kernel_profile();
    let i = |rel: &str| fx.ih.join(rel).display().to_string();
    t.row("kernel: mv lineage lin.x", fx.run(&r, &["rename", &i("lineage"), &i("lin.x")]), Err(EPERM));
    t.row("kernel: write lin.x/kernel.git/refs/heads/main", fx.run(&r, &["write", &i("lin.x/kernel.git/refs/heads/main")]), Err(2));
    t.row("kernel: swap lineage and work", fx.run(&r, &["swap", &i("lineage"), &i("work")]), Err(EPERM));
    t.row("kernel: write lineage/kernel.git/ (Z4, unchanged)", fx.run(&r, &["write", &i("lineage/kernel.git/refs/heads/main")]), Err(EPERM));
    let main = fs::read_to_string(fx.ih.join("lineage/kernel.git/refs/heads/main")).unwrap_or_default();
    t.holds("kernel: lineage/kernel.git's main is unchanged", main == KGIT_MAIN);
    t.row("kernel: write lineage/zk", fx.run(&r, &["write", &i("lineage/zk")]), Ok(()));
    t.row("kernel: rename lineage/body.git", fx.run(&r, &["rename", &i("lineage/body.git"), &i("lineage/body2.git")]), Ok(()));
    t.row("kernel: rename work/ (not above a protected path)", fx.run(&r, &["rename", &i("work"), &i("work2")]), Ok(()));
    t.done();
}

/// The review's path variants, which Seatbelt already resolves before it matches: the firmlink path, a case variant and a
/// decomposed-Unicode name of a pinned folder are refused; an editor's atomic save inside pinned folders works; renaming an
/// empty folder onto an empty pinned folder (one above a missing entry) is refused.
#[test]
fn full_path_variants_and_atomic_saves() {
    let mut t = Table::default();
    let fx = fixture("variants", false);
    let r = fx.profile_with(&full(), &["Documents/tax", "Documents/Café/tax", "Documents/empty/secret"]);
    let data = format!("/System/Volumes/Data{}", fx.h(".config"));
    t.row("full: rename /System/Volumes/Data/…/.config (the firmlink path)", fx.run(&r, &["rename", &data, &fx.h(".cfg")]), Err(EPERM));
    t.row("full: rename ~/.CONFIG (a case variant)", fx.run(&r, &["rename", &fx.h(".CONFIG"), &fx.h(".cfg")]), Err(EPERM));
    t.row("full: rename ~/LIBRARY (a case variant)", fx.run(&r, &["rename", &fx.h("LIBRARY"), &fx.h("Lib2")]), Err(EPERM));
    t.row("full: rename ~/Documents/Café (NFD name)", fx.run(&r, &["rename", &fx.h("Documents/Cafe\u{301}"), &fx.h("Documents/C2")]), Err(EPERM));
    t.row("full: rename ~/Documents/Café (NFC name)", fx.run(&r, &["rename", &fx.h("Documents/Caf\u{e9}"), &fx.h("Documents/C2")]), Err(EPERM));
    t.row(
        "full: rename an empty folder onto the empty, pinned ~/Documents/empty",
        fx.run(&r, &["rename", &fx.h("prepared-empty"), &fx.h("Documents/empty")]),
        Err(EPERM),
    );
    for (dir, name) in [(".config", "settings.json"), (".local/bin", "newtool"), ("Library", "x.plist")] {
        for round in ["create", "replace"] {
            let tmp = fx.h(&format!("{dir}/.{name}.tmp"));
            t.row(&format!("full: atomic save ({round}) of ~/{dir}/{name}: write the temp file"), fx.run(&r, &["write", &tmp]), Ok(()));
            t.row(
                &format!("full: atomic save ({round}) of ~/{dir}/{name}: rename it into place"),
                fx.run(&r, &["rename", &tmp, &fx.h(&format!("{dir}/{name}"))]),
                Ok(()),
            );
        }
    }
    t.done();
}

/// v0.5.5 (REL-055 review note 3): a folder above a deny entry that is missing can be made as a folder (`mkdir -p`), but not
/// as a symlink, nor by renaming a symlink onto its name; otherwise a key or launcher the owner adds later would land where
/// the link points, readable until the next spawn resolves the paths again. Here `~/.config` is missing, as on a fresh Mac.
#[test]
fn missing_ancestor_is_never_a_symlink() {
    let mut t = Table::default();
    let fx = fixture("symlink", false);
    fs::remove_dir_all(fx.home.join(".config")).unwrap(); // exact path, made by the fixture
    let r = fx.profile(&full());
    t.row("full: ln -s ~/agentdir ~/.config (missing, above ~/.config/init-agent)", fx.run(&r, &["symlink", &fx.h(".config"), &fx.h("agentdir")]), Err(EPERM));
    t.row("full: ln -s ~/agentdir ~/lnk (not above a protected path)", fx.run(&r, &["symlink", &fx.h("lnk"), &fx.h("agentdir")]), Ok(()));
    t.row("full: mv ~/lnk ~/.config (a symlink renamed onto the name)", fx.run(&r, &["rename", &fx.h("lnk"), &fx.h(".config")]), Err(EPERM));
    t.row("full: mkdir ~/.config", fx.run(&r, &["mkdir", &fx.h(".config")]), Ok(()));
    t.row("full: mkdir ~/.config/gh", fx.run(&r, &["mkdir", &fx.h(".config/gh")]), Ok(()));
    t.row("full: ln -s ~/agentdir ~/.config/gh/link (not above an entry)", fx.run(&r, &["symlink", &fx.h(".config/gh/link"), &fx.h("agentdir")]), Ok(()));
    t.row(
        "full: ln -s ~/agentdir ~/.config/init-agent (the entry itself, Z2)",
        fx.run(&r, &["symlink", &fx.h(".config/init-agent"), &fx.h("agentdir")]),
        Err(EPERM),
    );
    let fx = fixture("symlink-ws", false);
    let r = fx.profile_with(&Grant { read_home: true, write: Some(fx.home.join("proj")), full: false }, &["proj/missing/secret"]);
    t.row(
        "workspace ~/proj: ln -s ~/proj/src ~/proj/missing (missing, above a charter entry)",
        fx.run(&r, &["symlink", &fx.h("proj/missing"), &fx.h("proj/src")]),
        Err(EPERM),
    );
    t.row("workspace ~/proj: mkdir ~/proj/missing", fx.run(&r, &["mkdir", &fx.h("proj/missing")]), Ok(()));
    t.done();
}

/// KNOWN RESIDUAL (v0.5.5, REL-055 review; root/root.md Z3b): this test records today's behaviour, it does not endorse it.
/// Z3b exempts INIT_HOME, where the kernel clones and configures its own repositories, and Seatbelt checks a directory
/// rename only by its two paths. So in workspace mode a folder holding a `.git` that the body made in INIT_HOME/work can be
/// renamed into the workspace, and the owner's next `git` there runs its config and hooks. Writing `.git` in the workspace
/// directly stays refused. v0.5.6 moves the kernel's work trees out of the body's writable WORK, so the exemption no longer
/// covers WORK: then the `.git` mkdir below is refused, and this test flips (change its expectations then).
#[test]
fn z3b_residual_git_dir_moved_in_from_work() {
    let mut t = Table::default();
    let fx = fixture("z3b", false);
    let r = fx.profile(&Grant { read_home: true, write: Some(fx.home.join("proj")), full: false });
    let w = |rel: &str| fx.ih.join("work").join(rel).display().to_string();
    t.row("workspace ~/proj: mkdir ~/proj/src/.git (Z3b holds in the workspace)", fx.run(&r, &["mkdir", &fx.h("proj/src/.git")]), Err(EPERM));
    t.row(
        "workspace: mkdir INIT_HOME/work/x/.git (INIT_HOME is exempt from Z3b)",
        fx.run(&r, &["mkdir", &w("x")]).and(fx.run(&r, &["mkdir", &w("x/.git")])),
        Ok(()),
    );
    t.row("workspace: write INIT_HOME/work/x/.git/config", fx.run(&r, &["write", &w("x/.git/config")]), Ok(()));
    t.row("workspace: mv INIT_HOME/work/x ~/proj/sub (RESIDUAL: allowed)", fx.run(&r, &["rename", &w("x"), &fx.h("proj/sub")]), Ok(()));
    t.holds("workspace: ~/proj/sub/.git/config is the agent's (RESIDUAL)", fx.home.join("proj/sub/.git/config").is_file());
    t.done();
}
