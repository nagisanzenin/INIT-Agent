//! `gate --quick` argument resolution (kernel/initctl.md §4.5), using the `git` CLI as the agent,
//! inside the sandbox:
//!
//! 1. no argument: `repo` = the git toplevel of the current directory, `rev = HEAD`;
//! 2. an existing directory: `repo` = that directory's toplevel, `rev = HEAD`;
//! 3. otherwise: a ref in `$INIT_HOME/body` (`repo = "body"`, relative to `INIT_HOME`);
//! 4. for `HEAD` of a dirty worktree, snapshot it into a commit without touching the index, the
//!    worktree or any ref, and send that SHA.

use crate::cli::CliError;
use init_proto::errors::exit;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(repo: &Path, args: &[&str], index: Option<&Path>) -> Result<String, CliError> {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(i) = index {
        c.env("GIT_INDEX_FILE", i);
    }
    let out = c
        .output()
        .map_err(|e| CliError { code: exit::FAILED, message: format!("cannot run git: {e}") })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(CliError {
            code: exit::FAILED,
            message: format!("git {}: {}", args.join(" "), err.trim()),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string())
}

fn toplevel(dir: &Path) -> Result<PathBuf, CliError> {
    git(dir, &["rev-parse", "--show-toplevel"], None)
        .map(PathBuf::from)
        .map_err(|e| CliError::usage(format!("{} is not inside a git worktree ({})", dir.display(), e.message)))
}

/// Resolve `(repo, rev)` for the wire.
pub fn resolve(cwd: &Path, target: Option<&str>) -> Result<(String, String), CliError> {
    let dir = match target {
        None => Some(cwd.to_path_buf()),
        Some(t) => {
            let p = cwd.join(t);
            p.is_dir().then_some(p)
        }
    };
    let Some(dir) = dir else {
        // Case 3: a ref in $INIT_HOME/body.
        return Ok(("body".to_string(), target.unwrap_or("HEAD").to_string()));
    };
    let repo = toplevel(&dir)?;
    // `--no-optional-locks` keeps `status` from refreshing the real index.
    let dirty = git(&repo, &["--no-optional-locks", "status", "--porcelain"], None)?;
    let rev = if dirty.is_empty() { "HEAD".to_string() } else { snapshot(&repo)? };
    Ok((repo.to_string_lossy().into_owned(), rev))
}

/// Commit the worktree's current state (tracked and untracked, not ignored) on top of `HEAD`
/// through a temporary index, and return the commit SHA. No ref, index or worktree file changes.
pub fn snapshot(repo: &Path) -> Result<String, CliError> {
    let idx = std::env::temp_dir().join(format!("initctl-snap-{}.idx", std::process::id()));
    let r = (|| {
        git(repo, &["read-tree", "HEAD"], Some(&idx))?;
        git(repo, &["add", "-A"], Some(&idx))?;
        let tree = git(repo, &["write-tree"], Some(&idx))?;
        git(repo, &["commit-tree", &tree, "-p", "HEAD", "-m", "initctl: quick-gate snapshot"], None)
    })();
    // Exact-path cleanup of the temporary index (and git's lock file, if one was left).
    let _ = std::fs::remove_file(&idx);
    let _ = std::fs::remove_file(idx.with_extension("idx.lock"));
    r
}
