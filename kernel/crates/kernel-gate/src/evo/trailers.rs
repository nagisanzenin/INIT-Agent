//! Trailer parsing (evolution.md §3.9 steps 5–6): git itself is the parser, and a strict check of the
//! last paragraph turns git's silent drops into loud errors.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::proc::{self, GIT_TIMEOUT};
use super::{ValidationError, codes};

/// Parses trailers the way `git interpret-trailers --parse --no-divider` does: one `(key, value)` per
/// trailer, continuation lines unfolded, in message order.
pub trait TrailerParse {
    fn parse(&self, msg: &str) -> io::Result<Vec<(String, String)>>;
}

/// Runs the real `git interpret-trailers` exactly as §3.9's snippet: `env_clear`, `PATH=/usr/bin:/bin`,
/// `LC_ALL=C`, no system or global config, a kernel-owned empty cwd outside any repo, message on stdin.
#[derive(Debug, Clone)]
pub struct GitTrailerParser {
    /// Absolute path of the git binary (`[supervise] git`).
    pub git: PathBuf,
    /// A kernel-owned empty directory that is not inside any repository.
    pub empty_dir: PathBuf,
}

impl GitTrailerParser {
    pub fn new(git: impl Into<PathBuf>, empty_dir: impl Into<PathBuf>) -> Self {
        GitTrailerParser {
            git: git.into(),
            empty_dir: empty_dir.into(),
        }
    }

    /// The command of §3.9's `git_parse_trailers`, without stdio wiring.
    ///
    /// One addition to the snippet (deviation, hardening): `GIT_CEILING_DIRECTORIES` = the empty dir's
    /// parent, so git never discovers a repository above the empty dir. `interpret-trailers` reads the
    /// discovered repo's config, and `trailer.*` keys there would change the parse (verified: from a dir
    /// inside a checkout, git 2.39 finds the enclosing `.git`).
    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.git);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env(
                "GIT_CEILING_DIRECTORIES",
                self.empty_dir.parent().unwrap_or(&self.empty_dir),
            )
            .current_dir(&self.empty_dir)
            .args([
                "-c",
                "trailer.separators=:",
                "interpret-trailers",
                "--parse",
                "--no-divider",
            ]);
        c
    }
}

impl TrailerParse for GitTrailerParser {
    fn parse(&self, msg: &str) -> io::Result<Vec<(String, String)>> {
        let out = proc::run(self.command(), Some(msg.as_bytes()), GIT_TIMEOUT)?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "git interpret-trailers exited with {}: {}",
                out.status,
                proc::stderr_tail(&out.stderr)
            )));
        }
        parse_git_output(&out.stdout)
    }
}

/// Splits `git interpret-trailers --parse` output (`Key: value` per line, values already unfolded).
pub fn parse_git_output(stdout: &[u8]) -> io::Result<Vec<(String, String)>> {
    let text = std::str::from_utf8(stdout).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("git trailer output is not UTF-8: {e}"),
        )
    })?;
    let mut trailers = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (key, value) = line.split_once(':').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected git trailer output line '{line}'"),
            )
        })?;
        trailers.push((
            key.to_string(),
            value.strip_prefix(' ').unwrap_or(value).to_string(),
        ));
    }
    Ok(trailers)
}

/// CRLF → LF (git does the same before parsing).
pub fn normalize(msg: &str) -> String {
    msg.replace("\r\n", "\n")
}

/// One line of a paragraph: 1-based line number in the message, and its text.
pub type Line<'a> = (usize, &'a str);

fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| b.is_ascii_whitespace())
}

/// Splits a (normalized) message into paragraphs on blank lines, i.e. lines that are empty or
/// whitespace-only (§3.9 step 6). Leading and trailing blank lines produce no paragraph.
pub fn paragraphs(msg: &str) -> Vec<Vec<Line<'_>>> {
    let mut paras = Vec::new();
    let mut cur: Vec<Line<'_>> = Vec::new();
    for (i, line) in msg.split('\n').enumerate() {
        if is_blank(line) {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push((i + 1, line));
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    paras
}

/// `^[A-Za-z][A-Za-z0-9-]*[ \t]*:`
pub fn is_key_line(line: &str) -> bool {
    let b = line.as_bytes();
    if b.first().is_none_or(|c| !c.is_ascii_alphabetic()) {
        return false;
    }
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    b.get(i) == Some(&b':')
}

/// `^[ \t]+\S`
pub fn is_continuation_line(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    rest.len() < line.len() && rest.chars().next().is_some_and(|c| !c.is_whitespace())
}

/// §3.9 step 6 on the paragraphs of the normalized message, given `len(L_git)`.
pub fn strict_block_check(paras: &[Vec<Line<'_>>], n_git: usize) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let Some(block) = paras.last().filter(|_| paras.len() >= 2) else {
        errors.push(ValidationError::new(
            codes::BLOCK_MISSING,
            "no trailer block: the last paragraph, after the subject and a blank line, must hold the trailers",
        ));
        if n_git > 0 {
            errors.push(ValidationError::new(
                codes::PARSER_MISMATCH,
                format!("no trailer block, but git interpret-trailers parsed {n_git} trailers"),
            ));
        }
        return errors;
    };
    let mut keys = 0;
    for &(no, line) in block {
        if is_key_line(line) {
            keys += 1;
        } else if !is_continuation_line(line) {
            errors.push(ValidationError::new(
                codes::BLOCK_MIXED,
                format!("line {no}: '{line}'"),
            ));
        }
    }
    if keys != n_git {
        errors.push(ValidationError::new(
            codes::PARSER_MISMATCH,
            format!(
                "the trailer block has {keys} key lines, but git interpret-trailers parsed {n_git}"
            ),
        ));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_classes() {
        for k in [
            "Why: x",
            "Why : x",
            "Why\t:x",
            "Repro:x",
            "Signed-off-by: a",
            "X:",
        ] {
            assert!(is_key_line(k), "{k}");
        }
        for k in [
            "Why not: x",
            "-x: y",
            "1x: y",
            " Why: x",
            "Why",
            "see also the old loop",
            "# comment",
            "Wh_y: x",
        ] {
            assert!(!is_key_line(k), "{k}");
        }
        assert!(is_continuation_line("  more text"));
        assert!(is_continuation_line("\tmore"));
        assert!(!is_continuation_line("more"));
        assert!(!is_continuation_line("   "));
    }

    #[test]
    fn paragraph_split() {
        let msg = "subj\n\nbody a\nbody b\n \t \nWhy: x\n  cont\n\n\n";
        let p = paragraphs(msg);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], vec![(1, "subj")]);
        assert_eq!(p[1], vec![(3, "body a"), (4, "body b")]);
        assert_eq!(p[2], vec![(6, "Why: x"), (7, "  cont")]);
        assert!(paragraphs("").is_empty());
    }

    #[test]
    fn strict_check_without_git() {
        let p = paragraphs("s\n\nWhy: a\nsee also the old loop\nRepro: b");
        let e = strict_block_check(&p, 2);
        assert_eq!(
            e,
            vec![ValidationError::new(
                codes::BLOCK_MIXED,
                "line 4: 'see also the old loop'"
            )]
        );
        let e = strict_block_check(&p, 0);
        assert_eq!(
            e.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec![codes::BLOCK_MIXED, codes::PARSER_MISMATCH]
        );
        let e = strict_block_check(&paragraphs("only: one paragraph"), 0);
        assert_eq!(
            e.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec![codes::BLOCK_MISSING]
        );
    }

    #[test]
    fn git_output_split() {
        let v = parse_git_output(b"Why: a b\nRepro: task:x\nWhy: \n").unwrap();
        assert_eq!(
            v,
            vec![
                ("Why".to_string(), "a b".to_string()),
                ("Repro".to_string(), "task:x".to_string()),
                ("Why".to_string(), String::new())
            ]
        );
        assert!(parse_git_output(b"no separator\n").is_err());
    }
}
