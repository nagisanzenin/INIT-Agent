//! The file inventory and `source_digest` (selfmap.md §3.2).
//!
//! - Gate mode: every regular file of the export (the commit's exact tree).
//! - Write/check mode: `git -C <root> ls-files -z --cached --others --exclude-standard`, i.e. tracked plus
//!   untracked-but-not-ignored.
//!
//! The file itself (`self.json`, or `kernel-self.json` for the kernel tree) is excluded. Symlinks are
//! recorded by their target string, never followed. `vendor/`, `target/` and `.git/` at the root are not
//! inventoried (deviation, see `build/status/WP9.md`): vendored crates are pinned by `dependencies`
//! (`Cargo.lock` checksums, which cargo verifies against vendor/ in every `--frozen` build), and listing
//! and tokenizing ~10⁴ vendored files would break the < 2 s target (§6) for no information.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::icj::{sha256_hex, sha256_tag};
use crate::tokens::{MAX_TOKENIZE_BYTES, Tokenizer};

/// How the file set is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSetMode {
    /// Walk the directory (gate mode: an export holds exactly the commit's tree).
    Walk,
    /// Ask git for tracked plus untracked-but-not-ignored files (write/check mode).
    Git,
}

/// One inventoried file.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Root-relative path with `/` separators.
    pub path: String,
    /// Lowercase hex SHA-256 of the content (of the target string for a symlink).
    pub sha256: String,
    pub bytes: u64,
    /// The text, when the file is UTF-8 and at most 1 MiB (else `None`, and `tokens` is null).
    pub text: Option<String>,
    /// `o200k_base` tokens of `text`.
    pub tokens: Option<u64>,
    pub symlink: bool,
}

/// Top-level directories never inventoried.
pub const SKIPPED_DIRS: [&str; 3] = [".git", "target", "vendor"];

fn skipped(rel: &str, self_name: &str) -> bool {
    if rel == self_name {
        return true;
    }
    let first = rel.split('/').next().unwrap_or("");
    SKIPPED_DIRS.contains(&first) && rel.len() > first.len()
}

fn walk(root: &Path, rel: &str, out: &mut Vec<String>) -> io::Result<()> {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().into_string().map_err(|n| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("non-UTF-8 file name {n:?}"),
            )
        })?;
        let r = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let ft = e.file_type()?;
        if ft.is_dir() {
            if rel.is_empty() && SKIPPED_DIRS.contains(&r.as_str()) {
                continue;
            }
            walk(root, &r, out)?;
        } else {
            out.push(r);
        }
    }
    Ok(())
}

fn git_files(root: &Path) -> io::Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.quotepath=off",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let mut v = Vec::new();
    for p in out.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let s = String::from_utf8(p.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 path from git"))?;
        v.push(s);
    }
    v.sort();
    v.dedup();
    Ok(v)
}

/// Lists and reads the file set of `root`, sorted by path bytes. `self_name` (`self.json` or
/// `kernel-self.json`) is excluded. Files listed by git but missing from the worktree are skipped.
/// Returns the files and the warnings (`tokens_null: <path> (<why>)`).
pub fn list_files(
    root: &Path,
    mode: FileSetMode,
    self_name: &str,
    tok: &Tokenizer,
) -> io::Result<(Vec<SourceFile>, Vec<String>)> {
    let mut paths = match mode {
        FileSetMode::Walk => {
            let mut v = Vec::new();
            walk(root, "", &mut v)?;
            v
        }
        FileSetMode::Git => git_files(root)?,
    };
    paths.retain(|p| !skipped(p, self_name));
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    let mut warnings = Vec::new();
    for rel in paths {
        let full: PathBuf = root.join(&rel);
        let meta = match fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if meta.is_dir() {
            continue; // a gitlink (submodule) or a directory listed by git
        }
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&full)?;
            let t = target.to_string_lossy().into_owned();
            files.push(SourceFile {
                path: rel,
                sha256: sha256_hex(t.as_bytes()),
                bytes: t.len() as u64,
                text: None,
                tokens: None,
                symlink: true,
            });
            continue;
        }
        let content = fs::read(&full)?;
        let (text, why) = if content.len() > MAX_TOKENIZE_BYTES {
            (None, Some("larger than 1 MiB"))
        } else {
            match String::from_utf8(content.clone()) {
                Ok(s) => (Some(s), None),
                Err(_) => (None, Some("not UTF-8")),
            }
        };
        if let Some(w) = why {
            warnings.push(format!("tokens_null: {rel} ({w})"));
        }
        let tokens = text.as_deref().map(|t| tok.count(t));
        files.push(SourceFile {
            sha256: sha256_hex(&content),
            bytes: content.len() as u64,
            path: rel,
            text,
            tokens,
            symlink: false,
        });
    }
    Ok((files, warnings))
}

/// `source_digest = "sha256:" + hex(SHA-256(concat over files sorted by path bytes of
/// path ‖ 0x00 ‖ hex(sha256(content)) ‖ 0x0A))` (selfmap.md §3.2).
pub fn source_digest(files: &[SourceFile]) -> String {
    let mut sorted: Vec<&SourceFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut buf = Vec::new();
    for f in sorted {
        buf.extend_from_slice(f.path.as_bytes());
        buf.push(0);
        buf.extend_from_slice(f.sha256.as_bytes());
        buf.push(b'\n');
    }
    sha256_tag(&buf)
}

/// Simple glob for root-relative paths: `**` matches any number of segments (including none), `*`
/// matches within one segment.
pub fn glob(pattern: &str, path: &str) -> bool {
    fn seg_match(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => seg_match(&p[1..], s) || (!s.is_empty() && seg_match(p, &s[1..])),
            (Some(a), Some(b)) if a == b => seg_match(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn go(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => (0..=s.len()).any(|k| go(&p[1..], &s[k..])),
            Some(seg) => {
                !s.is_empty() && seg_match(seg.as_bytes(), s[0].as_bytes()) && go(&p[1..], &s[1..])
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    go(&p, &s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{put, remove_home, test_home};

    #[test]
    fn glob_rules() {
        assert!(glob("src/**/*.rs", "src/main.rs"));
        assert!(glob("src/**/*.rs", "src/a/b/c.rs"));
        assert!(!glob("src/**/*.rs", "src/a/b/c.toml"));
        assert!(glob("ext/*/src/**/*.rs", "ext/pty/src/lib.rs"));
        assert!(!glob("ext/*/src/**/*.rs", "ext/pty/tests/a.rs"));
        assert!(glob("skills/**", "skills/a/SKILL.md"));
        assert!(glob("**/tests/**", "crates/x/tests/a.rs"));
        assert!(glob("**/*.toml", "Cargo.toml"));
        assert!(!glob("build.rs", "src/build.rs"));
    }

    #[test]
    fn walk_digest_symlink_and_exclusions() {
        let home = test_home("inventory");
        put(&home, "src/main.rs", "fn main() {}\n");
        put(&home, "self.json", "{}");
        put(&home, "vendor/x/lib.rs", "x");
        put(&home, "target/debug/x", "x");
        put(&home, "notes/bin.dat", "");
        fs::write(home.join("notes/bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        std::os::unix::fs::symlink("src/main.rs", home.join("link")).unwrap();
        let tok = Tokenizer::o200k();
        let (files, warnings) = list_files(&home, FileSetMode::Walk, "self.json", &tok).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["link", "notes/bin.dat", "src/main.rs"]);
        assert!(files[0].symlink && files[0].sha256 == sha256_hex(b"src/main.rs"));
        assert_eq!(files[1].tokens, None);
        assert_eq!(warnings, ["tokens_null: notes/bin.dat (not UTF-8)"]);
        assert_eq!(files[2].tokens, Some(tok.count("fn main() {}\n")));
        // The digest is over path ‖ 0 ‖ hex ‖ \n in path order.
        let mut buf = Vec::new();
        for f in &files {
            buf.extend_from_slice(format!("{}\0{}\n", f.path, f.sha256).as_bytes());
        }
        assert_eq!(source_digest(&files), sha256_tag(&buf));
        remove_home(&home);
    }
}
