//! G0: compiler stage (kernel/gate.md §3.3).
//!
//! - [`steps`] and [`env`]: the exact command list, timeouts, failure codes and environment, as data;
//! - [`policy`]: step 2, the build-config policy on the tree's `.cargo/config.toml`;
//! - [`forbid`]: the `#![forbid(unsafe_code)]` check on a target root (a hand-written lexer);
//! - [`shape`]: step 3, the shape checks over both workspaces' `cargo metadata`;
//! - [`run`]: the runner, behind the [`Spawner`] trait so the build sandbox (WP6a) can wrap commands.

use std::ffi::OsString;
use std::path::PathBuf;

use serde::Serialize;

pub mod forbid;
pub mod policy;
mod run;
pub mod shape;

pub use run::{
    Advisory, DirectSpawner, G0Report, G0Spec, SpawnRequest, Spawner, StepRecord, lints_changed,
    run,
};

/// G0 failure codes (gate.md §4.1).
pub mod code {
    pub const METADATA: &str = "gate.g0.metadata";
    pub const BUILD_CONFIG: &str = "gate.g0.build_config";
    pub const UNCHECKED_CRATE: &str = "gate.g0.unchecked_crate";
    pub const GIT_DEPENDENCY: &str = "gate.g0.git_dependency";
    pub const FORBID_MISSING: &str = "gate.g0.forbid_missing";
    pub const EXT_NOT_CDYLIB: &str = "gate.g0.ext_not_cdylib";
    pub const EXT_DEPENDENCY: &str = "gate.g0.ext_dependency";
    pub const CHECK: &str = "gate.g0.check";
    pub const CLIPPY: &str = "gate.g0.clippy";
    pub const EXT_BUILD: &str = "gate.g0.ext_build";
    pub const EXT_CLIPPY: &str = "gate.g0.ext_clippy";
    pub const TEST: &str = "gate.g0.test";
    pub const EXT_TEST: &str = "gate.g0.ext_test";
    pub const TIMEOUT: &str = "gate.g0.timeout";

    /// ADR-011 A1 (§2.1): on the free path these commands run but are advisory: a failure is
    /// recorded ([`super::Advisory`]) and G0 goes on. The body's tests may still veto the change
    /// after it went live (A3, which reads the `test` step's record). The build, the policy, the
    /// shape checks and `cargo check` stay blocking.
    pub const ADVISORY: [&str; 4] = [CLIPPY, EXT_CLIPPY, TEST, EXT_TEST];

    /// The shape codes in their fixed reporting order (gate.md §3.3 step 3).
    pub const SHAPE_ORDER: [&str; 5] = [
        UNCHECKED_CRATE,
        GIT_DEPENDENCY,
        FORBID_MISSING,
        EXT_NOT_CDYLIB,
        EXT_DEPENDENCY,
    ];
}

/// Total G0 budget, `gate.g0.timeout_s` (kernel/config.md).
pub const TOTAL_TIMEOUT_S: u64 = 900;
/// Agent-visible diagnostics cap (gate.md §3.3 "first 4 KiB").
pub const DIAG_CAP: usize = 4096;
/// Per-step log cap (gate.md §3.3 "≤ 16 MiB each").
pub const LOG_CAP: usize = 16 << 20;

/// The two `--config` flags every cargo command gets (supervise.md §3.6.2).
pub const CONFIG_FLAGS: [&str; 4] = [
    "--config",
    "source.crates-io.replace-with=\"vendored-sources\"",
    "--config",
    "source.vendored-sources.directory=\"vendor\"",
];

/// `$E`: the extension workspace's manifest, used only if `ext/Cargo.toml` exists.
pub const EXT_MANIFEST: [&str; 2] = ["--manifest-path", "ext/Cargo.toml"];

/// One command of a step. `argv[0]` is `cargo`; the runner resolves it to `<toolchain bin>/cargo`
/// (never `cargo +<toolchain>`, which could auto-install a toolchain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cmd {
    pub argv: Vec<String>,
    /// The failure code when this command exits non-zero.
    pub code: &'static str,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Run these commands in order, stopping at the first failure.
    Commands { cmds: Vec<Cmd> },
    /// The build-config policy ([`policy`]), in process.
    Policy,
    /// The shape checks ([`shape`]), in process.
    Shape,
}

/// One row of gate.md §3.3's step table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    /// The row number in gate.md §3.3 (1..=7).
    pub n: u8,
    /// Step name; also the log file name `<log_dir>/<name>.log`.
    pub name: &'static str,
    pub action: Action,
    /// Wall-clock limit for the whole step (all its commands), if any. The total budget
    /// ([`TOTAL_TIMEOUT_S`]) applies on top.
    pub timeout_s: Option<u64>,
}

impl Step {
    /// Every failure code this step can produce (besides [`code::TIMEOUT`]).
    pub fn codes(&self) -> Vec<&'static str> {
        match &self.action {
            Action::Commands { cmds } => {
                let mut v: Vec<&'static str> = Vec::new();
                for c in cmds {
                    if !v.contains(&c.code) {
                        v.push(c.code);
                    }
                }
                v
            }
            Action::Policy => vec![code::BUILD_CONFIG],
            Action::Shape => code::SHAPE_ORDER.to_vec(),
        }
    }
}

/// `$C <sub> [$E] <rest>`: cargo, the subcommand, the two `--config` flags, then the arguments.
fn cargo(sub: &str, ext: bool, rest: &[&str]) -> Vec<String> {
    let mut v = vec!["cargo".to_string(), sub.to_string()];
    v.extend(CONFIG_FLAGS.iter().map(|s| s.to_string()));
    if ext {
        v.extend(EXT_MANIFEST.iter().map(|s| s.to_string()));
    }
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

fn cmd(code: &'static str, argv: Vec<String>) -> Cmd {
    Cmd { argv, code }
}

/// The G0 command list (gate.md §3.3). `has_ext` = the tree has `ext/Cargo.toml`; without it the
/// `$E` commands and step 6 are left out.
pub fn steps(has_ext: bool) -> Vec<Step> {
    let meta = ["--format-version", "1", "--frozen", "--no-deps"];
    let mut metadata = vec![cmd(code::METADATA, cargo("metadata", false, &meta))];
    if has_ext {
        metadata.push(cmd(code::METADATA, cargo("metadata", true, &meta)));
    }
    let mut v = vec![
        Step {
            n: 1,
            name: "metadata",
            action: Action::Commands { cmds: metadata },
            timeout_s: Some(60),
        },
        Step {
            n: 2,
            name: "build_config",
            action: Action::Policy,
            timeout_s: None,
        },
        Step {
            n: 3,
            name: "shape",
            action: Action::Shape,
            timeout_s: None,
        },
        Step {
            n: 4,
            name: "check",
            action: Action::Commands {
                cmds: vec![cmd(
                    code::CHECK,
                    cargo(
                        "check",
                        false,
                        &[
                            "--workspace",
                            "--all-targets",
                            "--frozen",
                            "--message-format",
                            "short",
                        ],
                    ),
                )],
            },
            timeout_s: Some(300),
        },
        Step {
            n: 5,
            name: "clippy",
            action: Action::Commands {
                cmds: vec![cmd(
                    code::CLIPPY,
                    cargo(
                        "clippy",
                        false,
                        &[
                            "--workspace",
                            "--all-targets",
                            "--frozen",
                            "--message-format",
                            "short",
                            "--",
                            "-D",
                            "warnings",
                        ],
                    ),
                )],
            },
            timeout_s: Some(300),
        },
    ];
    if has_ext {
        v.push(Step {
            n: 6,
            name: "ext",
            action: Action::Commands {
                cmds: vec![
                    cmd(
                        code::EXT_BUILD,
                        cargo(
                            "build",
                            true,
                            &["--workspace", "--frozen", "--target", "wasm32-wasip2"],
                        ),
                    ),
                    cmd(
                        code::EXT_CLIPPY,
                        cargo(
                            "clippy",
                            true,
                            &[
                                "--workspace",
                                "--all-targets",
                                "--frozen",
                                "--target",
                                "wasm32-wasip2",
                                "--message-format",
                                "short",
                                "--",
                                "-D",
                                "warnings",
                            ],
                        ),
                    ),
                ],
            },
            timeout_s: Some(300),
        });
    }
    let mut test = vec![cmd(
        code::TEST,
        cargo(
            "test",
            false,
            &["--workspace", "--frozen", "--no-fail-fast"],
        ),
    )];
    if has_ext {
        test.push(cmd(
            code::EXT_TEST,
            cargo("test", true, &["--workspace", "--frozen", "--no-fail-fast"]),
        ));
    }
    v.push(Step {
        n: 7,
        name: "test",
        action: Action::Commands { cmds: test },
        timeout_s: Some(600),
    });
    v
}

/// Inputs of the G0 environment: supervise.md §3.6.2's env table plus gate.md §3.3's additions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSpec {
    /// `<toolchain bin>`: `sandbox.toolchain`, resolved once at install (kernel/sandbox.md §4.3).
    pub toolchain_bin: PathBuf,
    /// `slot/out/home`.
    pub home: PathBuf,
    /// `slot/out/tmp/`.
    pub tmpdir: PathBuf,
    /// `slot/out/cargo-home`, fresh per build.
    pub cargo_home: PathBuf,
    /// `slot/out/target`.
    pub target_dir: PathBuf,
    /// Committer time of the candidate commit.
    pub source_date_epoch: u64,
    /// `$INIT_HOME`, for `INIT_MOCKLLM_BIN`.
    pub init_home: PathBuf,
    /// macOS: `xcrun --show-sdk-path`, resolved at install.
    pub sdkroot: Option<PathBuf>,
    /// macOS: `xcode-select -p`, resolved at install; `$DEVELOPER_DIR/usr/bin` goes on `PATH`.
    pub developer_dir: Option<PathBuf>,
}

/// The complete environment of every G0 command, sorted by name. Commands run with `env_clear()`
/// plus exactly this.
pub fn env(e: &EnvSpec) -> Vec<(String, OsString)> {
    let mut path = OsString::from(e.toolchain_bin.as_os_str());
    if let Some(dev) = &e.developer_dir {
        path.push(":");
        path.push(dev.join("usr/bin").as_os_str());
    }
    path.push(":/usr/bin:/bin");
    let mut v: Vec<(String, OsString)> = vec![
        ("PATH".into(), path),
        ("HOME".into(), e.home.clone().into_os_string()),
        ("TMPDIR".into(), e.tmpdir.clone().into_os_string()),
        ("CARGO_HOME".into(), e.cargo_home.clone().into_os_string()),
        (
            "CARGO_TARGET_DIR".into(),
            e.target_dir.clone().into_os_string(),
        ),
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("CARGO_INCREMENTAL".into(), "0".into()),
        (
            "SOURCE_DATE_EPOCH".into(),
            e.source_date_epoch.to_string().into(),
        ),
        // gate.md §3.3 additions
        (
            "RUSTC".into(),
            e.toolchain_bin.join("rustc").into_os_string(),
        ),
        (
            "RUSTDOC".into(),
            e.toolchain_bin.join("rustdoc").into_os_string(),
        ),
        ("RUSTC_WRAPPER".into(), OsString::new()),
        ("CARGO_BUILD_RUSTC_WRAPPER".into(), OsString::new()),
        (
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER".into(),
            OsString::new(),
        ),
        ("CARGO_TERM_COLOR".into(), "never".into()),
        (
            "INIT_MOCKLLM_BIN".into(),
            e.init_home.join("kernel/bin/init-mockllm").into_os_string(),
        ),
    ];
    if let Some(sdk) = &e.sdkroot {
        v.push(("SDKROOT".into(), sdk.clone().into_os_string()));
    }
    if let Some(dev) = &e.developer_dir {
        v.push(("DEVELOPER_DIR".into(), dev.clone().into_os_string()));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// One policy or shape finding: a code and a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_without_ext() {
        let s = steps(false);
        let names: Vec<_> = s.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "metadata",
                "build_config",
                "shape",
                "check",
                "clippy",
                "test"
            ]
        );
        let timeouts: Vec<_> = s.iter().map(|s| s.timeout_s).collect();
        assert_eq!(
            timeouts,
            [Some(60), None, None, Some(300), Some(300), Some(600)]
        );
        let Action::Commands { cmds } = &s[0].action else {
            panic!()
        };
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0].argv.join(" "),
            "cargo metadata --config source.crates-io.replace-with=\"vendored-sources\" \
             --config source.vendored-sources.directory=\"vendor\" --format-version 1 --frozen --no-deps"
        );
        let Action::Commands { cmds } = &s[4].action else {
            panic!()
        };
        assert_eq!(
            cmds[0].argv.join(" "),
            "cargo clippy --config source.crates-io.replace-with=\"vendored-sources\" \
             --config source.vendored-sources.directory=\"vendor\" --workspace --all-targets --frozen \
             --message-format short -- -D warnings"
        );
        assert_eq!(cmds[0].code, code::CLIPPY);
        for st in &s {
            if let Action::Commands { cmds } = &st.action {
                for c in cmds {
                    assert_eq!(c.argv[0], "cargo");
                    assert!(
                        !c.argv.iter().any(|a| a.starts_with('+')),
                        "never cargo +toolchain"
                    );
                    assert!(
                        !c.argv.iter().any(|a| a == "--manifest-path"),
                        "no $E without ext"
                    );
                }
            }
        }
        assert_eq!(s[2].codes(), code::SHAPE_ORDER.to_vec());
        assert_eq!(s[1].codes(), vec![code::BUILD_CONFIG]);
    }

    #[test]
    fn steps_with_ext() {
        let s = steps(true);
        let names: Vec<_> = s.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "metadata",
                "build_config",
                "shape",
                "check",
                "clippy",
                "ext",
                "test"
            ]
        );
        assert_eq!(
            s.iter().map(|s| s.n).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7]
        );
        let Action::Commands { cmds } = &s[0].action else {
            panic!()
        };
        assert_eq!(cmds.len(), 2);
        assert!(cmds[1].argv.join(" ").contains("metadata --config"));
        assert!(
            cmds[1]
                .argv
                .join(" ")
                .contains("--manifest-path ext/Cargo.toml --format-version 1")
        );
        assert_eq!(s[5].codes(), vec![code::EXT_BUILD, code::EXT_CLIPPY]);
        let Action::Commands { cmds } = &s[5].action else {
            panic!()
        };
        assert!(
            cmds[0]
                .argv
                .join(" ")
                .ends_with("--workspace --frozen --target wasm32-wasip2")
        );
        assert!(
            cmds[1]
                .argv
                .join(" ")
                .ends_with("--target wasm32-wasip2 --message-format short -- -D warnings")
        );
        assert_eq!(s[6].codes(), vec![code::TEST, code::EXT_TEST]);
        assert_eq!(s[6].timeout_s, Some(600));
    }

    #[test]
    fn env_table() {
        let e = EnvSpec {
            toolchain_bin: "/tc/bin".into(),
            home: "/s/out/home".into(),
            tmpdir: "/s/out/tmp/".into(),
            cargo_home: "/s/out/cargo-home".into(),
            target_dir: "/s/out/target".into(),
            source_date_epoch: 1_760_000_000,
            init_home: "/ih".into(),
            sdkroot: Some("/sdk".into()),
            developer_dir: Some("/dev".into()),
        };
        let v = env(&e);
        let get = |k: &str| {
            v.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.to_str().unwrap().to_string())
        };
        assert_eq!(get("PATH").unwrap(), "/tc/bin:/dev/usr/bin:/usr/bin:/bin");
        assert_eq!(get("RUSTC").unwrap(), "/tc/bin/rustc");
        assert_eq!(get("RUSTDOC").unwrap(), "/tc/bin/rustdoc");
        for k in [
            "RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        ] {
            assert_eq!(get(k).unwrap(), "", "{k} must be present and empty");
        }
        assert_eq!(get("CARGO_TERM_COLOR").unwrap(), "never");
        assert_eq!(get("CARGO_NET_OFFLINE").unwrap(), "true");
        assert_eq!(get("CARGO_INCREMENTAL").unwrap(), "0");
        assert_eq!(get("SOURCE_DATE_EPOCH").unwrap(), "1760000000");
        assert_eq!(
            get("INIT_MOCKLLM_BIN").unwrap(),
            "/ih/kernel/bin/init-mockllm"
        );
        assert_eq!(get("SDKROOT").unwrap(), "/sdk");
        assert_eq!(get("DEVELOPER_DIR").unwrap(), "/dev");
        assert_eq!(get("TMPDIR").unwrap(), "/s/out/tmp/");
        assert_eq!(v.len(), 17);
        let names: Vec<_> = v.iter().map(|(n, _)| n.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let linux = EnvSpec {
            sdkroot: None,
            developer_dir: None,
            ..e
        };
        let v = env(&linux);
        assert_eq!(v.len(), 15);
        let path = v.iter().find(|(n, _)| n == "PATH").unwrap().1.clone();
        assert_eq!(path, "/tc/bin:/usr/bin:/bin");
        assert!(
            !v.iter()
                .any(|(n, _)| n == "SDKROOT" || n == "DEVELOPER_DIR")
        );
    }
}
