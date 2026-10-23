//! Environment and process facts (body/boot.md §3.2, §4.1), `boot.json` (§3.3) and the model
//! check (§3.4). Every failure here is a config error: exit 78 with a `boot_error` line naming the
//! variable or field. The step budget is < 10 ms in total.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::BootError;
use crate::model::{Effort, ModelInfo};
use crate::provider::models::{SEED_MODELS, lookup};
use crate::provider::transport::MAX_SOCK_PATH;
use crate::tools::Mode;

/// The only control protocol this body speaks (loop.md §4.1).
pub const UI: &str = "stdio-jsonl/1";
pub const REASONS: [&str; 5] = ["cold", "promote", "rollback", "crash", "handoff"];
pub const MODES: [&str; 4] = ["normal", "probe", "safe", "check"];

/// A snapshot of the process environment (UTF-8 entries only), so validation is a pure function.
#[derive(Clone, Debug, Default)]
pub struct Vars(pub BTreeMap<String, String>);

impl Vars {
    pub fn from_process() -> Vars {
        Vars(
            std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
        )
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str)
    }

    /// `INIT_TEST_*` overrides are honoured only under `INIT_TEST=1` (boot.md §4.1).
    pub fn test_mode(&self) -> bool {
        self.get("INIT_TEST") == Some("1")
    }

    /// A millisecond override, only in test mode.
    pub fn test_ms(&self, k: &str) -> Option<Duration> {
        if !self.test_mode() {
            return None;
        }
        self.get(k)?
            .trim()
            .parse::<u64>()
            .ok()
            .map(Duration::from_millis)
    }
}

/// The validated environment (§3.2).
#[derive(Clone, Debug)]
pub struct BootEnv {
    pub home: PathBuf,
    /// Empty only in tests and manual runs: log-context then starts a new session.
    pub session: String,
    pub sha: String,
    pub reason: String,
    pub mode: Mode,
    pub boot_id: String,
    pub kernel_sock: PathBuf,
    pub proxy_sock: PathBuf,
    pub kernel_version: Option<String>,
    pub path_var: Option<String>,
    pub user_home: Option<PathBuf>,
    /// `INIT_WORKSPACE` (ADR-009): the owner's project directory, where the tools start. None when
    /// the variable is empty (no workspace), absent (an older kernel) or not absolute (a warning).
    pub workspace: Option<PathBuf>,
    /// `INIT_ACCESS` (ADR-009, SBX-009 §D): `none` · `read_home` · `workspace` · `full`, or a newer
    /// value kept verbatim.
    /// None when absent (an older kernel), empty or malformed (a warning).
    pub access: Option<String>,
    /// Warnings from the informational variables (`INIT_WORKSPACE`, `INIT_ACCESS`), which never
    /// fail the boot; `process_facts` carries them into the boot facts.
    pub warnings: Vec<String>,
}

pub fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::Normal => "normal",
        Mode::Probe => "probe",
        Mode::Safe => "safe",
        Mode::Check => "check",
    }
}

fn env_err(var: &str, msg: impl std::fmt::Display) -> BootError {
    BootError::config("env", format!("{var}: {msg}"))
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `^([0-9a-f]{40}|[0-9a-f]{64})$`
pub fn valid_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && is_hex(s)
}

/// `^[0-9A-Za-z._-]{1,64}$`
pub fn valid_session(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn sock_var(v: &Vars, var: &str) -> Result<PathBuf, BootError> {
    let s = v.get(var).ok_or_else(|| env_err(var, "missing"))?;
    let p = Path::new(s);
    if !p.is_absolute() {
        return Err(env_err(var, format!("must be an absolute path, got {s:?}")));
    }
    if s.len() > MAX_SOCK_PATH {
        return Err(env_err(
            var,
            format!(
                "{} bytes; a Unix socket path may have at most {MAX_SOCK_PATH} bytes here",
                s.len()
            ),
        ));
    }
    Ok(p.to_path_buf())
}

/// `^[a-z_]{1,32}$`
pub fn valid_access(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// At most 64 characters of a bad value, for a warning.
fn clip(s: &str) -> String {
    s.chars().take(64).collect()
}

/// `INIT_WORKSPACE` and `INIT_ACCESS` (ADR-009). Lenient: they are informational and
/// forward-compatible, so a bad value is dropped with a warning, never exit 78.
fn access_vars(v: &Vars, warnings: &mut Vec<String>) -> (Option<PathBuf>, Option<String>) {
    let workspace = match v.get("INIT_WORKSPACE").filter(|s| !s.is_empty()) {
        Some(s) if Path::new(s).is_absolute() => Some(PathBuf::from(s)),
        Some(s) => {
            warnings.push(format!(
                "INIT_WORKSPACE {:?} is not an absolute path; ignored (no workspace)",
                clip(s)
            ));
            None
        }
        None => None,
    };
    let access = match v.get("INIT_ACCESS").filter(|s| !s.is_empty()) {
        Some(a) if valid_access(a) => Some(a.to_string()),
        Some(a) => {
            warnings.push(format!(
                "INIT_ACCESS {:?} does not match ^[a-z_]{{1,32}}$; ignored",
                clip(a)
            ));
            None
        }
        None => None,
    };
    (workspace, access)
}

/// boot.md §3.2's table. Existence of the sockets is not checked (the proxy may be restarting).
pub fn validate(v: &Vars) -> Result<BootEnv, BootError> {
    let home = v.get("INIT_HOME").ok_or_else(|| env_err("INIT_HOME", "missing"))?;
    let hp = Path::new(home);
    if !hp.is_absolute() {
        return Err(env_err("INIT_HOME", format!("must be absolute, got {home:?}")));
    }
    if !hp.is_dir() {
        return Err(env_err("INIT_HOME", format!("{home:?} is not a directory")));
    }
    let session = v
        .get("INIT_SESSION")
        .ok_or_else(|| env_err("INIT_SESSION", "missing"))?;
    if !session.is_empty() && !valid_session(session) {
        return Err(env_err(
            "INIT_SESSION",
            format!("{session:?} does not match ^[0-9A-Za-z._-]{{1,64}}$"),
        ));
    }
    let sha = v.get("INIT_SHA").ok_or_else(|| env_err("INIT_SHA", "missing"))?;
    if !valid_sha(sha) {
        return Err(env_err("INIT_SHA", format!("{sha:?} is not a 40- or 64-hex sha")));
    }
    let reason = v
        .get("INIT_BOOT_REASON")
        .ok_or_else(|| env_err("INIT_BOOT_REASON", "missing"))?;
    if !REASONS.contains(&reason) {
        return Err(env_err(
            "INIT_BOOT_REASON",
            format!("{reason:?} is not one of {}", REASONS.join(", ")),
        ));
    }
    let mode = match v.get("INIT_MODE") {
        Some("normal") => Mode::Normal,
        Some("probe") => Mode::Probe,
        Some("safe") => Mode::Safe,
        Some("check") => Mode::Check,
        Some(m) => return Err(env_err("INIT_MODE", format!("{m:?} is not one of normal, probe, safe, check"))),
        None => return Err(env_err("INIT_MODE", "missing")),
    };
    let boot_id = v
        .get("INIT_BOOT_ID")
        .ok_or_else(|| env_err("INIT_BOOT_ID", "missing"))?;
    if boot_id.len() != 16 || !is_hex(boot_id) {
        return Err(env_err("INIT_BOOT_ID", format!("{boot_id:?} is not 16 lowercase hex")));
    }
    match v.get("INIT_UI") {
        Some(UI) => {}
        Some(u) => {
            return Err(env_err(
                "INIT_UI",
                format!("{u:?} is not supported; this body speaks only {UI}"),
            ));
        }
        None => return Err(env_err("INIT_UI", "missing")),
    }
    let kernel_sock = sock_var(v, "INIT_KERNEL_SOCK")?;
    let proxy_sock = sock_var(v, "INIT_PROXY_SOCK")?;
    let mut warnings = Vec::new();
    let (workspace, access) = access_vars(v, &mut warnings);
    Ok(BootEnv {
        home: hp.to_path_buf(),
        session: session.to_string(),
        sha: sha.to_string(),
        reason: reason.to_string(),
        mode,
        boot_id: boot_id.to_string(),
        kernel_sock,
        proxy_sock,
        kernel_version: v.get("INIT_KERNEL_VERSION").map(str::to_string),
        path_var: v.get("PATH").map(str::to_string),
        user_home: v.get("HOME").filter(|h| !h.is_empty()).map(PathBuf::from),
        workspace,
        access,
        warnings,
    })
}

// ---------------------------------------------------------------------------------------------
// Process facts (§3.2)
// ---------------------------------------------------------------------------------------------

fn executable_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The first absolute `PATH` entry holding an executable regular file `bash`; else `/bin/bash`;
/// else None (a warning; the bash tool then returns `spawn_failed`).
pub fn find_bash(path_var: Option<&str>) -> Option<PathBuf> {
    for dir in path_var.unwrap_or("").split(':') {
        let d = Path::new(dir);
        if d.is_absolute() && executable_file(&d.join("bash")) {
            return Some(d.join("bash"));
        }
    }
    let b = PathBuf::from("/bin/bash");
    executable_file(&b).then_some(b)
}

/// The first line of `bash --version` (one short child of our own, killed by handle on timeout).
pub fn bash_version(bash: &Path, timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(bash)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        let _ = tx.send(s);
    });
    match rx.recv_timeout(timeout) {
        Ok(s) => {
            let _ = child.wait();
            s.lines().next().map(|l| l.trim().to_string())
        }
        Err(_) => {
            let _ = child.kill(); // our own child, by handle
            let _ = child.wait();
            None
        }
    }
}

/// `/bin/kill`, `/usr/bin/kill`, else `bash-builtin` (tools.md §3.5.4).
pub fn find_kill() -> String {
    for k in ["/bin/kill", "/usr/bin/kill"] {
        if Path::new(k).exists() {
            return k.to_string();
        }
    }
    "bash-builtin".into()
}

/// cwd, bash and kill, captured once (§3.2).
#[derive(Clone, Debug)]
pub struct ProcFacts {
    pub cwd: PathBuf,
    pub bash: Option<PathBuf>,
    pub bash_version: Option<String>,
    pub kill: String,
    pub warnings: Vec<String>,
}

impl ProcFacts {
    /// Boot facts `bash`: `{"path","version"}`, or null.
    pub fn bash_value(&self) -> Value {
        match &self.bash {
            Some(p) => json!({"path": p.to_string_lossy(), "version": self.bash_version}),
            None => Value::Null,
        }
    }
}

/// cwd is the workspace when `INIT_WORKSPACE` names an existing directory (ADR-009: the tools start
/// there), else the process's own cwd. The body never calls `set_current_dir`.
pub fn process_facts(env: &BootEnv) -> Result<ProcFacts, BootError> {
    let process_cwd =
        || std::env::current_dir().map_err(|e| BootError::config("env", format!("cwd: {e}")));
    let mut warnings = env.warnings.clone();
    let cwd = match &env.workspace {
        Some(w) if w.is_dir() => w.clone(),
        Some(w) => {
            let cwd = process_cwd()?;
            warnings.push(format!(
                "INIT_WORKSPACE {} is not a directory; tools start in {}",
                w.display(),
                cwd.display()
            ));
            cwd
        }
        None => process_cwd()?,
    };
    let bash = find_bash(env.path_var.as_deref());
    let bash_version = match &bash {
        Some(b) => bash_version(b, Duration::from_secs(2)),
        None => {
            warnings.push("no bash on PATH and no /bin/bash: the bash tool will return spawn_failed".into());
            None
        }
    };
    let kill = find_kill();
    if kill == "bash-builtin" {
        warnings.push("no kill binary: process groups are killed with the bash builtin".into());
    }
    Ok(ProcFacts {
        cwd,
        bash,
        bash_version,
        kill,
        warnings,
    })
}

// ---------------------------------------------------------------------------------------------
// boot.json (§3.3) and the model check (§3.4)
// ---------------------------------------------------------------------------------------------

/// `state/kernel/boot.json`, checked against the env. `raw` is carried verbatim into the facts.
#[derive(Clone, Debug)]
pub struct BootJson {
    pub raw: Value,
    pub provider: String,
    pub model: String,
    /// `model.effort`; None = the model's default.
    pub effort: Option<Effort>,
    /// Safe mode reads T0 from `$INIT_HOME/<lkg_tree>`.
    pub lkg_tree: Option<String>,
}

impl BootJson {
    pub fn str(&self, k: &str) -> Option<&str> {
        self.raw.get(k).and_then(Value::as_str)
    }
}

pub fn boot_json_path(home: &Path) -> PathBuf {
    home.join("state").join("kernel").join("boot.json")
}

fn bj_err(msg: impl Into<String>) -> BootError {
    BootError::config("boot_json", msg)
}

/// Reads and checks `boot.json`: present, an object, `boot_id`/`mode`/`reason` equal to the env,
/// `model` present, `lkg_tree` present in safe mode. The model itself is checked by `check_model`.
pub fn read_boot_json(env: &BootEnv) -> Result<BootJson, BootError> {
    let p = boot_json_path(&env.home);
    let bytes = std::fs::read(&p).map_err(|e| bj_err(format!("{}: {e}", p.display())))?;
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|e| bj_err(format!("{}: not valid JSON: {e}", p.display())))?;
    if !raw.is_object() {
        return Err(bj_err("boot.json is not a JSON object"));
    }
    let field = |k: &str| raw.get(k).and_then(Value::as_str);
    match field("boot_id") {
        Some(b) if b == env.boot_id => {}
        Some(b) => {
            return Err(bj_err(format!(
                "boot_id {b:?} does not match INIT_BOOT_ID {:?} (a stale boot.json?)",
                env.boot_id
            )));
        }
        None => return Err(bj_err("boot_id is missing")),
    }
    let mode = mode_str(env.mode);
    match field("mode") {
        Some(m) if m == mode => {}
        other => return Err(bj_err(format!("mode {other:?} does not match INIT_MODE {mode:?}"))),
    }
    match field("reason") {
        Some(r) if r == env.reason => {}
        other => {
            return Err(bj_err(format!(
                "reason {other:?} does not match INIT_BOOT_REASON {:?}",
                env.reason
            )));
        }
    }
    let m = raw
        .get("model")
        .and_then(Value::as_object)
        .ok_or_else(|| BootError::config("model", "boot.json model is missing"))?;
    let provider = m
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| BootError::config("model", "boot.json model.provider is missing"))?;
    let model = m
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| BootError::config("model", "boot.json model.model is missing"))?;
    let effort = match m.get("effort") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(Effort::parse(s).ok_or_else(|| {
            BootError::config(
                "model",
                format!("bad effort {s:?}; known: low, medium, high, xhigh, max"),
            )
        })?),
        Some(other) => {
            return Err(BootError::config(
                "model",
                format!("bad effort {other}; known: low, medium, high, xhigh, max"),
            ));
        }
    };
    let lkg_tree = field("lkg_tree").map(str::to_string);
    if env.mode == Mode::Safe {
        match &lkg_tree {
            Some(t) if !t.is_empty() && !Path::new(t).is_absolute() && !t.split('/').any(|c| c == "..") => {}
            other => {
                return Err(bj_err(format!(
                    "safe mode needs lkg_tree (a path relative to INIT_HOME), got {other:?}"
                )));
            }
        }
    }
    Ok(BootJson {
        provider: provider.to_string(),
        model: model.to_string(),
        effort,
        lkg_tree,
        raw,
    })
}

/// "The model id is checked at boot" (CONVENTIONS §5): a row of provider.md's table, else 78.
pub fn check_model(b: &BootJson) -> Result<&'static ModelInfo, BootError> {
    if let Some(value) = b.raw.pointer("/model/descriptor").filter(|v| !v.is_null()) {
        let descriptor = serde_json::from_value(value.clone())
            .map_err(|e| BootError::config("model.descriptor", format!("invalid descriptor: {e}")))?;
        return crate::provider::models::register(&b.provider, &b.model, descriptor)
            .map_err(|e| BootError::config("model.descriptor", e));
    }
    lookup(&b.provider, &b.model).ok_or_else(|| {
        let known: Vec<String> = SEED_MODELS
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .collect();
        BootError::config(
            "model",
            format!(
                "unknown model \"{}/{}\"; known: {}",
                b.provider,
                b.model,
                known.join(", ")
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn base_vars(home: &Path) -> Vars {
        let mut m = BTreeMap::new();
        for (k, v) in [
            ("INIT_SESSION", "20261020-031500-a1b2c3"),
            ("INIT_SHA", "4b1e0c2d9a7f4b1e0c2d9a7f4b1e0c2d9a7f4b1e"),
            ("INIT_BOOT_REASON", "cold"),
            ("INIT_MODE", "normal"),
            ("INIT_BOOT_ID", "a3f09c1e5b7d2468"),
            ("INIT_UI", "stdio-jsonl/1"),
            ("INIT_KERNEL_SOCK", "/tmp/k.sock"),
            ("INIT_PROXY_SOCK", "/tmp/p.sock"),
            ("INIT_KERNEL_VERSION", "0.1.0"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        m.insert("INIT_HOME".into(), home.to_string_lossy().into_owned());
        Vars(m)
    }

    fn with(v: &Vars, k: &str, val: Option<&str>) -> Vars {
        let mut v = v.clone();
        match val {
            Some(x) => v.0.insert(k.into(), x.into()),
            None => v.0.remove(k),
        };
        v
    }

    /// §8 item 1: each var missing or invalid gives 78 naming the var.
    #[test]
    fn env_table() {
        let home = std::env::temp_dir();
        let ok = base_vars(&home);
        let e = validate(&ok).unwrap();
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.kernel_version.as_deref(), Some("0.1.0"));
        let long = format!("/{}", "s".repeat(MAX_SOCK_PATH));
        let at_limit = format!("/{}", "s".repeat(MAX_SOCK_PATH - 1));
        let long_session = "x".repeat(65);
        let cases: Vec<(&str, Option<&str>)> = vec![
            ("INIT_HOME", None),
            ("INIT_HOME", Some("relative/home")),
            ("INIT_HOME", Some("/nonexistent/init-home-wp13")),
            ("INIT_SESSION", None),
            ("INIT_SESSION", Some("bad session")),
            ("INIT_SESSION", Some(&long_session)),
            ("INIT_SHA", None),
            ("INIT_SHA", Some("4b1e0c2")),
            ("INIT_SHA", Some("4B1E0C2D9A7F4B1E0C2D9A7F4B1E0C2D9A7F4B1E")),
            ("INIT_BOOT_REASON", None),
            ("INIT_BOOT_REASON", Some("panic")),
            ("INIT_MODE", None),
            ("INIT_MODE", Some("fast")),
            ("INIT_BOOT_ID", None),
            ("INIT_BOOT_ID", Some("a3f09c1e5b7d246")),
            ("INIT_BOOT_ID", Some("A3F09C1E5B7D2468")),
            ("INIT_UI", None),
            ("INIT_UI", Some("stdio-jsonl/2")),
            ("INIT_KERNEL_SOCK", None),
            ("INIT_KERNEL_SOCK", Some("run/kernel.sock")),
            ("INIT_PROXY_SOCK", None),
            ("INIT_PROXY_SOCK", Some(&long)),
        ];
        for (k, val) in cases {
            let e = validate(&with(&ok, k, val)).unwrap_err();
            assert_eq!(e.exit_code(), 78, "{k}={val:?}");
            assert_eq!(e.step, "env");
            assert!(e.message.starts_with(k), "{k}: {}", e.message);
            let line: Value = serde_json::from_str(&e.to_json_line()).unwrap();
            assert_eq!(line["type"], "boot_error");
            assert_eq!(line["code"], 78);
        }
        // Empty INIT_SESSION is allowed (tests and manual runs); the socket limit is inclusive;
        // INIT_KERNEL_VERSION is informational.
        assert!(validate(&with(&ok, "INIT_SESSION", Some(""))).is_ok());
        assert!(validate(&with(&ok, "INIT_PROXY_SOCK", Some(&at_limit))).is_ok());
        assert!(validate(&with(&ok, "INIT_KERNEL_VERSION", None)).is_ok());
        // A 104-byte path fails on macOS (sun_path is 104 bytes with the NUL).
        if cfg!(target_os = "macos") {
            let p104 = format!("/{}", "s".repeat(103));
            assert_eq!(p104.len(), 104);
            assert!(validate(&with(&ok, "INIT_KERNEL_SOCK", Some(&p104))).is_err());
        }
    }

    /// ADR-009: `INIT_WORKSPACE` and `INIT_ACCESS` never fail the boot. Absent (an older kernel)
    /// or empty gives None; a malformed value gives None plus a warning; a good one is kept.
    #[test]
    fn workspace_and_access_are_lenient() {
        let home = std::env::temp_dir();
        let ok = base_vars(&home);
        let e = validate(&ok).unwrap();
        assert_eq!((e.workspace, e.access), (None, None));
        assert!(e.warnings.is_empty());
        let empty = with(&with(&ok, "INIT_WORKSPACE", Some("")), "INIT_ACCESS", Some(""));
        let e = validate(&empty).unwrap();
        assert_eq!((e.workspace, e.access), (None, None));
        assert!(e.warnings.is_empty(), "{:?}", e.warnings);

        let e = validate(&with(&ok, "INIT_WORKSPACE", Some("projects/app"))).unwrap();
        assert_eq!(e.workspace, None);
        assert_eq!(
            e.warnings,
            ["INIT_WORKSPACE \"projects/app\" is not an absolute path; ignored (no workspace)"]
        );
        let e = validate(&with(&ok, "INIT_WORKSPACE", Some("/Users/q/projects/app"))).unwrap();
        assert_eq!(e.workspace, Some(PathBuf::from("/Users/q/projects/app")));
        assert!(e.warnings.is_empty());

        let a32 = "a".repeat(32);
        for a in ["none", "read_home", "workspace", "full", "future_mode", a32.as_str()] {
            let e = validate(&with(&ok, "INIT_ACCESS", Some(a))).unwrap();
            assert_eq!(e.access.as_deref(), Some(a));
            assert!(e.warnings.is_empty(), "{a}: {:?}", e.warnings);
        }
        let a33 = "a".repeat(33);
        for a in ["FULL", "full\n", "work-space", "räd", " full", a33.as_str()] {
            let e = validate(&with(&ok, "INIT_ACCESS", Some(a))).unwrap();
            assert_eq!(e.access, None, "{a:?}");
            assert_eq!(e.warnings.len(), 1, "{a:?}");
            assert!(
                e.warnings[0].starts_with("INIT_ACCESS ")
                    && e.warnings[0].ends_with(" does not match ^[a-z_]{1,32}$; ignored"),
                "{}",
                e.warnings[0]
            );
        }
        // A huge value is clipped in the warning.
        let huge = "X".repeat(10_000);
        let e = validate(&with(&ok, "INIT_ACCESS", Some(&huge))).unwrap();
        assert!(e.warnings[0].len() < 200, "{}", e.warnings[0]);
    }

    /// `<target>/test-homes/wire009/<name>-<pid>` (`$CARGO_TARGET_DIR` when absolute, as under G0).
    fn wire009_dir(name: &str) -> PathBuf {
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
            .join("test-homes/wire009")
            .join(format!("{name}-{}", std::process::id()))
    }

    /// ADR-009 item 6: the tools start in the workspace when it is a directory (the process's own
    /// cwd is not changed); a workspace that is not a directory keeps the process cwd, with a
    /// warning. The informational warnings of `validate` are carried on.
    #[test]
    fn process_facts_cwd_is_the_workspace() {
        let dir = wire009_dir("pf");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let ws = dir.join("project");
        std::fs::create_dir_all(&ws).unwrap();
        let process_cwd = std::env::current_dir().unwrap();
        let home = std::env::temp_dir();
        let ok = base_vars(&home);

        let v = with(&ok, "INIT_WORKSPACE", Some(ws.to_str().unwrap()));
        let pf = process_facts(&validate(&v).unwrap()).unwrap();
        assert_eq!(pf.cwd, ws);
        assert!(!pf.warnings.iter().any(|w| w.contains("INIT_WORKSPACE")), "{:?}", pf.warnings);
        assert_eq!(std::env::current_dir().unwrap(), process_cwd, "no set_current_dir");

        let missing = ws.join("gone");
        let v = with(&ok, "INIT_WORKSPACE", Some(missing.to_str().unwrap()));
        let pf = process_facts(&validate(&v).unwrap()).unwrap();
        assert_eq!(pf.cwd, process_cwd);
        let want = format!(
            "INIT_WORKSPACE {} is not a directory; tools start in {}",
            missing.display(),
            process_cwd.display()
        );
        assert!(pf.warnings.contains(&want), "{:?}", pf.warnings);
        let file = ws.join("f.txt");
        std::fs::write(&file, "x").unwrap();
        let v = with(&ok, "INIT_WORKSPACE", Some(file.to_str().unwrap()));
        let pf = process_facts(&validate(&v).unwrap()).unwrap();
        assert_eq!(pf.cwd, process_cwd);

        // No workspace: the process cwd; validate's warnings reach the facts.
        let v = with(&ok, "INIT_WORKSPACE", Some("rel/dir"));
        let pf = process_facts(&validate(&v).unwrap()).unwrap();
        assert_eq!(pf.cwd, process_cwd);
        assert!(
            pf.warnings.iter().any(|w| w.starts_with("INIT_WORKSPACE \"rel/dir\"")),
            "{:?}",
            pf.warnings
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let _ = std::fs::remove_dir(dir.parent().unwrap()); // test-homes/wire009, only when empty
    }

    #[test]
    fn test_overrides_need_init_test() {
        let home = std::env::temp_dir();
        let v = with(&base_vars(&home), "INIT_TEST_PROBE_WATCHDOG_MS", Some("250"));
        assert_eq!(v.test_ms("INIT_TEST_PROBE_WATCHDOG_MS"), None);
        let v = with(&v, "INIT_TEST", Some("1"));
        assert_eq!(
            v.test_ms("INIT_TEST_PROBE_WATCHDOG_MS"),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn bash_lookup_prefers_path_order() {
        let found = find_bash(Some("relative/dir:/nonexistent-wp13:/bin"));
        assert_eq!(found, Some(PathBuf::from("/bin/bash")));
        let v = bash_version(Path::new("/bin/bash"), Duration::from_secs(2)).unwrap();
        assert!(v.contains("bash"), "{v}");
        assert!(["/bin/kill", "/usr/bin/kill", "bash-builtin"].contains(&find_kill().as_str()));
    }
}
