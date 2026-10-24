//! The parts of `$INIT_HOME/kernel/config.toml` supervise reads: `[supervise]` (supervise.md §4.9,
//! kernel/config.md §4.8), `[supervise.disk]`, `[supervise.fleet]`, `[sandbox]` paths,
//! `[model]`, and `proxy.test_mode`.
//!
//! The full schema and its cross-section validation belong to kernel/config.md's loader. This
//! reader checks only what supervise uses: types, ranges, the `_mib` test-mode rule and the
//! removed `notify_cmd`. Unknown sections are left to that loader; unknown `[supervise]` keys
//! are errors. Any error → the kernel exits 78 before `ready` (§3.9 step 4).

use crate::spawnspec::SandboxPaths;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `[supervise]` with defaults (§4.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuperviseCfg {
    pub ready_timeout_s: u64,
    pub settle_s: u64,
    pub probe_window_s: u64,
    pub probation_s: u64,
    pub heartbeat_stale_s: u64,
    pub pause_timeout_s: u64,
    pub rollback_max_defer_s: u64,
    pub exit_grace_s: u64,
    pub backoff_initial_ms: u64,
    pub backoff_max_ms: u64,
    pub backoff_reset_after_s: u64,
    pub crash_window_s: u64,
    pub crash_limit: u32,
    pub safe_crash_limit: u32,
    pub max_provisional_depth: u32,
    pub build_timeout_cold_s: u64,
    pub build_timeout_warm_s: u64,
    pub kupgrade_build_timeout_s: u64,
    pub deps_cache_keys: u32,
    pub max_concurrent_probes: u32,
    pub config_error_exits: u32,
    pub replay_frames: u64,
    pub replay_bytes: u64,
    /// Absolute path of the real git binary (§3.11.2).
    pub git: PathBuf,
    pub root_request_timeout_ms: u64,
    pub ledger_refresh_s: u64,
    /// `[supervise.disk]`, in bytes (from `_gib`, or `_mib` in test mode).
    pub min_free_build_bytes: u64,
    pub min_free_run_bytes: u64,
    /// `[supervise.fleet]`.
    pub member_crash_limit: u32,
    pub member_idle_stop_s: u64,
    pub member_keep_days: u64,
    pub default_share_musd_day: u64,
}

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

impl Default for SuperviseCfg {
    fn default() -> Self {
        SuperviseCfg {
            ready_timeout_s: 90,
            settle_s: 3,
            probe_window_s: 15,
            probation_s: 120,
            heartbeat_stale_s: 30,
            pause_timeout_s: 1800,
            rollback_max_defer_s: 3600,
            exit_grace_s: 10,
            backoff_initial_ms: 1000,
            backoff_max_ms: 60_000,
            backoff_reset_after_s: 300,
            crash_window_s: 600,
            crash_limit: 4,
            safe_crash_limit: 2,
            max_provisional_depth: 3,
            // REL-G0: a cold `init-body` build at SANDBOX_CARGO_JOBS took 500-877 s on an idle-ish M2
            // and over 900 s under load (owner install j-000006). 3600 / 1800 is what every real-install
            // run used. MERGE-T2CACHE: the first promote of a dependency set still pays the stub build
            // (cold bound); later ones and G3's inner gate start from cache/deps (warm bound).
            build_timeout_cold_s: 3600,
            build_timeout_warm_s: 1800,
            kupgrade_build_timeout_s: 1800,
            deps_cache_keys: 3,
            max_concurrent_probes: 6,
            config_error_exits: 2,
            replay_frames: 2000,
            replay_bytes: 262_144,
            git: default_git(),
            root_request_timeout_ms: 2000,
            ledger_refresh_s: 60,
            min_free_build_bytes: 5 * GIB,
            min_free_run_bytes: GIB,
            member_crash_limit: 3,
            member_idle_stop_s: 3600,
            member_keep_days: 7,
            default_share_musd_day: 500_000,
        }
    }
}

impl SuperviseCfg {
    pub fn ready_timeout(&self) -> Duration {
        Duration::from_secs(self.ready_timeout_s)
    }
    pub fn settle(&self) -> Duration {
        Duration::from_secs(self.settle_s)
    }
    pub fn probation(&self) -> Duration {
        Duration::from_secs(self.probation_s)
    }
    pub fn heartbeat_stale(&self) -> Duration {
        Duration::from_secs(self.heartbeat_stale_s)
    }
    pub fn pause_timeout(&self) -> Duration {
        Duration::from_secs(self.pause_timeout_s)
    }
    pub fn exit_grace(&self) -> Duration {
        Duration::from_secs(self.exit_grace_s)
    }
    pub fn crash_window(&self) -> Duration {
        Duration::from_secs(self.crash_window_s)
    }
    pub fn backoff_reset_after(&self) -> Duration {
        Duration::from_secs(self.backoff_reset_after_s)
    }
    pub fn rollback_max_defer(&self) -> Duration {
        Duration::from_secs(self.rollback_max_defer_s)
    }
    /// Backoff before the n-th consecutive crash's respawn (n ≥ 1): `min(max, initial × 2^(n−1))`.
    pub fn backoff(&self, n: u32) -> Duration {
        let shift = n.saturating_sub(1).min(32);
        let ms = self.backoff_initial_ms.saturating_mul(1u64 << shift).min(self.backoff_max_ms);
        Duration::from_millis(ms)
    }
}

/// The platform default git (§3.11.2): the real binary, not the `xcrun` shim.
pub fn default_git() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Developer/CommandLineTools/usr/bin/git")
    } else {
        PathBuf::from("/usr/bin/git")
    }
}

/// `[model]` (copied into `boot.json.model` and the proxy label).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCfg {
    pub descriptor: Option<init_proto::files::ModelDescriptor>,
    pub provider: String,
    pub id: String,
    pub effort: Option<String>,
    pub context_window: u64,
    pub max_output: u64,
}

impl Default for ModelCfg {
    fn default() -> Self {
        ModelCfg { descriptor: None, provider: "deepseek".into(), id: "deepseek-flash".into(), effort: None, context_window: 0, max_output: 0 }
    }
}

/// What supervise reads from `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KernelCfg {
    pub supervise: SuperviseCfg,
    pub sandbox: SandboxPaths,
    pub model: ModelCfg,
    /// `proxy.test_mode`: a test home (allows `_mib` thresholds; passes `INIT_TEST*` to bodies).
    pub test_mode: bool,
}

/// One configuration error, with its TOML key path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfgError {
    pub key: String,
    pub message: String,
}

impl std::fmt::Display for CfgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

/// Load `path`. A missing file gives the defaults (a fresh test home); a present but invalid
/// file gives every error at once.
pub fn load(path: &Path) -> Result<KernelCfg, Vec<CfgError>> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(defaults()),
        Err(e) => Err(vec![CfgError { key: "config.toml".into(), message: e.to_string() }]),
    }
}

/// Defaults with `[sandbox]` resolved from the environment the Root gave the kernel.
pub fn defaults() -> KernelCfg {
    KernelCfg {
        sandbox: SandboxPaths {
            toolchain: PathBuf::new(),
            devdir: PathBuf::from(if cfg!(target_os = "linux") { "/usr" } else { "/Library/Developer/CommandLineTools" }),
            home_real: crate::sys::passwd_home().unwrap_or_default(),
            sdkroot: if cfg!(target_os = "macos") { std::env::var_os("SDKROOT").map(PathBuf::from) } else { None },
        },
        ..KernelCfg::default()
    }
}

/// Parse config text.
pub fn parse(text: &str) -> Result<KernelCfg, Vec<CfgError>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| vec![CfgError { key: "config.toml".into(), message: e.to_string().replace('\n', " ") }])?;
    let mut errs = Vec::new();
    let mut cfg = defaults();
    let err = |errs: &mut Vec<CfgError>, key: &str, m: &str| errs.push(CfgError { key: key.into(), message: m.into() });

    cfg.test_mode = match table.get("proxy").and_then(|p| p.get("test_mode")) {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            err(&mut errs, "proxy.test_mode", "must be a boolean");
            false
        }
    };

    if let Some(m) = table.get("model") {
        let Some(m) = m.as_table() else {
            err(&mut errs, "model", "must be a table");
            return Err(errs);
        };
        if let Some(v) = m.get("provider") {
            match v.as_str() {
                Some(s) => cfg.model.provider = s.into(),
                None => err(&mut errs, "model.provider", "must be a string"),
            }
        }
        if let Some(v) = m.get("id") {
            match v.as_str() {
                Some(s) => cfg.model.id = s.into(),
                None => err(&mut errs, "model.id", "must be a string"),
            }
        }
        if let Some(v) = m.get("descriptor") {
            match v.clone().try_into::<init_proto::files::ModelDescriptor>() {
                Ok(d) => match d.validate() {
                    Ok(()) => {
                        cfg.model.context_window = u64::from(d.context_window);
                        cfg.model.max_output = u64::from(d.max_output);
                        cfg.model.descriptor = Some(d);
                    }
                    Err(message) => err(&mut errs, "model.descriptor", message),
                },
                Err(_) => err(&mut errs, "model.descriptor", "invalid typed descriptor"),
            }
        }
        if let Some(v) = m.get("effort") {
            match v.as_str() {
                Some(s) => cfg.model.effort = Some(s.into()),
                None => err(&mut errs, "model.effort", "must be a string"),
            }
        }
    }

    if let Some(sb) = table.get("sandbox").and_then(|v| v.as_table()) {
        for (k, slot) in [("toolchain", 0), ("developer_dir", 1), ("sdkroot", 2)] {
            if let Some(v) = sb.get(k) {
                match v.as_str() {
                    Some(s) if s.starts_with('/') => match slot {
                        0 => cfg.sandbox.toolchain = s.into(),
                        1 => cfg.sandbox.devdir = s.into(),
                        _ => cfg.sandbox.sdkroot = Some(s.into()),
                    },
                    _ => err(&mut errs, &format!("sandbox.{k}"), "must be an absolute path"),
                }
            }
        }
    }

    if let Some(sv) = table.get("supervise") {
        let Some(sv) = sv.as_table() else {
            err(&mut errs, "supervise", "must be a table");
            return Err(errs);
        };
        parse_supervise(sv, cfg.test_mode, &mut cfg.supervise, &mut errs);
    }
    if errs.is_empty() { Ok(cfg) } else { Err(errs) }
}

fn parse_supervise(t: &toml::Table, test_mode: bool, c: &mut SuperviseCfg, errs: &mut Vec<CfgError>) {
    let mut int = |key: &str, v: &toml::Value, min: u64, max: u64| -> Option<u64> {
        match v.as_integer() {
            Some(i) if i >= 0 && (i as u64) >= min && (i as u64) <= max => Some(i as u64),
            Some(_) => {
                errs.push(CfgError { key: format!("supervise.{key}"), message: format!("must be an integer in {min}..={max}") });
                None
            }
            None => {
                errs.push(CfgError { key: format!("supervise.{key}"), message: "must be an integer".into() });
                None
            }
        }
    };
    let mut disk_gib: (Option<u64>, Option<u64>) = (None, None);
    let mut disk_mib: (Option<u64>, Option<u64>) = (None, None);
    let mut later: Vec<CfgError> = Vec::new();
    for (k, v) in t {
        macro_rules! set {
            ($field:ident, $min:expr, $max:expr) => {
                if let Some(x) = int(k, v, $min, $max) {
                    c.$field = x as _;
                }
            };
        }
        match k.as_str() {
            "ready_timeout_s" => set!(ready_timeout_s, 1, 86_400),
            "settle_s" => set!(settle_s, 0, 3600),
            "probe_window_s" => set!(probe_window_s, 0, 3600),
            "probation_s" => set!(probation_s, 0, 86_400),
            "heartbeat_stale_s" => set!(heartbeat_stale_s, 1, 86_400),
            "pause_timeout_s" => set!(pause_timeout_s, 1, 86_400),
            "rollback_max_defer_s" => set!(rollback_max_defer_s, 0, 86_400),
            "exit_grace_s" => set!(exit_grace_s, 1, 3600),
            "backoff_initial_ms" => set!(backoff_initial_ms, 1, 3_600_000),
            "backoff_max_ms" => set!(backoff_max_ms, 1, 86_400_000),
            "backoff_reset_after_s" => set!(backoff_reset_after_s, 1, 86_400),
            "crash_window_s" => set!(crash_window_s, 1, 86_400),
            "crash_limit" => set!(crash_limit, 1, 1000),
            "safe_crash_limit" => set!(safe_crash_limit, 1, 1000),
            "max_provisional_depth" => set!(max_provisional_depth, 1, 100),
            "build_timeout_cold_s" => set!(build_timeout_cold_s, 1, 86_400),
            "build_timeout_warm_s" => set!(build_timeout_warm_s, 1, 86_400),
            "kupgrade_build_timeout_s" => set!(kupgrade_build_timeout_s, 1, 86_400),
            "deps_cache_keys" => set!(deps_cache_keys, 1, 100),
            "max_concurrent_probes" => set!(max_concurrent_probes, 1, 64),
            "config_error_exits" => set!(config_error_exits, 1, 100),
            "replay_frames" => set!(replay_frames, 0, 1_000_000),
            "replay_bytes" => set!(replay_bytes, 0, 1 << 30),
            "root_request_timeout_ms" => set!(root_request_timeout_ms, 1, 600_000),
            "ledger_refresh_s" => set!(ledger_refresh_s, 1, 86_400),
            "git" => match v.as_str() {
                Some(s) if s.starts_with('/') => c.git = s.into(),
                _ => later.push(CfgError { key: "supervise.git".into(), message: "must be an absolute path".into() }),
            },
            "notify_cmd" => later.push(CfgError { key: "supervise.notify_cmd".into(), message: "removed: alerts go through the Root (root/root.md X4)".into() }),
            "disk" => {
                let Some(d) = v.as_table() else {
                    later.push(CfgError { key: "supervise.disk".into(), message: "must be a table".into() });
                    continue;
                };
                for (dk, dv) in d {
                    let key = format!("disk.{dk}");
                    match dk.as_str() {
                        "min_free_build_gib" => disk_gib.0 = int(&key, dv, 0, 100_000),
                        "min_free_run_gib" => disk_gib.1 = int(&key, dv, 0, 100_000),
                        "min_free_build_mib" | "min_free_run_mib" if !test_mode => {
                            later.push(CfgError { key: format!("supervise.{key}"), message: "`_mib` thresholds are allowed only with proxy.test_mode = true (kernel/config.md §3.1)".into() })
                        }
                        "min_free_build_mib" => disk_mib.0 = int(&key, dv, 0, 100_000_000),
                        "min_free_run_mib" => disk_mib.1 = int(&key, dv, 0, 100_000_000),
                        _ => later.push(CfgError { key: format!("supervise.{key}"), message: "unknown key".into() }),
                    }
                }
            }
            "fleet" => {
                let Some(f) = v.as_table() else {
                    later.push(CfgError { key: "supervise.fleet".into(), message: "must be a table".into() });
                    continue;
                };
                for (fk, fv) in f {
                    let key = format!("fleet.{fk}");
                    match fk.as_str() {
                        "member_crash_limit" => {
                            if let Some(x) = int(&key, fv, 1, 1000) {
                                c.member_crash_limit = x as u32;
                            }
                        }
                        "member_idle_stop_s" => {
                            if let Some(x) = int(&key, fv, 1, 86_400 * 30) {
                                c.member_idle_stop_s = x;
                            }
                        }
                        "member_keep_days" => {
                            if let Some(x) = int(&key, fv, 0, 3650) {
                                c.member_keep_days = x;
                            }
                        }
                        "default_share_musd_day" => {
                            if let Some(x) = int(&key, fv, 0, u32::MAX as u64) {
                                c.default_share_musd_day = x;
                            }
                        }
                        _ => later.push(CfgError { key: format!("supervise.{key}"), message: "unknown key".into() }),
                    }
                }
            }
            _ => later.push(CfgError { key: format!("supervise.{k}"), message: "unknown key".into() }),
        }
    }
    if let Some(g) = disk_gib.0 {
        c.min_free_build_bytes = g * GIB;
    }
    if let Some(g) = disk_gib.1 {
        c.min_free_run_bytes = g * GIB;
    }
    if let Some(m) = disk_mib.0 {
        c.min_free_build_bytes = m * MIB;
    }
    if let Some(m) = disk_mib.1 {
        c.min_free_run_bytes = m * MIB;
    }
    errs.extend(later);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_sandbox_defaults_keep_absolute_developer_dir() {
        let paths = defaults().sandbox;
        #[cfg(target_os = "linux")]
        { assert_eq!(paths.devdir, PathBuf::from("/usr")); assert!(paths.sdkroot.is_none()); }
        #[cfg(target_os = "macos")]
        assert_eq!(paths.devdir, PathBuf::from("/Library/Developer/CommandLineTools"));
        assert!(paths.devdir.is_absolute());
        assert!(parse("[sandbox]\ndeveloper_dir = \"\"\n").unwrap_err().iter()
            .any(|error| error.key == "sandbox.developer_dir"));
        assert_eq!(parse("[sandbox]\ndeveloper_dir = \"/usr\"\n").unwrap().sandbox.devdir, PathBuf::from("/usr"));
    }

    #[test]
    fn explicit_model_descriptor_round_trips_and_rejects_invalid_limits() {
        let text = r#"[model]
provider = "local"
id = "org/custom"
[model.descriptor]
api = "chat_completions"
context_window = 131072
max_output = 32768
default_max_tokens = 8192
request_max_tokens = 4096
"#;
        let model = parse(text).unwrap().model;
        assert_eq!(model.context_window, 131072);
        assert_eq!(model.max_output, 32768);
        assert_eq!(model.descriptor.unwrap().request_max_tokens, 4096);
        assert!(parse(&text.replace("request_max_tokens = 4096", "request_max_tokens = 0")).is_err());
        assert!(parse(&format!("{text}images = true\n")).is_err());
    }

    #[test]
    fn defaults_and_backoff() {
        let c = SuperviseCfg::default();
        let seq: Vec<u64> = (1..=9).map(|n| c.backoff(n).as_secs()).collect();
        assert_eq!(seq, [1, 2, 4, 8, 16, 32, 60, 60, 60], "SV-2");
        assert_eq!(c.backoff(200).as_secs(), 60);
    }

    #[test]
    fn parse_supervise_and_test_mode_rule() {
        let ok = parse("[proxy]\ntest_mode = true\n[supervise]\nready_timeout_s = 2\nbackoff_initial_ms = 50\n[supervise.disk]\nmin_free_run_mib = 16\n[model]\nprovider = \"mock\"\nid = \"m\"\n[gate]\nx = 1\n").unwrap();
        assert_eq!(ok.supervise.ready_timeout_s, 2);
        assert_eq!(ok.supervise.min_free_run_bytes, 16 * MIB);
        assert_eq!(ok.model.id, "m");
        assert!(ok.test_mode);
        let e = parse("[supervise.disk]\nmin_free_run_mib = 16\n").unwrap_err();
        assert!(e[0].message.contains("test_mode"), "C-10");
        let e = parse("[supervise]\nnotify_cmd = []\nbogus = 1\nready_timeout_s = \"x\"\n").unwrap_err();
        assert_eq!(e.len(), 3);
        assert!(parse("not toml = = =").is_err());
    }
}
