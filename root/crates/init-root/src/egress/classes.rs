//! R3 action classes (root/egress.md §3.5): the built-in classes, `validate_actions` (charter rule
//! C11), the compiled class table, routing and the request policy for model classes.

use super::Refusal;
use super::tap::Family;
use crate::charter::{Finding, err};
use serde::Deserialize;
use serde::de::IgnoredAny;
use std::collections::BTreeMap;

/// Classes present even when the charter omits them (egress.md §3.5: `providers` DeepSeek only, `crates` D10).
/// A charter table with the same id is merged over the built-in one, key by key (so `enabled = false` works).
const BUILTIN: &str = r#"
[providers]
kind = "model"
min_level = 0
destinations = ["api.deepseek.com:443"]
methods = ["GET", "POST"]
paths = ["chat/completions", "beta/chat/completions", "models", "user/balance"]
requests_per_day = 100000
bytes_per_day = 8000000000
rate_per_min = 600
[providers.models."deepseek-flash"]
min_level = 0
input_musd_per_mtok = 300000
cache_read_musd_per_mtok = 6000
output_musd_per_mtok = 1200000
reserve_output_tokens = 65536
[crates]
kind = "http"
min_level = 0
destinations = ["index.crates.io:443", "static.crates.io:443"]
methods = ["GET", "HEAD"]
paths = ["*"]
requests_per_day = 5000
bytes_per_day = 2000000000
rate_per_min = 300
response_max_bytes = 67108864
"#;
const CRATES_HOSTS: [&str; 2] = ["index.crates.io:443", "static.crates.io:443"];
const CLASS_KEYS: [&str; 12] = ["kind", "min_level", "destinations", "providers", "methods", "paths", "requests_per_day",
    "bytes_per_day", "rate_per_min", "models", "response_max_bytes", "enabled"];
/// `cache_read_musd_per_mtok` is optional: absent, cache reads bill at the input price (egress.md §3.4).
/// `cache_write_musd_per_mtok` (5-minute or single-class writes) and `cache_write_1h_musd_per_mtok` are optional
/// (v0.5.0 L3): absent, writes bill at `WRITE_FALLBACK` × the input price, never below a write price the providers
/// charge (the highest in reports/INIT v1 providers UX vision.md Table A is 2× input, Anthropic's 1-hour TTL).
const MODEL_KEYS: [&str; 7] = ["min_level", "input_musd_per_mtok", "cache_read_musd_per_mtok", "cache_write_musd_per_mtok",
    "cache_write_1h_musd_per_mtok", "output_musd_per_mtok", "reserve_output_tokens"];
const WRITE_FALLBACK: u64 = 2;
/// kernel/proxy.md §3.5: the provider presets and their base URLs.
const PRESETS: [(&str, &str, Auth); 3] = [("deepseek", "api.deepseek.com:443", Auth::Bearer),
    ("anthropic", "api.anthropic.com:443", Auth::XApiKey), ("openai", "api.openai.com:443", Auth::Bearer)];

/// F2 (build/review/root-security-2026-10-20.md), default-deny: every (method, upstream path) the Root
/// forwards for a model provider (kernel/proxy.md §3.5 preset route tables), and whether it is billed. A
/// billed route is inspected, gated by model and level, reserved and settled, on the wire family
/// `Family::of` its path; the others are read-only or free provider endpoints ($0). The `""` rows serve a
/// provider without a preset. Anything else is refused `class.unmetered`, even when the class lists it.
const ROUTES: [(&str, &str, &str, bool); 19] = [("deepseek", "POST", "chat/completions", true), ("deepseek", "POST", "beta/chat/completions", true), ("deepseek", "POST", "responses", true),
    ("deepseek", "POST", "anthropic/v1/messages", true), ("deepseek", "GET", "models", false), ("deepseek", "GET", "user/balance", false),
    ("anthropic", "POST", "v1/messages", true), ("anthropic", "POST", "v1/messages/count_tokens", false), ("anthropic", "GET", "v1/models", false),
    ("openai", "POST", "v1/responses", true), ("openai", "POST", "v1/chat/completions", true), ("openai", "GET", "v1/models", false),
    ("", "POST", "chat/completions", true), ("", "POST", "v1/chat/completions", true), ("", "POST", "responses", true), ("", "POST", "v1/responses", true),
    ("", "POST", "v1/messages", true), ("", "GET", "models", false), ("", "GET", "v1/models", false)];

/// Whether `method path` (no query; any method when `method` is empty) of provider `pid` is billed, when the
/// Root has a rule for it.
pub fn billed(pid: &str, method: &str, path: &str) -> Option<bool> {
    let p = if PRESETS.iter().any(|x| x.0 == pid) { pid } else { "" };
    ROUTES.iter().find(|r| r.0 == p && (method.is_empty() || r.1 == method) && r.2 == path).map(|r| r.3)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Auth {
    Bearer,
    XApiKey,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    pub host: String,
    pub base_path: String,
    pub auth: Auth,
    pub api: Option<Family>,
    pub local: bool,
    pub no_auth: bool,
}

fn api(s: &str) -> Option<Family> {
    match s { "chat" => Some(Family::Chat), "responses" => Some(Family::Responses), "messages" => Some(Family::Messages), _ => None }
}

fn api_route(family: Family, method: &str, path: &str) -> Option<bool> {
    let endpoint = match family { Family::Chat => "chat/completions", Family::Responses => "responses", Family::Messages => "messages", Family::None => return None };
    ((method.is_empty() || method == "POST") && (path == endpoint || path.strip_prefix("v1/") == Some(endpoint))).then_some(true)
}

/// Test keys never reach a remote host; production loopback requires a provider opt-in.
pub fn endpoint_allowed(test: bool, dest: &str, provider: Option<&Provider>) -> bool {
    if test { loopback(dest) } else { !loopback(dest) || provider.is_some_and(|p| p.local && p.host == dest) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub min_level: u8,
    pub input: u64,
    /// µUSD per 1M cache-hit input tokens, at the peak window (provisional owner decision 2026-10-20, §3.13).
    pub cache_read: u64,
    /// µUSD per 1M tokens written to the cache: 5-minute (or single-class) and 1-hour writes (v0.5.0 L3).
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub reserve_out: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub id: String,
    pub model: bool,
    pub min_level: u8,
    pub enabled: bool,
    pub destinations: Vec<String>,
    pub methods: Vec<String>,
    pub paths: Vec<String>,
    pub requests_per_day: u64,
    pub bytes_per_day: u64,
    pub rate_per_min: u64,
    pub response_max: u64,
    pub providers: BTreeMap<String, Provider>,
    pub models: BTreeMap<String, Model>,
}

impl Class {
    /// A class with external effects (§3.6): a non-GET/HEAD `http` class (git-push and email come later).
    pub fn effectful(&self) -> bool {
        !self.model && self.methods.iter().any(|m| m != "GET" && m != "HEAD")
    }
}

/// `127.0.0.1:<port>` or `localhost:<port>`: test-mode upstreams only (testing.md §3.4.1).
pub fn loopback(dest: &str) -> bool {
    dest.rsplit_once(':').is_some_and(|(h, _)| h == "127.0.0.1" || h == "localhost")
}

pub fn effective_actions(user: Option<&toml::Value>, test_mode: bool, e: &mut Vec<Finding>) -> toml::Table {
    let mut t: toml::Table = toml::from_str(BUILTIN).expect("built-in classes parse");
    match user {
        None => {}
        Some(toml::Value::Table(u)) => {
            for (k, v) in u {
                match (t.get_mut(k), v) {
                    (Some(toml::Value::Table(b)), toml::Value::Table(o)) => b.extend(o.clone()),
                    _ => drop(t.insert(k.clone(), v.clone())),
                }
            }
        }
        Some(_) => e.push(err("actions", "C11", "must be a table of classes")),
    }
    e.extend(validate_actions(&t, test_mode));
    t
}

/// Class ids `^[a-z0-9][a-z0-9-]{0,31}$`; provider ids `^[a-z][a-z0-9_-]{0,31}$` (kernel/proxy.md §3.5).
fn id_ok(s: &str, provider: bool) -> bool {
    let b = s.as_bytes();
    let first = b.first().is_some_and(|c| c.is_ascii_lowercase() || (!provider && c.is_ascii_digit()));
    first && b.len() <= 32 && b.iter().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-') || (provider && *c == b'_'))
}

fn dest_ok(d: &str, model: bool, test: bool) -> bool {
    let Some((h, p)) = d.rsplit_once(':') else { return false };
    if loopback(d) {
        return test && p.parse::<u16>().is_ok_and(|p| p > 0);
    }
    let ip = h.bytes().all(|c| c.is_ascii_digit() || c == b'.') || h.contains(':') || h.starts_with('[');
    !h.is_empty() && h.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-') && p.parse::<u16>().is_ok_and(|p| p > 0) && !(model && ip)
}

/// No `..`, `//`, `%2e`, `%2f` in any case (egress.md §3.5).
pub fn traversal(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.contains("..") || l.contains("//") || l.contains("%2e") || l.contains("%2f")
}

fn path_ok(p: &str) -> bool {
    !p.starts_with('/') && !traversal(p) && !p[..p.len().saturating_sub(1)].contains('*')
}

/// root/egress.md §4.3: unknown keys, unsupported kinds, id grammar, `host:port`, path grammar,
/// non-negative limits, model prices, an L0 model, providers, and `crates` only on its two hosts.
/// Loopback destinations are accepted only in test mode (testing.md §3.4.1).
pub fn validate_actions(t: &toml::Table, test_mode: bool) -> Vec<Finding> {
    let mut e = Vec::new();
    for (id, v) in t {
        let k = |f: &str| format!("actions.{id}.{f}");
        let Some(c) = v.as_table() else {
            e.push(err(&format!("actions.{id}"), "C11", "must be a table"));
            continue;
        };
        if !id_ok(id, false) {
            e.push(err(&format!("actions.{id}"), "C11", "class id must match ^[a-z0-9][a-z0-9-]{0,31}$"));
        }
        e.extend(c.keys().filter(|f| !CLASS_KEYS.contains(&f.as_str())).map(|f| err(&k(f), "C11", "unknown key")));
        let kind = c.get("kind").and_then(toml::Value::as_str).unwrap_or("");
        match kind {
            "model" | "http" => {}
            "git-push" | "email" => e.push(err(&k("kind"), "C11", "actions.unsupported_kind: not implemented by this Root release")),
            _ => e.push(err(&k("kind"), "C11", "must be \"model\" or \"http\"")),
        }
        let strs = |f: &str| -> Option<Vec<String>> {
            c.get(f)?.as_array()?.iter().map(|x| x.as_str().map(String::from)).collect()
        };
        let int_ok = |f: &str, lo: i64, hi: i64| c.get(f).is_none_or(|x| x.as_integer().is_some_and(|n| (lo..=hi).contains(&n)));
        if !c.contains_key("min_level") || !int_ok("min_level", 0, 4) {
            e.push(err(&k("min_level"), "C11", "required, 0–4"));
        }
        for f in ["requests_per_day", "bytes_per_day", "rate_per_min", "response_max_bytes"] {
            if !int_ok(f, 0, i64::MAX) {
                e.push(err(&k(f), "C11", "must be a non-negative integer"));
            }
        }
        if c.get("enabled").is_some_and(|x| !x.is_bool()) {
            e.push(err(&k("enabled"), "C11", "must be a boolean"));
        }
        let dests = strs("destinations").unwrap_or_default();
        let local_dest = |d: &str| kind == "model" && c.get("providers").and_then(toml::Value::as_table).is_some_and(|ps|
            ps.values().any(|p| p.get("host").and_then(toml::Value::as_str) == Some(d) && p.get("local").and_then(toml::Value::as_bool) == Some(true)));
        let dests_ok = !dests.is_empty() && dests.iter().all(|x| dest_ok(x, kind == "model", test_mode || local_dest(x)));
        if !dests_ok {
            e.push(err(&k("destinations"), "C11", "exact host:port strings (no wildcards; no IP literal for model; loopback requires test mode or an explicit local model provider)"));
        } else if id == "crates" && !dests.iter().all(|x| CRATES_HOSTS.contains(&x.as_str()) || (test_mode && loopback(x))) {
            e.push(err(&k("destinations"), "C11", "crates may list only index.crates.io:443 and static.crates.io:443"));
        }
        let methods = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
        if !strs("methods").is_some_and(|m| !m.is_empty() && m.iter().all(|x| methods.contains(&x.as_str()))) {
            e.push(err(&k("methods"), "C11", "exact uppercase HTTP methods"));
        }
        if !strs("paths").is_some_and(|p| !p.is_empty() && p.iter().all(|x| path_ok(x))) {
            e.push(err(&k("paths"), "C11", "no leading /, .., //, %2e, %2f; * only at the end"));
        }
        if kind != "model" {
            if c.contains_key("models") || c.contains_key("providers") {
                e.push(err(&k("models"), "C11", "models and providers are for kind = \"model\" only"));
            }
            continue;
        }
        match c.get("providers") {
            _ if !dests_ok => {}
            None => e.extend(dests.iter().filter(|d| !PRESETS.iter().any(|p| p.1 == d.as_str()))
                .map(|d| err(&k("providers"), "C11", format!("{d} has no provider preset; list it under providers")))),
            Some(toml::Value::Table(p)) => {
                for (pid, pv) in p {
                    let pk = k(&format!("providers.{pid}"));
                    let t = pv.as_table();
                    let s = |f: &str| t.and_then(|t| t.get(f)).map(|x| x.as_str());
                    let base = s("base_path").unwrap_or(Some("")).unwrap_or("x");
                    let ok = id_ok(pid, true) && t.is_some_and(|t| t.keys().all(|f| ["host", "base_path", "auth", "api", "local"].contains(&f.as_str())))
                        && s("host").flatten().is_some_and(|h| dests.iter().any(|d| d == h))
                        && s("auth").is_none_or(|a| matches!(a, Some("bearer" | "x-api-key" | "none")))
                        && s("api").is_none_or(|v| v.and_then(api).is_some())
                        && t.and_then(|t| t.get("local")).is_none_or(toml::Value::is_bool)
                        && (!t.and_then(|t| t.get("local")).and_then(toml::Value::as_bool).unwrap_or(false)
                            || (s("host").flatten().is_some_and(loopback) && s("api").flatten().and_then(api).is_some()))
                        && (s("auth").flatten() != Some("none") || (t.and_then(|t| t.get("local")).and_then(toml::Value::as_bool) == Some(true)
                            && s("host").flatten().is_some_and(loopback)))
                        && (base.is_empty() || (base.starts_with('/') && !base.ends_with('/') && path_ok(&base[1..])));
                    if !ok {
                        e.push(err(&pk, "C11", "needs id ^[a-z][a-z0-9_-]{0,31}$, host (one of destinations), optional base_path \"/x\", api chat|responses|messages; auth none requires local=true and loopback"));
                    }
                }
            }
            Some(_) => e.push(err(&k("providers"), "C11", "must be a table")),
        }
        // F2: a listed path with no metering rule for any of the class's providers is refused at run time
        // (not when the destinations or providers error already says why nothing routes)
        let pids: Vec<&str> = c.get("providers").and_then(toml::Value::as_table).map_or_else(
            || PRESETS.iter().filter(|p| dests.iter().any(|d| d == p.1)).map(|p| p.0).collect(), |p| p.keys().map(String::as_str).collect());
        for p in strs("paths").unwrap_or_default().iter().filter(|p| dests_ok && path_ok(p) && !pids.is_empty() && !pids.iter().any(|pid| {
            let dialect = c.get("providers").and_then(|ps| ps.get(*pid)).and_then(|pv| pv.get("api")).and_then(toml::Value::as_str).and_then(api);
            dialect.map_or_else(|| billed(pid, "", p), |f| api_route(f, "", p)).is_some()
        })) {
            e.push(Finding { warn: true, ..err(&k("paths"), "C11", format!("{p:?} has no Root metering rule for this class's providers; refused (class.unmetered)")) });
        }
        let models = c.get("models").and_then(toml::Value::as_table);
        let mut l0 = false;
        for (m, mv) in models.into_iter().flatten() {
            let mk = k(&format!("models.{m}"));
            let Some(mt) = mv.as_table() else {
                e.push(err(&mk, "C11", "must be a table"));
                continue;
            };
            e.extend(mt.keys().filter(|f| !MODEL_KEYS.contains(&f.as_str())).map(|f| err(&format!("{mk}.{f}"), "C11", "unknown key")));
            let n = |f: &str| mt.get(f).and_then(toml::Value::as_integer);
            l0 |= n("min_level") == Some(0);
            let prices = ["input_musd_per_mtok", "output_musd_per_mtok"].iter().all(|f| n(f).is_some_and(|x| x >= 0));
            let cache = ["cache_read_musd_per_mtok", "cache_write_musd_per_mtok", "cache_write_1h_musd_per_mtok"].iter()
                .all(|f| mt.get(*f).is_none_or(|v| v.as_integer().is_some_and(|x| x >= 0)));
            if !prices || !cache || !n("min_level").is_some_and(|x| (0..=4).contains(&x)) || n("reserve_output_tokens").is_some_and(|x| x <= 0) {
                e.push(err(&mk, "C11", "needs min_level 0–4 and non-negative integer prices (µUSD per 1M tokens)"));
            }
        }
        if !l0 {
            e.push(err(&k("models"), "C11", "at least one model must have min_level = 0"));
        }
    }
    e
}

/// The validated `[actions]` table as typed classes.
pub fn compile(t: &toml::Table) -> BTreeMap<String, Class> {
    let int = |c: &toml::Table, k: &str, d: i64| c.get(k).and_then(toml::Value::as_integer).unwrap_or(d).max(0) as u64;
    let strs = |c: &toml::Table, k: &str| -> Vec<String> {
        c.get(k).and_then(toml::Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
    };
    let mut out = BTreeMap::new();
    for (id, v) in t {
        let Some(c) = v.as_table() else { continue };
        let destinations = strs(c, "destinations");
        let mut providers = BTreeMap::new();
        match c.get("providers").and_then(toml::Value::as_table) {
            Some(p) => {
                for (pid, pv) in p {
                    let s = |f: &str| pv.get(f).and_then(toml::Value::as_str);
                    let preset = PRESETS.iter().find(|x| x.0 == pid.as_str()).map_or(Auth::Bearer, |x| x.2);
                    let auth = match s("auth") { Some("x-api-key") => Auth::XApiKey, Some(_) => Auth::Bearer, None => preset };
                    let host = s("host").unwrap_or("").to_string();
                    providers.insert(pid.clone(), Provider { host, base_path: s("base_path").unwrap_or("").into(), auth, api: s("api").and_then(api), local: pv.get("local").and_then(toml::Value::as_bool).unwrap_or(false), no_auth: s("auth") == Some("none") });
                }
            }
            None => {
                for p in PRESETS.iter().filter(|p| destinations.iter().any(|d| d == p.1)) {
                    providers.insert(p.0.to_string(), Provider { host: p.1.into(), base_path: String::new(), auth: p.2, api: None, local: false, no_auth: false });
                }
            }
        }
        let models = c.get("models").and_then(toml::Value::as_table).into_iter().flatten()
            .filter_map(|(m, mv)| {
                let mt = mv.as_table()?;
                let input = int(mt, "input_musd_per_mtok", 0);
                let fallback = input.saturating_mul(WRITE_FALLBACK).min(i64::MAX as u64) as i64;
                let cache_write = int(mt, "cache_write_musd_per_mtok", fallback);
                Some((m.clone(), Model { min_level: int(mt, "min_level", 0) as u8, input, cache_read: int(mt, "cache_read_musd_per_mtok", input as i64),
                    cache_write, cache_write_1h: int(mt, "cache_write_1h_musd_per_mtok", fallback.max(cache_write.min(i64::MAX as u64) as i64)),
                    output: int(mt, "output_musd_per_mtok", 0), reserve_out: int(mt, "reserve_output_tokens", 65_536) }))
            })
            .collect();
        out.insert(id.clone(), Class {
            id: id.clone(), model: c.get("kind").and_then(toml::Value::as_str) == Some("model"),
            min_level: int(c, "min_level", 0) as u8, enabled: c.get("enabled").and_then(toml::Value::as_bool).unwrap_or(true),
            destinations, methods: strs(c, "methods"), paths: strs(c, "paths"),
            requests_per_day: int(c, "requests_per_day", 0), bytes_per_day: int(c, "bytes_per_day", 0),
            rate_per_min: int(c, "rate_per_min", 0), response_max: int(c, "response_max_bytes", 64 << 20), providers, models,
        });
    }
    out
}

/// Where a request goes once it passed route, destination, method and path (§3.5 matching order).
pub struct Route<'a> {
    pub class: &'a Class,
    pub provider: Option<(&'a str, &'a Provider)>,
    pub dest: String,
    /// The upstream path with its leading `/`, base path and query.
    pub path: String,
    pub family: Family,
    /// A billed route of a model class (F2): reserved, gated and settled.
    pub metered: bool,
}

fn no(code: &'static str, class: Option<&Class>, msg: impl Into<String>) -> Refusal {
    Refusal { code, class: class.map(|c| c.id.clone()), msg: msg.into(), ..Refusal::default() }
}

/// §4.1: `/<provider_id>/<path>` (model classes) or `/x/<class_id>/<host>/<path>` (http classes).
pub fn route<'a>(classes: &'a BTreeMap<String, Class>, method: &str, target: &str) -> Result<Route<'a>, Refusal> {
    if !target.starts_with('/') {
        return Err(no("class.destination", None, "only origin-form requests (no CONNECT, no absolute URL)"));
    }
    let (path, query) = target.split_once('?').map_or((target, ""), |(p, q)| (p, q));
    if traversal(path) {
        return Err(no("class.path", None, "the path contains .., //, %2e or %2f"));
    }
    let (class, provider, dest, rest) = match path.strip_prefix("/x/") {
        Some(x) => {
            let mut it = x.splitn(3, '/');
            let (cid, host, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
            let c = classes.get(cid).filter(|c| !c.model).ok_or_else(|| no("class.unknown", None, format!("no http class {cid:?}")))?;
            (c, None, if host.contains(':') { host.to_string() } else { format!("{host}:443") }, rest)
        }
        None => {
            let (pid, rest) = path[1..].split_once('/').unwrap_or((&path[1..], ""));
            let found = classes.values().filter(|c| c.model).find_map(|c| c.providers.get_key_value(pid).map(|p| (c, p)));
            let (c, (pid, p)) = found.ok_or_else(|| no("class.unknown", None, format!("no model class serves provider {pid:?}")))?;
            (c, Some((pid.as_str(), p)), p.host.clone(), rest)
        }
    };
    if !class.destinations.contains(&dest) {
        return Err(no("class.destination", Some(class), format!("{dest} is not a destination of class {}", class.id)));
    }
    if !class.methods.iter().any(|m| m == method) {
        return Err(no("class.method", Some(class), format!("{method} is not allowed in class {}", class.id)));
    }
    let listed = class.paths.iter().any(|p| p.strip_suffix('*').map_or(p == rest, |pre| rest.starts_with(pre)));
    if !listed || (class.id == "crates" && dest == "static.crates.io:443" && !rest.starts_with("crates/")) {
        return Err(no("class.path", Some(class), format!("path {rest:?} is not allowed in class {}", class.id)));
    }
    let base = provider.map_or("", |p| p.1.base_path.as_str());
    let q = if query.is_empty() { String::new() } else { format!("?{query}") };
    let unmetered = |pid: &str| no("class.unmetered", Some(class), format!("{method} {rest:?} has no Root metering rule for provider {pid}; refused, never forwarded unmetered"));
    let metered = match provider { Some((pid, p)) => p.api.map_or_else(|| billed(pid, method, rest), |f| api_route(f, method, rest)).ok_or_else(|| unmetered(pid))?, None => false };
    let family = if metered { provider.and_then(|p| p.1.api).unwrap_or_else(|| Family::of(rest)) } else { Family::None };
    Ok(Route { class, provider, dest, path: format!("{base}/{rest}{q}"), family, metered })
}

#[derive(Deserialize)]
struct Probe {
    model: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "present_u64")]
    max_tokens: Option<u64>,
    #[serde(default, deserialize_with = "present_u64")]
    max_completion_tokens: Option<u64>,
    #[serde(default, deserialize_with = "present_u64")]
    max_output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "present_u64", alias = "candidateCount", alias = "candidate_count", alias = "num_return_sequences", alias = "num_generations")]
    n: Option<u64>,
    #[serde(default, deserialize_with = "present_u64")]
    best_of: Option<u64>,
    #[serde(alias = "generationConfig")]
    generation_config: Option<IgnoredAny>,
    extra_body: Option<IgnoredAny>,
    tools: Option<Vec<ToolProbe>>,
    mcp_servers: Option<IgnoredAny>,
    background: Option<serde_json::Value>,
}

// Missing is distinct from explicit null: forwarding a null cap can invoke an upstream default.
fn present_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct ToolProbe {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// What the Root reads from a metered model request body: `model` and the `max_*` output field.
pub struct Inspected {
    pub model: String,
    pub max_out: Option<u64>,
}

/// §3.5 request policy: the model is named, and every tool is client-executed (no provider tools,
/// no `mcp_servers`, no `background: true`). Every metered call must also carry exactly
/// one explicit, positive, family-correct output bound and request at most one answer.
/// No provider default or unsupported alias may substitute for an enforceable wire cap.
/// Generic compatible endpoints must honor the selected API's cap contract; custom
/// provider semantics still require qualification before real-money use.
pub fn inspect(family: Family, class: &Class, provider: Option<(&str, &Provider)>, body: &[u8]) -> Result<Inspected, Refusal> {
    let p: Probe = serde_json::from_slice(body).map_err(|e| no("request.invalid", Some(class), format!("the request body is not the expected JSON: {e}")))?;
    let model = p.model.as_ref().and_then(|m| m.as_str()).ok_or_else(|| no("class.model", Some(class), "the request names no model"))?;
    let client_tool = |t: &ToolProbe| match family {
        Family::Chat => t.kind.as_deref() == Some("function"),
        Family::Responses => matches!(t.kind.as_deref(), Some("function" | "custom")),
        _ => matches!(t.kind.as_deref(), None | Some("custom")),
    };
    if p.mcp_servers.is_some() || p.background.as_ref().is_some_and(|b| b == true) || !p.tools.iter().flatten().all(client_tool) {
        return Err(no("class.provider_tool", Some(class), "provider-executed tools, mcp_servers and background runs are refused"));
    }
    let limit = |message| no("request.output_limit", Some(class), message);
    if p.n.is_some_and(|n| n != 1) || p.best_of.is_some_and(|n| n != 1) {
        return Err(limit("only one completion/candidate is metered per request"));
    }
    if p.generation_config.is_some() || p.extra_body.is_some() {
        return Err(limit("nested generation options are unsupported in these metered API families"));
    }
    let fields = [p.max_tokens, p.max_completion_tokens, p.max_output_tokens];
    if fields.iter().flatten().count() != 1 {
        return Err(limit("exactly one explicit output-token cap is required; aliases cannot be combined"));
    }
    let deepseek = provider.is_some_and(|(id, p)| id == "deepseek" || p.host == "api.deepseek.com:443");
    let max_out = match family {
        Family::Chat if p.max_output_tokens.is_none() && !(deepseek && p.max_completion_tokens.is_some()) => p.max_tokens.or(p.max_completion_tokens),
        Family::Messages => p.max_tokens,
        Family::Responses => p.max_output_tokens,
        _ => None,
    }.ok_or_else(|| limit("output cap does not match the endpoint: Chat max_tokens/max_completion_tokens (DeepSeek max_tokens only), Messages max_tokens, Responses max_output_tokens"))?;
    if max_out == 0 || max_out > super::money::MAX_OUT_TOKENS {
        return Err(limit("output-token cap must be positive and no greater than Root MAX_OUT_TOKENS"));
    }
    Ok(Inspected { model: model.to_string(), max_out: Some(max_out) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errs_mode(extra: &str, test: bool) -> Vec<String> {
        let mut e = Vec::new();
        let user: toml::Value = toml::from_str::<toml::Table>(extra).unwrap().into();
        effective_actions(Some(&user), test, &mut e);
        e.into_iter().map(|f| f.key).collect()
    }
    fn errs(extra: &str) -> Vec<String> {
        errs_mode(extra, false)
    }
    fn table(extra: &str, test: bool) -> BTreeMap<String, Class> {
        let mut e = Vec::new();
        let user: toml::Value = toml::from_str::<toml::Table>(extra).unwrap().into();
        let t = effective_actions(Some(&user), test, &mut e);
        assert!(e.is_empty(), "{e:?}");
        compile(&t)
    }

    #[test]
    fn explicit_dialects_route_aliases_and_reject_other_endpoints() {
        for (api, endpoint, family, cap) in [("chat", "chat/completions", Family::Chat, "max_tokens"),
            ("responses", "responses", Family::Responses, "max_output_tokens"), ("messages", "messages", Family::Messages, "max_tokens")] {
            for endpoint in [endpoint.to_string(), format!("v1/{endpoint}")] {
            let src = format!("[providers]\ndestinations=[\"proxy.example:443\"]\npaths=[\"{endpoint}\"]\n[providers.providers.custom]\nhost=\"proxy.example:443\"\nbase_path=\"/team/v1\"\napi=\"{api}\"\n");
            let cs = table(&src, false);
            let route = route(&cs, "POST", &format!("/custom/{endpoint}")).unwrap_or_else(|e| panic!("{}", e.msg));
            assert_eq!((route.family, route.metered, route.path.as_str()), (family, true, format!("/team/v1/{endpoint}").as_str()));
            let body = format!("{{\"model\":\"deepseek-flash\",\"{cap}\":12}}");
            assert_eq!(inspect(route.family, route.class, route.provider, body.as_bytes()).unwrap_or_else(|e| panic!("{}", e.msg)).max_out, Some(12));
            assert!(inspect(route.family, route.class, route.provider, br#"{"model":"deepseek-flash"}"#).is_err());
            assert!(super::route(&cs, "GET", &format!("/custom/{endpoint}")).is_err());
            assert!(super::route(&cs, "POST", "/custom/unmetered").is_err());
            assert!(super::route(&cs, "POST", "/custom/v1/v1/messages").is_err());
            }
        }
    }

    #[test]
    fn local_no_auth_is_an_explicit_loopback_only_opt_in() {
        let src = "[providers]\ndestinations=[\"127.0.0.1:11434\"]\npaths=[\"chat/completions\"]\n[providers.providers.localmodel]\nhost=\"127.0.0.1:11434\"\nbase_path=\"/v1\"\napi=\"chat\"\nlocal=true\nauth=\"none\"\n";
        let cs = table(src, false);
        let r = route(&cs, "POST", "/localmodel/chat/completions").unwrap_or_else(|e| panic!("{}", e.msg));
        let p = r.provider.unwrap().1;
        assert!(p.local && p.no_auth && r.metered);
        assert!(endpoint_allowed(false, &r.dest, Some(p)));
        assert!(endpoint_allowed(true, &r.dest, Some(p)));
        assert!(!endpoint_allowed(false, &r.dest, None));
        assert!(!endpoint_allowed(true, "api.deepseek.com:443", Some(p)));
        for invalid in [src.replace("local=true", "local=false"), src.replace("api=\"chat\"", "api=\"gemini\""),
            src.replace("127.0.0.1:11434", "remote.example:443"), src.replace("local=true", "local=\"true\"")] {
            assert!(!errs(&invalid).is_empty(), "accepted {invalid}");
        }
    }

    #[test]
    fn builtins_are_valid_and_mergeable() {
        assert!(errs("").is_empty());
        assert!(errs("[crates]\nenabled = false\n").is_empty());
        let mut e = Vec::new();
        let t = effective_actions(None, false, &mut e);
        assert!(e.is_empty() && t.contains_key("crates") && t.contains_key("providers"));
        let c = compile(&t);
        assert_eq!(c["providers"].providers["deepseek"], Provider { host: "api.deepseek.com:443".into(), base_path: "".into(), auth: Auth::Bearer, api: None, local: false, no_auth: false });
        // the built-in DeepSeek row has no write price: writes would bill at 2× input (DeepSeek reports none)
        assert_eq!(c["providers"].models["deepseek-flash"], Model { min_level: 0, input: 300_000, cache_read: 6_000, cache_write: 600_000,
            cache_write_1h: 600_000, output: 1_200_000, reserve_out: 65_536 });
        // a charter model without a cache-read price bills cache hits at its input price (the safe default)
        assert_eq!(table("[providers.models.m]\nmin_level = 0\ninput_musd_per_mtok = 5\noutput_musd_per_mtok = 7\n", false)["providers"].models["m"].cache_read, 5);
        assert_eq!(errs("[providers.models.m]\nmin_level = 0\ninput_musd_per_mtok = 5\noutput_musd_per_mtok = 7\ncache_read_musd_per_mtok = -1\n"),
            vec!["actions.providers.models.m"]);
        // v0.5.0 L3 write prices: as given; a missing 1-hour price at least the 5-minute one and 2× input; none: 2× input
        let m = |extra: &str| table(&format!("[providers.models.m]\nmin_level = 0\ninput_musd_per_mtok = 2000000\noutput_musd_per_mtok = 7\n{extra}"),
            false)["providers"].models["m"].clone();
        let w = |x: Model| (x.cache_write, x.cache_write_1h);
        assert_eq!(w(m("cache_write_musd_per_mtok = 2500000\ncache_write_1h_musd_per_mtok = 4000000\n")), (2_500_000, 4_000_000));
        assert_eq!(w(m("cache_write_musd_per_mtok = 2500000\n")), (2_500_000, 4_000_000));
        assert_eq!(w(m("cache_write_musd_per_mtok = 9000000\n")), (9_000_000, 9_000_000));
        assert_eq!(w(m("")), (4_000_000, 4_000_000));
        for bad in ["cache_write_musd_per_mtok = -1\n", "cache_write_1h_musd_per_mtok = \"2.50\"\n"] {
            assert_eq!(errs(&format!("[providers.models.m]\nmin_level = 0\ninput_musd_per_mtok = 5\noutput_musd_per_mtok = 7\n{bad}")),
                vec!["actions.providers.models.m"], "{bad}");
        }
        assert_eq!(errs("[providers.models.m]\nmin_level = 0\ninput_musd_per_mtok = 5\noutput_musd_per_mtok = 7\ncache_write_2h_musd_per_mtok = 1\n"),
            vec!["actions.providers.models.m.cache_write_2h_musd_per_mtok"], "unknown keys stay refused");
        assert!(!c["crates"].model && !c["crates"].effectful() && c["crates"].enabled);
    }

    #[test]
    fn schema_violations() {
        assert_eq!(errs("[Bad]\nkind=\"http\"\nmin_level=0\ndestinations=[\"a.b:443\"]\nmethods=[\"GET\"]\npaths=[\"x\"]\n"), vec!["actions.Bad"]);
        assert_eq!(errs("[crates]\ndestinations = [\"evil.example:443\"]\n"), vec!["actions.crates.destinations"]);
        assert_eq!(errs("[providers]\ndestinations = [\"1.2.3.4:443\"]\n"), vec!["actions.providers.destinations"]);
        assert_eq!(errs("[providers]\npaths = [\"/v1/x\"]\n"), vec!["actions.providers.paths"]);
        assert_eq!(errs("[providers]\npaths = [\"a/%2E%2E/b\"]\n"), vec!["actions.providers.paths"]);
        assert_eq!(errs("[providers]\nrate_per_min = -1\n"), vec!["actions.providers.rate_per_min"]);
        assert_eq!(errs("[providers]\nmethods = [\"get\"]\n"), vec!["actions.providers.methods"]);
        assert_eq!(errs("[providers]\nwildcard = true\n"), vec!["actions.providers.wildcard"]);
        // a charter `models` table replaces the built-in one: no prices and no L0 model left
        assert_eq!(errs("[providers.models.\"deepseek-flash\"]\nmin_level = 1\n"),
            vec!["actions.providers.models.deepseek-flash", "actions.providers.models"]);
        assert_eq!(errs("[mail]\nkind=\"email\"\nmin_level=4\ndestinations=[\"smtp.x:587\"]\nmethods=[\"POST\"]\npaths=[\"*\"]\n"), vec!["actions.mail.kind"]);
        // R3-06: git-push is specified but unsupported in this release
        assert_eq!(errs("[push]\nkind=\"git-push\"\nmin_level=4\ndestinations=[\"github.com:22\"]\nmethods=[\"POST\"]\npaths=[\"*\"]\n"), vec!["actions.push.kind"]);
        assert_eq!(errs("[providers]\ndestinations = [\"api.example.com:443\"]\n"), vec!["actions.providers.providers"]);
        assert_eq!(errs("[providers.providers.deepseek]\nhost = \"api.other.com:443\"\n"), vec!["actions.providers.providers.deepseek"]);
        assert_eq!(errs("[providers.providers.deepseek]\nhost = \"api.deepseek.com:443\"\nauth = \"basic\"\n"), vec!["actions.providers.providers.deepseek"]);
    }

    #[test]
    fn loopback_only_in_test_mode() {
        let mock = "[providers]\ndestinations = [\"127.0.0.1:4000\"]\n[providers.providers.deepseek]\nhost = \"127.0.0.1:4000\"\n";
        assert_eq!(errs_mode(mock, false), vec!["actions.providers.destinations"]);
        assert!(errs_mode(mock, true).is_empty());
        assert_eq!(errs_mode("[providers]\ndestinations = [\"localhost:4000\"]\n", false), vec!["actions.providers.destinations"]);
        assert!(errs_mode("[crates]\ndestinations = [\"127.0.0.1:4001\"]\n", true).is_empty());
        assert_eq!(errs_mode("[crates]\ndestinations = [\"127.0.0.1:4001\"]\n", false), vec!["actions.crates.destinations"]);
    }

    #[test]
    fn routing_follows_the_matching_order() {
        let c = table("[providers]\ndestinations = [\"api.deepseek.com:443\", \"api.anthropic.com:443\"]\npaths = [\"chat/completions\", \"anthropic/v1/messages\", \"v1/messages\", \"models\"]\n", false);
        let r = route(&c, "POST", "/deepseek/chat/completions").ok().unwrap();
        assert_eq!((r.class.id.as_str(), r.provider.map(|p| p.0), r.dest.as_str(), r.path.as_str(), r.family),
            ("providers", Some("deepseek"), "api.deepseek.com:443", "/chat/completions", Family::Chat));
        let a = route(&c, "POST", "/anthropic/v1/messages").ok().unwrap();
        assert_eq!((a.provider.unwrap().1.auth, a.family), (Auth::XApiKey, Family::Messages));
        assert_eq!(route(&c, "GET", "/deepseek/models?x=1").ok().unwrap().path, "/models?x=1");
        let code = |m: &str, t: &str| route(&c, m, t).err().map(|r| r.code);
        assert_eq!(code("POST", "/deepseek/fine_tuning/jobs"), Some("class.path"));
        assert_eq!(code("POST", "/deepseek/a/../chat/completions"), Some("class.path"));
        assert_eq!(code("POST", "/deepseek/chat%2Fcompletions"), Some("class.path"));
        assert_eq!(code("DELETE", "/deepseek/models"), Some("class.method"));
        assert_eq!(code("POST", "/nope/chat/completions"), Some("class.unknown"));
        assert_eq!(code("CONNECT", "example.com:443"), Some("class.destination"));
        assert_eq!(code("GET", "http://example.com/x"), Some("class.destination"));
        // R3-04: crates
        let ok = route(&c, "GET", "/x/crates/index.crates.io/se/rd/serde").ok().unwrap();
        assert_eq!((ok.dest.as_str(), ok.path.as_str(), ok.family), ("index.crates.io:443", "/se/rd/serde", Family::None));
        assert!(route(&c, "GET", "/x/crates/static.crates.io/crates/serde/serde-1.0.0.crate").is_ok());
        assert_eq!(code("GET", "/x/crates/static.crates.io/other/x"), Some("class.path"));
        assert_eq!(code("POST", "/x/crates/index.crates.io/se/rd/serde"), Some("class.method"));
        assert_eq!(code("GET", "/x/crates/example.com/x"), Some("class.destination"));
        assert_eq!(code("GET", "/x/providers/api.deepseek.com/models"), Some("class.unknown"));
    }

    /// F2: billable endpoints the class lists but the Root cannot meter are refused, never forwarded unmetered.
    #[test]
    fn unmeterable_routes_are_refused() {
        let user: toml::Value = toml::from_str::<toml::Table>("[providers]\nmethods = [\"GET\", \"HEAD\", \"POST\"]\n\
            paths = [\"chat/completions\", \"completions\", \"embeddings\", \"models\", \"user/balance\", \"v1/*\"]\n").unwrap().into();
        let mut e = Vec::new();
        let c = compile(&effective_actions(Some(&user), false, &mut e));
        assert_eq!(e.iter().map(|f| (f.key.as_str(), f.warn)).collect::<Vec<_>>(), vec![("actions.providers.paths", true); 3], "C11 warns, once per path");
        let r = |m: &str, t: &str| route(&c, m, t).map(|r| (r.metered, r.family)).map_err(|r| r.code);
        assert_eq!(r("POST", "/deepseek/chat/completions"), Ok((true, Family::Chat)));
        assert_eq!((r("GET", "/deepseek/user/balance"), r("GET", "/deepseek/models?x=1")), (Ok((false, Family::None)), Ok((false, Family::None))));
        for (m, p) in [("POST", "/deepseek/embeddings"), ("POST", "/deepseek/completions"), ("POST", "/deepseek/v1/images/generations"),
            ("GET", "/deepseek/chat/completions"), ("POST", "/deepseek/user/balance"), ("HEAD", "/deepseek/models")] {
            assert_eq!(r(m, p), Err("class.unmetered"), "{m} {p}");
        }
        assert_eq!(r("POST", "/deepseek/fine_tuning/jobs"), Err("class.path"), "an unlisted path stays a class.path refusal");
        assert!(ROUTES.iter().filter(|r| r.3).all(|r| Family::of(r.2) != Family::None), "every billed route has a wire family");
        assert_eq!((billed("dead", "POST", "chat/completions"), billed("dead", "GET", "user/balance")), (Some(true), None), "no preset: the generic rows");
        assert_eq!(billed("anthropic", "POST", "v1/messages/count_tokens"), Some(false));
    }

    #[test]
    fn request_policy_refuses_provider_tools() {
        let c = table("", false);
        let p = &c["providers"];
        let ok = inspect(Family::Chat, p, None, br#"{"model":"deepseek-flash","stream":true,"max_tokens":100,"tools":[{"type":"function","function":{}}]}"#).ok().unwrap();
        assert_eq!((ok.model.as_str(), ok.max_out), ("deepseek-flash", Some(100)));
        let code = |f, b: &str| inspect(f, p, None, b.as_bytes()).err().map(|r| r.code);
        assert_eq!(code(Family::Chat, r#"{"model":"m","tools":[{"type":"web_search"}]}"#), Some("class.provider_tool"));
        assert_eq!(code(Family::Messages, r#"{"model":"m","tools":[{"type":"web_search_20250305","name":"w"}]}"#), Some("class.provider_tool"));
        assert_eq!(code(Family::Messages, r#"{"model":"m","max_tokens":100,"tools":[{"name":"f"},{"type":"custom","name":"g"}]}"#), None);
        assert_eq!(code(Family::Messages, r#"{"model":"m","mcp_servers":[{"url":"x"}]}"#), Some("class.provider_tool"));
        assert_eq!(code(Family::Responses, r#"{"model":"m","background":true}"#), Some("class.provider_tool"));
        assert_eq!(code(Family::Responses, r#"{"model":"m","max_output_tokens":100,"tools":[{"type":"custom"},{"type":"function"}]}"#), None);
        assert_eq!(code(Family::Chat, r#"{"messages":[]}"#), Some("class.model"));
        assert_eq!(code(Family::Chat, "not json"), Some("request.invalid"));
    }

    #[test]
    fn output_cap_is_explicit_family_correct_and_single_completion() {
        let classes = table("", false);
        let class = &classes["providers"];
        for (family, field) in [(Family::Chat, "max_tokens"), (Family::Chat, "max_completion_tokens"), (Family::Messages, "max_tokens"), (Family::Responses, "max_output_tokens")] {
            let body = serde_json::json!({"model":"m",field:super::super::money::MAX_OUT_TOKENS,"n":1,"best_of":1});
            assert_eq!(inspect(family, class, None, &serde_json::to_vec(&body).unwrap()).ok().unwrap().max_out, Some(super::super::money::MAX_OUT_TOKENS));
        }
        let deepseek = Some(("deepseek", &class.providers["deepseek"]));
        assert!(inspect(Family::Chat, class, deepseek, br#"{"model":"deepseek-flash","max_tokens":100}"#).is_ok());
        assert!(inspect(Family::Chat, class, deepseek, br#"{"model":"deepseek-flash","max_completion_tokens":100}"#).is_err());
        let renamed = Some(("renamed", &class.providers["deepseek"]));
        assert!(inspect(Family::Chat, class, renamed, br#"{"model":"deepseek-flash","max_completion_tokens":100}"#).is_err());
        for (family, body) in [
            (Family::Chat, r#"{"model":"m"}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":0}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":1048577}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"max_completion_tokens":10}"#),
            (Family::Chat, r#"{"model":"m","max_output_tokens":10}"#),
            (Family::Messages, r#"{"model":"m","max_completion_tokens":10}"#),
            (Family::Responses, r#"{"model":"m","max_tokens":10}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"n":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"best_of":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"candidateCount":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"candidate_count":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"num_return_sequences":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"num_generations":2}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"generationConfig":{"candidateCount":2}}"#),
            (Family::Chat, r#"{"model":"m","max_tokens":10,"extra_body":{"n":2}}"#),
        ] {
            assert_eq!(inspect(family, class, None, body.as_bytes()).err().unwrap().code, "request.output_limit", "{body}");
        }
    }

    #[test]
    fn malformed_null_and_duplicate_bound_fields_are_not_forwarded() {
        let classes = table("", false);
        let class = &classes["providers"];
        for body in [
            r#"{"model":"m","max_tokens":null}"#,
            r#"{"model":"m","max_tokens":"10"}"#,
            r#"{"model":"m","max_tokens":1.5}"#,
            r#"{"model":"m","max_tokens":-1}"#,
            r#"{"model":"m","max_tokens":true}"#,
            r#"{"model":"m","max_tokens":10,"max_tokens":1000}"#,
            r#"{"model":"m","max_tokens":10,"max_output_tokens":null}"#,
            r#"{"model":"m","max_tokens":10,"n":null}"#,
            r#"{"model":"m","max_tokens":10,"n":1,"candidateCount":2}"#,
        ] {
            assert_eq!(inspect(Family::Chat, class, None, body.as_bytes()).err().unwrap().code, "request.invalid", "{body}");
        }
    }

}
