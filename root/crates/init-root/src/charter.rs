//! R5: the charter (root/root.md §3.12): schema with every default, validation C1–C17, digest,
//! published view, diff. `egress::validate_actions` (C11) and `sealed::validate` (C12) own their tables.
//! ADR-011 (X7): the `[extend]` section and its rule C17; `check` re-validates an effective charter (grant.rs).

use crate::util::{icj, sha256_hex};
use crate::{egress, ledger, sealed};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub const CONSTITUTION: &str = include_str!("../assets/CONSTITUTION.md");
const TEMPLATE_HEAD: &str = include_str!("../assets/charter-template.toml");

/// §3.12.2: the template `install` writes when no charter exists.
pub fn template() -> String {
    format!("{TEMPLATE_HEAD}[constitution]\ntext = '''\n{CONSTITUTION}'''\n")
}

macro_rules! table {
    ($name:ident { $($f:ident : $t:ty = $d:expr),* $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
        #[serde(deny_unknown_fields, default)]
        pub struct $name { $(pub $f: $t),* }
        impl Default for $name { fn default() -> Self { Self { $($f: ($d).into()),* } } }
    };
}

/// `purpose.text` and `autonomy.max_level` are required, so these two tables take no serde defaults.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Purpose {
    pub text: String,
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Autonomy {
    pub max_level: i64,
}
table!(Constitution { text: String = CONSTITUTION });
table!(Money { day_usd: String = "5.00", total_usd: String = "25.00", day_starts_at_utc_offset: String = "+00:00" });
table!(Quotas { max_instances: i64 = 12, parallel_bodies: i64 = 4, cpu_core_hours_day: i64 = 48,
    disk_min_free_gib: i64 = 20, init_home_max_gib: i64 = 80, store_max_gib: i64 = 8, disk_pause_grace_s: i64 = 600 });
/// `[sandbox]`. The ADR-009 keys (v0.1.1) are left out of the effective JSON while they hold their defaults,
/// so a charter that does not set them keeps its v0.1.0 digest (status file SBX-009, deviations).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Sandbox {
    pub extra_read: Vec<String>,
    #[serde(skip_serializing_if = "is_true")]
    pub read_home: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read_deny: Vec<String>,
    #[serde(skip_serializing_if = "is_true")]
    pub allow_full: bool,
    /// MEM-RUNAWAY: the physical-footprint cap of one workload process tree (root/root.md §3.14.1), in MiB.
    #[serde(skip_serializing_if = "is_mem_cap_default")]
    pub mem_cap_mib: i64,
    /// MEM-RUNAWAY: below this much free disk the Root refuses new workloads and kills running ones, in GiB.
    #[serde(skip_serializing_if = "is_disk_floor_default")]
    pub disk_floor_gib: i64,
}
/// `sandbox.mem_cap_mib` and `sandbox.disk_floor_gib` defaults and production bounds (C16).
pub const MEM_CAP_MIB: (i64, i64, i64) = (4096, 512, 65_536);
pub const DISK_FLOOR_GIB: (i64, i64, i64) = (5, 1, 1024);
impl Default for Sandbox {
    fn default() -> Self {
        Sandbox { extra_read: Vec::new(), read_home: true, read_deny: Vec::new(), allow_full: true, mem_cap_mib: MEM_CAP_MIB.0,
            disk_floor_gib: DISK_FLOOR_GIB.0 }
    }
}
fn is_true(b: &bool) -> bool {
    *b
}
// Like the ADR-009 keys: left out of the effective JSON at their defaults, so existing charters keep their digest.
fn is_mem_cap_default(v: &i64) -> bool {
    *v == MEM_CAP_MIB.0
}
fn is_disk_floor_default(v: &i64) -> bool {
    *v == DISK_FLOOR_GIB.0
}
table!(PanicCfg { watch_paths: Vec<String> = Vec::<String>::new(), mirror_ref: bool = false, mirror_poll_s: i64 = 300 });
table!(Notify { cmd: Vec<String> = Vec::<String>::new() });
table!(Mirror { url: String = "", branch: String = "lineage", push_interval_s: i64 = 600 });
table!(L1 { min_promotions: i64 = 20, audit_clean_days: i64 = 14, min_audits_in_window: i64 = 1, max_demoting_hits: i64 = 0 });
table!(L2 { min_days: i64 = 30, min_audits: i64 = 4, trend_min_permille: i64 = 0 });
table!(L3 { min_days: i64 = 60, min_mutation_runs: i64 = 1, mutation_pass_permille: i64 = 1000 });
table!(L4 { min_days: i64 = 60 });
table!(Ladder { l1: L1 = L1::default(), l2: L2 = L2::default(), l3: L3 = L3::default(), l4: L4 = L4::default() });
/// ADR-011 Evaluation plan: the frozen twin's nightly replay (X9 reads it). `window_utc` (INTEG-A) is left out of the
/// effective JSON at its default, so a charter that does not set it keeps its digest.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Twin {
    pub enabled: bool,
    pub k: i64,
    pub usd_day: String,
    pub run_usd: String,
    pub run_requests: i64,
    pub run_minutes: i64,
    /// The off-peak window, UTC, `HH:MM-HH:MM` (it may wrap midnight): a replay pair starts only if both arms can
    /// finish inside it. The Root's model table carries no off-peak schedule, so the default is DeepSeek's current
    /// discounted span with egress.md's margins (17:00–00:30 UTC, 00:00–07:30 at +07:00).
    #[serde(skip_serializing_if = "is_twin_window_default")]
    pub window_utc: String,
}
pub const TWIN_WINDOW_UTC: &str = "17:00-00:30";
impl Default for Twin {
    fn default() -> Self {
        Twin { enabled: true, k: 6, usd_day: "0.30".into(), run_usd: "0.30".into(), run_requests: 60, run_minutes: 15,
            window_utc: TWIN_WINDOW_UTC.into() }
    }
}
#[allow(clippy::ptr_arg)] // serde's skip_serializing_if passes the field's own type
fn is_twin_window_default(v: &String) -> bool {
    v == TWIN_WINDOW_UTC
}
/// `extend.twin.window_utc` as (start, end) in ms from 00:00 UTC of the night's date, with end > start (a window that
/// wraps midnight ends on the next day). None: not `HH:MM-HH:MM` (00–23, 00–59), or an empty window.
pub fn twin_window(s: &str) -> Option<(i64, i64)> {
    let hm = |x: &str| -> Option<i64> {
        let (h, m) = x.split_once(':')?;
        let two = |d: &str| (d.len() == 2 && d.bytes().all(|c| c.is_ascii_digit())).then(|| d.parse::<i64>().ok()).flatten();
        let (h, m) = (two(h)?, two(m)?);
        (h < 24 && m < 60).then_some((h * 60 + m) * 60_000)
    };
    let (a, b) = s.split_once('-')?;
    let (a, b) = (hm(a)?, hm(b)?);
    (a != b).then_some((a, if b > a { b } else { b + 86_400_000 }))
}
// ADR-011 §9 `[extend]` (C17): the free path's switch, the group window (§3.2), the H5a floor, the blast-radius budget
// by level L0…L4 (§4.5; one kernel change open from L1 is a constant), decay (§6.2; longer only), the bloat budget at
// L0 (§6.4; × the level's `bloat_permille`), the snapshot cap (§4.2) and the twin. The owner may move any value in
// either direction within its bounds; the agent can only ask for the grant kinds of §7.4 (grant.rs).
table!(Extend { mode: String = "gated", win_turns: i64 = 20, win_max_h: i64 = 72, win_min_h: i64 = 2, burst_floor_usd: String = "0.50",
    open_groups: Vec<i64> = vec![4i64, 6, 8, 12, 12], changes_day: Vec<i64> = vec![48i64, 96, 192, 384, 384],
    group_max: Vec<i64> = vec![16i64, 16, 24, 32, 32], kernel_day: Vec<i64> = vec![0i64, 1, 3, 6, 6],
    background_permille: Vec<i64> = vec![300i64, 400, 500, 600, 600], bloat_permille: Vec<i64> = vec![1000i64, 1500, 2000, 3000, 3000],
    cold_turns: i64 = 40, cold_days: i64 = 7, archive_turns: i64 = 120, archive_days: i64 = 21, archive_debt_days: i64 = 7,
    bloat_caps: i64 = 64, bloat_t0_kib: i64 = 4096, bloat_ext_mib: i64 = 128, bloat_boot_permille: i64 = 1500,
    bloat_body_permille: i64 = 2000, snap_max_gib: i64 = 16, twin: Twin = Twin::default() });

/// The blast-radius budget of one level (ADR-011 §4.5), from `[extend]` and the grant overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub open_groups: i64,
    pub changes_day: i64,
    pub group_max: i64,
    pub kernel_open: i64,
    pub kernel_day: i64,
    pub background_permille: i64,
    pub bloat_permille: i64,
}

impl Extend {
    pub fn free(&self) -> bool {
        self.mode == "free"
    }
    fn is_default(&self) -> bool {
        *self == Extend::default()
    }
    pub fn budget(&self, level: i64) -> Budget {
        let i = level.clamp(0, 4) as usize;
        let at = |v: &[i64]| v.get(i).copied().unwrap_or(0);
        Budget { open_groups: at(&self.open_groups), changes_day: at(&self.changes_day), group_max: at(&self.group_max),
            kernel_open: i64::from(level >= 1), kernel_day: at(&self.kernel_day), background_permille: at(&self.background_permille),
            bloat_permille: at(&self.bloat_permille) }
    }
}

table!(RootCfg { test_mode: bool = false, kernel_hello_timeout_s: i64 = 30, kernel_ready_timeout_s: i64 = 1200,
    hb_interval_s: i64 = 5, hb_stale_s: i64 = 30, backoff_initial_ms: i64 = 1000, backoff_max_ms: i64 = 60000,
    backoff_reset_after_s: i64 = 300, crash_window_s: i64 = 600, crash_limit: i64 = 3, safe_crash_limit: i64 = 2,
    config_error_exits: i64 = 2, halt_retry_s: i64 = 21600, kernel_probation_s: i64 = 1800, safe_auto_clear_s: i64 = 86400,
    stop_grace_s: i64 = 40, swap_prepare_timeout_s: i64 = 1800, store_keep_tuples: i64 = 5,
    kernel_lineage_per_hour: i64 = 600, lineage_verify_tail: i64 = 1000, replay_frames: i64 = 2000,
    replay_bytes: i64 = 262144, owner_queue_max: i64 = 200 });

/// C13 production minimums for `root.*` (lifted in test mode; never negative).
const ROOT_MIN: [(&str, i64); 22] = [("kernel_hello_timeout_s", 10), ("kernel_ready_timeout_s", 60), ("hb_interval_s", 1),
    ("hb_stale_s", 3), ("backoff_initial_ms", 100), ("backoff_max_ms", 1000), ("backoff_reset_after_s", 60),
    ("crash_window_s", 60), ("crash_limit", 1), ("safe_crash_limit", 1), ("config_error_exits", 1), ("halt_retry_s", 600),
    ("kernel_probation_s", 300), ("safe_auto_clear_s", 3600), ("stop_grace_s", 5), ("swap_prepare_timeout_s", 60),
    ("store_keep_tuples", 2), ("kernel_lineage_per_hour", 1), ("lineage_verify_tail", 100), ("replay_frames", 100),
    ("replay_bytes", 65536), ("owner_queue_max", 10)];
const SECTIONS: [&str; 15] = ["schema_version", "purpose", "constitution", "autonomy", "money", "quotas", "actions",
    "sandbox", "panic", "notify", "mirror", "ladder", "sealed", "root", "extend"];

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Charter {
    pub schema_version: i64,
    pub purpose: Purpose,
    pub constitution: Constitution,
    pub autonomy: Autonomy,
    pub money: Money,
    pub quotas: Quotas,
    pub actions: toml::Table,
    pub sandbox: Sandbox,
    pub panic: PanicCfg,
    pub notify: Notify,
    pub mirror: Mirror,
    pub ladder: Ladder,
    pub sealed: sealed::Sealed,
    pub root: RootCfg,
    /// ADR-011: left out of the effective JSON at its defaults, so a charter without `[extend]` keeps its digest.
    #[serde(skip_serializing_if = "Extend::is_default")]
    pub extend: Extend,
}

/// A validation finding with its TOML key path; warnings do not make a charter invalid.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub key: String,
    pub rule: &'static str,
    pub msg: String,
    pub warn: bool,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if self.warn { "warning" } else { "error" };
        write!(f, "{kind} {} {}: {}", self.rule, if self.key.is_empty() { "(file)" } else { &self.key }, self.msg)
    }
}

pub fn err(key: &str, rule: &'static str, msg: impl Into<String>) -> Finding {
    Finding { key: key.into(), rule, msg: msg.into(), warn: false }
}

/// Paths the containment rules (C8, C14) are checked against.
pub struct Ctx<'a> {
    pub root_home: &'a Path,
    pub init_home: &'a Path,
    pub home: &'a Path,
}

pub struct Loaded {
    pub ch: Charter,
    pub digest: String,
    pub warnings: Vec<Finding>,
    pub src: String,
}

pub fn load(path: &Path, cx: &Ctx) -> Result<Loaded, Vec<Finding>> {
    let src = std::fs::read_to_string(path).map_err(|e| vec![err("", "C1", format!("{}: {e}", path.display()))])?;
    parse(&src, cx).map(|(ch, digest, warnings)| Loaded { ch, digest, warnings, src })
}

fn sect<T: DeserializeOwned + Default>(t: &toml::Table, pre: &str, k: &str, req: bool, e: &mut Vec<Finding>) -> T {
    let path = format!("{pre}{k}");
    let v = match t.get(k) {
        Some(v) => v.clone(),
        None if req => {
            e.push(err(&path, "C1", "required table is missing"));
            return T::default();
        }
        None => toml::Value::Table(toml::Table::new()),
    };
    v.try_into().unwrap_or_else(|x: toml::de::Error| {
        let m = x.message().trim().to_string();
        let field = ["unknown field `", "missing field `"].iter().find_map(|p| m.strip_prefix(p)?.split('`').next());
        e.push(err(&field.map_or(path.clone(), |f| format!("{path}.{f}")), "C1", m.clone()));
        T::default()
    })
}

/// Parses and validates; every error is reported together (C1–C14).
pub fn parse(src: &str, cx: &Ctx) -> Result<(Charter, String, Vec<Finding>), Vec<Finding>> {
    let t: toml::Table = toml::from_str(src).map_err(|x| vec![err("", "C1", x.to_string().trim().to_string())])?;
    let mut e: Vec<Finding> = t.keys().filter(|k| !SECTIONS.contains(&k.as_str())).map(|k| err(k, "C1", "unknown key")).collect();
    if t.get("schema_version").and_then(toml::Value::as_integer) != Some(1) {
        e.push(err("schema_version", "C1", "must be the integer 1"));
    }
    let lt = match t.get("ladder") {
        Some(toml::Value::Table(l)) => l.clone(),
        Some(_) => { e.push(err("ladder", "C1", "must be a table")); toml::Table::new() }
        None => toml::Table::new(),
    };
    e.extend(lt.keys().filter(|k| !["l1", "l2", "l3", "l4"].contains(&k.as_str())).map(|k| err(&format!("ladder.{k}"), "C1", "unknown key")));
    let test_mode = t.get("root").and_then(|r| r.get("test_mode")).and_then(toml::Value::as_bool) == Some(true);
    let ch = Charter {
        schema_version: 1,
        purpose: sect(&t, "", "purpose", true, &mut e),
        constitution: sect(&t, "", "constitution", false, &mut e),
        autonomy: sect(&t, "", "autonomy", true, &mut e),
        money: sect(&t, "", "money", false, &mut e),
        quotas: sect(&t, "", "quotas", false, &mut e),
        actions: egress::effective_actions(t.get("actions"), test_mode, &mut e),
        sandbox: sect(&t, "", "sandbox", false, &mut e),
        panic: sect(&t, "", "panic", false, &mut e),
        notify: sect(&t, "", "notify", false, &mut e),
        mirror: sect(&t, "", "mirror", false, &mut e),
        ladder: Ladder { l1: sect(&lt, "ladder.", "l1", false, &mut e), l2: sect(&lt, "ladder.", "l2", false, &mut e),
            l3: sect(&lt, "ladder.", "l3", false, &mut e), l4: sect(&lt, "ladder.", "l4", false, &mut e) },
        sealed: sect(&t, "", "sealed", false, &mut e),
        root: sect(&t, "", "root", false, &mut e),
        extend: sect(&t, "", "extend", false, &mut e),
    };
    // the rules run on the parsed values; a table that failed C1 holds defaults, so its rule findings are dropped
    let c1: Vec<String> = e.iter().map(|f| f.key.split('.').next().unwrap_or("").to_string()).collect();
    let mut r = Vec::new();
    rules(&ch, cx, &mut r);
    e.extend(r.into_iter().filter(|f| !c1.iter().any(|k| f.key.split('.').next() == Some(k.as_str()))));
    let digest = digest(&ch).map_err(|x| vec![err("actions", "C11", x.to_string())]);
    let (warn, errs): (Vec<_>, Vec<_>) = e.into_iter().partition(|f| f.warn);
    match digest {
        Ok(d) if errs.is_empty() => Ok((ch, d, warn)),
        Ok(_) => Err(errs.into_iter().chain(warn).collect()),
        Err(mut x) => Err({ x.extend(errs); x }),
    }
}

/// C5: `^[0-9]{1,7}(\.[0-9]{1,6})?$` parsed exactly into micro-USD.
pub fn musd(s: &str) -> Option<u64> {
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    let digits = |x: &str| x.bytes().all(|c| c.is_ascii_digit());
    let ok = (1..=7).contains(&i.len()) && digits(i) && digits(f) && f.len() <= 6 && (!f.is_empty() || !s.contains('.'));
    ok.then(|| i.parse::<u64>().ok().map(|i| i * 1_000_000 + format!("{f:0<6}").parse::<u64>().unwrap_or(0)))?
}

fn offset_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let d = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    b.len() == 6 && (b[0] == b'+' || b[0] == b'-') && d(1) && d(2) && b[3] == b':' && d(4) && d(5)
        && s[1..3].parse::<u8>().is_ok_and(|h| h <= 14) && s[4..6].parse::<u8>().is_ok_and(|m| m <= 59)
}

/// `git check-ref-format refs/heads/<b>`, natively.
fn ref_ok(b: &str) -> bool {
    let bad_char = |c: char| c.is_ascii_control() || " ~^:?*[\\".contains(c);
    !b.is_empty() && b != "@" && !b.contains("..") && !b.contains("@{") && !b.contains("//") && !b.chars().any(bad_char)
        && !b.starts_with('/') && !b.ends_with('/') && !b.ends_with('.')
        && b.split('/').all(|c| !c.starts_with('.') && !c.ends_with(".lock"))
}

fn ranges(v: &Value, pre: &str, rule: &'static str, spec: &[(&str, i64, i64)], e: &mut Vec<Finding>) {
    for &(k, lo, hi) in spec {
        let x = v[k].as_i64().unwrap_or(i64::MIN);
        if x < lo || x > hi {
            let hi = if hi == i64::MAX { String::new() } else { hi.to_string() };
            e.push(err(&format!("{pre}.{k}"), rule, format!("must be in {lo}..{hi} (got {x})")));
        }
    }
}

fn rules(c: &Charter, cx: &Ctx, e: &mut Vec<Finding>) {
    let test = c.root.test_mode;
    let warn = |key: &str, rule, msg: String| Finding { key: key.into(), rule, msg, warn: true };
    let p = &c.purpose.text;
    if p.is_empty() || p.len() > 4096 || p.contains("REPLACE ME") {
        e.push(err("purpose.text", "C2", "must be 1–4096 bytes and not the template's REPLACE ME text"));
    }
    if c.constitution.text.is_empty() || c.constitution.text.len() > 32_768 {
        e.push(err("constitution.text", "C3", "must be 1–32768 bytes"));
    }
    if !(0..=4).contains(&c.autonomy.max_level) {
        e.push(err("autonomy.max_level", "C4", "must be 0–4"));
    }
    let (day, total) = (musd(&c.money.day_usd), musd(&c.money.total_usd));
    for (k, v) in [("money.day_usd", day), ("money.total_usd", total)] {
        if v.is_none() {
            e.push(err(k, "C5", "must be a decimal string like \"5.00\" (≤ 7 digits, ≤ 6 decimals)"));
        }
    }
    if !offset_ok(&c.money.day_starts_at_utc_offset) {
        e.push(err("money.day_starts_at_utc_offset", "C5", "must match ^[+-](0[0-9]|1[0-4]):[0-5][0-9]$"));
    }
    let q = serde_json::to_value(&c.quotas).unwrap_or_default();
    let m = i64::MAX;
    ranges(&q, "quotas", "C6", &[("max_instances", 2, 64), ("parallel_bodies", 1, 32), ("cpu_core_hours_day", 0, m),
        ("disk_min_free_gib", i64::from(!test), m), ("init_home_max_gib", 10, m), ("store_max_gib", 1, 64), ("disk_pause_grace_s", 0, m)], e);
    if c.quotas.parallel_bodies + 7 > c.quotas.max_instances {
        e.push(warn("quotas.parallel_bodies", "C6", "parallel_bodies + 7 > max_instances: a full fleet plus a G4 gate would not fit".into()));
    }
    let (lad, base) = (serde_json::to_value(&c.ladder).unwrap_or_default(), serde_json::to_value(Ladder::default()).unwrap_or_default());
    for (lvl, o) in base.as_object().into_iter().flatten() {
        for (k, b) in o.as_object().into_iter().flatten() {
            let (v, b) = (lad[lvl][k].as_i64().unwrap_or(0), b.as_i64().unwrap_or(0));
            let looser = if k.starts_with("max_") { v > b } else { v < b || (k.ends_with("_permille") && k.starts_with("mutation") && v > 1000) };
            if looser {
                e.push(err(&format!("ladder.{lvl}.{k}"), "C7", format!("charter.ladder.looser: {v} is looser than the built-in {b}")));
            }
        }
    }
    let abs = |s: &String| s.starts_with('/');
    for w in &c.panic.watch_paths {
        if !abs(w) || Path::new(w).starts_with(cx.init_home) {
            e.push(err("panic.watch_paths", "C8", format!("{w}: must be absolute and outside INIT_HOME")));
        }
    }
    if c.panic.watch_paths.len() > 8 {
        e.push(err("panic.watch_paths", "C8", "at most 8 entries"));
    }
    let u = &c.mirror.url;
    let scp = u.split_once('@').and_then(|(a, r)| Some((a, r.split_once(':')?))).is_some_and(|(a, (h, p))| {
        !a.is_empty() && !h.is_empty() && !p.is_empty() && !a.contains('/') && !h.contains('/')
    });
    let url_ok = u.is_empty() || u.starts_with("ssh://") || scp || (test && u.starts_with("file://"));
    if !url_ok || u.chars().any(char::is_whitespace) {
        e.push(err("mirror.url", "C9", "must be \"\", ssh://…, or user@host:path (file:// only in test mode)"));
    }
    if !ref_ok(&c.mirror.branch) {
        e.push(err("mirror.branch", "C9", "fails git check-ref-format"));
    }
    let lo = if test { 1 } else { 60 };
    ranges(&json!({"push_interval_s": c.mirror.push_interval_s}), "mirror", "C9", &[("push_interval_s", lo, 86_400)], e);
    ranges(&json!({"mirror_poll_s": c.panic.mirror_poll_s}), "panic", "C9", &[("mirror_poll_s", lo, 86_400)], e);
    if c.notify.cmd.first().is_some_and(|a| !abs(a)) || c.notify.cmd.len() > 16 {
        e.push(err("notify.cmd", "C10", "argv[0] must be absolute; at most 16 elements"));
    }
    e.extend(sealed::validate(&c.sealed, total, c.quotas.max_instances));
    let r = serde_json::to_value(&c.root).unwrap_or_default();
    let mins: Vec<(&str, i64, i64)> = ROOT_MIN.iter().map(|&(k, lo)| (k, if test { 0 } else { lo }, m)).collect();
    ranges(&r, "root", "C13", &mins, e);
    if !test && (c.root.hb_stale_s < 3 * c.root.hb_interval_s || c.root.backoff_max_ms < c.root.backoff_initial_ms) {
        e.push(err("root.hb_stale_s", "C13", "needs hb_stale_s ≥ 3 × hb_interval_s and backoff_max_ms ≥ backoff_initial_ms"));
    }
    if c.sandbox.extra_read.len() > 8 {
        e.push(err("sandbox.extra_read", "C14", "at most 8 entries"));
    }
    for x in &c.sandbox.extra_read {
        let p = Path::new(x);
        let canon = std::fs::canonicalize(p).is_ok_and(|q| q == p);
        let clash = [cx.root_home, cx.init_home].iter().any(|h| p.starts_with(h) || h.starts_with(p));
        if !abs(x) || !canon || p == Path::new("/") || p == cx.home || clash {
            e.push(err("sandbox.extra_read", "C14", format!("{x}: must be absolute, canonical, existing, not / or your home, and unrelated to ROOT_HOME/INIT_HOME")));
        }
        for s in [".ssh", ".aws", ".gnupg", "Library/Keychains"].map(|s| cx.home.join(s)) {
            if s.starts_with(p) || p.starts_with(&s) {
                e.push(warn("sandbox.extra_read", "C14", format!("{x} exposes {}", s.display())));
            }
        }
    }
    // C16 (MEM-RUNAWAY): the workload memory cap and disk floor. Test mode lifts the minimums (a 64 MiB cap, a 0 floor
    // that turns the floor off) so tests need neither a large allocation nor a nearly full disk.
    let (cap_lo, floor_lo) = if test { (64, 0) } else { (MEM_CAP_MIB.1, DISK_FLOOR_GIB.1) };
    ranges(&json!({"mem_cap_mib": c.sandbox.mem_cap_mib, "disk_floor_gib": c.sandbox.disk_floor_gib}), "sandbox", "C16",
        &[("mem_cap_mib", cap_lo, MEM_CAP_MIB.2), ("disk_floor_gib", floor_lo, DISK_FLOOR_GIB.2)], e);
    // C15 (ADR-009): read_deny entries; a deny on a missing path still blocks its creation, so none need exist.
    if c.sandbox.read_deny.len() > init_sandbox_profiles::access::MAX_CHARTER_DENY {
        e.push(err("sandbox.read_deny", "C15", "at most 32 entries"));
    }
    for x in &c.sandbox.read_deny {
        let ok = init_sandbox_profiles::access::charter_entry(x).is_some_and(|p| {
            p != cx.home && !cx.init_home.starts_with(&p) && !p.starts_with(cx.init_home)
        });
        if !ok {
            e.push(err("sandbox.read_deny", "C15", format!("{x}: must be absolute (≤ 1024 bytes, no . or ..), not / or your home, and neither hold nor lie in INIT_HOME")));
        }
    }
    extend_rules(&c.extend, test, e);
}

/// C17 (ADR-011 §9): `[extend]` bounds. Decay is "longer only" and the window has the ADR's bounds; test mode lifts
/// those minimums to 1 (short test ledgers). L0 never allows a kernel change (the kernel lane starts at L1, §1.2).
fn extend_rules(x: &Extend, test: bool, e: &mut Vec<Finding>) {
    let lo = |v: i64| if test { 1 } else { v };
    if !["free", "gated"].contains(&x.mode.as_str()) {
        e.push(err("extend.mode", "C17", "must be \"free\" or \"gated\""));
    }
    let v = serde_json::to_value(x).unwrap_or_default();
    ranges(&v, "extend", "C17", &[("win_turns", lo(10), 60), ("win_max_h", lo(24), 240), ("win_min_h", lo(1), 24),
        ("cold_turns", lo(40), 100_000), ("cold_days", lo(7), 3650), ("archive_turns", lo(120), 100_000), ("archive_days", lo(21), 3650),
        ("archive_debt_days", lo(7), 365), ("bloat_caps", 1, 100_000), ("bloat_t0_kib", 64, 8192), ("bloat_ext_mib", 1, 4096),
        ("bloat_boot_permille", 1000, 10_000), ("bloat_body_permille", 1000, 10_000), ("snap_max_gib", 1, 256)], e);
    if x.win_min_h > x.win_max_h || x.archive_turns < x.cold_turns || x.archive_days < x.cold_days {
        e.push(err("extend.win_min_h", "C17", "needs win_min_h ≤ win_max_h, archive_turns ≥ cold_turns and archive_days ≥ cold_days"));
    }
    for (k, a, lo, hi) in [("open_groups", &x.open_groups, 1, 64), ("changes_day", &x.changes_day, 1, 10_000), ("group_max", &x.group_max, 1, 256),
        ("kernel_day", &x.kernel_day, 0, 24), ("background_permille", &x.background_permille, 0, 1000), ("bloat_permille", &x.bloat_permille, 1000, 10_000)] {
        if a.len() != 5 || a.iter().any(|v| *v < lo || *v > hi) {
            e.push(err(&format!("extend.{k}"), "C17", format!("must be 5 values (L0…L4), each in {lo}..{hi}")));
        }
    }
    if x.kernel_day.first().is_some_and(|v| *v != 0) {
        e.push(err("extend.kernel_day", "C17", "L0 allows no kernel change (the kernel lane starts at L1)"));
    }
    if !musd(&x.burst_floor_usd).is_some_and(|m| (100_000..=10_000_000).contains(&m)) {
        e.push(err("extend.burst_floor_usd", "C17", "must be a decimal string in 0.10..10.00"));
    }
    let t = &x.twin;
    let (day, run) = (musd(&t.usd_day), musd(&t.run_usd));
    if day.is_none() || run.is_none() {
        e.push(err("extend.twin.usd_day", "C17", "usd_day and run_usd must be decimal strings (nonenforcing metadata)"));
    }
    ranges(&json!({"k": t.k, "run_minutes": t.run_minutes, "run_requests": t.run_requests}), "extend.twin", "C17",
        &[("k", 1, 24), ("run_minutes", 1, 120), ("run_requests", 1, 1000)], e);
    if !twin_window(&t.window_utc).is_some_and(|(a, b)| b - a >= 2 * t.run_minutes.max(1) * 60_000) {
        e.push(err("extend.twin.window_utc", "C17", "must be \"HH:MM-HH:MM\" (UTC; it may wrap midnight) and hold one pair, 2 × run_minutes"));
    }
}

/// The errors (not the warnings) of C2–C17 over a parsed charter: grant.rs re-validates an effective charter with it.
pub fn check(c: &Charter, cx: &Ctx) -> Vec<Finding> {
    let mut e = Vec::new();
    rules(c, cx, &mut e);
    e.retain(|f| !f.warn);
    e
}

/// The `[extend]` digest the `charter` record carries (ADR-011 Records: "`charter` records the `[extend]` digest").
pub fn extend_sha256(c: &Charter) -> String {
    sha256_hex(icj(&serde_json::to_value(&c.extend).unwrap_or_default()).unwrap_or_default().as_bytes())
}

/// The charter with every default filled in, as JSON (the digest's input).
pub fn effective(c: &Charter) -> Value {
    serde_json::to_value(c).unwrap_or_default()
}

pub fn digest(c: &Charter) -> std::io::Result<String> {
    Ok(sha256_hex(icj(&effective(c))?.as_bytes()))
}

/// §3.12.5: `INIT_HOME/root/charter.json`.
pub fn view(c: &Charter, digest: &str) -> Value {
    let mut v = effective(c);
    v["mirror"]["url"] = json!(if c.mirror.url.is_empty() { "" } else { "set" });
    v["panic"]["watch_paths"] = json!(c.panic.watch_paths.len());
    v["notify"]["cmd"] = json!(c.notify.cmd.len());
    v["constitution"] = json!({"sha256": format!("sha256:{}", sha256_hex(c.constitution.text.as_bytes()))});
    v["actions"] = c.actions.iter().map(|(k, t)| (k.clone(), json!({"kind": t.get("kind").and_then(|x| x.as_str()),
        "min_level": t.get("min_level").and_then(|x| x.as_integer())}))).collect::<serde_json::Map<_, _>>().into();
    v["charter_sha256"] = json!(digest);
    v["supported_max_level"] = json!(ledger::SUPPORTED_MAX_LEVEL);
    v
}

/// Leaf key paths → ICJ values (objects are walked; arrays are leaves).
pub fn flatten(v: &Value, pre: &str, out: &mut BTreeMap<String, String>) {
    match v {
        Value::Object(o) if !o.is_empty() => o.iter().for_each(|(k, x)| flatten(x, &if pre.is_empty() { k.clone() } else { format!("{pre}.{k}") }, out)),
        _ => { out.insert(pre.to_string(), icj(v).unwrap_or_default()); }
    }
}

/// Sorted changed key paths with old and new values.
pub fn diff(old: Option<&Value>, new: &Value) -> Vec<(String, Option<String>, Option<String>)> {
    let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
    if let Some(o) = old {
        flatten(o, "", &mut a);
    }
    flatten(new, "", &mut b);
    let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).cloned().collect();
    keys.into_iter().filter(|k| a.get(k) != b.get(k)).map(|k| (k.clone(), a.get(&k).cloned(), b.get(&k).cloned())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cx() -> (PathBuf, PathBuf, PathBuf) {
        (PathBuf::from("/Users/o/.init-root"), PathBuf::from("/Users/o/.init"), PathBuf::from("/Users/o"))
    }
    fn check(src: &str) -> Result<(Charter, String, Vec<Finding>), Vec<Finding>> {
        let (r, i, h) = cx();
        parse(src, &Ctx { root_home: &r, init_home: &i, home: &h })
    }
    fn valid_template() -> String {
        template().replace("REPLACE ME: what INIT is for, in your words.", "Grow a coding agent that improves itself.")
    }
    fn keys(src: &str) -> Vec<(String, &'static str)> {
        check(src).expect_err("invalid").into_iter().filter(|f| !f.warn).map(|f| (f.key, f.rule)).collect()
    }

    #[test]
    fn legacy_dollar_values_are_metadata_not_cross_field_limits() {
        let t = valid_template().replace("day_usd = \"5.00\"", "day_usd = \"0\"")
            .replace("total_usd = \"25.00\"", "total_usd = \"0\"");
        let t = format!("{t}\n[sealed]\nreserve_usd = \"30.00\"\n[extend.twin]\nusd_day = \"0\"\nrun_usd = \"20.00\"\n");
        assert!(check(&t).is_ok(), "legacy monetary values must not impose admission constraints: {:?}", check(&t).err());
    }

    #[test]
    fn template_defaults_and_digest() {
        assert_eq!(keys(&template()), vec![("purpose.text".to_string(), "C2")]);
        let (c, d, w) = check(&valid_template()).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.autonomy.max_level, 4);
        assert_eq!(c.constitution.text, CONSTITUTION);
        assert_eq!((c.quotas.disk_pause_grace_s, c.root.hb_stale_s, c.sealed.parallel), (600, 30, 2));
        assert_eq!(c.mirror.url, "", "the mirror stays off until M0");
        assert!(c.actions.contains_key("crates") && c.actions.contains_key("providers"));
        assert_eq!((musd(&c.money.day_usd), musd(&c.money.total_usd)), (Some(5_000_000), Some(25_000_000)));
        assert_eq!(d.len(), 64);
        // comments and formatting do not change the digest; values do
        let reformatted = valid_template().replace("max_instances = 12", "max_instances   =  12 # same");
        assert_eq!(check(&reformatted).unwrap().1, d);
        assert_ne!(check(&valid_template().replace("max_instances = 12", "max_instances = 13")).unwrap().1, d);
    }

    #[test]
    fn minimal_charter_takes_every_default() {
        let (c, _, _) = check("schema_version = 1\n[purpose]\ntext = \"p\"\n[autonomy]\nmax_level = 0\n").unwrap();
        assert_eq!(c.money, Money::default());
        assert_eq!(c.mirror.url, "");
        assert_eq!(c.root, RootCfg::default());
        assert_eq!(c.actions["providers"]["models"]["deepseek-flash"]["min_level"].as_integer(), Some(0));
    }

    #[test]
    fn each_rule_reports_its_key_path() {
        let t = valid_template();
        let cases: Vec<(String, &str, &str)> = vec![
            (t.replace("schema_version = 1", "schema_version = 2"), "schema_version", "C1"),
            (format!("{t}\n[bogus]\nx = 1\n"), "bogus", "C1"),
            (t.replace("max_instances = 12", "max_instances = 12\nmax_instancez = 3"), "quotas.max_instancez", "C1"),
            (t.replace("[autonomy]\nmax_level = 4", "[autonomy]"), "autonomy.max_level", "C1"),
            (t.replace("Grow a coding agent that improves itself.", ""), "purpose.text", "C2"),
            (t.replace("max_level = 4 ", "max_level = 5 "), "autonomy.max_level", "C4"),
            (t.replace("day_usd = \"5.00\"", "day_usd = \"5.0000001\""), "money.day_usd", "C5"),
            (t.replace("\"+00:00\"", "\"+15:00\""), "money.day_starts_at_utc_offset", "C5"),
            (t.replace("max_instances = 12", "max_instances = 1"), "quotas.max_instances", "C6"),
            (t.replace("store_max_gib = 8", "store_max_gib = 65"), "quotas.store_max_gib", "C6"),
            (t.replace("disk_min_free_gib = 20", "disk_min_free_gib = 0"), "quotas.disk_min_free_gib", "C6"),
            (format!("{t}\n[ladder.l1]\nmin_promotions = 19\n"), "ladder.l1.min_promotions", "C7"),
            (format!("{t}\n[ladder.l1]\nmax_demoting_hits = 1\n"), "ladder.l1.max_demoting_hits", "C7"),
            (format!("{t}\n[panic]\nwatch_paths = [\"/Users/o/.init/state/x\"]\n"), "panic.watch_paths", "C8"),
            (format!("{t}\n[panic]\nwatch_paths = [\"rel\"]\n"), "panic.watch_paths", "C8"),
            (t.replace("url = \"\"", "url = \"https://x/y\""), "mirror.url", "C9"),
            (t.replace("url = \"\"", "url = \"file:///tmp/m\""), "mirror.url", "C9"),
            (t.replace("branch = \"lineage\"", "branch = \"a..b\""), "mirror.branch", "C9"),
            (t.replace("cmd = [\"/usr/bin/osascript\"", "cmd = [\"osascript\""), "notify.cmd", "C10"),
            (t.replace("kind = \"model\"", "kind = \"email\""), "actions.providers.kind", "C11"),
            (t.replace("output_musd_per_mtok = 1200000\n", ""), "actions.providers.models.deepseek-flash", "C11"),
            (format!("{t}\n[sealed]\naudit_n = 7\n"), "sealed.audit_n", "C12"),
            (format!("{t}\n[root]\nhb_interval_s = 20\n"), "root.hb_stale_s", "C13"),
            (format!("{t}\n[root]\ncrash_limit = 0\n"), "root.crash_limit", "C13"),
            (t.replace("extra_read = []", "extra_read = [\"/\"]"), "sandbox.extra_read", "C14"),
            (t.replace("extra_read = []", "extra_read = [\"/Users/o\"]"), "sandbox.extra_read", "C14"),
            (t.replace("read_deny = []", "read_deny = [\"~/x\"]"), "sandbox.read_deny", "C15"),
            (t.replace("read_deny = []", "read_deny = [\"/Users/o\"]"), "sandbox.read_deny", "C15"),
            (t.replace("read_deny = []", "read_deny = [\"/Users\"]"), "sandbox.read_deny", "C15"),
            (t.replace("read_deny = []", "read_deny = [\"/Users/o/.init/work\"]"), "sandbox.read_deny", "C15"),
            (t.replace("read_deny = []", "read_deny = [\"/a/../b\"]"), "sandbox.read_deny", "C15"),
            (t.replace("read_home = true", "read_home = 1"), "sandbox", "C1"),
            (t.replace("read_deny = []", "read_deny = []\nmem_cap_mib = 511"), "sandbox.mem_cap_mib", "C16"),
            (t.replace("read_deny = []", "read_deny = []\nmem_cap_mib = 65537"), "sandbox.mem_cap_mib", "C16"),
            (t.replace("read_deny = []", "read_deny = []\nmem_cap_mib = \"4G\""), "sandbox", "C1"),
            (t.replace("read_deny = []", "read_deny = []\ndisk_floor_gib = 0"), "sandbox.disk_floor_gib", "C16"),
            (t.replace("read_deny = []", "read_deny = []\ndisk_floor_gib = 1025"), "sandbox.disk_floor_gib", "C16"),
        ];
        for (src, key, rule) in cases {
            let got = keys(&src);
            assert!(got.contains(&(key.to_string(), rule)), "{key} {rule}: got {got:?}");
        }
    }

    #[test]
    fn all_errors_reported_together_and_test_mode_lifts_minimums() {
        let t = valid_template().replace("max_instances = 12", "max_instances = 1").replace("max_level = 4 ", "max_level = 9 ");
        assert!(keys(&format!("{t}\n[root]\ncrash_limit = 0\n")).len() >= 3);
        let test = format!("{}\n[root]\ntest_mode = true\ncrash_limit = 0\nsafe_auto_clear_s = 5\n",
            valid_template().replace("url = \"\"", "url = \"file:///tmp/m\"").replace("disk_min_free_gib = 20", "disk_min_free_gib = 0"));
        assert!(check(&test).is_ok(), "{:?}", check(&test).err());
        assert!(!keys(&format!("{}\n[root]\ntest_mode = true\ncrash_limit = -1\n", valid_template())).is_empty());
    }

    /// MEM-RUNAWAY (C16): `sandbox.mem_cap_mib` 512–65536 (default 4096) and `sandbox.disk_floor_gib` 1–1024 (default 5);
    /// both bounds are inclusive; test mode lifts only the minimums; the defaults leave the digest unchanged.
    #[test]
    fn mem_cap_and_disk_floor_bounds() {
        let t = valid_template();
        let with = |k: &str, v: i64| t.replace("read_deny = []", &format!("read_deny = []\n{k} = {v}"));
        let (c, d, _) = check(&t).unwrap();
        assert_eq!((c.sandbox.mem_cap_mib, c.sandbox.disk_floor_gib), (4096, 5));
        for (k, v) in [("mem_cap_mib", 512), ("mem_cap_mib", 65_536), ("disk_floor_gib", 1), ("disk_floor_gib", 1024)] {
            assert!(check(&with(k, v)).is_ok(), "{k} = {v} is in bounds");
        }
        assert_eq!(check(&with("mem_cap_mib", 4096)).unwrap().1, d, "an explicit default keeps the digest");
        assert_eq!(check(&with("disk_floor_gib", 5)).unwrap().1, d, "an explicit default keeps the digest");
        let (c2, d2, _) = check(&with("mem_cap_mib", 8192)).unwrap();
        assert_eq!(c2.sandbox.mem_cap_mib, 8192);
        assert_ne!(d2, d, "a changed cap changes the digest (the owner's charter edit flow sees it)");
        assert_eq!(effective(&c2)["sandbox"]["mem_cap_mib"], 8192);
        assert!(effective(&c)["sandbox"].get("mem_cap_mib").is_none(), "left out at its default");
        let test = |k: &str, v: i64| format!("{}\n[root]\ntest_mode = true\n", with(k, v));
        assert!(check(&test("mem_cap_mib", 64)).is_ok() && check(&test("disk_floor_gib", 0)).is_ok(), "test mode lifts the minimums");
        assert!(check(&test("mem_cap_mib", 63)).is_err() && check(&test("mem_cap_mib", 65_537)).is_err(), "not the maximum");
    }

    /// C17 (ADR-011 §9): `[extend]` defaults leave the digest unchanged; bounds per key; decay longer only (lifted in
    /// test mode); L0 allows no kernel change; the budget by level.
    #[test]
    fn extend_section_and_c17() {
        let t = valid_template();
        let (c, d, _) = check(&t).unwrap();
        assert_eq!(c.extend, Extend::default());
        assert!(!c.extend.free() && effective(&c).get("extend").is_none(), "left out at its defaults");
        assert_eq!(check(&format!("{t}\n[extend]\nmode = \"gated\"\n")).unwrap().1, d, "an explicit default keeps the digest");
        let (f, d2, _) = check(&format!("{t}\n[extend]\nmode = \"free\"\nopen_groups = [5, 6, 8, 12, 12]\n[extend.twin]\nk = 3\n")).unwrap();
        assert!(f.extend.free() && d2 != d);
        assert_eq!(effective(&f)["extend"]["mode"], "free", "X3 reads it here");
        assert_eq!(f.extend.budget(0), Budget { open_groups: 5, changes_day: 48, group_max: 16, kernel_open: 0, kernel_day: 0,
            background_permille: 300, bloat_permille: 1000 });
        assert_eq!((f.extend.budget(3).kernel_open, f.extend.budget(9).open_groups), (1, 12));
        let x = |body: &str| format!("{t}\n[extend]\n{body}\n");
        for (src, key) in [(x("mode = \"wild\""), "extend.mode"), (x("win_turns = 9"), "extend.win_turns"), (x("win_max_h = 241"), "extend.win_max_h"),
            (x("cold_days = 6"), "extend.cold_days"), (x("archive_turns = 119"), "extend.archive_turns"), (x("open_groups = [4, 6, 8, 12]"), "extend.open_groups"),
            (x("kernel_day = [1, 1, 3, 6, 6]"), "extend.kernel_day"), (x("bloat_permille = [900, 1500, 2000, 3000, 3000]"), "extend.bloat_permille"),
            (x("burst_floor_usd = \"0.05\""), "extend.burst_floor_usd"), (x("snap_max_gib = 0"), "extend.snap_max_gib"),
            (x("bogus = 1"), "extend.bogus")] {
            let got = keys(&src);
            assert!(got.iter().any(|(k, r)| k == key && (*r == "C17" || *r == "C1")), "{key}: {got:?}");
        }
        let test = format!("{}\n[root]\ntest_mode = true\n[extend]\ncold_days = 1\narchive_days = 2\nwin_turns = 2\n", t);
        assert!(check(&test).is_ok(), "test mode lifts the decay and window minimums: {:?}", check(&test).err());
        assert_eq!(extend_sha256(&c).len(), 64);
        assert!(super::check(&c, &Ctx { root_home: &cx().0, init_home: &cx().1, home: &cx().2 }).is_empty());
    }

    /// INTEG-A: `extend.twin.window_utc` (X9's off-peak window) is left out of the effective JSON at its default, so
    /// digests do not change; a set window is in the digest; C17 wants `HH:MM-HH:MM` holding one pair (2 × run_minutes).
    #[test]
    fn twin_window_key_c17_and_digest() {
        let t = valid_template();
        let (c, d, _) = check(&t).unwrap();
        assert_eq!((c.extend.twin.window_utc.as_str(), twin_window(TWIN_WINDOW_UTC)), (TWIN_WINDOW_UTC, Some((61_200_000, 88_200_000))));
        assert_eq!(check(&format!("{t}\n[extend.twin]\nwindow_utc = \"17:00-00:30\"\n")).unwrap().1, d, "an explicit default keeps the digest");
        let free = format!("{t}\n[extend]\nmode = \"free\"\n");
        let (f, df, _) = check(&free).unwrap();
        assert!(effective(&f)["extend"]["twin"].get("window_utc").is_none(), "left out at its default inside a set [extend]");
        let (f2, df2, _) = check(&format!("{free}[extend.twin]\nwindow_utc = \"17:00-00:30\"\n")).unwrap();
        assert_eq!((df2, extend_sha256(&f2)), (df.clone(), extend_sha256(&f)));
        let (g, dg, _) = check(&format!("{free}[extend.twin]\nwindow_utc = \"16:00-23:00\"\n")).unwrap();
        assert!(dg != df && effective(&g)["extend"]["twin"]["window_utc"] == "16:00-23:00" && extend_sha256(&g) != extend_sha256(&f));
        for ok in ["23:30-00:00", "00:00-23:59", "22:00-06:00"] {
            assert!(check(&format!("{free}[extend.twin]\nwindow_utc = \"{ok}\"\n")).is_ok(), "{ok}");
        }
        for bad in ["17:00", "17:00-17:00", "24:00-01:00", "17:60-18:00", "7:00-09:00", "17:00-17:29", "17:00 - 18:00", ""] {
            let got = keys(&format!("{free}[extend.twin]\nwindow_utc = \"{bad}\"\n"));
            assert!(got.iter().any(|(k, r)| k == "extend.twin.window_utc" && *r == "C17"), "{bad}: {got:?}");
        }
        let longer = format!("{free}[extend.twin]\nrun_minutes = 60\nwindow_utc = \"17:00-18:59\"\n");
        assert!(keys(&longer).iter().any(|(k, _)| k == "extend.twin.window_utc"), "a pair of 60-minute arms needs 2 h");
        assert!(keys(&format!("{t}\n[extend.twin]\nwindow_utc = 5\n")).iter().any(|x| x.1 == "C1"), "a string");
    }

    #[test]
    fn warnings_do_not_invalidate() {
        let t = valid_template().replace("parallel_bodies = 4", "parallel_bodies = 6");
        let (_, _, w) = check(&t).unwrap();
        assert_eq!(w.iter().map(|f| f.rule).collect::<Vec<_>>(), vec!["C6"]);
    }

    #[test]
    fn money_parsing_is_exact() {
        assert_eq!(musd("0.000001"), Some(1));
        assert_eq!(musd("1234567.5"), Some(1_234_567_500_000));
        for bad in ["", ".5", "5.", "1e3", "-1", "12345678", "0.1234567", "5,00"] {
            assert_eq!(musd(bad), None, "{bad}");
        }
    }

    #[test]
    fn view_hides_what_the_zone_must_not_see() {
        let (c, d, _) = check(&valid_template()).unwrap();
        let v = view(&c, &d);
        assert_eq!(v["mirror"]["url"], "");
        assert_eq!(v["notify"]["cmd"], 3);
        assert_eq!(v["actions"]["providers"], json!({"kind":"model","min_level":0}));
        assert!(v["constitution"]["sha256"].as_str().unwrap().starts_with("sha256:"));
        assert!(!icj(&v).unwrap().contains("api.deepseek.com") && !icj(&v).unwrap().contains("Article 0"));
        assert_eq!(v["charter_sha256"], d.as_str());
    }

    #[test]
    fn diff_lists_changed_leaf_paths() {
        let (a, _, _) = check(&valid_template()).unwrap();
        let (b, _, _) = check(&valid_template().replace("day_usd = \"5.00\"", "day_usd = \"4.00\"")).unwrap();
        let d = diff(Some(&effective(&a)), &effective(&b));
        assert_eq!(d, vec![("money.day_usd".into(), Some("\"5.00\"".into()), Some("\"4.00\"".into()))]);
        assert!(diff(None, &effective(&a)).len() > 40);
    }
}
