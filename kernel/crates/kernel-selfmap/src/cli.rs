//! `init-kernel selfmap …` (selfmap.md §4.1). `init-kernel` calls [`run`] with the arguments after
//! `selfmap` and the command that re-invokes this CLI (for the `--describe` child); the crate's own
//! `kernel-selfmap` binary does the same with itself.
//!
//! ```text
//! --root <dir> --binary <path> --components <dir> --out <file>   gate mode (G1)
//! --root <dir> --write | --check | --print [--no-build]          agent modes (write/check build first)
//! --describe <component.wasm>                                    child mode
//! --capability --results <dir> --lkg <sha> [--prev <sha>]        capability-v1 object on stdout
//! --kernel --root <dir> --write | --check | --out <file>         kernel-self.json
//! --manifest-json <file>                                         stub for `--manifest` until WP13 (any body mode)
//! ```
//!
//! Exit codes: 0 ok, 1 drift or a G1 check failed, 2 usage, 3 build failed, 4 generator error.
//! Diagnostics go to stderr as JSON lines `{"code","message","path"?,"line"?}`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::body::{BodyConfig, generate};
use crate::describe::DescribeRunner;
use crate::diag::{Class, Diag, has_errors};
use crate::drift;
use crate::icj::to_pretty;
use crate::inventory::FileSetMode;
use crate::manifest::ManifestSource;

pub const EXIT_OK: i32 = 0;
pub const EXIT_DRIFT: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_BUILD: i32 = 3;
pub const EXIT_GENERATOR: i32 = 4;

#[derive(Debug, Default)]
struct Args {
    root: Option<PathBuf>,
    binary: Option<PathBuf>,
    components: Option<PathBuf>,
    out: Option<PathBuf>,
    write: bool,
    check: bool,
    print: bool,
    no_build: bool,
    describe: Option<PathBuf>,
    capability: bool,
    results: Option<PathBuf>,
    lkg: Option<String>,
    prev: Option<String>,
    kernel: bool,
    manifest_json: Option<PathBuf>,
}

fn parse(args: &[OsString]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let s = arg
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 argument {arg:?}"))?;
        let mut val = |name: &str| -> Result<OsString, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match s {
            "--root" => a.root = Some(val(s)?.into()),
            "--binary" => a.binary = Some(val(s)?.into()),
            "--components" => a.components = Some(val(s)?.into()),
            "--out" => a.out = Some(val(s)?.into()),
            "--describe" => a.describe = Some(val(s)?.into()),
            "--results" => a.results = Some(val(s)?.into()),
            "--manifest-json" => a.manifest_json = Some(val(s)?.into()),
            "--lkg" => a.lkg = Some(val(s)?.to_string_lossy().into_owned()),
            "--prev" => a.prev = Some(val(s)?.to_string_lossy().into_owned()),
            "--write" => a.write = true,
            "--check" => a.check = true,
            "--print" => a.print = true,
            "--no-build" => a.no_build = true,
            "--capability" => a.capability = true,
            "--kernel" => a.kernel = true,
            _ => return Err(format!("unknown argument `{s}`")),
        }
    }
    Ok(a)
}

fn emit(diags: &[Diag]) {
    for d in diags {
        eprintln!("{}", d.json_line());
    }
}

fn usage(msg: &str) -> i32 {
    emit(&[Diag::new(
        "usage",
        format!("{msg}; see `init-kernel selfmap` in kernel/selfmap.md §4.1"),
    )]);
    EXIT_USAGE
}

fn exit_for(diags: &[Diag]) -> i32 {
    if has_errors(diags) {
        EXIT_GENERATOR
    } else if diags.iter().any(|d| d.class() == Class::Check) {
        EXIT_DRIFT
    } else {
        EXIT_OK
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Diag> {
    crate::serve::write_atomic(path, bytes).map_err(|e| {
        Diag::new(
            crate::diag::SELFMAP_ERROR,
            format!("cannot write {}: {e}", path.display()),
        )
    })
}

/// Runs the CLI. `self_cmd` re-invokes this CLI (e.g. `[<init-kernel>, "selfmap"]`); the describe child
/// is `self_cmd --describe <wasm>`.
pub fn run(args: &[OsString], self_cmd: &[OsString]) -> i32 {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => return usage(&e),
    };
    if let Some(w) = &a.describe {
        return crate::describe::child_main(w);
    }
    if a.capability {
        let (Some(results), Some(lkg)) = (&a.results, &a.lkg) else {
            return usage("--capability needs --results and --lkg");
        };
        return match crate::capability::capability(results, lkg, a.prev.as_deref()) {
            Ok(v) => {
                print!("{}", String::from_utf8_lossy(&to_pretty(&v)));
                EXIT_OK
            }
            Err(e) => {
                emit(&[Diag::new(crate::diag::SELFMAP_ERROR, e)]);
                EXIT_GENERATOR
            }
        };
    }
    let Some(root) = a.root.clone() else {
        return usage("--root is required");
    };
    let modes = [a.write, a.check, a.print, a.out.is_some()]
        .iter()
        .filter(|x| **x)
        .count();
    if modes != 1 {
        return usage("exactly one of --write, --check, --print, --out");
    }
    if a.kernel {
        return kernel_mode(&a, &root);
    }
    body_mode(&a, &root, self_cmd)
}

fn kernel_mode(a: &Args, root: &Path) -> i32 {
    if a.print {
        return usage("--kernel takes --write, --check or --out");
    }
    let mode = if a.out.is_some() {
        FileSetMode::Walk
    } else {
        FileSetMode::Git
    };
    let g = crate::kernel_tree::generate(root, mode);
    emit(&g.diags);
    if has_errors(&g.diags) {
        return EXIT_GENERATOR;
    }
    let target = root.join("kernel-self.json");
    if a.check {
        return check(
            std::fs::read(&target).ok().as_deref(),
            &g.bytes,
            "kernel_self_drift",
        );
    }
    let path = a.out.clone().unwrap_or(target);
    if let Err(d) = write_file(&path, &g.bytes) {
        emit(&[d]);
        return EXIT_GENERATOR;
    }
    exit_for(&g.diags)
}

/// `--check`: compares, prints the report on stdout, warnings on stderr.
fn check(committed: Option<&[u8]>, regenerated: &[u8], drift_code: &'static str) -> i32 {
    match drift::compare(committed, regenerated) {
        Ok(warnings) => {
            for w in warnings {
                emit(&[Diag::new(
                    w.code(),
                    "the committed file matches the regenerated one",
                )]);
            }
            EXIT_OK
        }
        Err(f) => {
            let code = if f.code == "self_json_drift" {
                drift_code
            } else {
                f.code
            };
            println!("{code}\n{}", f.detail);
            EXIT_DRIFT
        }
    }
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

/// `cargo build --frozen` of the body binary and every ext crate (selfmap.md §3.1), with the gate's
/// vendored-source flags when `<root>/vendor/` exists.
fn build(root: &Path, target: &Path) -> Result<(PathBuf, PathBuf), String> {
    // The ext builds run with cwd `<root>/ext`, so a relative `--root .` or target dir would
    // resolve there (`ext/vendor`, `ext/target`) instead of under the body root.
    let (root, target) = (&absolute(root)?, &absolute(target)?);
    let vendor = root.join("vendor");
    let cargo = |dir: &Path, extra: &[&str]| -> Result<(), String> {
        let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        c.current_dir(dir)
            .arg("build")
            .arg("--frozen")
            .args(extra)
            .env("CARGO_TARGET_DIR", target);
        if vendor.is_dir() {
            c.args([
                "--config",
                "source.crates-io.replace-with=\"vendored-sources\"",
            ]);
            c.arg("--config").arg(format!(
                "source.vendored-sources.directory={:?}",
                vendor.to_string_lossy()
            ));
        }
        let o = c.output().map_err(|e| format!("cannot run cargo: {e}"))?;
        if !o.status.success() {
            let err = String::from_utf8_lossy(&o.stderr);
            let tail: Vec<&str> = err.lines().rev().take(20).collect();
            return Err(format!(
                "cargo build {} failed:\n{}",
                extra.join(" "),
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ));
        }
        Ok(())
    };
    let krate = std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|d| d.get("package")?.get("name")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "init-body".into());
    cargo(root, &["-p", &krate, "--bin", "init-body"])?;
    let ext = root.join("ext");
    if ext.join("Cargo.toml").is_file() {
        let mut names: Vec<String> = std::fs::read_dir(&ext)
            .map_err(|e| format!("cannot read ext/: {e}"))?
            .filter_map(Result::ok)
            .filter(|e| e.path().join("Cargo.toml").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        for n in names {
            let krate = std::fs::read_to_string(ext.join(&n).join("Cargo.toml"))
                .ok()
                .and_then(|t| t.parse::<toml::Table>().ok())
                .and_then(|d| d.get("package")?.get("name")?.as_str().map(str::to_string))
                .unwrap_or(n);
            cargo(&ext, &["-p", &krate, "--target", "wasm32-wasip2"])?;
        }
    }
    Ok((
        target.join("debug/init-body"),
        target.join("wasm32-wasip2/debug"),
    ))
}

/// `p` as an absolute path (the current directory joined to a relative one), without resolving
/// symlinks.
fn absolute(p: &Path) -> Result<PathBuf, String> {
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    std::env::current_dir()
        .map(|d| d.join(p))
        .map_err(|e| format!("cannot resolve {}: {e}", p.display()))
}

#[cfg(test)]
mod absolute_tests {
    use super::*;

    #[test]
    fn relative_roots_are_anchored_at_the_current_dir() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(Path::new(".")).unwrap(), cwd.join("."));
        assert_eq!(absolute(Path::new("target")).unwrap(), cwd.join("target"));
        assert!(absolute(Path::new(".")).unwrap().join("vendor").is_absolute());
        assert_eq!(absolute(Path::new("/x/y")).unwrap(), PathBuf::from("/x/y"));
    }
}

fn body_mode(a: &Args, root: &Path, self_cmd: &[OsString]) -> i32 {
    let gate = a.out.is_some();
    let target = target_dir(root);
    let (binary, components) = if gate || a.no_build || (a.print && a.binary.is_some()) {
        if gate && (a.binary.is_none() && a.manifest_json.is_none() || a.components.is_none()) {
            return usage("gate mode needs --binary (or --manifest-json) and --components");
        }
        (a.binary.clone(), a.components.clone())
    } else {
        match build(root, &target) {
            Ok((b, c)) => (Some(b), Some(c)),
            Err(e) => {
                emit(&[Diag::new("build_failed", e)]);
                return EXIT_BUILD;
            }
        }
    };
    let manifest = match (&a.manifest_json, binary) {
        (Some(p), _) => match std::fs::read(p) {
            Ok(b) => ManifestSource::Json(b),
            Err(e) => return usage(&format!("cannot read {}: {e}", p.display())),
        },
        (None, Some(b)) => ManifestSource::Binary(b),
        (None, None) => return usage("--no-build needs --binary or --manifest-json"),
    };
    let scratch_parent = match &a.out {
        Some(o) => o
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => target.clone(),
    };
    let scratch = scratch_parent.join(format!(".selfmap-scratch-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        emit(&[Diag::new(
            crate::diag::SELFMAP_ERROR,
            format!("cannot create {}: {e}", scratch.display()),
        )]);
        return EXIT_GENERATOR;
    }
    let cfg = BodyConfig {
        root: root.to_path_buf(),
        mode: if gate {
            FileSetMode::Walk
        } else {
            FileSetMode::Git
        },
        manifest,
        components,
        describe: DescribeRunner::new(self_cmd.to_vec()),
        scratch: scratch.clone(),
    };
    let g = generate(&cfg);
    let _ = std::fs::remove_dir_all(&scratch);
    emit(&g.diags);
    if has_errors(&g.diags) {
        return EXIT_GENERATOR;
    }
    if a.check {
        let committed = std::fs::read(root.join("self.json")).ok();
        let c = check(committed.as_deref(), &g.bytes, "self_json_drift");
        return if c == EXIT_OK { exit_for(&g.diags) } else { c };
    }
    if a.print {
        print!("{}", String::from_utf8_lossy(&g.bytes));
        return exit_for(&g.diags);
    }
    let path = a.out.clone().unwrap_or_else(|| root.join("self.json"));
    if let Err(d) = write_file(&path, &g.bytes) {
        emit(&[d]);
        return EXIT_GENERATOR;
    }
    exit_for(&g.diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn usage_errors_exit_2() {
        assert_eq!(run(&os(&["--bogus"]), &[]), EXIT_USAGE);
        assert_eq!(run(&os(&["--write"]), &[]), EXIT_USAGE);
        assert_eq!(
            run(&os(&["--root", "x", "--write", "--check"]), &[]),
            EXIT_USAGE
        );
        assert_eq!(run(&os(&["--root", "x", "--out", "y"]), &[]), EXIT_USAGE);
        assert_eq!(run(&os(&["--capability", "--lkg", "a"]), &[]), EXIT_USAGE);
        assert_eq!(
            run(&os(&["--kernel", "--root", "x", "--print"]), &[]),
            EXIT_USAGE
        );
    }
}
