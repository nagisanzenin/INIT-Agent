//! UX-KEYMODEL (v0.2.0-preview): providers, keys and the model selection from inside the app, so the owner never
//! hand-edits two TOML files with different price units (docs/provider-configuration.md).
//!
//! - A built-in catalog of five presets. Only DeepSeek is live-tested; the others are marked experimental.
//! - `providers add` writes the charter's `[actions.providers]` class (destinations, paths, the provider entry, the
//!   model tariff in µUSD/MTok) as one explicit block, checks it with the charter rules and applies it through the
//!   existing `charter.apply` path. It adds the matching `INIT_HOME/kernel/config.toml` provider table (decimal
//!   USD/MTok) without selecting it.
//! - `model use` sets `[model]` (and `[model.descriptor]` for a custom model) in the kernel config; the Root then
//!   restarts the kernel through its supervisor (`Ev::Restart`).
//! - `keys.set` over owner.sock: the secret goes from the TUI to the Root only. It is never logged, never echoed,
//!   never in an argv; the request bytes are zeroed after use.
//!
//! Text edits keep the owner's other sections and comments: whole sections are replaced, and the result is checked
//! by parsing it again and comparing it with the intended table; any mismatch refuses the edit.

use crate::charter;
use crate::install::Paths;
use crate::link::{Fail, Root, lock};
use crate::sup::{Ev, Plan, Slot};
use crate::util::{self, write_atomic};
use crate::sys;
use serde_json::{Value, json};
use toml::Table;

/// One catalog model. Prices are µUSD per 1M tokens (the Root's unit); `offpeak` only for DeepSeek's schedule.
#[derive(Clone, Copy, Debug)]
pub struct CatModel {
    pub id: &'static str,
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub offpeak: Option<(u64, u64, u64)>,
    /// Anthropic cache writes (5 min, 1 h). The Root bills them at the input price (provider-configuration.md).
    pub cache_write: Option<(u64, u64)>,
    /// Input window, used only when a custom alias (a gateway) serves this model and the kernel needs a descriptor.
    pub context: u64,
    pub max_output: u64,
    pub reserve: u64,
}

/// One provider preset.
#[derive(Debug)]
pub struct Preset {
    pub id: &'static str,
    pub display: &'static str,
    /// The Root dialect: `chat`, `messages` or `responses`.
    pub api: &'static str,
    /// The official host; None when the owner gives a URL.
    pub host: Option<&'static str>,
    pub paths: &'static [&'static str],
    pub kernel_preset: &'static str,
    pub local: bool,
    pub default_url: Option<&'static str>,
    /// `live` or `experimental` (not live-tested).
    pub tested: &'static str,
    pub models: &'static [CatModel],
    pub env: Option<&'static str>,
}

const fn m(id: &'static str, p: (u64, u64, u64), (context, max_output): (u64, u64), reserve: u64) -> CatModel {
    CatModel { id, input: p.0, cache_read: p.1, output: p.2, offpeak: None, cache_write: None, context, max_output, reserve }
}

/// DeepSeek prices as `scripts/install.sh` writes them; Anthropic and OpenAI from
/// research_notes/INIT v1 providers UX vision/llm_api_providers.md (standard tier, verified 2026-10-20).
pub const CATALOG: [Preset; 5] = [
    Preset { id: "deepseek", display: "DeepSeek", api: "chat", host: Some("api.deepseek.com:443"),
        paths: &["chat/completions", "beta/chat/completions", "models", "user/balance"], kernel_preset: "deepseek", local: false,
        default_url: None, tested: "live", env: Some("DEEPSEEK_API_KEY"), models: &[
            CatModel { offpeak: Some((150_000, 3_000, 600_000)), ..m("deepseek-flash", (300_000, 6_000, 1_200_000), (1_000_000, 393_216), 65_536) },
            CatModel { offpeak: Some((660_000, 22_000, 1_980_000)), ..m("deepseek-v4-pro", (1_320_000, 44_000, 3_960_000), (1_000_000, 393_216), 65_536) },
        ] },
    Preset { id: "gateway", display: "OpenAI-compatible gateway (Chat Completions)", api: "chat", host: None, paths: &["chat/completions"],
        kernel_preset: "deepseek", local: false, default_url: None, tested: "experimental", env: None, models: &[] },
    Preset { id: "anthropic", display: "Anthropic (Messages API)", api: "messages", host: Some("api.anthropic.com:443"), paths: &["v1/messages"],
        kernel_preset: "anthropic", local: false, default_url: None, tested: "experimental", env: Some("ANTHROPIC_API_KEY"), models: &[
            CatModel { cache_write: Some((5_000_000, 8_000_000)), ..m("claude-opus-5-5", (4_000_000, 200_000, 20_000_000), (1_000_000, 131_072), 32_768) },
            CatModel { cache_write: Some((2_500_000, 4_000_000)), ..m("claude-sonnet-5-5", (2_000_000, 100_000, 10_000_000), (1_000_000, 131_072), 32_768) },
        ] },
    Preset { id: "openai", display: "OpenAI (Responses API)", api: "responses", host: Some("api.openai.com:443"), paths: &["v1/responses"],
        kernel_preset: "openai", local: false, default_url: None, tested: "experimental", env: Some("OPENAI_API_KEY"), models: &[
            m("gpt-6.1-sol", (2_000_000, 100_000, 10_000_000), (922_000, 128_000), 32_768),
            m("gpt-6-astra", (10_000_000, 1_000_000, 50_000_000), (922_000, 128_000), 32_768),
        ] },
    Preset { id: "local", display: "Local server, no key (Ollama, LM Studio, llama.cpp; Chat Completions)", api: "chat", host: None,
        paths: &["chat/completions"], kernel_preset: "deepseek", local: true, default_url: Some("http://127.0.0.1:11434/v1"),
        tested: "experimental", env: None, models: &[] },
];

/// The ids whose official host the Root knows (classes.rs PRESETS); a custom URL never takes one of them.
const OFFICIAL: [&str; 3] = ["deepseek", "anthropic", "openai"];
/// Kernel defaults for a custom model (provider-configuration.md's example values).
const CUSTOM_CONTEXT: u64 = 131_072;
const CUSTOM_MAX_OUT: u64 = 32_768;
const CUSTOM_REQUEST: u64 = 4_096;

pub fn preset(id: &str) -> Option<&'static Preset> {
    CATALOG.iter().find(|p| p.id == id)
}

fn cat_model(provider: &str, model: &str) -> Option<&'static CatModel> {
    preset(provider).filter(|p| OFFICIAL.contains(&p.id))?.models.iter().find(|x| x.id == model)
}

/// Provider ids `^[a-z][a-z0-9_-]{0,31}$` (kernel/proxy.md §3.5).
pub fn provider_id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.first().is_some_and(u8::is_ascii_lowercase) && b.len() <= 32 && b.iter().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

fn model_id_ok(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-:/@+".contains(&c))
}

/// µUSD per MTok as decimal USD for the kernel config: at least two decimals (`300000` → `"0.30"`).
pub fn usd(musd: u64) -> String {
    let f = format!("{:06}", musd % 1_000_000);
    let f = f.trim_end_matches('0');
    format!("{}.{}", musd / 1_000_000, if f.len() < 2 { format!("{f:0<2}") } else { f.to_string() })
}

/// `IN,OUT` or `IN,CACHED,OUT` in USD per 1M tokens → µUSD (in, cache_read, out).
pub fn parse_price(s: &str) -> Result<(u64, u64, u64), String> {
    let v: Vec<Option<u64>> = s.split(',').map(|x| charter::musd(x.trim())).collect();
    match v.as_slice() {
        [Some(i), Some(o)] => Ok((*i, *i, *o)),
        [Some(i), Some(c), Some(o)] => Ok((*i, *c, *o)),
        _ => Err(format!("--price {s:?}: give USD per 1M tokens as IN,OUT or IN,CACHED,OUT (for example 1.00,0.10,2.00)")),
    }
}

/// `https://host[:port][/path]` (remote) or `http://127.0.0.1:PORT[/path]` / `http://localhost:PORT[/path]` (local)
/// → (`host:port`, `/path` without a trailing slash, or "").
pub fn parse_url(url: &str, local: bool) -> Result<(String, String), String> {
    let bad = |why: &str| Err(format!("{url:?}: {why}"));
    let rest = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(r), _) if !local => r,
        (_, Some(r)) if local => r,
        _ => return bad(if local { "a local server URL starts with http://127.0.0.1:PORT" } else { "the URL must start with https://" }),
    };
    if rest.contains(['?', '#', '@', ' ', '\\']) {
        return bad("no query, fragment, user name or spaces");
    }
    let (hp, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let (host, port) = match hp.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("{url:?}: bad port"))?),
        None if local => return bad("a local server URL needs a port (http://127.0.0.1:11434/v1)"),
        None => (hp, 443),
    };
    let host = host.to_ascii_lowercase();
    if local && host != "127.0.0.1" && host != "localhost" {
        return bad("a local server must be 127.0.0.1 or localhost (no LAN addresses)");
    }
    let ip = host.bytes().all(|c| c.is_ascii_digit() || c == b'.');
    if !local && (host.is_empty() || ip || host == "localhost" || !host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')) {
        return bad("the host must be a DNS name (no IP literal, no localhost; use the local preset for a server on this computer)");
    }
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { String::new() } else { format!("/{path}") };
    if path.split('/').skip(1).any(|s| s.is_empty() || s == "." || s == "..") || path.to_ascii_lowercase().contains("%2") {
        return bad("the path may not contain //, . or .. segments or %2e/%2f");
    }
    Ok((format!("{host}:{port}"), path))
}

/// What `providers add` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddReq {
    pub preset: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
    pub price: Option<String>,
    pub context: Option<u64>,
    pub max_output: Option<u64>,
}

impl AddReq {
    pub fn from_json(a: &Value) -> AddReq {
        let s = |k: &str| a[k].as_str().filter(|x| !x.is_empty()).map(String::from);
        AddReq { preset: s("preset").unwrap_or_default(), base_url: s("base_url"), model: s("model"), name: s("name"), price: s("price"),
            context: a["context"].as_u64(), max_output: a["max_output"].as_u64() }
    }
}

/// A resolved `providers add`: everything both config files need.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub preset: &'static str,
    pub alias: String,
    pub host: String,
    pub base_path: String,
    /// Root dialect.
    pub api: &'static str,
    pub local: bool,
    /// A provider alias or model the body does not know: the kernel config needs a descriptor.
    pub custom: bool,
    pub kernel_preset: &'static str,
    pub paths: Vec<String>,
    pub model: String,
    pub price: (u64, u64, u64),
    pub offpeak: Option<(u64, u64, u64)>,
    pub cache_write: Option<(u64, u64)>,
    pub priced: bool,
    pub context: u64,
    pub max_output: u64,
    pub reserve: u64,
    pub tested: &'static str,
}

pub fn spec(r: &AddReq) -> Result<Spec, String> {
    let ids = CATALOG.iter().map(|p| p.id).collect::<Vec<_>>().join(", ");
    let pr = preset(&r.preset).ok_or_else(|| format!("unknown preset {:?}; one of: {ids}", r.preset))?;
    let url = r.base_url.clone().or(pr.default_url.map(String::from));
    let official = pr.host.is_some() && url.is_none();
    if pr.host.is_none() && url.is_none() {
        return Err(format!("{}: needs --base-url (for example https://openrouter.ai/api/v1)", pr.id));
    }
    if pr.id == "deepseek" && !official {
        return Err("deepseek: the official host only; use the gateway preset for a DeepSeek-compatible gateway".into());
    }
    let alias = match &r.name {
        Some(n) => n.clone(),
        None if official || pr.host.is_none() => pr.id.to_string(),
        None => format!("{}-gw", pr.id),
    };
    if !provider_id_ok(&alias) {
        return Err(format!("--name {alias:?}: a provider name matches ^[a-z][a-z0-9_-]{{0,31}}$"));
    }
    if OFFICIAL.contains(&alias.as_str()) && (!official || alias != pr.id) {
        return Err(format!("{alias:?} is the name of the official {alias} provider; choose another --name"));
    }
    let (host, mut base_path) = match &url {
        None => (pr.host.unwrap_or_default().to_string(), String::new()),
        Some(u) => parse_url(u, pr.local)?,
    };
    // the native adapters emit `v1/…`: an upstream `/team/v1/messages` is base_path "/team" (provider-configuration.md)
    if pr.api != "chat" && url.is_some() && let Some(b) = base_path.strip_suffix("/v1") {
        base_path = b.to_string();
    }
    let cat = r.model.as_deref().map_or(pr.models.first(), |want| pr.models.iter().find(|x| x.id == want));
    let model = match (&r.model, cat) {
        (_, Some(c)) => c.id.to_string(),
        (Some(m), None) if pr.models.is_empty() || !official => m.clone(),
        (Some(m), None) => {
            let have: Vec<&str> = pr.models.iter().map(|x| x.id).collect();
            return Err(format!("{m:?} is not in the {} catalog ({}); for another model use the gateway preset", pr.id, have.join(", ")));
        }
        (None, None) => return Err(format!("{}: needs --model (the id exactly as the server names it)", pr.id)),
    };
    if !model_id_ok(&model) {
        return Err(format!("--model {model:?}: 1–128 characters of A-Z a-z 0-9 . _ - : / @ +"));
    }
    let custom = !official || cat.is_none();
    let price = match (&r.price, cat) {
        (Some(p), _) => Some(parse_price(p)?),
        (None, Some(c)) => Some((c.input, c.cache_read, c.output)),
        (None, None) => None,
    };
    let max_output = r.max_output.or(cat.map(|c| c.max_output)).unwrap_or(CUSTOM_MAX_OUT);
    let context = r.context.or(cat.map(|c| c.context)).unwrap_or(CUSTOM_CONTEXT);
    if custom && !(max_output > 0 && max_output < context && (1024..=16_777_216).contains(&context)) {
        return Err(format!("--context {context} / --max-output {max_output}: need 0 < max_output < context, context 1024–16777216"));
    }
    Ok(Spec {
        preset: pr.id, host, base_path, api: pr.api, local: pr.local, custom, kernel_preset: pr.kernel_preset,
        paths: pr.paths.iter().map(|p| p.to_string()).collect(), price: price.unwrap_or((0, 0, 0)), priced: price.is_some() || pr.local,
        offpeak: cat.and_then(|c| c.offpeak).filter(|_| official), cache_write: cat.and_then(|c| c.cache_write).filter(|_| official),
        context, max_output, reserve: cat.map_or(CUSTOM_REQUEST.min(max_output), |c| c.reserve), tested: pr.tested, model, alias,
    })
}

// ------------------------------------------------------------------------------------------------
// The charter fragment (µUSD per MTok) and the kernel fragment (decimal USD per MTok)
// ------------------------------------------------------------------------------------------------

fn tv(v: impl Into<toml::Value>) -> toml::Value {
    v.into()
}

/// `[actions.providers.providers.<alias>]` and `[actions.providers.models."<model>"]`.
pub fn charter_fragment(s: &Spec) -> (Table, Table) {
    let mut p = Table::new();
    p.insert("host".into(), tv(s.host.clone()));
    if s.custom || !OFFICIAL.contains(&s.alias.as_str()) {
        p.insert("api".into(), tv(s.api));
        if !s.base_path.is_empty() || s.local {
            p.insert("base_path".into(), tv(s.base_path.clone()));
        }
        if s.local {
            p.insert("local".into(), tv(true));
            p.insert("auth".into(), tv("none"));
        } else {
            p.insert("auth".into(), tv(if s.api == "messages" { "x-api-key" } else { "bearer" }));
        }
    }
    let mut md = Table::new();
    md.insert("min_level".into(), tv(0));
    md.insert("input_musd_per_mtok".into(), tv(s.price.0 as i64));
    md.insert("cache_read_musd_per_mtok".into(), tv(s.price.1 as i64));
    md.insert("output_musd_per_mtok".into(), tv(s.price.2 as i64));
    md.insert("reserve_output_tokens".into(), tv(s.reserve as i64));
    (p, md)
}

fn price_inline(p: (u64, u64, u64), cw: Option<(u64, u64)>) -> String {
    let w = cw.map_or(String::new(), |(a, b)| format!(", cache_write = \"{}\", cache_write_1h = \"{}\"", usd(a), usd(b)));
    format!("{{ input = \"{}\", cache_read = \"{}\"{w}, output = \"{}\" }}", usd(p.0), usd(p.1), usd(p.2))
}

/// TOML basic string or bare key.
fn key(k: &str) -> String {
    if !k.is_empty() && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') { k.into() } else { q(k) }
}
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The kernel's `[providers.<alias>.models."<model>"]` body lines (provider-configuration.md's layout).
pub fn kernel_model_lines(s: &Spec, scheduled: bool) -> String {
    let mut o = String::new();
    if s.custom {
        o += &format!("context_window = {}\n", s.context);
    }
    o += &format!("max_output = {}\nreserve_output_tokens = {}\n", s.max_output, s.reserve);
    match (scheduled, s.offpeak) {
        (true, Some(off)) => {
            o += "reference_window = \"peak\"\n";
            o += &format!("prices.peak = {}\nprices.offpeak = {}\n", price_inline(s.price, s.cache_write), price_inline(off, None));
        }
        _ => o += &format!("prices.standard = {}\n", price_inline(s.price, s.cache_write)),
    }
    o
}

/// The kernel's `[model.descriptor]` for a custom model.
pub fn descriptor_lines(api: &str, context: u64, max_output: u64, request: u64) -> String {
    let api = match api { "chat" => "chat_completions", a => a };
    let mut o = format!("api = {}\ncontext_window = {context}\nmax_output = {max_output}\ndefault_max_tokens = {request}\nrequest_max_tokens = {request}\n", q(api));
    if api == "chat_completions" {
        o += "chat_stream_usage = true\n";
    }
    o += "native_extensions = false\n";
    o
}

// ------------------------------------------------------------------------------------------------
// Section-level TOML text edits
// ------------------------------------------------------------------------------------------------

/// The dotted key path of a table header line, or None.
fn header(line: &str) -> Option<Vec<String>> {
    let t = line.trim_start();
    let (inner, after) = if let Some(x) = t.strip_prefix("[[") { x.split_once("]]")? } else { t.strip_prefix('[')?.split_once(']')? };
    if !after.trim().is_empty() && !after.trim_start().starts_with('#') {
        return None;
    }
    let (mut out, mut cur, mut chars) = (Vec::new(), String::new(), inner.chars().peekable());
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                quoted = true;
                loop {
                    match chars.next()? {
                        '\\' if c == '"' => cur.push(chars.next()?),
                        x if x == c => break,
                        x => cur.push(x),
                    }
                }
            }
            '.' => out.push(std::mem::take(&mut cur)),
            ' ' | '\t' => {}
            x if x.is_ascii_alphanumeric() || x == '_' || x == '-' => cur.push(x),
            _ => return None,
        }
    }
    if cur.is_empty() && !quoted {
        return None;
    }
    out.push(cur);
    Some(out)
}

/// Lines grouped into sections: `(header path or None for the top, text)`, multi-line strings respected.
fn sections(src: &str) -> Vec<(Option<Vec<String>>, String)> {
    let mut out: Vec<(Option<Vec<String>>, String)> = vec![(None, String::new())];
    let mut ml: Option<&str> = None;
    for line in src.split_inclusive('\n') {
        if ml.is_none() && let Some(h) = header(line) {
            out.push((Some(h), String::new()));
        }
        out.last_mut().expect("non-empty").1.push_str(line);
        let mut rest = line;
        loop {
            let next = match ml {
                Some(d) => rest.find(d).map(|i| (i, d)),
                None => [rest.find("'''").map(|i| (i, "'''")), rest.find("\"\"\"").map(|i| (i, "\"\"\""))].into_iter().flatten().min(),
            };
            let Some((i, d)) = next else { break };
            ml = if ml.is_some() { None } else { Some(d) };
            rest = &rest[i + 3..];
        }
    }
    out
}

/// Removes every section whose path `drop` selects and puts `block` where the first one was (else at the end).
fn replace_sections(src: &str, drop: impl Fn(&[String]) -> bool, block: &str) -> String {
    let mut out = String::new();
    let mut placed = false;
    for (h, text) in sections(src) {
        if h.as_deref().is_some_and(&drop) {
            if !placed {
                out.push_str(block);
                out.push('\n');
                placed = true;
            }
            continue;
        }
        out.push_str(&text);
    }
    if !placed {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(block);
    }
    out
}

fn starts(path: &[String], pre: &[&str]) -> bool {
    path.len() >= pre.len() && path.iter().zip(pre).all(|(a, b)| a == b)
}

/// A table of scalars, arrays and sub-tables as `[prefix]` sections (no header for a table that holds only tables).
fn render(prefix: &str, t: &Table, out: &mut String) {
    let headed = |t: &Table| t.values().any(|v| !v.is_table()) || t.is_empty();
    if headed(t) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[{prefix}]\n"));
    }
    for (k, v) in t.iter().filter(|(_, v)| !v.is_table()) {
        out.push_str(&format!("{} = {v}\n", key(k)));
    }
    for (k, v) in t.iter().filter_map(|(k, v)| v.as_table().map(|t| (k, t))) {
        render(&format!("{prefix}.{}", key(k)), v, out);
    }
}

fn get<'a>(t: &'a Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (first, rest) = path.split_first()?;
    let v = t.get(*first)?;
    if rest.is_empty() { Some(v) } else { get(v.as_table()?, rest) }
}

fn tbl<'a>(t: &'a mut Table, k: &str) -> &'a mut Table {
    let e = t.entry(k.to_string()).or_insert_with(|| toml::Value::Table(Table::new()));
    if !e.is_table() {
        *e = toml::Value::Table(Table::new());
    }
    e.as_table_mut().expect("a table")
}

fn strs(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// The effective `providers` class with its provider table made explicit (the preset fallback of classes.rs).
fn explicit_class(effective: &Table) -> Table {
    let mut c = effective.clone();
    if c.get("providers").and_then(toml::Value::as_table).is_none() {
        let dests = strs(c.get("destinations"));
        let ps = tbl(&mut c, "providers");
        for pr in CATALOG.iter().filter(|p| OFFICIAL.contains(&p.id) && p.host.is_some_and(|h| dests.iter().any(|d| d == h))) {
            let mut e = Table::new();
            e.insert("host".into(), tv(pr.host.unwrap_or_default()));
            ps.insert(pr.id.into(), toml::Value::Table(e));
        }
    }
    c
}

/// `providers add`, charter side: the new charter text (unchanged when nothing changes).
pub fn charter_add(src: &str, effective: &Table, s: &Spec) -> Result<String, String> {
    let doc: Table = toml::from_str(src).map_err(|e| format!("charter.toml does not parse: {e}"))?;
    let mut c = explicit_class(effective);
    let (pv, md) = charter_fragment(s);
    let old_host = get(&c, &["providers", &s.alias, "host"]).and_then(|v| v.as_str().map(String::from));
    // an official provider that is already there keeps its entry (its host may be a test mock): only a model is added
    let keep = !s.custom && OFFICIAL.contains(&s.alias.as_str()) && old_host.is_some();
    let host = if keep { old_host.clone().unwrap_or_default() } else { s.host.clone() };
    if !keep {
        tbl(&mut c, "providers").insert(s.alias.clone(), toml::Value::Table(pv));
    }
    tbl(&mut c, "models").insert(s.model.clone(), toml::Value::Table(md));
    let hosts: Vec<String> = c["providers"].as_table().into_iter().flatten().filter_map(|(_, p)| p.get("host")?.as_str().map(String::from)).collect();
    let mut dests = strs(c.get("destinations"));
    dests.retain(|d| old_host.as_deref() != Some(d.as_str()) || hosts.contains(d));
    if !dests.contains(&host) {
        dests.push(host);
    }
    let mut paths = strs(c.get("paths"));
    paths.extend(s.paths.iter().filter(|p| !paths.contains(p)).cloned().collect::<Vec<_>>());
    let mut methods = strs(c.get("methods"));
    if !methods.iter().any(|x| x == "POST") {
        methods.push("POST".into());
    }
    c.insert("destinations".into(), tv(dests));
    c.insert("paths".into(), tv(paths));
    c.insert("methods".into(), tv(methods));
    if explicit_class(effective) == c {
        return Ok(src.to_string()); // already configured exactly so (e.g. the built-in deepseek-flash)
    }
    let mut want = doc.clone();
    tbl(&mut want, "actions").insert("providers".into(), toml::Value::Table(c.clone()));
    let mut block = String::from("# The model providers (init-root providers add rewrites this block; docs/provider-configuration.md).\n\
        # Prices are micro-USD per 1M tokens: 1000000 = USD 1.\n");
    let mut body = String::new();
    render("actions.providers", &c, &mut body);
    block += &body;
    let out = replace_sections(src, |p| starts(p, &["actions", "providers"]), &block);
    let got: Table = toml::from_str(&out).map_err(|e| format!("could not edit charter.toml safely ({e}); edit it by hand (docs/provider-configuration.md)"))?;
    if got != want {
        return Err("could not edit charter.toml safely (its [actions.providers] is written in a form this edit does not handle); \
            edit it by hand (docs/provider-configuration.md)".into());
    }
    Ok(out)
}

/// `providers add`, kernel side: `[providers.<alias>]` and its model table when missing (nothing is selected).
pub fn kernel_add(src: &str, s: &Spec) -> Result<String, String> {
    let doc: Table = toml::from_str(src).map_err(|e| format!("kernel/config.toml does not parse: {e}"))?;
    if get(&doc, &["providers", &s.alias, "models", &s.model]).is_some() {
        return Ok(src.to_string());
    }
    let have = get(&doc, &["providers", &s.alias]).is_some();
    let scheduled = get(&doc, &["providers", &s.alias, "schedule"]).is_some();
    let mut block = String::new();
    if !have {
        block += &format!("[providers.{}]\n", key(&s.alias));
        if s.alias != s.kernel_preset {
            block += &format!("preset = {}\n", q(s.kernel_preset));
        }
        block.push('\n');
    }
    block += &format!("[providers.{}.models.{}]\n{}", key(&s.alias), q(&s.model), kernel_model_lines(s, scheduled));
    let mut out = src.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out += &block;
    let got: Table = toml::from_str(&out).map_err(|e| format!("could not edit kernel/config.toml safely ({e}); edit it by hand"))?;
    let mut want = doc;
    let added: Table = toml::from_str(&block).map_err(|e| e.to_string())?;
    let pv = tbl(tbl(&mut want, "providers"), &s.alias);
    for (k, v) in added["providers"][&s.alias].as_table().into_iter().flatten() {
        match (pv.get_mut(k), v) {
            (Some(toml::Value::Table(a)), toml::Value::Table(b)) => a.extend(b.clone()),
            _ => drop(pv.insert(k.clone(), v.clone())),
        }
    }
    if got != want {
        return Err("could not edit kernel/config.toml safely; edit it by hand (docs/provider-configuration.md)".into());
    }
    Ok(out)
}

/// `model use`, kernel side: `[model]` with provider and id (other keys kept), `[model.descriptor]` when `desc`.
pub fn kernel_select(src: &str, provider: &str, model: &str, desc: Option<String>) -> Result<String, String> {
    let doc: Table = toml::from_str(src).map_err(|e| format!("kernel/config.toml does not parse: {e}"))?;
    let old = doc.get("model").and_then(toml::Value::as_table).cloned().unwrap_or_default();
    let mut block = String::from("[model]\n");
    block += &format!("provider = {}\nid = {}\n", q(provider), q(model));
    for (k, v) in old.iter().filter(|(k, v)| !["provider", "id", "descriptor"].contains(&k.as_str()) && !v.is_table()) {
        block += &format!("{} = {v}\n", key(k));
    }
    if let Some(d) = &desc {
        block += &format!("\n[model.descriptor]\n{d}");
    }
    let out = replace_sections(src, |p| starts(p, &["model"]), &block);
    let got: Table = toml::from_str(&out).map_err(|e| format!("could not edit kernel/config.toml safely ({e}); edit it by hand"))?;
    let mut want = doc;
    let new: Table = toml::from_str(&block).map_err(|e| e.to_string())?;
    want.insert("model".into(), new["model"].clone());
    if got != want {
        return Err("could not edit kernel/config.toml safely ([model] is written in a form this edit does not handle); edit it by hand".into());
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------
// What is configured
// ------------------------------------------------------------------------------------------------

fn active_charter(p: &Paths) -> Result<charter::Loaded, String> {
    charter::load(&p.r("state/charter.active.toml"), &p.ctx())
        .or_else(|_| charter::load(&p.r("charter.toml"), &p.ctx()))
        .map_err(|f| format!("no valid charter: {}", f.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))
}

fn kernel_cfg(p: &Paths) -> (String, Table) {
    let src = std::fs::read_to_string(p.i("kernel/config.toml")).unwrap_or_default();
    let t = toml::from_str(&src).unwrap_or_default();
    (src, t)
}

/// `(provider, model)` the kernel config selects (kernel defaults: deepseek / deepseek-flash).
fn selected(k: &Table) -> (String, String) {
    let s = |f: &str, d: &str| get(k, &["model", f]).and_then(toml::Value::as_str).unwrap_or(d).to_string();
    (s("provider", "deepseek"), s("id", "deepseek-flash"))
}

/// The key state of a provider: `present`, `missing`, `insecure`, `refused` (test mode, not `test-`) or
/// `not_required` (local no-auth). Never reads more than it must, and never returns the value.
fn key_state(p: &Paths, pid: &str, prov: &Table, test_mode: bool) -> &'static str {
    if prov.get("auth").and_then(toml::Value::as_str) == Some("none") && prov.get("local").and_then(toml::Value::as_bool) == Some(true) {
        return "not_required";
    }
    match util::read_secret(&p.r("keys/providers").join(pid)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "insecure",
        Ok(mut b) => {
            let st = if test_mode && !b.starts_with(b"test-") { "refused" } else { "present" };
            sys::zero(&mut b);
            st
        }
    }
}

/// `providers.list` / `init-root providers list`: the configured providers and models, the selection, whether it
/// is ready (a key, or none needed) and the catalog. No secret, host or account data beyond the charter's.
pub fn list(p: &Paths) -> Result<Value, String> {
    let l = active_charter(p)?;
    let cls = explicit_class(l.ch.actions.get("providers").and_then(toml::Value::as_table).unwrap_or(&Table::new()));
    let (_, k) = kernel_cfg(p);
    let (sp, sm) = selected(&k);
    let test_mode = l.ch.root.test_mode;
    let provs = cls.get("providers").and_then(toml::Value::as_table).cloned().unwrap_or_default();
    let models = cls.get("models").and_then(toml::Value::as_table).cloned().unwrap_or_default();
    let mut pv = Vec::new();
    let mut mv = Vec::new();
    for (pid, pt) in &provs {
        let pt = pt.as_table().cloned().unwrap_or_default();
        let ks = key_state(p, pid, &pt, test_mode);
        let pre = if OFFICIAL.contains(&pid.as_str()) { pid.as_str() } else if pt.get("local").and_then(toml::Value::as_bool) == Some(true) { "local" }
            else { match pt.get("api").and_then(toml::Value::as_str) { Some("messages") => "anthropic", Some("responses") => "openai", _ => "gateway" } };
        let tested = if pid == "deepseek" { "live" } else { "experimental" };
        pv.push(json!({"id": pid, "preset": pre, "key": ks, "tested": tested, "api": pt.get("api").and_then(toml::Value::as_str).unwrap_or(preset(pre).map_or("chat", |x| x.api))}));
        for (mid, md) in &models {
            let mine = cat_model(pid, mid).is_some() || get(&k, &["providers", pid, "models", mid]).is_some() || provs.len() == 1;
            if !mine {
                continue;
            }
            let n = |f: &str| md.get(f).and_then(toml::Value::as_integer).unwrap_or(0).max(0) as u64;
            mv.push(json!({"provider": pid, "model": mid, "selected": *pid == sp && *mid == sm, "tested": tested, "key": ks,
                "input_usd": usd(n("input_musd_per_mtok")), "output_usd": usd(n("output_musd_per_mtok")), "min_level": n("min_level")}));
        }
    }
    let ready = mv.iter().any(|m| m["selected"] == true && matches!(m["key"].as_str(), Some("present" | "not_required")));
    let catalog: Vec<Value> = CATALOG.iter().map(|c| json!({"preset": c.id, "display": c.display, "tested": c.tested, "api": c.api,
        "needs_url": c.host.is_none(), "default_url": c.default_url, "needs_key": !c.local, "env": c.env,
        "models": c.models.iter().map(|x| json!({"id": x.id, "input_usd": usd(x.input), "output_usd": usd(x.output)})).collect::<Vec<_>>()})).collect();
    Ok(json!({"v": 1, "ready": ready, "selected": {"provider": sp, "model": sm}, "providers": pv, "models": mv, "catalog": catalog, "test_mode": test_mode}))
}

// ------------------------------------------------------------------------------------------------
// Actions: add, use, keys
// ------------------------------------------------------------------------------------------------

/// `providers add`: checks, writes charter.toml (0600) and the kernel provider table. The caller applies the charter,
/// and on failure puts the returned previous text back (`restore`).
pub fn add_files(p: &Paths, r: &AddReq) -> Result<(Spec, Option<String>), String> {
    let s = spec(r)?;
    let cp = p.r("charter.toml");
    let src = std::fs::read_to_string(&cp).map_err(|e| format!("{}: {e}", cp.display()))?;
    if std::fs::read_to_string(p.r("state/charter.active.toml")).is_ok_and(|a| a != src) {
        return Err("charter.toml has edits that are not applied yet; run `init-root charter check --apply` first".into());
    }
    let (ch, ..) = charter::parse(&src, &p.ctx()).map_err(|f| format!("charter.toml is invalid: {}", f.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))?;
    let eff = ch.actions.get("providers").and_then(toml::Value::as_table).cloned().unwrap_or_default();
    let new = charter_add(&src, &eff, &s)?;
    if let Err(f) = charter::parse(&new, &p.ctx()) {
        return Err(format!("the new charter would be invalid: {}", f.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")));
    }
    let changed = new != src;
    let (ksrc, _) = kernel_cfg(p);
    let knew = kernel_add(&ksrc, &s)?;
    if changed {
        write_atomic(&cp, new.as_bytes(), 0o600).map_err(|e| format!("{}: {e}", cp.display()))?;
    }
    if knew != ksrc && p.i("kernel").is_dir() {
        write_atomic(&p.i("kernel/config.toml"), knew.as_bytes(), 0o644).map_err(|e| format!("kernel/config.toml: {e}"))?;
    }
    Ok((s, changed.then_some(src)))
}

/// After a failed apply: charter.toml back to the text it had (the kernel table added beside it is harmless).
fn restore(p: &Paths, old: &str) {
    if let Err(e) = write_atomic(&p.r("charter.toml"), old.as_bytes(), 0o600) {
        util::log(&p.rh, "warn", &format!("providers add: charter.toml could not be restored after a failed apply: {e}"));
    }
}

fn add_result(s: &Spec, changed: bool, applied: &Value) -> Value {
    let unpriced = !s.priced;
    json!({"provider": s.alias, "model": s.model, "preset": s.preset, "tested": s.tested, "changed": changed, "applied": applied,
        "needs_key": !s.local, "unpriced": unpriced,
        "next": if s.local { format!("init-root model use {}/{}", s.alias, s.model) }
            else { format!("init-root keys set {}; then init-root model use {}/{}", s.alias, s.alias, s.model) }})
}

/// `model use`: the kernel config change, checked against the active charter. Ok(true) when the file changed.
pub fn use_files(p: &Paths, provider: &str, model: &str) -> Result<bool, String> {
    let l = active_charter(p)?;
    let cls = explicit_class(l.ch.actions.get("providers").and_then(toml::Value::as_table).unwrap_or(&Table::new()));
    let pt = get(&cls, &["providers", provider]).and_then(toml::Value::as_table)
        .ok_or_else(|| format!("provider {provider:?} is not configured; add it first: init-root providers add <preset>"))?;
    let md = get(&cls, &["models", model]).and_then(toml::Value::as_table)
        .ok_or_else(|| format!("model {model:?} has no tariff in the charter; add it: init-root providers add <preset> --model {model}"))?;
    let (src, k) = kernel_cfg(p);
    let api = pt.get("api").and_then(toml::Value::as_str).unwrap_or(preset(provider).map_or("chat", |x| x.api));
    let cat = cat_model(provider, model);
    let mut src2 = src.clone();
    if get(&k, &["providers", provider, "models", model]).is_none() {
        // a model the charter prices but the kernel config does not: its table from the charter's tariff
        let n = |f: &str| md.get(f).and_then(toml::Value::as_integer).unwrap_or(0).max(0) as u64;
        let pre = match api { "messages" => "anthropic", "responses" => "openai", _ => "deepseek" };
        let s = Spec { preset: "", alias: provider.into(), host: String::new(), base_path: String::new(), api: preset(pre).map_or("chat", |x| x.api),
            local: false, custom: cat.is_none(), kernel_preset: pre, paths: vec![], model: model.into(),
            price: (n("input_musd_per_mtok"), n("cache_read_musd_per_mtok"), n("output_musd_per_mtok")), offpeak: cat.and_then(|c| c.offpeak),
            cache_write: cat.and_then(|c| c.cache_write), priced: true, context: CUSTOM_CONTEXT, max_output: cat.map_or(CUSTOM_MAX_OUT, |c| c.max_output),
            reserve: cat.map_or(CUSTOM_REQUEST, |c| c.reserve), tested: "experimental" };
        src2 = kernel_add(&src2, &s)?;
    }
    let k2: Table = toml::from_str(&src2).unwrap_or_default();
    let desc = cat.is_none().then(|| {
        let n = |f: &str, d: u64| get(&k2, &["providers", provider, "models", model, f]).and_then(toml::Value::as_integer).map_or(d, |x| x.max(1) as u64);
        let (ctx, mo) = (n("context_window", CUSTOM_CONTEXT), n("max_output", CUSTOM_MAX_OUT));
        descriptor_lines(api, ctx, mo, n("reserve_output_tokens", CUSTOM_REQUEST).min(mo))
    });
    let out = kernel_select(&src2, provider, model, desc)?;
    if out == src {
        return Ok(false);
    }
    if !p.i("kernel").is_dir() {
        return Err(format!("{} is missing (not installed?)", p.i("kernel").display()));
    }
    write_atomic(&p.i("kernel/config.toml"), out.as_bytes(), 0o644).map_err(|e| format!("kernel/config.toml: {e}"))?;
    Ok(true)
}

/// Splits `provider/model` (the model id may itself contain `/`).
pub fn split_pm(s: &str) -> Option<(&str, &str)> {
    s.split_once('/').filter(|(a, b)| provider_id_ok(a) && model_id_ok(b))
}

/// Validates and stores a provider key (0600). The secret is trimmed, then zeroed by the caller's buffer owner.
pub fn store_key(p: &Paths, provider: &str, secret: &[u8], test_mode: bool) -> Result<(), (&'static str, String)> {
    if !provider_id_ok(provider) {
        return Err(("root.args", "provider must match ^[a-z][a-z0-9_-]{0,31}$".into()));
    }
    let t = secret.trim_ascii();
    if !(1..=512).contains(&t.len()) || !std::str::from_utf8(t).is_ok_and(|s| !s.chars().any(char::is_control)) {
        return Err(("keys.invalid", "a key is 1–512 bytes of UTF-8 without control characters".into()));
    }
    if test_mode && !t.starts_with(b"test-") {
        return Err(("keys.test_mode", "this Root is in test mode: it accepts only keys that start with \"test-\"".into()));
    }
    let dir = p.r("keys/providers");
    if !dir.is_dir() {
        return Err(("root.mode", format!("{} is missing; is INIT installed?", dir.display())));
    }
    write_atomic(&dir.join(provider), t, 0o600).map_err(|e| ("root.internal", format!("the key was not stored: {}", e.kind())))
}

// ------------------------------------------------------------------------------------------------
// Owner-socket requests (the daemon)
// ------------------------------------------------------------------------------------------------

type Refusal = (&'static str, String);

pub fn list_req(root: &Root) -> Result<Value, Refusal> {
    list(&root.p).map_err(|m| ("root.mode", m))
}

pub fn add_req(root: &Root, a: &Value) -> Result<Value, Refusal> {
    let (s, old) = add_files(&root.p, &AddReq::from_json(a)).map_err(|m| ("root.args", m))?;
    let applied = match &old {
        None => json!({"changed": []}),
        Some(o) => root.apply_charter().inspect_err(|_| restore(&root.p, o))?,
    };
    Ok(add_result(&s, old.is_some(), &applied))
}

pub fn use_req(root: &Root, a: &Value) -> Result<Value, Refusal> {
    let (pv, mv) = (a["provider"].as_str().unwrap_or(""), a["model"].as_str().unwrap_or(""));
    let changed = use_files(&root.p, pv, mv).map_err(|m| ("root.args", m))?;
    let slot = lock(&root.st).slot.clone();
    let restart = match (&slot, changed) {
        (_, false) => "none",
        (Slot::Starting { .. } | Slot::Running | Slot::Backoff { .. }, true) => {
            root.sup.send(Ev::Restart(Plan::plain("cold")));
            "now"
        }
        _ => "next_start",
    };
    if changed {
        root.notice("info", &format!("[root] model: {pv}/{mv}{}", if restart == "now" { "; the agent restarts with it" } else { "" }));
    }
    Ok(json!({"provider": pv, "model": mv, "changed": changed, "restart": restart}))
}

/// `keys.set{provider, secret}`: takes the secret out of the request and zeroes it. Never logged.
pub fn keys_set_req(root: &Root, v: &mut Value) -> Result<Value, Refusal> {
    let provider = v["args"]["provider"].as_str().unwrap_or("").to_string();
    let mut b = match v.get_mut("args").and_then(|a| a.get_mut("secret")).map(Value::take) {
        Some(Value::String(s)) => s.into_bytes(),
        _ => return Err(("root.args", "keys.set needs provider and secret".into())),
    };
    let test_mode = lock(&root.st).charter.as_ref().is_some_and(|l| l.ch.root.test_mode);
    let r = store_key(&root.p, &provider, &b, test_mode);
    sys::zero(&mut b);
    r?;
    let state = root.egress.get().map(|e| e.reload_keys()).map_or(Value::Null, |r| r["keys"][&provider].clone());
    root.notice("info", &format!("[root] key stored for {provider} (mode 0600; never shown)"));
    Ok(json!({"provider": provider, "stored": true, "tested": false, "key": state}))
}

// ------------------------------------------------------------------------------------------------
// CLI
// ------------------------------------------------------------------------------------------------

fn opt(a: &mut Vec<String>, name: &str) -> Option<String> {
    let i = a.iter().position(|x| x == name)?;
    (i + 1 < a.len()).then(|| a.drain(i..=i + 1).nth(1))?
}

fn cli_fail(code: u8, m: &str) -> u8 {
    eprintln!("init-root: {m}");
    code
}

fn print_list(v: &Value, only: Option<&str>, models_only: bool) {
    if !models_only {
        println!("Configured providers:");
        for p in v["providers"].as_array().into_iter().flatten() {
            let t = if p["tested"] == "live" { "live-tested" } else { "experimental (not live-tested)" };
            println!("  {:<14} {:<10} key {:<12} {t}", p["id"].as_str().unwrap_or(""), p["api"].as_str().unwrap_or(""), p["key"].as_str().unwrap_or(""));
        }
    }
    println!("Configured models (USD per 1M tokens, input/output):");
    for m in v["models"].as_array().into_iter().flatten().filter(|m| only.is_none_or(|o| m["provider"] == o)) {
        let sel = if m["selected"] == true { "*" } else { " " };
        let t = if m["tested"] == "live" { "" } else { "  experimental (not live-tested)" };
        println!("  {sel} {}/{}  ${}/${}{t}", m["provider"].as_str().unwrap_or(""), m["model"].as_str().unwrap_or(""),
            m["input_usd"].as_str().unwrap_or(""), m["output_usd"].as_str().unwrap_or(""));
    }
    println!("Catalog (init-root providers add <preset> [--base-url URL] [--model ID]):");
    for c in v["catalog"].as_array().into_iter().flatten().filter(|c| only.is_none_or(|o| c["preset"] == o)) {
        let t = if c["tested"] == "live" { "live-tested" } else { "experimental (not live-tested)" };
        let ms: Vec<&str> = c["models"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).collect();
        let ms = if ms.is_empty() { "--model ID".to_string() } else { ms.join(", ") };
        println!("  {:<10} {}  [{ms}]  {t}", c["preset"].as_str().unwrap_or(""), c["display"].as_str().unwrap_or(""));
    }
    if v["ready"] != true {
        println!("Not ready: the selected model {}/{} has no usable key. Run `init-agent` for the guided setup.",
            v["selected"]["provider"].as_str().unwrap_or(""), v["selected"]["model"].as_str().unwrap_or(""));
    }
}

/// `providers list|add`, `models [--provider P]`, `model use <provider>/<model>`. Through the running Root when it
/// answers (it applies the charter and restarts the kernel); otherwise on the files, under the Root lock.
pub fn cli(p: &Paths, mut a: Vec<String>) -> u8 {
    let json_out = { let n = a.len(); a.retain(|x| x != "--json"); a.len() != n };
    let t = std::time::Duration::from_secs(30);
    let only = opt(&mut a, "--provider");
    let r = AddReq { base_url: opt(&mut a, "--base-url"), model: opt(&mut a, "--model"), name: opt(&mut a, "--name"), price: opt(&mut a, "--price"),
        context: opt(&mut a, "--context").and_then(|x| x.parse().ok()), max_output: opt(&mut a, "--max-output").and_then(|x| x.parse().ok()),
        preset: a.iter().filter(|x| !x.starts_with("--")).nth(2).cloned().unwrap_or_default() };
    let words: Vec<&str> = a.iter().map(String::as_str).filter(|w| !w.starts_with("--")).collect();
    match words.as_slice() {
        ["providers", "list"] | ["models"] => {
            match list(p) {
                Ok(v) if json_out => println!("{}", util::icj(&v).unwrap_or_default()),
                Ok(v) => print_list(&v, only.as_deref(), words[0] == "models"),
                Err(m) => return cli_fail(1, &m),
            }
            0
        }
        ["providers", "add", _] => {
            let args = json!({"preset": r.preset, "base_url": r.base_url, "model": r.model, "name": r.name, "price": r.price, "context": r.context, "max_output": r.max_output});
            let res = match crate::link::request(p, "providers.add", args, t) {
                Ok(v) => Ok(v),
                // no Root answers: the files here, then the same apply path as `charter check --apply` (it takes the lock)
                Err(Fail::Unreachable(_)) => add_files(p, &r).and_then(|(s, old)| {
                    let applied = match &old {
                        None => json!({"changed": []}),
                        Some(o) => crate::cli::apply_own_charter(p).inspect_err(|_| restore(p, o)).map_err(|(_, m)| m)?,
                    };
                    Ok(add_result(&s, old.is_some(), &applied))
                }),
                Err(Fail::Refused(_, m)) | Err(Fail::Protocol(m)) => Err(m),
            };
            match res {
                Ok(v) if json_out => { println!("{}", util::icj(&v).unwrap_or_default()); 0 }
                Ok(v) => {
                    let what = format!("{}/{}", v["provider"].as_str().unwrap_or(""), v["model"].as_str().unwrap_or(""));
                    println!("{} {what}{}", if v["changed"] == true { "added" } else { "already configured:" },
                        if v["tested"] == "live" { "" } else { " (experimental: not live-tested)" });
                    if v["unpriced"] == true {
                        println!("no --price given: this model's spend shows as $0.00 (unknown). Add --price IN,CACHED,OUT to see estimates.");
                    }
                    println!("next: {}", v["next"].as_str().unwrap_or(""));
                    0
                }
                Err(m) => cli_fail(2, &m),
            }
        }
        ["model", "use", pm] => {
            let Some((pv, mv)) = split_pm(pm).map(|(x, y)| (x.to_string(), y.to_string())) else {
                return cli_fail(64, "usage: init-root model use <provider>/<model> (see init-root models)");
            };
            match crate::link::request(p, "model.use", json!({"provider": pv, "model": mv}), t) {
                Ok(v) => {
                    let r = v["restart"].as_str().unwrap_or("");
                    println!("model {pv}/{mv}: {}", match (v["changed"] == true, r) {
                        (false, _) => "already selected", (true, "now") => "selected; the kernel restarts with it", _ => "selected; it takes effect at the next start" });
                    0
                }
                Err(Fail::Unreachable(_)) => match use_files(p, &pv, &mv) {
                    Ok(c) => { println!("model {pv}/{mv}: {}", if c { "selected; it takes effect when the Root starts" } else { "already selected" }); 0 }
                    Err(m) => cli_fail(2, &m),
                },
                Err(Fail::Refused(_, m)) | Err(Fail::Protocol(m)) => cli_fail(2, &m),
            }
        }
        _ => cli_fail(64, "usage: init-root providers list|add <preset> [--base-url URL] [--model ID] [--name ALIAS] [--price IN,CACHED,OUT] · models [--provider P] · model use <provider>/<model>"),
    }
}

/// For tests: a path-only check that a file never holds a given secret.
#[cfg(test)]
pub fn file_holds(path: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
}

#[cfg(test)]
mod tests;
