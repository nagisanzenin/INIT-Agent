//! G0 step 3: shape (gate.md §3.3; CONVENTIONS §5).
//!
//! Inputs are data, so the checks run on synthetic metadata in tests: both workspaces'
//! `cargo metadata --format-version 1 --no-deps` JSON, the `Cargo.toml` paths in the tree
//! ([`find_manifests`]), both lockfile texts, and a reader for target root files. Every finding is
//! collected; the step's code is the first in [`code::SHAPE_ORDER`].
//!
//! Paths are compared relative to the body workspace root that cargo reports (`workspace_root`),
//! so a symlinked spelling of the export path cannot cause a mismatch.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Finding, code, forbid};

/// What the shape step looks at.
#[derive(Debug, Clone, Copy)]
pub struct ShapeInput<'a> {
    /// `cargo metadata --no-deps` of the body workspace.
    pub body_meta: &'a Value,
    /// `cargo metadata $E --no-deps` of the extension workspace, if `ext/Cargo.toml` exists.
    pub ext_meta: Option<&'a Value>,
    /// Every `Cargo.toml` in the tree, relative to the tree root ([`find_manifests`]).
    pub manifests: &'a [PathBuf],
    /// `Cargo.lock` text, if present.
    pub body_lock: Option<&'a str>,
    /// `ext/Cargo.lock` text, if present.
    pub ext_lock: Option<&'a str>,
}

/// Lists every `Cargo.toml` under `root` (relative paths, sorted), skipping the root's `vendor/`
/// (gate.md: "every `Cargo.toml` outside `vendor/`") and `target/`, and every `.git`. Symlinks are
/// not followed. The name is matched ASCII case-insensitively, since macOS file systems are.
pub fn find_manifests(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(root.join(&rel))? {
            let entry = entry?;
            let name = entry.file_name();
            let ft = entry.file_type()?;
            let child = rel.join(&name);
            if ft.is_dir() {
                let at_root = rel.as_os_str().is_empty();
                // ADR-011 §6: `archive/` and `quarantine/` keep removed caps (an archived ext
                // component keeps its Cargo.toml); nothing builds them, and a path dependency on
                // one inside the workspace would make it a member, checked through the metadata.
                let skip = name == ".git"
                    || (at_root
                        && (name == "vendor"
                            || name == "target"
                            || name == "archive"
                            || name == "quarantine"));
                if !skip {
                    stack.push(child);
                }
            } else if name
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case("Cargo.toml"))
            {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// One workspace as `cargo metadata` describes it.
struct Ws<'a> {
    /// `workspace_root`, absolute.
    root: PathBuf,
    /// The member packages.
    members: Vec<&'a Value>,
}

fn workspace(meta: &Value) -> Ws<'_> {
    let ids: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let members = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["id"].as_str().is_some_and(|id| ids.contains(id)))
        .collect();
    Ws {
        root: PathBuf::from(meta["workspace_root"].as_str().unwrap_or("")),
        members,
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn strs(v: &Value) -> Vec<&str> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// `p` relative to `root` when it is under it, else unchanged.
fn rel(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| p.to_path_buf())
}

/// Crate kinds that make a target a library.
const LIB_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

/// Runs every shape check. `read` reads a target root file (absolute `src_path`).
pub fn check(input: &ShapeInput<'_>, read: &dyn Fn(&Path) -> io::Result<String>) -> Vec<Finding> {
    let body = workspace(input.body_meta);
    let ext = input.ext_meta.map(workspace);
    let root = body.root.clone();
    let mut out: Vec<Finding> = Vec::new();
    let mut push = |code: &'static str, detail: String| out.push(Finding { code, detail });

    // unchecked_crate: every Cargo.toml is a member manifest or a workspace root.
    let mut known: BTreeSet<PathBuf> = BTreeSet::new();
    known.insert(PathBuf::from("Cargo.toml"));
    for ws in std::iter::once(&body).chain(ext.as_ref()) {
        known.insert(rel(&root, &ws.root.join("Cargo.toml")));
        for p in &ws.members {
            known.insert(rel(&root, Path::new(s(&p["manifest_path"]))));
        }
    }
    for m in input.manifests {
        if !known.contains(m) {
            push(
                code::UNCHECKED_CRATE,
                format!(
                    "`{}` is not a member of the body or the extension workspace",
                    m.display()
                ),
            );
        }
    }

    // Lockfiles: git sources (git_dependency); path packages that are not members (unchecked_crate:
    // a non-member path dependency or a `[patch]` to a path escapes every check above).
    let locks = [
        ("Cargo.lock", input.body_lock, Some(&body)),
        ("ext/Cargo.lock", input.ext_lock, ext.as_ref()),
    ];
    for (label, text, ws) in locks {
        let Some(text) = text else { continue };
        let members: BTreeSet<(&str, &str)> = ws
            .map(|w| {
                w.members
                    .iter()
                    .map(|p| (s(&p["name"]), s(&p["version"])))
                    .collect()
            })
            .unwrap_or_default();
        match lock_packages(text) {
            Err(e) => push(
                code::GIT_DEPENDENCY,
                format!("{label}: cannot check package sources: {e}"),
            ),
            Ok(pkgs) => {
                for (name, version, source) in pkgs {
                    match source {
                        Some(src) if src.starts_with("git+") => push(
                            code::GIT_DEPENDENCY,
                            format!(
                                "{label}: `{name} {version}` comes from a git source (`{src}`)"
                            ),
                        ),
                        Some(_) => {}
                        None if !members.contains(&(name.as_str(), version.as_str())) => push(
                            code::UNCHECKED_CRATE,
                            format!(
                                "{label}: path package `{name} {version}` is not a member of its workspace"
                            ),
                        ),
                        None => {}
                    }
                }
            }
        }
    }

    // forbid_missing: every target root of every body-workspace member.
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for p in &body.members {
        for t in p["targets"].as_array().into_iter().flatten() {
            let src = PathBuf::from(s(&t["src_path"]));
            if !seen.insert(src.clone()) {
                continue;
            }
            let what = format!(
                "{} `{}` of package `{}`",
                strs(&t["kind"]).join("+"),
                s(&t["name"]),
                s(&p["name"])
            );
            let shown = rel(&root, &src);
            match read(&src) {
                Err(e) => push(
                    code::FORBID_MISSING,
                    format!("`{}` ({what}): cannot be read: {e}", shown.display()),
                ),
                Ok(text) => {
                    if let Err(why) = forbid::check(&text) {
                        push(
                            code::FORBID_MISSING,
                            format!("`{}` ({what}): {why}", shown.display()),
                        );
                    }
                }
            }
        }
    }

    if let Some(ext) = &ext {
        // ext_not_cdylib: every ext member's lib target is `crate-type = ["cdylib"]` and nothing else.
        for p in &ext.members {
            let libs: Vec<&Value> = p["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| strs(&t["kind"]).iter().any(|k| LIB_KINDS.contains(k)))
                .collect();
            if libs.is_empty() {
                push(
                    code::EXT_NOT_CDYLIB,
                    format!(
                        "extension package `{}` has no lib target (it must be a cdylib)",
                        s(&p["name"])
                    ),
                );
            }
            for t in libs {
                let types = strs(&t["crate_types"]);
                if types != ["cdylib"] {
                    push(
                        code::EXT_NOT_CDYLIB,
                        format!(
                            "extension package `{}`: lib target has crate-type {:?}; it must be [\"cdylib\"] only",
                            s(&p["name"]),
                            types
                        ),
                    );
                }
            }
        }

        // ext_dependency: no body member depends (any kind) on an ext package, by name or by path.
        let names: BTreeSet<&str> = ext.members.iter().map(|p| s(&p["name"])).collect();
        let dirs: BTreeSet<PathBuf> = ext
            .members
            .iter()
            .filter_map(|p| {
                Path::new(s(&p["manifest_path"]))
                    .parent()
                    .map(|d| rel(&root, d))
            })
            .collect();
        let ext_root = rel(&root, &ext.root);
        for p in &body.members {
            for d in p["dependencies"].as_array().into_iter().flatten() {
                let name = s(&d["name"]);
                let kind = d["kind"].as_str().unwrap_or("normal");
                let by_path = d["path"]
                    .as_str()
                    .map(|dp| rel(&root, Path::new(dp)))
                    .filter(|dp| {
                        dirs.contains(dp)
                            || (!ext_root.as_os_str().is_empty() && dp.starts_with(&ext_root))
                    });
                if names.contains(name) || by_path.is_some() {
                    push(
                        code::EXT_DEPENDENCY,
                        format!(
                            "body package `{}` depends ({kind}) on `{name}`, a package of the extension workspace",
                            s(&p["name"])
                        ),
                    );
                }
            }
        }
    }

    out.sort_by_key(|f| {
        code::SHAPE_ORDER
            .iter()
            .position(|c| *c == f.code)
            .unwrap_or(usize::MAX)
    });
    out
}

/// `[[package]]` entries of a lockfile: (name, version, source).
fn lock_packages(text: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
    let doc = text.parse::<toml::Table>().map_err(|e| {
        let msg = e.to_string();
        format!(
            "not valid TOML ({})",
            msg.lines().next().unwrap_or("").trim()
        )
    })?;
    let mut out = Vec::new();
    for p in doc
        .get("package")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
        out.push((
            get("name").unwrap_or_default(),
            get("version").unwrap_or_default(),
            get("source"),
        ));
    }
    Ok(out)
}

/// The step's code: the first finding's (findings come sorted in [`code::SHAPE_ORDER`]).
pub fn first_code(findings: &[Finding]) -> Option<&'static str> {
    findings.first().map(|f| f.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    const OK: &str = "#![forbid(unsafe_code)]\n";

    fn target(kind: &str, name: &str, src: &str) -> Value {
        let ct = if ["bin", "test", "bench", "example", "custom-build"].contains(&kind) {
            "bin"
        } else {
            kind
        };
        json!({"kind": [kind], "crate_types": [ct], "name": name, "src_path": format!("/r/{src}")})
    }

    fn dep(name: &str, path: Option<&str>, kind: Option<&str>) -> Value {
        let mut d =
            json!({"name": name, "source": null, "req": "*", "kind": kind, "optional": false});
        if let Some(p) = path {
            d["path"] = json!(format!("/r/{p}"));
        }
        d
    }

    fn pkg(name: &str, dir: &str, targets: Vec<Value>, deps: Vec<Value>) -> Value {
        let mp = if dir.is_empty() {
            "/r/Cargo.toml".to_string()
        } else {
            format!("/r/{dir}/Cargo.toml")
        };
        json!({"name": name, "version": "0.1.0", "id": format!("path+file:///r/{dir}#{name}@0.1.0"),
               "manifest_path": mp, "targets": targets, "dependencies": deps})
    }

    fn meta(root: &str, pkgs: Vec<Value>) -> Value {
        let ids: Vec<Value> = pkgs.iter().map(|p| p["id"].clone()).collect();
        json!({"packages": pkgs, "workspace_members": ids, "workspace_root": root, "version": 1})
    }

    /// A clean body: root package `body` (lib, bin, test, bench, example, build script) plus
    /// member `crates/util`; ext workspace with `ext/hello` (cdylib).
    struct Fx {
        body: Value,
        ext: Option<Value>,
        manifests: Vec<PathBuf>,
        files: BTreeMap<PathBuf, String>,
        body_lock: String,
        ext_lock: String,
    }

    impl Fx {
        fn new() -> Self {
            let body = meta(
                "/r",
                vec![
                    pkg(
                        "body",
                        "",
                        vec![
                            target("lib", "body", "src/lib.rs"),
                            target("bin", "init-body", "src/main.rs"),
                            target("test", "it", "tests/it.rs"),
                            target("bench", "b", "benches/b.rs"),
                            target("example", "e", "examples/e.rs"),
                            target("custom-build", "build-script-build", "build.rs"),
                        ],
                        vec![
                            dep("util", Some("crates/util"), None),
                            dep("serde", None, Some("dev")),
                        ],
                    ),
                    pkg(
                        "util",
                        "crates/util",
                        vec![target("lib", "util", "crates/util/src/lib.rs")],
                        vec![],
                    ),
                ],
            );
            let ext = meta(
                "/r/ext",
                vec![pkg(
                    "hello",
                    "ext/hello",
                    vec![target("cdylib", "hello", "ext/hello/src/lib.rs")],
                    vec![],
                )],
            );
            let files = [
                "src/lib.rs",
                "src/main.rs",
                "tests/it.rs",
                "benches/b.rs",
                "examples/e.rs",
                "build.rs",
                "crates/util/src/lib.rs",
            ]
            .iter()
            .map(|f| (PathBuf::from(format!("/r/{f}")), OK.to_string()))
            .chain([(
                PathBuf::from("/r/ext/hello/src/lib.rs"),
                "fn unsafe_ok() {}".to_string(),
            )])
            .collect();
            let manifests = [
                "Cargo.toml",
                "crates/util/Cargo.toml",
                "ext/Cargo.toml",
                "ext/hello/Cargo.toml",
            ]
            .iter()
            .map(PathBuf::from)
            .collect();
            let body_lock = "version = 4\n\n[[package]]\nname = \"body\"\nversion = \"0.1.0\"\n\n\
                 [[package]]\nname = \"serde\"\nversion = \"1.0.229\"\n\
                 source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\n\n\
                 [[package]]\nname = \"util\"\nversion = \"0.1.0\"\n"
                .to_string();
            let ext_lock =
                "version = 4\n\n[[package]]\nname = \"hello\"\nversion = \"0.1.0\"\n".to_string();
            Fx {
                body,
                ext: Some(ext),
                manifests,
                files,
                body_lock,
                ext_lock,
            }
        }

        fn run(&self) -> Vec<Finding> {
            let input = ShapeInput {
                body_meta: &self.body,
                ext_meta: self.ext.as_ref(),
                manifests: &self.manifests,
                body_lock: Some(&self.body_lock),
                ext_lock: self.ext.as_ref().map(|_| self.ext_lock.as_str()),
            };
            let read = |p: &Path| {
                self.files
                    .get(p)
                    .cloned()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
            };
            check(&input, &read)
        }

        fn codes(&self) -> Vec<&'static str> {
            self.run().iter().map(|f| f.code).collect()
        }
    }

    #[test]
    fn clean_tree_has_no_findings() {
        let f = Fx::new();
        assert_eq!(f.run(), vec![]);
        let mut f = Fx::new();
        f.ext = None;
        f.manifests.retain(|m| !m.starts_with("ext"));
        assert_eq!(f.run(), vec![]);
    }

    #[test]
    fn unchecked_crate() {
        let mut f = Fx::new();
        f.manifests.push("tools/helper/Cargo.toml".into());
        let r = f.run();
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].code, code::UNCHECKED_CRATE);
        assert!(r[0].detail.contains("tools/helper/Cargo.toml"));

        // a non-member path package in a lockfile (non-member path dependency or `[patch]` to a path)
        let mut f = Fx::new();
        f.body_lock
            .push_str("\n[[package]]\nname = \"evil\"\nversion = \"0.1.0\"\n");
        let r = f.run();
        assert_eq!(f.codes(), vec![code::UNCHECKED_CRATE]);
        assert!(r[0].detail.contains("`evil 0.1.0`"), "{r:?}");

        // the ext manifest is unchecked when there is no ext workspace metadata
        let mut f = Fx::new();
        f.ext = None;
        assert_eq!(
            f.codes(),
            vec![code::UNCHECKED_CRATE, code::UNCHECKED_CRATE]
        );
    }

    #[test]
    fn git_dependency() {
        let mut f = Fx::new();
        f.body_lock.push_str(
            "\n[[package]]\nname = \"evil\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/evil#0123abcd\"\n",
        );
        let r = f.run();
        assert_eq!(f.codes(), vec![code::GIT_DEPENDENCY]);
        assert!(
            r[0].detail.starts_with("Cargo.lock:") && r[0].detail.contains("evil"),
            "{r:?}"
        );

        let mut f = Fx::new();
        f.ext_lock.push_str("\n[[package]]\nname = \"g\"\nversion = \"1.0.0\"\nsource = \"git+ssh://x/y?rev=1#1\"\n");
        let r = f.run();
        assert_eq!(f.codes(), vec![code::GIT_DEPENDENCY]);
        assert!(r[0].detail.starts_with("ext/Cargo.lock:"), "{r:?}");

        // fail closed on a lockfile that cannot be parsed
        let mut f = Fx::new();
        f.body_lock = "[[package]\nname = ".into();
        assert_eq!(f.codes(), vec![code::GIT_DEPENDENCY]);
    }

    #[test]
    fn forbid_missing_on_every_target_kind() {
        for file in [
            "src/lib.rs",
            "src/main.rs",
            "tests/it.rs",
            "benches/b.rs",
            "examples/e.rs",
            "build.rs",
        ] {
            let mut f = Fx::new();
            f.files.insert(
                PathBuf::from(format!("/r/{file}")),
                "#![deny(unsafe_code)]\n".into(),
            );
            let r = f.run();
            assert_eq!(f.codes(), vec![code::FORBID_MISSING], "{file}");
            assert!(r[0].detail.starts_with(&format!("`{file}`")), "{r:?}");
        }
        let mut f = Fx::new();
        f.files
            .insert("/r/crates/util/src/lib.rs".into(), "fn main() {}".into());
        f.files.remove(Path::new("/r/build.rs"));
        let r = f.run();
        assert_eq!(f.codes(), vec![code::FORBID_MISSING, code::FORBID_MISSING]);
        assert!(
            r.iter().any(|x| x.detail.contains("cannot be read")),
            "{r:?}"
        );
        assert!(
            r.iter().any(|x| x.detail.contains("package `util`")),
            "{r:?}"
        );
    }

    #[test]
    fn ext_crates_are_exempt_from_forbid_but_must_be_cdylib() {
        let mut f = Fx::new();
        f.ext.as_mut().unwrap()["packages"][0]["targets"][0]["crate_types"] =
            json!(["cdylib", "rlib"]);
        let r = f.run();
        assert_eq!(f.codes(), vec![code::EXT_NOT_CDYLIB]);
        assert!(r[0].detail.contains("hello"), "{r:?}");

        let mut f = Fx::new();
        f.ext.as_mut().unwrap()["packages"][0]["targets"][0] =
            target("lib", "hello", "ext/hello/src/lib.rs");
        assert_eq!(f.codes(), vec![code::EXT_NOT_CDYLIB]);

        let mut f = Fx::new();
        f.ext.as_mut().unwrap()["packages"][0]["targets"][0] =
            target("bin", "hello", "ext/hello/src/main.rs");
        let r = f.run();
        assert_eq!(f.codes(), vec![code::EXT_NOT_CDYLIB]);
        assert!(r[0].detail.contains("no lib target"), "{r:?}");
    }

    #[test]
    fn ext_dependency_by_name_and_by_path() {
        // by name, any kind
        let mut f = Fx::new();
        f.body["packages"][1]["dependencies"] = json!([dep("hello", None, Some("dev"))]);
        let r = f.run();
        assert_eq!(f.codes(), vec![code::EXT_DEPENDENCY]);
        assert!(
            r[0].detail.contains("`util`") && r[0].detail.contains("(dev)"),
            "{r:?}"
        );

        // by path, renamed
        let mut f = Fx::new();
        let mut d = dep("not-hello", Some("ext/hello"), Some("build"));
        d["rename"] = json!("h");
        f.body["packages"][0]["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(d);
        assert_eq!(f.codes(), vec![code::EXT_DEPENDENCY]);

        // by path under the ext workspace root (a non-member there)
        let mut f = Fx::new();
        f.body["packages"][0]["dependencies"]
            .as_array_mut()
            .unwrap()
            .push(dep("x", Some("ext/other"), None));
        assert_eq!(f.codes(), vec![code::EXT_DEPENDENCY]);
    }

    #[test]
    fn all_findings_sorted_in_code_order() {
        let mut f = Fx::new();
        f.body["packages"][1]["dependencies"] = json!([dep("hello", None, None)]);
        f.ext.as_mut().unwrap()["packages"][0]["targets"][0]["crate_types"] = json!(["rlib"]);
        f.files.insert("/r/src/main.rs".into(), "".into());
        f.body_lock.push_str(
            "\n[[package]]\nname = \"g\"\nversion = \"1.0.0\"\nsource = \"git+https://x/g#1\"\n",
        );
        f.manifests.push("tools/x/Cargo.toml".into());
        assert_eq!(f.codes(), code::SHAPE_ORDER.to_vec());
        assert_eq!(first_code(&f.run()), Some(code::UNCHECKED_CRATE));
    }

    #[test]
    fn walker_skips_vendor_target_git() {
        let home = crate::testutil::test_home("g0-walk");
        let w = |p: &str| {
            let p = home.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        };
        for p in [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "tools/x/cargo.toml",
            "vendor/serde/Cargo.toml",
            "target/debug/build/Cargo.toml",
            ".git/x/Cargo.toml",
            "crates/a/.git/Cargo.toml",
            "crates/a/vendor/Cargo.toml",
            "crates/a/target/Cargo.toml",
            "ext/Cargo.toml",
            "ext/hello/Cargo.toml",
            "crates/a/src/lib.rs",
            "archive/ext/old/Cargo.toml",
            "quarantine/ext/bad/Cargo.toml",
            "crates/a/archive/Cargo.toml",
        ] {
            w(p);
        }
        let got = find_manifests(&home).unwrap();
        let want: Vec<PathBuf> = [
            "Cargo.toml",
            "crates/a/Cargo.toml",
            "crates/a/archive/Cargo.toml",
            "crates/a/target/Cargo.toml",
            "crates/a/vendor/Cargo.toml",
            "ext/Cargo.toml",
            "ext/hello/Cargo.toml",
            "tools/x/cargo.toml",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
        crate::testutil::remove_home(&home);
    }
}
