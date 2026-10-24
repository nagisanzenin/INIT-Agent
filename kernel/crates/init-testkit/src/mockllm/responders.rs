//! Built-in responders (testing.md §3.4.4; accepted by provider.md §3.12): probe, G3, per-task
//! sessions and give-up. Probe and G3 are stateless: they answer from the request alone.

use super::script::Reply;
use super::wire::{NMsg, Role};
use serde_json::json;

/// The marker of the probe instruction (body/boot.md §3.9).
pub const PROBE_MARKER: &str = "[init:probe]";
/// The marker of the G3 instruction (kernel/gate.md §3.6).
pub const G3_MARKER: &str = "[init:g3 v1 nonce=";
/// The give-up text for a G4 task with no script.
pub const GIVE_UP_TEXT: &str = "I cannot do this task.";

/// A body or kernel annotation the body appends as a user message (`[init:notice] …`, `[init:signals] …`,
/// `[init:boot] …`), not the probe or G3 instruction. On a bundle install the gate's own stage notices
/// (`[init:notice] gate: {…"stage":"G0"…}`) followed the probe line, and `[init:signals]` followed the
/// probe's tool result, in every G2 probe (REL-G0).
fn is_notice(m: &NMsg) -> bool {
    m.role == Role::User
        && m.text.trim_start().starts_with("[init:")
        && !m.text.contains(PROBE_MARKER)
        && !m.text.contains(G3_MARKER)
}

/// Probe responder: one `bash {"command":"true"}` call; `ready` once the last message (annotations
/// aside) is a tool result.
pub fn probe_reply(msgs: &[NMsg]) -> Reply {
    let last = msgs.iter().rev().find(|m| m.role != Role::System && !is_notice(m));
    if last.is_some_and(|m| m.role == Role::Tool) {
        Reply::text("ready")
    } else {
        Reply::call("bash", json!({"command": "true"}))
    }
}

/// Content-based probe detection, for upstream mode where the proxy strips `x-init-purpose`:
/// the most recent user message that is not an annotation carries the probe marker.
pub fn looks_like_probe(msgs: &[NMsg]) -> bool {
    msgs.iter()
        .rev()
        .find(|m| m.role == Role::User && !is_notice(m))
        .is_some_and(|m| m.text.contains(PROBE_MARKER))
}

/// The give-up reply.
pub fn give_up_reply() -> Reply {
    Reply::text(GIVE_UP_TEXT)
}

/// A G4 task session: the id from the first owner user message with a line starting `[task <id>]`
/// (kernel/gate.md §3.7.3), and that message's index. The body's own notices (`[init:boot]`, `[init:reload]`, …)
/// are skipped: a real body's cold boot puts its `[init:boot]` block before the task line (found by the REAL-M7
/// dry run, where every G4 and sealed task session of the real body fell through to the owner script).
pub fn detect_task(msgs: &[NMsg]) -> Option<(String, usize)> {
    for (i, m) in msgs.iter().enumerate() {
        if m.role != Role::User || m.text.trim_start().starts_with("[init:") {
            continue;
        }
        for line in m.text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("[task ")
                && let Some(end) = rest.find(']')
            {
                let id = &rest[..end];
                if valid_task_id(id) {
                    return Some((id.to_string(), i));
                }
            }
        }
        // Only the first user message can start a task session.
        return None;
    }
    None
}

/// Task ids name script files, so only `[A-Za-z0-9._-]{1,128}` without a leading dot is accepted.
pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// What the G3 responder needs from the gate's instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct G3 {
    /// Index of the instruction message.
    pub at: usize,
    /// The nonce `<n>`.
    pub nonce: String,
    /// The probe root `<H>`.
    pub home: String,
    /// Step 2's lines, verbatim.
    pub block: String,
    /// Step 4's commit message, verbatim.
    pub message: String,
}

/// Find and parse the G3 instruction (the first user message containing the marker).
pub fn detect_g3(msgs: &[NMsg]) -> Option<G3> {
    let (at, m) = msgs
        .iter()
        .enumerate()
        .find(|(_, m)| m.role == Role::User && m.text.contains(G3_MARKER))?;
    let text = &m.text[m.text.find(G3_MARKER)?..];
    let rest = &text[G3_MARKER.len()..];
    let nonce = rest[..rest.find(']')?].to_string();
    let home = {
        let a = text.find("git -C ")? + "git -C ".len();
        let b = text[a..].find("/body worktree add")? + a;
        text[a..b].to_string()
    };
    let lines: Vec<&str> = text.lines().collect();
    let section = |start: &str, end: &str| -> Option<String> {
        let s = lines
            .iter()
            .position(|l| l.trim_start().starts_with(start))?;
        let e = lines[s + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with(end))
            .map(|p| p + s + 1)?;
        let body: Vec<&str> = lines[s + 1..e].to_vec();
        let first = body.iter().position(|l| !l.trim().is_empty())?;
        let last = body.iter().rposition(|l| !l.trim().is_empty())?;
        Some(body[first..=last].join("\n"))
    };
    let block = section("2. ", "3. ").unwrap_or_else(|| default_block(&nonce));
    let message = section("4. ", "5. ").unwrap_or_else(|| default_message(&nonce));
    Some(G3 {
        at,
        nonce,
        home,
        block,
        message,
    })
}

fn default_block(n: &str) -> String {
    format!(
        "#[cfg(test)]\nmod g3_{n} {{\n    #[test]\n    fn probe() {{\n        assert_eq!(\"{n}\".len(), 6);\n    }}\n}}"
    )
}

fn default_message(n: &str) -> String {
    format!(
        "refactor(g3): evolvability probe {n}\n\nWhy: kernel G3 evolvability probe {n}, requested by the gate.\n\
         Predicts: pass_rate >= parent\nCompensates-for: none (gate probe)\nPrefix-change: no"
    )
}

/// POSIX shell single-quoting.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A heredoc delimiter that does not occur as a line of `body`.
fn delimiter(body: &str) -> String {
    let mut d = "G3EOF".to_string();
    while body.lines().any(|l| l == d) {
        d.push('X');
    }
    d
}

/// G3 responder: the reply by assistant-turn count since the instruction (gate.md §3.6 steps 1–5).
/// The bash timeout of the G3 responder's selfmap step (the tool's maximum is 3600 s).
pub const G3_SELFMAP_TIMEOUT_S: u64 = 1800;

pub fn g3_reply(msgs: &[NMsg], g3: &G3) -> Reply {
    let k = msgs[g3.at + 1..]
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    let h = &g3.home;
    let n = &g3.nonce;
    let wt = format!("{h}/work/wt-g3-{n}");
    let bash = |cmd: String| Reply::call("bash", json!({"command": cmd}));
    match k {
        0 => bash(format!(
            "git -C {} worktree add {} -b {}",
            sh_quote(&format!("{h}/body")),
            sh_quote(&wt),
            sh_quote(&format!("evo/g3-{n}"))
        )),
        1 => {
            let d = delimiter(&g3.block);
            bash(format!(
                "cat >> {} <<'{d}'\n\n{}\n{d}",
                sh_quote(&format!("{wt}/src/main.rs")),
                g3.block
            ))
        }
        // selfmap builds the body (minutes when cold): the bash tool's 120 s default would kill it and leave
        // self.json unchanged, which G3's shape check rejects (REAL-M7 dry run). A careful model sets a long timeout.
        2 => Reply::call(
            "bash",
            json!({"command": format!("{} selfmap --root {} --write", sh_quote(&format!("{h}/kernel/bin/init-kernel")), sh_quote(&wt)),
                   "timeout": G3_SELFMAP_TIMEOUT_S}),
        ),
        3 => {
            let d = delimiter(&g3.message);
            let w = sh_quote(&wt);
            bash(format!(
                "git -C {w} add -A && git -C {w} commit -q -F - <<'{d}' && git -C {w} rev-parse HEAD\n{}\n{d}",
                g3.message
            ))
        }
        _ => {
            let sha = msgs[g3.at + 1..]
                .iter()
                .rev()
                .filter(|m| m.role == Role::Tool)
                .find_map(|m| find_sha(&m.text))
                .unwrap_or_else(|| "unknown".to_string());
            Reply::text(&format!("G3 DONE {sha}"))
        }
    }
}

/// The first 40-character lowercase hex word in `s`.
pub fn find_sha(s: &str) -> Option<String> {
    s.split(|c: char| !c.is_ascii_hexdigit())
        .find(|w| {
            w.len() == 40
                && w.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(String::from)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mockllm::script::Stop;

    fn m(role: Role, text: &str) -> NMsg {
        NMsg {
            role,
            text: text.into(),
            call_ids: vec![],
            tool_call_id: None,
            has_reasoning: false,
        }
    }

    /// REAL-M7: the real body's cold boot sends `[init:boot]` first and the task line in the next user message.
    #[test]
    fn detect_task_skips_body_notices() {
        let task = vec![m(Role::System, "s"), m(Role::User, "[init:boot] Cold start\n- you: body x"), m(Role::User, "[task abc123] Workspace: /w\n\ndo it")];
        assert_eq!(detect_task(&task), Some(("abc123".into(), 2)));
        let live = vec![m(Role::System, "s"), m(Role::User, "[init:boot] Cold start"), m(Role::User, "hello\nnot [task x]")];
        assert_eq!(detect_task(&live), None);
        let later = vec![m(Role::User, "hello"), m(Role::User, "[task abc] x")];
        assert_eq!(detect_task(&later), None, "only the first owner message can start a task session");
    }

    /// The instruction exactly as kernel/gate.md §3.6 renders it, with a home containing a space.
    pub(crate) fn g3_instruction(h: &str, n: &str) -> String {
        const TEMPLATE: &str = r#"[init:g3 v1 nonce=<n>] Kernel evolvability check. Do exactly these steps, then stop.
1. Run: git -C <H>/body worktree add <H>/work/wt-g3-<n> -b evo/g3-<n>
2. Append these lines to the end of <H>/work/wt-g3-<n>/src/main.rs:

#[cfg(test)]
mod g3_<n> {
    #[test]
    fn probe() {
        assert_eq!("<n>".len(), 6);
    }
}

3. Run: <H>/kernel/bin/init-kernel selfmap --root <H>/work/wt-g3-<n> --write
4. In <H>/work/wt-g3-<n>, commit all changes with exactly this message:

refactor(g3): evolvability probe <n>

Why: kernel G3 evolvability probe <n>, requested by the gate.
Predicts: pass_rate >= parent
Compensates-for: none (gate probe)
Prefix-change: no

5. Reply with exactly: G3 DONE <full commit sha>
"#;
        TEMPLATE.replace("<H>", h).replace("<n>", n)
    }

    #[test]
    fn probe_is_stateless() {
        let a = probe_reply(&[
            m(Role::System, "S"),
            m(Role::User, "[init:probe] Gate probe"),
        ]);
        assert_eq!(a.calls[0].input, json!({"command": "true"}));
        assert_eq!(a.stop, Stop::ToolUse);
        let b = probe_reply(&[
            m(Role::User, "x"),
            m(Role::Assistant, ""),
            m(Role::Tool, ""),
        ]);
        assert_eq!(b.text, "ready");
        assert_eq!(b.stop, Stop::EndTurn);
    }

    #[test]
    fn probe_detection_by_content() {
        assert!(looks_like_probe(&[
            m(Role::User, "boot\n[init:probe] Gate probe"),
            m(Role::Assistant, ""),
            m(Role::Tool, "")
        ]));
        assert!(!looks_like_probe(&[
            m(Role::User, "[init:probe] x"),
            m(Role::Assistant, ""),
            m(Role::User, "[init:g3 v1 nonce=abc]")
        ]));
    }

    /// REL-G0: kernel notices after the probe line (and after its tool result) do not hide the probe,
    /// and the probe still makes exactly one call, then says ready.
    #[test]
    fn probe_with_trailing_notices() {
        let notice = r#"[init:notice] gate: {"gate_id":"j-000001","pass":true,"stage":"G0"}"#;
        let first = [
            m(Role::System, "S"),
            m(Role::User, "[init:boot] GATE PROBE: you are a staging copy"),
            m(Role::User, "[init:probe] Gate probe: call the bash tool once"),
            m(Role::User, notice),
        ];
        assert!(looks_like_probe(&first));
        assert_eq!(probe_reply(&first).calls[0].input, json!({"command": "true"}));
        let second = [
            m(Role::User, "[init:probe] Gate probe: call the bash tool once"),
            m(Role::User, notice),
            m(Role::Assistant, ""),
            m(Role::Tool, ""),
            m(Role::User, "[init:signals] L0 · context 2% (5k of 200k)"),
            m(Role::User, notice),
        ];
        assert!(looks_like_probe(&second));
        assert_eq!(probe_reply(&second).text, "ready");
        // An owner line after the probe is not a notice: not a probe any more.
        assert!(!looks_like_probe(&[m(Role::User, "[init:probe] x"), m(Role::User, "hello")]));
    }

    #[test]
    fn task_detection() {
        let msgs = [
            m(Role::System, "S"),
            m(
                Role::User,
                "boot block\n[task t-01] Workspace: /h/work/task\n\nDo it",
            ),
        ];
        assert_eq!(detect_task(&msgs), Some(("t-01".into(), 1)));
        assert_eq!(
            detect_task(&[m(Role::User, "hello"), m(Role::User, "[task x]")]),
            None
        );
        assert_eq!(detect_task(&[m(Role::User, "[task ../etc]")]), None);
    }

    #[test]
    fn g3_walks_the_five_steps() {
        let h = "/tmp/probe root/p1";
        let n = "a1b2c3";
        let mut msgs = vec![
            m(Role::System, "S"),
            m(Role::User, "[init:probe] Gate probe"),
            m(Role::Assistant, ""),
            m(Role::Tool, ""),
            m(Role::Assistant, "ready"),
            m(Role::User, &g3_instruction(h, n)),
        ];
        let g3 = detect_g3(&msgs).unwrap();
        assert_eq!((g3.at, g3.nonce.as_str(), g3.home.as_str()), (5, n, h));
        assert!(g3.block.starts_with("#[cfg(test)]\nmod g3_a1b2c3 {"));
        assert!(
            g3.message
                .starts_with("refactor(g3): evolvability probe a1b2c3\n\nWhy:")
        );
        assert!(g3.message.ends_with("Prefix-change: no"));
        let cmd = |r: &Reply| r.calls[0].input["command"].as_str().unwrap().to_string();
        let step = |msgs: &Vec<NMsg>| g3_reply(msgs, &detect_g3(msgs).unwrap());

        let r0 = step(&msgs);
        assert_eq!(
            cmd(&r0),
            "git -C '/tmp/probe root/p1/body' worktree add '/tmp/probe root/p1/work/wt-g3-a1b2c3' -b 'evo/g3-a1b2c3'"
        );
        msgs.extend([m(Role::Assistant, ""), m(Role::Tool, "Preparing worktree")]);
        let r1 = step(&msgs);
        assert!(cmd(&r1).starts_with(
            "cat >> '/tmp/probe root/p1/work/wt-g3-a1b2c3/src/main.rs' <<'G3EOF'\n\n#[cfg(test)]\n"
        ));
        assert!(cmd(&r1).ends_with("    }\n}\nG3EOF"));
        msgs.extend([m(Role::Assistant, ""), m(Role::Tool, "")]);
        let r2 = step(&msgs);
        assert!(cmd(&r2).contains(
            "init-kernel' selfmap --root '/tmp/probe root/p1/work/wt-g3-a1b2c3' --write"
        ));
        assert_eq!(r2.calls[0].input["timeout"], G3_SELFMAP_TIMEOUT_S, "a cold selfmap outlives the 120 s default");
        msgs.extend([m(Role::Assistant, ""), m(Role::Tool, "")]);
        let r3 = step(&msgs);
        assert!(cmd(&r3).contains("commit -q -F - <<'G3EOF' && git -C"));
        assert!(cmd(&r3).contains("\nrefactor(g3): evolvability probe a1b2c3\n"));
        let sha = "0123456789abcdef0123456789abcdef01234567";
        msgs.extend([
            m(Role::Assistant, ""),
            m(Role::Tool, &format!("{sha}\n[exit_code=0 wall_time=0.01s]")),
        ]);
        let r4 = step(&msgs);
        assert_eq!(r4.text, format!("G3 DONE {sha}"));
        assert_eq!(r4.stop, Stop::EndTurn);
    }

    #[test]
    fn quoting_and_sha() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(
            find_sha("x 0123456789abcdef0123456789abcdef01234567 y").as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(find_sha("short abc"), None);
    }
}
