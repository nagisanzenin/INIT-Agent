//! G3, the scripted self-edit (gate.md §3.6): the nonce, the instruction text, and the
//! verification of the fetched result. The probe, the sandboxed fetch and the inner gate are the
//! driver's (`kernel-supervise::promote`); everything here is pure.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// `gate.g3.agent_timeout_s` default.
pub const AGENT_TIMEOUT_S: u64 = 300;
/// `gate.g3.inner_timeout_s` default.
pub const INNER_TIMEOUT_S: u64 = 300;
/// The agent part ends when the lane's last finish is `stop` with nothing in flight for this long.
pub const QUIET_S: u64 = 5;

/// The agent identity every agent commit carries (CONVENTIONS §4).
pub const AGENT_NAME: &str = "INIT Agent";
pub const AGENT_EMAIL: &str = "agent@init.local";

/// `<n>`: the first 6 hex characters of `sha256(job_id + "/g3")`.
pub fn nonce(job_id: &str) -> String {
    let d = Sha256::digest(format!("{job_id}/g3").as_bytes());
    d.iter().take(3).map(|b| format!("{b:02x}")).collect()
}

/// The lines step 2 appends to `src/main.rs`.
pub fn block(n: &str) -> String {
    format!(
        "#[cfg(test)]\nmod g3_{n} {{\n    #[test]\n    fn probe() {{\n        assert_eq!(\"{n}\".len(), 6);\n    }}\n}}"
    )
}

/// The commit message of step 4.
pub fn message(n: &str) -> String {
    format!(
        "refactor(g3): evolvability probe {n}\n\nWhy: kernel G3 evolvability probe {n}, requested by the gate.\nPredicts: pass_rate >= parent\nCompensates-for: none (gate probe)\nPrefix-change: no\n"
    )
}

/// The branch the agent creates in the probe's `body/`.
pub fn branch(n: &str) -> String {
    format!("evo/g3-{n}")
}

/// The instruction sent with `send_input` (§3.6, verbatim). `home` is the probe root `<H>`.
pub fn instruction(n: &str, home: &str) -> String {
    format!(
        "[init:g3 v1 nonce={n}] Kernel evolvability check. Do exactly these steps, then stop.\n\
1. Run: git -C {home}/body worktree add {home}/work/wt-g3-{n} -b evo/g3-{n}\n\
2. Append these lines to the end of {home}/work/wt-g3-{n}/src/main.rs:\n\
\n\
{block}\n\
\n\
3. Run: {home}/kernel/bin/init-kernel selfmap --root {home}/work/wt-g3-{n} --write\n\
   It builds the body and can take up to 15 minutes: give the bash tool a timeout of 1800 seconds.\n\
4. In {home}/work/wt-g3-{n}, commit all changes with exactly this message:\n\
\n\
{msg}\n\
5. Reply with exactly: G3 DONE <full commit sha>\n",
        block = block(n),
        msg = message(n)
    )
}

/// What the kernel learned about `C3 = refs/init/g3/<job>` with `$G` on lineage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub parents: Vec<String>,
    pub author: (String, String),
    pub committer: (String, String),
    /// `$G diff --name-only <candidate> C3`.
    pub changed: Vec<String>,
    /// The `+` lines of `$G diff <candidate> C3 -- src/main.rs`, without the leading `+`.
    pub main_rs_added: Vec<String>,
}

/// A G3 failure: `gate.g3.<reason>` and the agent-visible detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct G3Failure {
    pub code: String,
    pub detail: String,
}

impl G3Failure {
    pub fn new(reason: &str, detail: impl Into<String>) -> Self {
        G3Failure {
            code: format!("gate.g3.{reason}"),
            detail: detail.into(),
        }
    }
}

/// Verification steps 1–4 (§3.6). Step 5 (`evo::validate`) and 6 (the inner gate) are the driver's.
pub fn verify(f: &Facts, candidate: &str, n: &str) -> Result<(), G3Failure> {
    if f.parents.len() != 1 || f.parents[0] != candidate {
        return Err(G3Failure::new(
            "shape",
            format!(
                "the G3 commit must have exactly one parent, the candidate {candidate}; it has {:?}",
                f.parents
            ),
        ));
    }
    let agent = (AGENT_NAME.to_string(), AGENT_EMAIL.to_string());
    if f.author != agent || f.committer != agent {
        return Err(G3Failure::new(
            "identity",
            format!(
                "author {:?} and committer {:?} must both be {AGENT_NAME} <{AGENT_EMAIL}>",
                f.author, f.committer
            ),
        ));
    }
    let mut changed = f.changed.clone();
    changed.sort();
    if changed != ["self.json", "src/main.rs"] {
        return Err(G3Failure::new(
            "shape",
            format!(
                "the G3 commit must change exactly self.json and src/main.rs, it changes {changed:?}"
            ),
        ));
    }
    let has_mod = f
        .main_rs_added
        .iter()
        .any(|l| l.trim() == format!("mod g3_{n} {{"));
    let assert_line = format!("assert_eq!(\"{n}\".len(), 6);");
    let has_assert = f.main_rs_added.iter().any(|l| l.trim() == assert_line);
    if !has_mod || !has_assert {
        return Err(G3Failure::new(
            "shape",
            format!("src/main.rs must add `mod g3_{n}` and the line `{assert_line}`"),
        ));
    }
    Ok(())
}

/// The `+` lines of a unified diff (headers excluded).
pub fn added_lines(diff: &str) -> Vec<String> {
    diff.lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| l[1..].to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_six_hex_and_stable() {
        let n = nonce("j-000043");
        assert_eq!(n.len(), 6);
        assert!(n.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(n, nonce("j-000043"));
        assert_ne!(n, nonce("j-000044"));
    }

    #[test]
    fn instruction_carries_every_step() {
        let t = instruction("abc123", "/h/probe/p1");
        assert!(t.starts_with("[init:g3 v1 nonce=abc123] Kernel evolvability check."));
        assert!(t.contains("1. Run: git -C /h/probe/p1/body worktree add /h/probe/p1/work/wt-g3-abc123 -b evo/g3-abc123\n"));
        assert!(t.contains("mod g3_abc123 {\n"));
        assert!(t.contains("3. Run: /h/probe/p1/kernel/bin/init-kernel selfmap --root /h/probe/p1/work/wt-g3-abc123 --write\n"));
        assert!(t.contains("--write\nIt builds the body and can take up to 15 minutes: give the bash tool a timeout of 1800 seconds.\n4. "));
        assert!(t.contains("refactor(g3): evolvability probe abc123\n\nWhy: kernel G3"));
        assert!(
            t.contains("Prefix-change: no\n\n5. Reply with exactly: G3 DONE <full commit sha>")
        );
    }

    fn facts(n: &str) -> Facts {
        Facts {
            parents: vec!["c".into()],
            author: (AGENT_NAME.into(), AGENT_EMAIL.into()),
            committer: (AGENT_NAME.into(), AGENT_EMAIL.into()),
            changed: vec!["src/main.rs".into(), "self.json".into()],
            main_rs_added: added_lines(&format!(
                "+++ b/src/main.rs\n+\n+{}\n",
                block(n).replace('\n', "\n+")
            )),
        }
    }

    #[test]
    fn verify_each_rule() {
        let n = "abc123";
        assert_eq!(verify(&facts(n), "c", n), Ok(()));
        let mut f = facts(n);
        f.parents = vec!["x".into()];
        assert_eq!(verify(&f, "c", n).unwrap_err().code, "gate.g3.shape");
        let mut f = facts(n);
        f.author.1 = "me@example.com".into();
        assert_eq!(verify(&f, "c", n).unwrap_err().code, "gate.g3.identity");
        let mut f = facts(n);
        f.changed.push("src/loop.rs".into());
        assert_eq!(verify(&f, "c", n).unwrap_err().code, "gate.g3.shape");
        let mut f = facts(n);
        f.main_rs_added.retain(|l| !l.contains("assert_eq"));
        assert_eq!(verify(&f, "c", n).unwrap_err().code, "gate.g3.shape");
    }
}
