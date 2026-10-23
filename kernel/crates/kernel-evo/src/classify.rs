//! The per-tool-result extractors of evolution-v2 §3.4.2 (pure): one tool call and its result in,
//! at most one signal out (the most specific one).
//!
//! The same classifier reads the body's session log (`tool_result` events, body/log-context.md
//! §3.3) and the proxy trace (chat-format `tool` messages, kernel/proxy.md §3.13), so a failure
//! counts the same whichever source carried it.
//!
//! Order (first match wins): owner abort → `initctl` result envelope → path errors →
//! silent doubled-prefix paths → bash timeout / background kill → `tool.error` / `bash.exit`.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The body's abort marker (seed `loop::ABORTED_TEXT`).
pub const ABORTED_TEXT: &str = "error: aborted: not executed (owner abort)";
/// The bash tool's background-kill note (seed `tools::bash::BG_NOTE`), matched by its prefix.
pub const BG_NOTE_PREFIX: &str = "[background processes still writing to this command's output were killed";

/// `INIT_HOME`'s top-level directories (supervise.md §3.1 layout). A relative path whose first
/// component is one of these was meant relative to `INIT_HOME`.
pub const HOME_DIRS: &[&str] = &["body", "work", "state", "kernel", "bin", "lineage", "vault", "cache", "root", "run", "probe", "fleet", "logs"];

/// First path components that are system locations, never a guessed home path.
const SYSTEM_DIRS: &[&str] = &["usr", "bin", "sbin", "etc", "opt", "System", "Library", "private", "tmp", "var", "dev", "proc", "sys", "work", "nix", "Applications", "Volumes"];

/// Where relative paths resolve and what a guessed absolute path is.
#[derive(Clone, Debug)]
pub struct PathCtx {
    /// The body's working directory relative to `INIT_HOME` (`work`).
    pub cwd_rel: String,
    /// `INIT_HOME` of the run that wrote the log, if known.
    pub init_home: Option<PathBuf>,
    /// Check the live file system (`INIT_HOME`-relative existence, missing parents). Off for
    /// fixtures from another machine.
    pub check_fs: bool,
}

impl Default for PathCtx {
    fn default() -> Self {
        PathCtx { cwd_rel: "work".into(), init_home: None, check_fs: false }
    }
}

/// One tool call with its result.
#[derive(Clone, Debug)]
pub struct ToolEvent<'a> {
    pub tool: &'a str,
    pub input: &'a Value,
    pub text: &'a str,
    /// The bash exit code when the source carries one (`None`: not bash, or signal / timeout).
    pub exit_code: Option<i64>,
    pub is_error: bool,
}

/// A signal hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The signal id with its subclass, e.g. `path.home_relative`, `bash.exit:git`,
    /// `initctl.refused:trailer.subject`. It is the cluster signature.
    pub signal: String,
    pub weight: u32,
    /// The call failed (as opposed to a silent hit on a successful call).
    pub failure: bool,
    /// The command (bash) or path (file tools) for the later-success checks.
    pub key: String,
    /// ≤ 200 bytes.
    pub detail: String,
}

/// The signal weights of §3.4.2.
pub fn weight(signal: &str) -> u32 {
    let base = signal.split(':').next().unwrap_or(signal);
    match base {
        "path.home_relative" | "path.doubled_prefix" | "path.guessed_absolute" => 2,
        "bash.timeout" | "bash.bg_killed" => 2,
        "initctl.refused" | "initctl.failed" => 2,
        "owner.interrupt" | "finish.tests_failing" | "task.fail" => 3,
        "owner.steer" | "cap.hit" | "finish.untested" => 2,
        _ => 1,
    }
}

/// Caps `s` at `n` bytes on a char boundary.
pub fn cap(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut i = n;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    s[..i].to_string()
}

fn hit(signal: impl Into<String>, failure: bool, key: &str, detail: &str) -> Hit {
    let signal = signal.into();
    Hit { weight: weight(&signal), signal, failure, key: key.to_string(), detail: cap(detail.trim(), 200) }
}

/// The bash exit code from the result text's trailer (`[exit_code=N …]`), `Some(-1)` for
/// `[exit_code=none signal=…]`, timeouts and aborts; `None` without a trailer.
pub fn exit_from_text(text: &str) -> Option<i64> {
    let last = text.trim_end().rsplit('\n').next().unwrap_or("");
    if let Some(r) = last.strip_prefix("[exit_code=") {
        let v = r.split([' ', ']']).next().unwrap_or("");
        return Some(v.parse().unwrap_or(-1));
    }
    if last.starts_with("[timeout after ") || last.starts_with("[aborted after ") {
        return Some(-1);
    }
    None
}

/// True when a tool result's text shows a failure: an `error:` first line, a non-zero exit
/// trailer, a timeout or an abort (evolution-v2 §3.4.7 rule 2, `trace:` refs).
pub fn text_is_failure(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("error:") || exit_from_text(text).is_some_and(|c| c != 0)
}

fn input_str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The bash command class of §3.4.2: the first command that is not a `cd` or an assignment.
pub fn command_class(cmd: &str) -> &'static str {
    let mut only_cd = false;
    for seg in segments(cmd).iter() {
        let mut words = seg.split_whitespace().filter(|w| !is_assignment(w));
        let Some(first) = words.next() else { continue };
        let first = first.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == '{');
        let base = first.rsplit('/').next().unwrap_or(first);
        if base == "cd" || base == "pushd" {
            only_cd = true;
            continue;
        }
        return match base {
            "cargo" => "cargo",
            "python3" | "python" => "python3",
            "pytest" => "pytest",
            "git" => "git",
            "make" => "make",
            "initctl" => "initctl",
            _ => "other",
        };
    }
    if only_cd { "cd" } else { "other" }
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

/// The command without heredoc bodies: `<<TAG` / `<<'TAG'` / `<<-TAG` and the lines up to `TAG`
/// are dropped (their content is data for another program, not shell words).
pub fn strip_heredocs(cmd: &str) -> String {
    let mut out = String::new();
    let mut end_tag: Option<String> = None;
    for line in cmd.split('\n') {
        if let Some(t) = &end_tag {
            if line.trim() == t {
                end_tag = None;
            }
            continue;
        }
        match line.find("<<") {
            Some(i) if !line[i..].starts_with("<<<") => {
                let rest = line[i + 2..].trim_start_matches('-').trim_start();
                let tag: String = rest.trim_start_matches(['\'', '"']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                out.push_str(&line[..i]);
                out.push('\n');
                if !tag.is_empty() {
                    end_tag = Some(tag);
                }
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Shell segments split on `&&`, `||`, `;`, `|` and newlines (heredoc bodies removed first).
fn segments(cmd: &str) -> Vec<String> {
    let head = strip_heredocs(cmd);
    let mut out = Vec::new();
    let bytes = head.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let c = bytes[i];
        let two = bytes.get(i..i + 2);
        if two == Some(b"&&") || two == Some(b"||") {
            out.push(&head[start..i]);
            i += 2;
            start = i;
            continue;
        }
        if c == b';' || c == b'|' || c == b'\n' {
            out.push(&head[start..i]);
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    out.push(&head[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// A `cd` target that keeps the cwd (`.`, `$PWD`).
fn is_noop_cd(target: &str) -> bool {
    let t = target.trim_matches(|c| c == '"' || c == '\'');
    t.is_empty() || t == "." || t == "$PWD" || t == "${PWD}" || t == "./"
}

/// The words of a segment with quotes stripped.
fn words(seg: &str) -> Vec<String> {
    seg.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '(' || c == ')' || c == '`')
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether the cwd is still the body's when `path` is first mentioned in `cmd`: no `cd` to
/// elsewhere before it (a `cd` whose target is `path` itself counts as mentioning it).
fn cwd_known_at(cmd: &str, path: &str) -> bool {
    for seg in segments(cmd).iter() {
        let w = words(seg);
        let is_cd = w.first().is_some_and(|f| f == "cd" || f == "pushd");
        if w.iter().any(|x| x == path || x.trim_end_matches('/') == path) {
            return true;
        }
        if is_cd && !is_noop_cd(w.get(1).map(String::as_str).unwrap_or("")) {
            return false;
        }
    }
    // Not found as a word (e.g. inside a string): known only if no cd precedes anything.
    !segments(cmd).iter().any(|seg| {
        let w = words(seg);
        w.first().is_some_and(|f| f == "cd" || f == "pushd") && !is_noop_cd(w.get(1).map(String::as_str).unwrap_or(""))
    })
}

/// Paths named missing by a failing result's text.
pub fn missing_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(i) = line.find(": No such file or directory") {
            let before = &line[..i];
            let p = before.rsplit(": ").next().unwrap_or(before).trim();
            if !p.is_empty() {
                out.push(p.to_string());
            }
        } else if let Some(r) = line.strip_prefix("error: not_found: ") {
            let p = r.split(" does not exist").next().unwrap_or(r).trim();
            if !p.is_empty() {
                out.push(p.to_string());
            }
        }
    }
    out
}

/// Classifies a path the body could not find (or silently mis-wrote): the `path.*` subclass.
fn path_class(p: &str, cwd_known: bool, cx: &PathCtx) -> Option<&'static str> {
    let c = cx.cwd_rel.trim_matches('/');
    if c.is_empty() {
        return None;
    }
    let doubled = format!("/{c}/{c}/");
    if p.contains(&doubled) || p.starts_with(&format!("{c}/{c}/")) || (cwd_known && !p.starts_with('/') && (p.starts_with(&format!("{c}/")) || p == c)) {
        return Some("path.doubled_prefix");
    }
    if p.starts_with('/') {
        let under_home = cx.init_home.as_deref().is_some_and(|h| Path::new(p).starts_with(h));
        if under_home {
            return None;
        }
        let first = p.trim_start_matches('/').split('/').next().unwrap_or("");
        if SYSTEM_DIRS.contains(&first) {
            return None;
        }
        if cx.check_fs && Path::new(p).parent().is_some_and(Path::exists) {
            return None;
        }
        if cx.init_home.is_none() && !cx.check_fs {
            // No home to compare with: only `/Users/<x>` and `/home/<x>` shapes are guesses.
            if !(first == "Users" || first == "home") {
                return None;
            }
        }
        return Some("path.guessed_absolute");
    }
    if !cwd_known {
        return None;
    }
    let first = p.trim_start_matches("./").split('/').next().unwrap_or("");
    if first != c && HOME_DIRS.contains(&first) {
        if cx.check_fs
            && let Some(h) = &cx.init_home
            && !h.join(p).exists()
        {
            return None;
        }
        return Some("path.home_relative");
    }
    None
}

/// Words of a bash command that start with `<cwd>/` before any `cd` elsewhere (silent writes).
fn doubled_words(cmd: &str, cx: &PathCtx) -> Option<String> {
    let c = cx.cwd_rel.trim_matches('/');
    let prefix = format!("{c}/");
    for seg in segments(cmd).iter() {
        let w = words(seg);
        let is_cd = w.first().is_some_and(|f| f == "cd" || f == "pushd");
        for x in w.iter().skip(if is_cd { 1 } else { 0 }) {
            if x.starts_with(&prefix) && !x.contains('$') && !x.contains('*') {
                return Some(x.clone());
            }
        }
        if is_cd && !is_noop_cd(w.get(1).map(String::as_str).unwrap_or("")) {
            return None;
        }
    }
    None
}

/// The test invocation in a bash command, normalized, with the `cd` target before it (the key of
/// the red-then-green exclusion and of `finish.*`).
pub fn test_key(cmd: &str) -> Option<String> {
    let mut dir = String::new();
    for seg in segments(cmd).iter() {
        let w = words(seg);
        if w.first().is_some_and(|f| f == "cd") {
            dir = w.get(1).cloned().unwrap_or_default();
            continue;
        }
        let j = w.iter().filter(|x| !is_assignment(x)).cloned().collect::<Vec<_>>().join(" ");
        for pat in ["python3 -m unittest", "python -m unittest", "python3 -m pytest", "python -m pytest", "pytest", "cargo test", "make test", "npm test", "go test"] {
            if j.contains(pat) {
                return Some(format!("{dir}|{pat}"));
            }
        }
    }
    None
}

/// An `initctl` JSON result inside a bash output: an error envelope or a gate result with
/// `pass:false`. Returns the signal (`initctl.refused:<code>` for validator refusals,
/// `initctl.failed:<code>` for kernel-side failures) and the code.
fn initctl_signal(cmd: &str, text: &str) -> Option<(String, String)> {
    if !segments(cmd).iter().any(|s| words(s).iter().any(|w| w == "initctl" || w.ends_with("/initctl"))) {
        return None;
    }
    let start = text.find("\n{").map(|i| i + 1).or_else(|| text.starts_with('{').then_some(0))?;
    let rest = &text[start..];
    let end = rest.rfind("\n}").map(|i| i + 2)?;
    let v: Value = serde_json::from_str(&rest[..end]).ok()?;
    if let Some(e) = v.get("error") {
        let code = e.get("code").and_then(Value::as_str).unwrap_or("E_UNKNOWN");
        let reason = e.get("data").and_then(|d| d.get("reason")).and_then(Value::as_str);
        if is_refusal_code(code) {
            return Some((format!("initctl.refused:{code}"), code.to_string()));
        }
        let c = reason.unwrap_or(code);
        return Some((format!("initctl.failed:{c}"), c.to_string()));
    }
    if v.get("pass") == Some(&Value::Bool(false)) {
        let w = v
            .get("reason")
            .and_then(Value::as_str)
            .or_else(|| v.get("warnings").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str))
            .unwrap_or("gate.failed");
        return Some((format!("initctl.failed:{w}"), w.to_string()));
    }
    None
}

/// A dotted lowercase code of the commit validator or the engine: the agent's input was refused
/// (evolution.md §4.2, evolution-v2 §4.4).
pub fn is_refusal_code(code: &str) -> bool {
    const FAMILIES: &[&str] = &["trailer.", "predicts.", "repro.", "promote.", "skill.", "retire.", "thin.", "upstream.", "revert.", "proposal.", "evo."];
    FAMILIES.iter().any(|f| code.starts_with(f)) && code != "evo.io"
}

/// Classifies one tool call and its result.
pub fn classify(ev: &ToolEvent<'_>, cx: &PathCtx) -> Option<Hit> {
    let text = ev.text;
    let first = text.trim_start().lines().next().unwrap_or("");
    let cmd = if ev.tool == "bash" { input_str(ev.input, "command").unwrap_or("") } else { "" };
    let file_path = input_str(ev.input, "path").or_else(|| ev.input.get("input").and_then(|i| input_str(i, "path")));
    let key = if ev.tool == "bash" { cmd.trim().to_string() } else { file_path.unwrap_or("").to_string() };
    let exit = if ev.tool == "bash" { ev.exit_code.or_else(|| exit_from_text(text)) } else { None };
    let failed = ev.is_error || first.starts_with("error:") || exit.is_some_and(|c| c != 0);

    if first.starts_with(ABORTED_TEXT) || text.trim_end().rsplit('\n').next().is_some_and(|l| l.starts_with("[aborted after ")) {
        return Some(hit("owner.interrupt", true, &key, first));
    }
    if ev.tool == "bash"
        && let Some((sig, code)) = initctl_signal(cmd, text)
    {
        return Some(hit(sig, true, &key, &format!("{code}: {}", cap(cmd, 120))));
    }
    if failed {
        for p in missing_paths(text) {
            let known = if ev.tool == "bash" { cwd_known_at(cmd, &p) } else { true };
            if let Some(class) = path_class(&p, known, cx) {
                return Some(hit(class, true, &p, &format!("{}: {p}", ev.tool)));
            }
        }
    }
    let c = cx.cwd_rel.trim_matches('/');
    if ev.tool == "bash" {
        if let Some(p) = doubled_words(cmd, cx) {
            return Some(hit("path.doubled_prefix", failed, &p, &format!("bash: {p}")));
        }
    } else if let Some(p) = file_path
        && !c.is_empty()
        && (p.starts_with(&format!("{c}/")) || p.contains(&format!("/{c}/{c}/")))
        && matches!(ev.tool, "read" | "write" | "edit" | "ext")
    {
        return Some(hit("path.doubled_prefix", failed, p, &format!("{}: {p}", ev.tool)));
    }
    if ev.tool == "bash" {
        let last = text.trim_end().rsplit('\n').next().unwrap_or("");
        if last.starts_with("[timeout after ") {
            return Some(hit("bash.timeout", true, &key, &cap(cmd, 160)));
        }
        if text.contains(BG_NOTE_PREFIX) {
            return Some(hit("bash.bg_killed", failed, &key, &cap(cmd, 160)));
        }
    }
    if !failed {
        return None;
    }
    if ev.tool == "bash" {
        let class = command_class(cmd);
        return Some(hit(format!("bash.exit:{class}"), true, &key, &format!("exit {}: {}", exit.unwrap_or(-1), cap(cmd, 160))));
    }
    let code = first
        .strip_prefix("error:")
        .map(|r| r.trim_start().split(|c: char| c == ':' || c.is_whitespace()).next().unwrap_or("").to_string())
        .filter(|c| !c.is_empty() && c.chars().all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '.' || x == '-'))
        .unwrap_or_else(|| "error".into());
    Some(hit(format!("tool.error:{}:{code}", ev.tool), true, &key, first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev<'a>(tool: &'a str, input: &'a Value, text: &'a str, exit: Option<i64>, err: bool) -> ToolEvent<'a> {
        ToolEvent { tool, input, text, exit_code: exit, is_error: err }
    }

    fn cx() -> PathCtx {
        PathCtx { cwd_rel: "work".into(), init_home: Some(PathBuf::from("/Users/q/Documents/INIT Agent/target/real-homes/r/ih")), check_fs: false }
    }

    #[test]
    fn s_real_paths() {
        let i = json!({"command": "cd work/a1-calc && ls -la && python3 -m unittest -q 2>&1 | head -50"});
        let t = "/opt/homebrew/bin/bash: line 1: cd: work/a1-calc: No such file or directory\n[exit_code=1 wall_time=0.01s]";
        assert_eq!(classify(&ev("bash", &i, t, Some(1), true), &cx()).unwrap().signal, "path.doubled_prefix");
        let i = json!({"path": "body/skills/ext-authoring/SKILL.md"});
        let t = "error: not_found: body/skills/ext-authoring/SKILL.md does not exist\nhint: the parent directory body/skills/ext-authoring does not exist either";
        assert_eq!(classify(&ev("read", &i, t, None, true), &cx()).unwrap().signal, "path.home_relative");
        let i = json!({"command": "cd \"$PWD\"; cat body/skills/ext-authoring/SKILL.md; ls body/ext/count-lines"});
        let t = "cat: body/skills/ext-authoring/SKILL.md: No such file or directory\n[exit_code=1 wall_time=0.01s]";
        assert_eq!(classify(&ev("bash", &i, t, Some(1), true), &cx()).unwrap().signal, "path.home_relative");
        let i = json!({"name": "semver-sort", "input": {"path": "work/a2/versions.txt"}});
        let t = "error: ext_failed: ext semver-sort 0.1.0: failed: /work/work/a2/versions.txt: No such file or directory (os error 44)";
        assert_eq!(classify(&ev("ext", &i, t, None, true), &cx()).unwrap().signal, "path.doubled_prefix");
        let i = json!({"path": "/Users/init/work/wt-doc/src/loop/mod.rs", "edits": []});
        let t = "error: not_found: /Users/init/work/wt-doc/src/loop/mod.rs does not exist\nhint: use write to create a new file";
        assert_eq!(classify(&ev("edit", &i, t, None, true), &cx()).unwrap().signal, "path.guessed_absolute");
    }

    #[test]
    fn silent_doubled_write_and_cd_elsewhere() {
        let i = json!({"command": "mkdir -p work/e2e && printf 'INIT OK\\n' > work/e2e/hello.txt && cat work/e2e/hello.txt"});
        let h = classify(&ev("bash", &i, "INIT OK\n[exit_code=0 wall_time=0.01s]", Some(0), false), &cx()).unwrap();
        assert_eq!((h.signal.as_str(), h.failure), ("path.doubled_prefix", false));
        let i = json!({"command": "cd \"$INIT_HOME\"; cat work/a2/versions.txt"});
        assert!(classify(&ev("bash", &i, "1.0.0\n[exit_code=0 wall_time=0.01s]", Some(0), false), &cx()).is_none());
        let i = json!({"path": "work/e2e/hello.txt", "content": "x"});
        assert_eq!(classify(&ev("write", &i, "wrote 1 line", None, false), &cx()).unwrap().signal, "path.doubled_prefix");
    }

    #[test]
    fn initctl_results() {
        let i = json!({"command": "cd \"$INIT_HOME/work/wt-doc\" && initctl promote evo/doc-loop --wait 2>&1 | tail -30"});
        let t = "{\n  \"error\": {\n    \"code\": \"trailer.subject\",\n    \"details\": []\n  }\n}\n[exit_code=0 wall_time=0.63s]";
        let h = classify(&ev("bash", &i, t, Some(0), false), &cx()).unwrap();
        assert_eq!(h.signal, "initctl.refused:trailer.subject");
        let i = json!({"command": "cd \"$INIT_HOME/body\" && initctl promote evo/m4 --wait"});
        let t = "[j-000004 build 5%] E_GATE_FAILED: G0 failed: gate.g0.test\n{\n  \"error\": {\n    \"code\": \"E_GATE_FAILED\",\n    \"data\": {\"reason\": \"gate.g0.test\", \"stage\": \"G0\"}\n  }\n}\n[exit_code=1 wall_time=1s]";
        assert_eq!(classify(&ev("bash", &i, t, Some(1), true), &cx()).unwrap().signal, "initctl.failed:gate.g0.test");
        let i = json!({"command": "cd \"$INIT_HOME/body\" && initctl gate --quick main"});
        let t = "[j-000002 build 100% eta 0s] done\n{\n  \"job\": \"j-000002\",\n  \"pass\": false,\n  \"warnings\": [\n    \"gate.g0.forbid_missing\"\n  ]\n}\n[exit_code=1 wall_time=368.17s]";
        assert_eq!(classify(&ev("bash", &i, t, Some(1), true), &cx()).unwrap().signal, "initctl.failed:gate.g0.forbid_missing");
    }

    #[test]
    fn exits_errors_aborts() {
        let i = json!({"command": "cd \"$INIT_HOME/body\" && git rev-parse --short HEAD evo/m4"});
        assert_eq!(classify(&ev("bash", &i, "fatal: Needed a single revision\n[exit_code=128 wall_time=0.01s]", None, true), &cx()).unwrap().signal, "bash.exit:git");
        assert!(classify(&ev("bash", &i, "abc\n[exit_code=0 wall_time=0.01s]", None, false), &cx()).is_none());
        let i = json!({"path": "x.txt"});
        assert_eq!(classify(&ev("read", &i, "error: too_large: x.txt is 9 MiB", None, true), &cx()).unwrap().signal, "tool.error:read:too_large");
        assert_eq!(classify(&ev("read", &i, ABORTED_TEXT, None, true), &cx()).unwrap().signal, "owner.interrupt");
        let i = json!({"command": "sleep 99"});
        assert_eq!(classify(&ev("bash", &i, "[timeout after 5s: process group killed (TERM) wall_time=5.0s]", None, true), &cx()).unwrap().signal, "bash.timeout");
        assert_eq!(command_class("IH=/x; cd \"$IH\" && cargo build"), "cargo");
        assert_eq!(command_class("cd a"), "cd");
        assert_eq!(test_key("cd a1-calc && cat -n calc.py && python3 -m unittest -q 2>&1").as_deref(), Some("a1-calc|python3 -m unittest"));
        assert!(text_is_failure("x\n[exit_code=2 wall_time=0.1s]") && !text_is_failure("x\n[exit_code=0 wall_time=0.1s]"));
    }
}
