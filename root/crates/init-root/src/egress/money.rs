//! Money telemetry (2026-10-23 owner policy): the owner day and in-flight estimates in
//! integer micro-USD, epochs and the guard ring (D6 rule c), class volume and rate counters, and the
//! usage ledger `ROOT_HOME/egress/usage/<UTC date>.jsonl` with its rebuild at start.

use super::classes::{Class, Model};
use super::tap;
use crate::charter::{self, Charter};
use crate::util::{self, icj, rfc3339};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const DAY_MS: i64 = 86_400_000;
const RING: usize = 1024;
/// `egress.guard_stale_s`: three kernel poll intervals (§3.6 rule 2).
#[allow(dead_code)] // kept with the observe-only money model (2026-10-23): no Root money guard reads it now
pub const GUARD_STALE_MS: i64 = 30_000;
/// A rate bucket holds `rate_per_min × 60_000` units; one request costs 60,000 and it refills `rate` per ms.
const UNIT: u64 = 60_000;
/// The largest explicit wire output bound accepted by Root request inspection.
/// Larger requests are rejected, never forwarded with a silently smaller reservation.
pub const MAX_OUT_TOKENS: u64 = 1 << 20;

/// §3.4 admission figures for one metered request: (the reservation the Root holds, the figure the kernel's
/// guard computes). The Root holds input at one token per request byte, which never under-counts (a token
/// covers at least one byte, and the JSON framing is counted too), so the actual cost stays within the
/// estimate is conservative for text (review F4). The legacy guard figure keeps egress.md §3.4's
/// `ceil(bytes / 3)` that the kernel proxy mirrors, and only D6 rule (c) 3 reads it, so a guard that did its
/// sum remains readable by older clients. Inspection rejects absent/invalid wire caps before this
/// call; defense in depth saturates both figures if that precondition is violated (F1). Cache writes (v0.5.0 L3)
/// bill at up to 2× the input price (every write price in Table A, and the fallback): the hold at the input price
/// per byte still covers them for any request of 2 or more bytes per input token, which every tokenizer's text and
/// the JSON framing give, so the input price stays the hold's price and DeepSeek's holds are unchanged. v0.5.3 L6:
/// a charter whose cache price is above 2× its input price raises the hold's price per byte to half that price, so
/// the hold covers every input class under the same 2-bytes-per-token rule (`hold_price`); no catalog model has one.
pub fn reservation(body_len: u64, max_out: Option<u64>, md: &Model) -> (u64, u64) {
    let Some(out) = max_out.filter(|n| (1..=MAX_OUT_TOKENS).contains(n)) else { return (u64::MAX, u64::MAX) };
    (tap::musd(&[(body_len, hold_price(md)), (out, md.output)]), tap::musd(&[(body_len.div_ceil(3), md.input), (out, md.output)]))
}

/// The hold's input price per request byte: the input price (a token is at least one byte), or half the dearest cache
/// price when that is higher (at least two bytes per token, as `reservation` says).
fn hold_price(md: &Model) -> u64 {
    md.input.max(md.cache_read.max(md.cache_write).max(md.cache_write_1h).div_ceil(2))
}

/// The model's dearest price per token (µUSD per 1M tokens), at which a usage class without a price is booked.
pub fn dearest(md: &Model) -> u64 {
    md.input.max(md.cache_read).max(md.cache_write).max(md.cache_write_1h).max(md.output)
}

/// v0.5.3 L6: what one settled metered request books, and its ledger flags (root/egress.md §3.4). `hold` is its
/// reservation, `max_out` its wire output cap and `ok` a 2xx answer.
/// - Usage with every class priced: its list price (`Usage::cost`). It is never clamped to the hold, so a provider
///   that bills more than the Root held is still booked in full: the meter never under-counts.
/// - A stream cut before its final usage (`partial`): the input classes as reported and the output at the wire cap,
///   which the provider may have generated in full (`usage_partial`).
/// - Usage in a class the Root has no price for (`unknown`): those tokens at the model's dearest price, at most the
///   hold, which is what the Root books when a request reports no usage at all, and never below the priced classes
///   (`price_drift`; the egress also raises an alert).
/// - No usage: the hold on a 2xx answer (`usage_estimated`, RT-C7); 0 when the provider refused the request.
pub fn book(usage: Option<&tap::Usage>, md: &Model, hold: u64, max_out: u64, ok: bool) -> (u64, Vec<&'static str>) {
    let Some(u) = usage else {
        return if ok { (hold, vec!["usage_estimated"]) } else { (0, Vec::new()) };
    };
    let mut flags = Vec::new();
    let mut priced = *u;
    if u.partial {
        priced.output = u.output.max(max_out);
        flags.push("usage_partial");
    }
    let known = priced.cost(md);
    if u.unknown == 0 {
        return (known, flags);
    }
    flags.push("price_drift");
    let with = priced.cost_with(md, &[(u.unknown, dearest(md))]);
    (known.max(with.min(hold)), flags)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub day: u64,
    pub total: u64,
    pub reserve: u64,
    pub offset_ms: i64,
}

impl Limits {
    pub fn of(ch: &Charter) -> Limits {
        let o = &ch.money.day_starts_at_utc_offset;
        let hm = |a: usize| o.get(a..a + 2).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let off = (hm(1) * 60 + hm(4)) * 60_000;
        Limits { day: charter::musd(&ch.money.day_usd).unwrap_or(0), total: charter::musd(&ch.money.total_usd).unwrap_or(0),
            reserve: charter::musd(&ch.sealed.reserve_usd).unwrap_or(0), offset_ms: if o.starts_with('-') { -off } else { off } }
    }
    /// Legacy configured total minus reserve; metadata only, never admission policy.
    pub fn zone_total(&self) -> u64 {
        self.total.saturating_sub(self.reserve)
    }
}

/// The start of the owner day containing `now` (the charter's fixed UTC offset).
pub fn day_start(now: i64, offset_ms: i64) -> i64 {
    (now + offset_ms).div_euclid(DAY_MS) * DAY_MS - offset_ms
}

/// The start of the owner day containing `now` under the charter's `money.day_starts_at_utc_offset`: the one day
/// rule for the Root's daily windows (money here; the `Halted` retry cap in sup.rs, WP21 D9).
pub fn owner_day(ch: &Charter, now: i64) -> i64 {
    day_start(now, Limits::of(ch).offset_ms)
}

pub fn date(ms: i64) -> String {
    rfc3339(ms)[..10].to_string()
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // the ring is kept for the ledger rebuild; its fields are read only in Debug output
struct Snap {
    epoch: u64,
    at: i64,
    day_start: i64,
    day: u64,
    total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Vol {
    pub requests: u64,
    pub bytes: u64,
    units: Option<u64>,
    refill_at: i64,
}

/// Settled live metered requests only. Missing provider usage remains unknown, never zero.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub complete: bool,
}
impl Default for TokenTotals {
    fn default() -> Self { Self { input: 0, output: 0, complete: true } }
}
impl TokenTotals {
    pub fn observe(&mut self, usage: Option<tap::Usage>) {
        match usage {
            Some(u) => {
                // v0.5.3 L6: a cut stream's output, or tokens in a class the Root does not price, are not known counts
                if u.partial || u.unknown > 0 { self.complete = false; }
                if self.input.checked_add(u.input).is_none() || self.output.checked_add(u.output).is_none() { self.complete = false; }
                self.input = self.input.saturating_add(u.input);
                self.output = self.output.saturating_add(u.output);
            }
            None => self.complete = false,
        }
    }
    fn record(&mut self, start: &Value, end: Option<&Value>) {
        if start["model"].as_str().is_none() { return; } // free metadata/http operations
        let usage = end.and_then(|e| Some(tap::Usage {
            input: e["usage"]["input"].as_u64()?, output: e["usage"]["output"].as_u64()?,
            cache_read: e["usage"]["cache_read"].as_u64().unwrap_or(0),
            cache_write: e["usage"]["cache_write"].as_u64().unwrap_or(0), cache_write_1h: e["usage"]["cache_write_1h"].as_u64().unwrap_or(0),
            cache_write_unsplit: e["usage"]["cache_write_unsplit"].as_u64().unwrap_or(0),
            unknown: e["usage"]["unknown"].as_u64().unwrap_or(0), unknown_write: e["usage"]["unknown_write"].as_u64().unwrap_or(0),
            partial: e["usage"]["partial"] == true,
        }));
        self.observe(usage);
    }
}

pub struct Money {
    pub tokens: TokenTotals,
    pub lim: Limits,
    pub epoch: u64,
    pub day_start: i64,
    pub day_spent: u64,
    pub day_reserved: u64,
    pub total_spent: u64,
    pub total_reserved: u64,
    /// v0.5.3 L6: today's settled live spend by the kernel's `x-egress-scope` label (`scope_label`), so `/usage` shows a
    /// self-change's checks at the Root's price (the label is attribution only, never an input to admission).
    pub day_scope: BTreeMap<&'static str, u64>,
    ring: VecDeque<Snap>,
    pub vol: BTreeMap<String, Vol>,
    hits: BTreeSet<String>,
    /// The usage directory once `rebuild` set it: the zone's lifetime room is then shared with the twin pool there.
    usage: Option<PathBuf>,
}

/// A refused class volume or rate check: code, limit, seconds until it may pass.
pub type VolRefusal = (&'static str, u64, u64);

impl Money {
    pub fn new(lim: Limits, now: i64) -> Money {
        let ds = day_start(now, lim.offset_ms);
        Money { tokens: TokenTotals::default(), lim, epoch: 0, day_start: ds, day_spent: 0, day_reserved: 0, total_spent: 0, total_reserved: 0,
            day_scope: BTreeMap::new(), ring: VecDeque::new(), vol: BTreeMap::new(), hits: BTreeSet::new(), usage: None }
    }

    /// A new owner day clears the day counters (only forward: a clock stepping back changes nothing).
    pub fn roll(&mut self, now: i64) {
        let ds = day_start(now, self.lim.offset_ms);
        if ds > self.day_start {
            (self.day_start, self.day_spent, self.day_reserved) = (ds, 0, 0);
            self.day_scope.clear();
            self.vol.values_mut().for_each(|v| (v.requests, v.bytes) = (0, 0));
            self.hits.clear();
        }
    }

    pub fn resets_at(&self) -> i64 {
        self.day_start + DAY_MS
    }

    /// Record an estimate for in-flight telemetry. Legacy dollar limits do not gate work.
    pub fn admit(&mut self, r: u64) -> Result<(), &'static str> {
        self.total_reserved = self.total_reserved.saturating_add(r);
        self.day_reserved = self.day_reserved.saturating_add(r);
        let used = self.total_spent.saturating_add(self.total_reserved);
        with_pool(self.usage.as_deref(), |p| p.map(|p| p.zone = used));
        Ok(())
    }

    /// The figures a money refusal reports: (limit, spent, reserved). The total's limit is the zone's room left after
    /// the twin pool's lifetime spend and holds.
    #[allow(dead_code)] // observe-only money (2026-10-23): no money refusal reports these figures now
    pub fn view(&self, code: &str) -> (u64, u64, u64) {
        if code == "ceiling.day" {
            (self.lim.day, self.day_spent, self.day_reserved)
        } else {
            let twin = with_pool(self.usage.as_deref(), |p| p.map_or(0, |p| p.committed()));
            (self.lim.zone_total().saturating_sub(twin), self.total_spent, self.total_reserved)
        }
    }

    /// Releases a reservation and books the cost; the cost counts for the day it was admitted in.
    pub fn settle(&mut self, r: u64, cost: u64, admitted_day: i64, now: i64) {
        self.total_reserved = self.total_reserved.saturating_sub(r);
        self.total_spent = self.total_spent.saturating_add(cost);
        if admitted_day == self.day_start {
            self.day_reserved = self.day_reserved.saturating_sub(r);
            self.day_spent = self.day_spent.saturating_add(cost);
        }
        let used = self.total_spent.saturating_add(self.total_reserved);
        with_pool(self.usage.as_deref(), |p| p.map(|p| p.zone = used));
        self.epoch += 1;
        self.snap(now);
    }

    /// Attributes a settled cost to its scope label, for the owner day it was admitted in (as `settle` counts it).
    pub fn book_scope(&mut self, scope: &'static str, cost: u64, admitted_day: i64) {
        if admitted_day == self.day_start {
            let s = self.day_scope.entry(scope).or_default();
            *s = s.saturating_add(cost);
        }
    }

    fn snap(&mut self, now: i64) {
        self.ring.push_back(Snap { epoch: self.epoch, at: now, day_start: self.day_start, day: self.day_spent, total: self.total_spent });
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
    }

    /// Legacy guard headers never trigger a monetary refusal or demotion.
    #[allow(dead_code)] // observe-only money: kept for the tests that pin "never a refusal"
    pub fn bypass(&self, _guard: Option<&str>, _code: &str, _r: u64, _now: i64) -> bool { false }

    /// True the first time `key` is seen this owner day (one non-demoting hit record per ceiling per day).
    pub fn first_today(&mut self, key: String) -> bool {
        self.hits.insert(key)
    }

    /// The class's day volume and token bucket (§3.5 `requests_per_day`, `bytes_per_day`, `rate_per_min`).
    pub fn vol_check(&mut self, c: &Class, req_bytes: u64, now: i64) -> Result<(), VolRefusal> {
        let wait = ((self.resets_at() - now).max(0) as u64).div_ceil(1000);
        let v = self.vol.entry(c.id.clone()).or_default();
        if v.requests + 1 > c.requests_per_day {
            return Err(("class.volume", c.requests_per_day, wait));
        }
        if v.bytes + req_bytes > c.bytes_per_day {
            return Err(("class.volume", c.bytes_per_day, wait));
        }
        let units = refill(v, c.rate_per_min, now);
        if units < UNIT {
            let ms = if c.rate_per_min == 0 { DAY_MS as u64 } else { (UNIT - units).div_ceil(c.rate_per_min) };
            return Err(("class.rate", c.rate_per_min, ms.div_ceil(1000)));
        }
        v.units = Some(units - UNIT);
        v.requests += 1;
        v.bytes += req_bytes;
        Ok(())
    }

    /// Gives back what `vol_check` took, for a request refused later (money) or never sent (ledger).
    pub fn vol_undo(&mut self, c: &Class, req_bytes: u64) {
        if let Some(v) = self.vol.get_mut(&c.id) {
            v.requests = v.requests.saturating_sub(1);
            v.bytes = v.bytes.saturating_sub(req_bytes);
            v.units = v.units.map(|u| (u + UNIT).min(c.rate_per_min * UNIT));
        }
    }

    /// Whole tokens left in a class bucket, for `GET /_root/ceilings`.
    pub fn rate_tokens(&mut self, c: &Class, now: i64) -> u64 {
        let v = self.vol.entry(c.id.clone()).or_default();
        let u = refill(v, c.rate_per_min, now);
        v.units = Some(u);
        u / UNIT
    }
}

fn refill(v: &mut Vol, rate: u64, now: i64) -> u64 {
    let cap = rate * UNIT;
    let u = match v.units {
        None => cap,
        Some(u) => u.saturating_add((now - v.refill_at).max(0) as u64 * rate).min(cap),
    };
    v.refill_at = now;
    u
}

/// The `x-egress-scope` label of a live request (root/egress.md §4.1: `live`, `gate`, `member`), as a fixed key:
/// anything else is `other`, and a request without one `none`.
pub fn scope_label(s: Option<&str>) -> &'static str {
    match s {
        Some("live") => "live",
        Some("gate") => "gate",
        Some("member") => "member",
        Some(_) => "other",
        None => "none",
    }
}

/// The append-only usage ledger: no content, no URLs, no keys (§3.4).
pub struct Ledger {
    pub dir: PathBuf,
}

impl Ledger {
    /// One ICJ line in `<date>.jsonl` (0600); `sync` for `start` records, which precede the upstream request.
    pub fn write(&self, date: &str, v: &Value, sync: bool) -> io::Result<()> {
        let mut o = OpenOptions::new();
        o.create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let mut f = o.open(self.dir.join(format!("{date}.jsonl")))?;
        f.write_all(format!("{}\n", icj(v)?).as_bytes())?;
        if sync { f.sync_data() } else { Ok(()) }
    }
}

/// Ledger files `YYYY-MM-DD.jsonl`, oldest first, with their dates.
fn files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let d = n.strip_suffix(".jsonl").filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-')?.to_string();
            Some((d, e.path()))
        })
        .collect();
    v.sort();
    v
}

/// The `start` records of a file with their `end` (None: the Root died mid-request). A torn line is skipped.
fn pairs(path: &Path) -> Vec<(Value, Option<Value>)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out: Vec<(Value, Option<Value>)> = Vec::new();
    let mut at: BTreeMap<String, usize> = BTreeMap::new();
    for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let id = v["id"].as_str().unwrap_or("").to_string();
        match v["t"].as_str() {
            Some("start") => {
                at.insert(id, out.len());
                out.push((v, None));
            }
            Some("end") => {
                if let Some(&i) = at.get(&id) {
                    out[i].1 = Some(v);
                }
            }
            _ => {}
        }
    }
    out
}

/// A finished request costs what its `end` says; one without an `end` is charged its reservation.
fn cost(s: &Value, e: Option<&Value>) -> u64 {
    e.and_then(|e| e["cost_musd"].as_u64()).unwrap_or_else(|| s["reserved_musd"].as_u64().unwrap_or(0))
}

/// `totals.json`: the lifetime total and epoch count of every ledger file dated before `upto`.
fn read_totals(dir: &Path) -> (String, u64, u64) {
    let v: Value = std::fs::read_to_string(dir.join("../totals.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    match (v["upto"].as_str(), v["total_spent_musd"].as_u64(), v["epoch"].as_u64()) {
        (Some(u), Some(t), Some(e)) => (u.to_string(), t, e),
        _ => (String::new(), 0, 0),
    }
}

fn read_token_totals(dir: &Path, old_epoch: u64) -> TokenTotals {
    let value: Value = std::fs::read_to_string(dir.join("../totals.json")).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    serde_json::from_value(value["tokens"].clone()).unwrap_or(TokenTotals {
        complete: old_epoch == 0, ..TokenTotals::default()
    })
}

/// Σ sealed spend in the usage ledger (WP24 D6): every sealed lane's `t:"sealed"` line at its cost, and a sealed
/// `start` with no line of its id at its reservation (the Root died mid-request). Read from every file: sealed
/// spend is small and `totals.json` checkpoints only the zone's.
pub fn sealed_spent(dir: &Path) -> u64 {
    let (mut open, mut spent) = (BTreeMap::<String, u64>::new(), 0u64);
    for (_, f) in files(dir) {
        for v in std::fs::read_to_string(&f).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if v["pool"] == TWIN_POOL {
                continue; // the twin pool's own lines (X9 D3): never charged to the reserve
            }
            match v["t"].as_str() {
                Some("start") if v["lane"] == "sealed" => drop(open.insert(v["id"].as_str().unwrap_or("").into(), v["reserved_musd"].as_u64().unwrap_or(0))),
                Some("sealed") => {
                    v["id"].as_str().map(|id| open.remove(id));
                    spent = spent.saturating_add(v["cost_musd"].as_u64().unwrap_or(0));
                }
                _ => {}
            }
        }
    }
    open.values().fold(spent, |a, r| a.saturating_add(*r))
}

/// The sealed accounting pool, seeded from durable usage. Legacy reserve values do not gate admission.
static SEALED: Mutex<BTreeMap<PathBuf, (u64, u64)>> = Mutex::new(BTreeMap::new());

/// Record a sealed estimate; the legacy reserve argument is nonenforcing metadata.
pub fn sealed_admit(dir: &Path, _reserve: u64, r: u64) -> Result<(), (u64, u64)> {
    let mut g = SEALED.lock().unwrap_or_else(|e| e.into_inner());
    let p = g.entry(dir.to_path_buf()).or_insert_with(|| (sealed_spent(dir), 0));
    p.1 = p.1.saturating_add(r);
    Ok(())
}

/// Releases a hold and books its cost.
pub fn sealed_settle(dir: &Path, r: u64, cost: u64) {
    let mut g = SEALED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = g.get_mut(dir) {
        *p = (p.0.saturating_add(cost), p.1.saturating_sub(r));
    }
}

/// The `pool` tag of a twin lane's ledger lines (`start` and `t:"sealed"`, which also carry `night`).
pub const TWIN_POOL: &str = "twin";

/// The twin accounting pool. Costs remain attributed separately by night; no USD stop is applied.
#[derive(Clone, Debug, Default)]
struct TwinPool {
    spent: u64,
    held: u64,
    /// Spent + held per night date.
    nights: BTreeMap<String, u64>,
    /// The zone's lifetime spent + reserved, as its `Money` last published it (seeded from the ledger).
    zone: u64,
}

impl TwinPool {
    #[allow(dead_code)] // used by `view` only
    fn committed(&self) -> u64 {
        self.spent.saturating_add(self.held)
    }
}

static TWIN: Mutex<BTreeMap<PathBuf, TwinPool>> = Mutex::new(BTreeMap::new());

/// Runs `f` on the twin pool of `dir` (seeded from the ledger the first time) under the pool lock; `None`: no pool.
fn with_pool<R>(dir: Option<&Path>, f: impl FnOnce(Option<&mut TwinPool>) -> R) -> R {
    let Some(d) = dir else { return f(None) };
    let mut g = TWIN.lock().unwrap_or_else(|e| e.into_inner());
    let p = g.entry(d.to_path_buf()).or_insert_with(|| twin_seed(d));
    f(Some(p))
}

/// The pool from the ledger: each twin line at its cost and an orphan twin `start` at its reservation, by night; the
/// zone's lifetime spend as `rebuild` counts it (`totals.json` plus the newer files, sealed lanes excluded).
fn twin_seed(dir: &Path) -> TwinPool {
    let (mut p, mut open) = (TwinPool::default(), BTreeMap::<String, (String, u64)>::new());
    for (_, f) in files(dir) {
        for v in std::fs::read_to_string(&f).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["pool"] == TWIN_POOL) {
            let (id, night) = (v["id"].as_str().unwrap_or("").to_string(), v["night"].as_str().unwrap_or("").to_string());
            match v["t"].as_str() {
                Some("start") => drop(open.insert(id, (night, v["reserved_musd"].as_u64().unwrap_or(0)))),
                Some("sealed") => {
                    open.remove(&id);
                    let c = v["cost_musd"].as_u64().unwrap_or(0);
                    p.spent = p.spent.saturating_add(c);
                    let n = p.nights.entry(night).or_default();
                    *n = n.saturating_add(c);
                }
                _ => {}
            }
        }
    }
    for (night, r) in open.into_values() {
        p.spent = p.spent.saturating_add(r);
        let n = p.nights.entry(night).or_default();
        *n = n.saturating_add(r);
    }
    let (upto, mut total, _) = read_totals(dir);
    for (_, f) in files(dir).into_iter().filter(|(d, _)| *d >= upto) {
        for (s, e) in pairs(&f).into_iter().filter(|(s, _)| s["lane"] != "sealed") {
            total = total.saturating_add(cost(&s, e.as_ref()));
        }
    }
    p.zone = total;
    p
}

/// Legacy return shape retained for callers; observe-only admission never returns a monetary refusal.
pub type TwinRefusal = (&'static str, u64, u64);

/// Record a twin estimate by night. Legacy day/room arguments do not stop work.
pub fn twin_admit(dir: &Path, night: &str, _day: u64, _room: u64, r: u64) -> Result<(), TwinRefusal> {
    with_pool(Some(dir), |p| {
        let p = p.expect("a pool");
        p.held = p.held.saturating_add(r);
        let n = p.nights.entry(night.to_string()).or_default();
        *n = n.saturating_add(r);
        Ok(())
    })
}

/// Releases a twin hold and books its cost, in the pool and in its night.
pub fn twin_settle(dir: &Path, night: &str, r: u64, cost: u64) {
    with_pool(Some(dir), |p| {
        let p = p.expect("a pool");
        p.held = p.held.saturating_sub(r);
        p.spent = p.spent.saturating_add(cost);
        let n = p.nights.entry(night.to_string()).or_default();
        *n = n.saturating_sub(r).saturating_add(cost);
    })
}

/// Rebuilds the counters from the ledger before the socket binds (§3.4; EG-06). From here on the zone shares its
/// lifetime room with the twin pool of `dir`.
pub fn rebuild(dir: &Path, lim: Limits, now: i64) -> Money {
    let mut m = rebuild_counts(dir, lim, now);
    m.usage = Some(dir.to_path_buf());
    let used = m.total_spent;
    with_pool(Some(dir), |p| p.map(|p| p.zone = used));
    m
}

fn rebuild_counts(dir: &Path, lim: Limits, now: i64) -> Money {
    let mut m = Money::new(lim, now);
    let (upto, total, epoch) = read_totals(dir);
    (m.total_spent, m.epoch) = (total, epoch);
    m.tokens = read_token_totals(dir, epoch);
    for (_, f) in files(dir).into_iter().filter(|(d, _)| *d >= upto) {
        for (s, e) in pairs(&f).into_iter().filter(|(s, _)| s["lane"] != "sealed") {
            let c = cost(&s, e.as_ref());
            m.tokens.record(&s, e.as_ref());
            m.total_spent = m.total_spent.saturating_add(c);
            m.epoch += 1;
            let at = s["at"].as_str().and_then(util::parse_rfc3339).unwrap_or(0);
            if s["lane"] == "live" && at >= m.day_start {
                m.day_spent = m.day_spent.saturating_add(c);
                if s["model"].is_string() { // metered requests, as the egress books them (`book_scope`)
                    let sc = m.day_scope.entry(scope_label(s["scope"].as_str())).or_default();
                    *sc = sc.saturating_add(c);
                }
                let v = m.vol.entry(s["class"].as_str().unwrap_or("").to_string()).or_default();
                v.requests += 1;
                v.bytes += s["req_bytes"].as_u64().unwrap_or(0) + e.as_ref().and_then(|e| e["bytes"].as_u64()).unwrap_or(0);
            }
        }
    }
    m.snap(now);
    m
}

/// Moves `totals.json` forward to files older than two days, which no in-flight request still writes to.
pub fn checkpoint(dir: &Path, now: i64) -> io::Result<()> {
    let to = date(now - 2 * DAY_MS);
    let (upto, mut total, mut epoch) = read_totals(dir);
    let mut tokens = read_token_totals(dir, epoch);
    if to <= upto {
        return Ok(());
    }
    for (_, f) in files(dir).into_iter().filter(|(d, _)| *d >= upto && *d < to) {
        for (s, e) in pairs(&f).into_iter().filter(|(s, _)| s["lane"] != "sealed") {
            total = total.saturating_add(cost(&s, e.as_ref()));
            tokens.record(&s, e.as_ref());
            epoch += 1;
        }
    }
    let v = json!({"epoch": epoch, "total_spent_musd": total, "tokens": tokens, "upto": to, "v": 1});
    util::write_atomic(&dir.join("../totals.json"), format!("{}\n", icj(&v)?).as_bytes(), 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim(day: u64, total: u64) -> Limits {
        Limits { day, total, reserve: 0, offset_ms: 7 * 3_600_000 }
    }
    const T0: i64 = 1_792_000_000_000; // 2026-10-15T…Z

    #[test]
    fn owner_day_and_offsets() {
        let noon = crate::util::parse_rfc3339("2026-10-20T10:00:00Z").unwrap();
        assert_eq!(rfc3339(day_start(noon, 7 * 3_600_000)), "2026-10-19T17:00:00.000Z");
        assert_eq!(rfc3339(day_start(noon, -5 * 3_600_000)), "2026-10-20T05:00:00.000Z");
        assert_eq!(rfc3339(day_start(noon, 0)), "2026-10-20T00:00:00.000Z");
        let mut ch = crate::charter::Charter { schema_version: 1, purpose: Default::default(), constitution: Default::default(),
            autonomy: Default::default(), money: Default::default(), quotas: Default::default(), actions: Default::default(),
            sandbox: Default::default(), panic: Default::default(), notify: Default::default(), mirror: Default::default(),
            ladder: Default::default(), sealed: Default::default(), root: Default::default(), extend: Default::default() };
        ch.money.day_starts_at_utc_offset = "-03:30".into();
        let l = Limits::of(&ch);
        assert_eq!((l.day, l.total, l.reserve, l.offset_ms), (5_000_000, 25_000_000, 3_000_000, -12_600_000));
        assert_eq!(l.zone_total(), 22_000_000);
    }

    #[test]
    fn reservations_track_cost_without_enforcing_legacy_limits() {
        let mut m = Money::new(lim(1, 1), T0);
        let ds = m.day_start;
        for _ in 0..20 { m.admit(120).unwrap(); }
        assert_eq!((m.day_reserved, m.total_reserved), (2400, 2400));
        for _ in 0..20 { m.settle(120, 100, ds, T0); }
        assert_eq!((m.day_spent, m.total_spent, m.total_reserved), (2000, 2000, 0));
        m.roll(T0 + DAY_MS);
        assert_eq!((m.day_spent, m.total_spent), (0, 2000));
        m.admit(100).unwrap();
        m.settle(100, 90, ds, T0 + DAY_MS);
        assert_eq!((m.day_spent, m.total_spent), (0, 2090));
    }

    /// Arithmetic saturates; invalid wire caps are rejected by inspection, not treated as smaller estimates.
    /// Review F4: the held reservation counts input at one token per byte, so an answer whose input is denser
    /// than the guard's `bytes / 3` still costs no more than was reserved.
    #[test]
    fn accounting_saturates_and_invalid_wire_caps_never_get_smaller_estimates() {
        let md = Model { min_level: 0, input: 300_000, cache_read: 6_000, cache_write: 600_000, cache_write_1h: 600_000, output: 1_200_000, reserve_out: 65_536 };
        let mut m = Money::new(lim(1000, 10_000), T0);
        let ds = m.day_start;
        m.admit(100).unwrap();
        m.settle(100, 100, ds, T0); // some spend exists, so an unchecked sum would wrap
        assert_eq!(m.admit(u64::MAX), Ok(()));
        assert_eq!(m.admit(u64::MAX - 50), Ok(()));
        assert_eq!(m.total_reserved, u64::MAX);
        let mut d = Money::new(lim(1000, u64::MAX), T0);
        d.admit(10).unwrap();
        assert_eq!(d.admit(u64::MAX - 5), Ok(()), "telemetry saturates rather than refusing");
        assert_eq!((d.day_reserved, d.total_reserved), (u64::MAX, u64::MAX));
        let huge = reservation(1000, Some(u64::MAX), &md);
        assert_eq!(huge, (u64::MAX, u64::MAX), "invalid cap never silently clamps");
        assert_eq!(reservation(1000, Some(MAX_OUT_TOKENS), &md).0, (1000 * 300_000 + MAX_OUT_TOKENS * 1_200_000).div_ceil(1_000_000));
        assert_eq!(reservation(1000, None, &md), (u64::MAX, u64::MAX), "an absent wire cap cannot rely on a model default");
        assert_eq!(reservation(1000, Some(0), &md), (u64::MAX, u64::MAX));
        let pricey = Model { input: u64::MAX, output: u64::MAX, ..md.clone() };
        assert_eq!(reservation(1 << 20, Some(u64::MAX), &pricey).0, u64::MAX, "musd saturates");
        assert_eq!(Money::new(lim(u64::MAX, u64::MAX), T0).admit(u64::MAX), Ok(()), "only a sum past the limit is refused");
        // F4: 3000 request bytes, guard 1000 input tokens; the provider bills 2500 (dense text): still within the hold
        let (hold, guard) = reservation(3000, Some(10), &md);
        assert_eq!((hold, guard), (912, 312));
        let billed = tap::Usage { input: 2500, cache_read: 0, output: 10, ..tap::Usage::default() }.cost(&md);
        assert!(guard < billed && billed <= hold, "{guard} < {billed} ≤ {hold}");
    }

    fn sonnet() -> Model {
        Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 4_000_000, output: 10_000_000, reserve_out: 32_768 }
    }

    /// v0.5.3 L6, money.rs `book`: priced usage at its list price, never clamped to the hold; a cut stream's output at
    /// its wire cap; a class without a price at the dearest price, up to the hold and never below the priced classes;
    /// no usage: the hold on a 2xx, 0 on a refusal.
    #[test]
    fn the_booking_rule() {
        let md = sonnet();
        let u = tap::Usage { input: 1_000, cache_read: 600, cache_write: 100, output: 50, ..tap::Usage::default() };
        let list = tap::musd(&[(300, 2_000_000), (600, 100_000), (100, 2_500_000), (50, 10_000_000)]);
        assert_eq!(book(Some(&u), &md, 1_000_000, 4_096, true), (list, vec![]));
        assert_eq!(book(Some(&u), &md, 1, 4_096, true), (list, vec![]), "a bill above the hold is booked in full");
        let cut = tap::Usage { partial: true, ..u };
        assert_eq!(book(Some(&cut), &md, 1_000_000, 4_096, true).0, tap::musd(&[(300, 2_000_000), (600, 100_000), (100, 2_500_000), (4_096, 10_000_000)]));
        assert_eq!(book(Some(&cut), &md, 1_000_000, 4_096, true).1, vec!["usage_partial"]);
        assert_eq!(book(Some(&tap::Usage { output: 9_000, ..cut }), &md, 1_000_000, 4_096, true).0, tap::musd(&[(300, 2_000_000), (600, 100_000),
            (100, 2_500_000), (9_000, 10_000_000)]), "never below what was reported");
        let drift = tap::Usage { unknown: 20, ..u };
        let with = tap::musd(&[(300, 2_000_000), (600, 100_000), (100, 2_500_000), (50, 10_000_000), (20, 10_000_000)]);
        assert_eq!(dearest(&md), 10_000_000);
        assert_eq!(book(Some(&drift), &md, 1_000_000, 4_096, true), (with, vec!["price_drift"]));
        assert_eq!(book(Some(&drift), &md, list + 3, 4_096, true).0, list + 3, "at most the hold");
        assert_eq!(book(Some(&drift), &md, 1, 4_096, true).0, list, "never below the priced classes");
        assert_eq!(book(None, &md, 777, 4_096, true), (777, vec!["usage_estimated"]));
        assert_eq!(book(None, &md, 777, 4_096, false), (0, vec![]));
        assert_eq!(book(Some(&tap::Usage { unknown: 5, ..cut }), &md, u64::MAX, 4_096, true).1, vec!["usage_partial", "price_drift"]);
    }

    /// v0.5.3 L6: the hold's price per byte covers a charter whose cache price is above 2× its input price; the
    /// catalog's models (all at or below 2×) keep the input price, so their holds are unchanged.
    #[test]
    fn the_hold_covers_every_input_class() {
        let md = sonnet();
        assert_eq!(reservation(1_000, Some(10), &md).0, tap::musd(&[(1_000, 2_000_000), (10, 10_000_000)]), "1 h writes at 2×: unchanged");
        let odd = Model { cache_write_1h: 9_000_001, ..md.clone() };
        assert_eq!(reservation(1_000, Some(10), &odd).0, tap::musd(&[(1_000, 4_500_001), (10, 10_000_000)]));
        // 500 tokens (2 bytes each) all in that class still fit the hold
        let u = tap::Usage { input: 500, cache_write_1h: 500, output: 10, ..tap::Usage::default() };
        assert!(u.cost(&odd) <= reservation(1_000, Some(10), &odd).0);
        assert_eq!(reservation(1_000, Some(10), &md).1, reservation(1_000, Some(10), &odd).1, "the legacy guard figure is unchanged");
    }

    /// v0.5.3 L6: today's settled live spend by scope label, booked for the day it was admitted, rebuilt from the
    /// `start` records' `scope`, cleared by a new owner day.
    #[test]
    fn spend_by_scope_today() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp22/unit-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); // exact path, created by this test
        std::fs::create_dir_all(d.join("usage")).unwrap();
        let l = Ledger { dir: d.join("usage") };
        let rec = |t: &str, id: &str, at: i64, x: Value| {
            let mut v = json!({"at": rfc3339(at), "id": id, "lane": "live", "t": t, "v": 1});
            v.as_object_mut().unwrap().extend(x.as_object().unwrap().clone());
            l.write(&date(at), &v, true).unwrap();
        };
        rec("start", "a", T0, json!({"class": "providers", "model": "m", "reserved_musd": 900, "scope": "gate"}));
        rec("end", "a", T0, json!({"cost_musd": 70}));
        rec("start", "b", T0, json!({"class": "providers", "model": "m", "reserved_musd": 900, "scope": "live"}));
        rec("end", "b", T0, json!({"cost_musd": 30}));
        rec("start", "c", T0, json!({"class": "providers", "model": "m", "reserved_musd": 400, "scope": "gate"})); // died: its reservation
        rec("start", "d", T0 - DAY_MS, json!({"class": "providers", "model": "m", "reserved_musd": 900, "scope": "gate"}));
        rec("end", "d", T0 - DAY_MS, json!({"cost_musd": 5}));
        let mut m = rebuild(&l.dir, lim(0, 0), T0 + 1000);
        assert_eq!(m.day_scope, BTreeMap::from([("gate", 470), ("live", 30)]));
        let ds = m.day_start;
        m.book_scope(scope_label(Some("gate")), 11, ds);
        m.book_scope(scope_label(Some("evil")), 2, ds);
        m.book_scope(scope_label(None), 3, ds);
        m.book_scope(scope_label(Some("gate")), 99, ds - DAY_MS); // admitted yesterday
        assert_eq!(m.day_scope, BTreeMap::from([("gate", 481), ("live", 30), ("none", 3), ("other", 2)]));
        m.roll(T0 + DAY_MS);
        assert!(m.day_scope.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn legacy_guard_headers_never_demote_for_money() {
        let m = Money::new(lim(0, 0), T0);
        for header in [None, Some("epoch=x"), Some("epoch=99")] {
            assert!(!m.bypass(header, "ceiling.day", u64::MAX, T0));
            assert!(!m.bypass(header, "ceiling.total", u64::MAX, T0));
        }
    }

    #[test]
    fn class_volume_and_rate() {
        let mut e = Vec::new();
        let t = super::super::classes::effective_actions(None, false, &mut e);
        let mut c = super::super::classes::compile(&t).remove("crates").unwrap();
        c.rate_per_min = 2;
        c.requests_per_day = 3;
        let mut m = Money::new(lim(1000, 1000), T0);
        assert!(m.vol_check(&c, 10, T0).is_ok() && m.vol_check(&c, 10, T0).is_ok());
        assert_eq!(m.vol_check(&c, 10, T0), Err(("class.rate", 2, 30)));
        assert_eq!(m.rate_tokens(&c, T0 + 29_999), 0);
        assert!(m.vol_check(&c, 10, T0 + 30_000).is_ok());
        assert_eq!(m.vol_check(&c, 10, T0 + 60_000).map_err(|e| e.0), Err("class.volume"));
        c.bytes_per_day = 25;
        m.roll(T0 + DAY_MS);
        assert!(m.vol_check(&c, 20, T0 + DAY_MS).is_ok());
        assert_eq!(m.vol_check(&c, 10, T0 + DAY_MS).map_err(|e| e.0), Err("class.volume"));
    }

    #[test]
    fn rebuild_and_checkpoint_agree() {
        // per process: lanes share the target dir and may run this test at the same time
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp22/unit-money-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); // exact path, created by this test
        std::fs::create_dir_all(d.join("usage")).unwrap();
        let l = Ledger { dir: d.join("usage") };
        let rec = |t: &str, id: &str, at: i64, x: Value| {
            let mut v = json!({"at": rfc3339(at), "id": id, "lane": "live", "t": t, "v": 1});
            v.as_object_mut().unwrap().extend(x.as_object().unwrap().clone());
            l.write(&date(at), &v, true).unwrap();
        };
        let old = T0 - 5 * DAY_MS;
        rec("start", "a", old, json!({"class": "providers", "reserved_musd": 500}));
        rec("end", "a", old, json!({"bytes": 10, "cost_musd": 70}));
        rec("start", "b", old, json!({"class": "providers", "reserved_musd": 400})); // died mid-request
        rec("start", "c", T0, json!({"class": "providers", "req_bytes": 5, "reserved_musd": 300}));
        rec("end", "c", T0, json!({"bytes": 20, "cost_musd": 30}));
        let lm = lim(1_000_000, 10_000_000);
        let m = rebuild(&l.dir, lm.clone(), T0 + 1000);
        assert_eq!((m.total_spent, m.day_spent, m.epoch), (500, 30, 3));
        assert_eq!((m.vol["providers"].requests, m.vol["providers"].bytes), (1, 25));
        checkpoint(&l.dir, T0 + 1000).unwrap();
        assert_eq!(read_totals(&l.dir), (date(T0 + 1000 - 2 * DAY_MS), 470, 2));
        let m2 = rebuild(&l.dir, lm, T0 + 1000);
        assert_eq!((m2.total_spent, m2.day_spent, m2.epoch), (500, 30, 3));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The sealed pool records usage beyond legacy reserve values; an orphan `start` is charged
    /// its reservation; sealed lines stay out of the zone's money.
    #[test]
    fn the_sealed_reserve_is_cumulative() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp22/unit-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); // exact path, created by this test
        std::fs::create_dir_all(&d).unwrap();
        let l = Ledger { dir: d.clone() };
        let w = |v: Value| l.write(&date(T0), &v, true).unwrap();
        w(json!({"at": rfc3339(T0), "id": "r1-0", "lane": "sealed", "reserved_musd": 400, "run": "r1", "t": "start", "v": 1}));
        w(json!({"at": rfc3339(T0), "cost_musd": 120, "id": "r1-0", "lane": "sealed", "run": "r1", "t": "sealed", "v": 1}));
        w(json!({"at": rfc3339(T0), "cost_musd": 30, "lane": "sealed", "run": "r0", "t": "sealed", "v": 1})); // a line from before WP24 D6
        w(json!({"at": rfc3339(T0), "id": "r1-1", "lane": "sealed", "reserved_musd": 200, "run": "r1", "t": "start", "v": 1})); // died
        assert_eq!(sealed_spent(&d), 350);
        let m = rebuild(&d, lim(1_000_000, 10_000_000), T0 + 1000);
        assert_eq!((m.total_spent, m.day_spent, m.epoch), (0, 0, 0), "sealed spend is the reserve's, not the zone's");
        assert_eq!(sealed_admit(&d, 1000, 600), Ok(()));
        assert_eq!(sealed_admit(&d, 1000, 51), Ok(()), "legacy reserve is advisory");
        sealed_settle(&d, 51, 0);

        sealed_settle(&d, 600, 500);
        assert_eq!(sealed_admit(&d, 1000, 151), Ok(()));
        sealed_settle(&d, 151, 0);
        assert_eq!(sealed_admit(&d, 1000, 150), Ok(()));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Twin, live and sealed attribution remains separate even when old allowances are zero.
    #[test]
    fn the_twin_pool_has_its_own_nights_and_shares_the_zone_room() {
        let b = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp22/unit-twin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&b); // exact path, created by this test
        let d = b.join("usage");
        std::fs::create_dir_all(&d).unwrap();
        let l = Ledger { dir: d.clone() };
        let w = |v: Value| l.write(&date(T0), &v, true).unwrap();
        let (n1, n2) = ("2026-10-14", "2026-10-15");
        w(json!({"at": rfc3339(T0), "id": "a-0", "lane": "sealed", "night": n1, "pool": "twin", "reserved_musd": 400, "run": "a", "t": "start", "v": 1}));
        w(json!({"at": rfc3339(T0), "cost_musd": 100, "id": "a-0", "lane": "sealed", "night": n1, "pool": "twin", "run": "a", "t": "sealed", "v": 1}));
        w(json!({"at": rfc3339(T0), "id": "a-1", "lane": "sealed", "night": n1, "pool": "twin", "reserved_musd": 50, "run": "a", "t": "start", "v": 1})); // died
        w(json!({"at": rfc3339(T0), "id": "s-0", "lane": "sealed", "reserved_musd": 90, "run": "s", "t": "start", "v": 1}));
        w(json!({"at": rfc3339(T0), "cost_musd": 70, "id": "s-0", "lane": "sealed", "run": "s", "t": "sealed", "v": 1}));
        w(json!({"at": rfc3339(T0), "class": "providers", "id": "z-0", "lane": "live", "reserved_musd": 300, "t": "start", "v": 1}));
        w(json!({"at": rfc3339(T0), "cost_musd": 200, "id": "z-0", "lane": "live", "t": "end", "v": 1}));
        assert_eq!(sealed_spent(&d), 70, "twin lines are never charged to the reserve");
        let p = twin_seed(&d);
        assert_eq!((p.spent, p.held, p.nights.get(n1).copied(), p.zone), (150, 0, Some(150), 200), "an orphan twin start at its reservation");
        let mut m = rebuild(&d, Limits { day: 0, total: 0, reserve: 0, offset_ms: 0 }, T0 + 1000);
        assert_eq!(m.total_spent, 200);
        twin_admit(&d, n1, 0, 0, 500).unwrap();
        twin_admit(&d, n2, 0, 0, 500).unwrap();
        m.admit(500).unwrap();
        sealed_admit(&d, 0, 500).unwrap();
        twin_settle(&d, n1, 500, 20);
        twin_settle(&d, n2, 500, 50);
        m.settle(500, 100, m.day_start, T0 + 2000);
        sealed_settle(&d, 500, 30);
        with_pool(Some(&d), |p| {
            let p = p.unwrap();
            assert_eq!((p.spent, p.held, p.zone), (220, 0, 300));
        });
        assert_eq!((m.total_spent, m.total_reserved), (300, 0));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn tokens_keep_known_subtotals_and_mark_missing_usage_unknown() {
        let mut tokens = TokenTotals::default();
        tokens.observe(Some(tap::Usage { input: 12, output: 3, cache_read: 4, ..tap::Usage::default() }));
        assert!(tokens.complete);
        tokens.observe(None);
        tokens.observe(Some(tap::Usage { input: 5, output: 2, cache_read: 0, ..tap::Usage::default() }));
        assert_eq!((tokens.input, tokens.output, tokens.complete), (17, 5, false));
        // v0.5.3 L6: a cut stream's output and a class without a price are not known counts
        for u in [tap::Usage { input: 1, partial: true, ..tap::Usage::default() }, tap::Usage { input: 1, unknown: 1, ..tap::Usage::default() }] {
            let mut t = TokenTotals::default();
            t.observe(Some(u));
            assert!(!t.complete, "{u:?}");
        }
    }

    #[test]
    fn token_counts_survive_checkpoint_and_old_checkpoints_remain_unknown() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-homes/wp22/tokens-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dir = d.join("usage");
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = Ledger { dir: dir.clone() };
        ledger.write(&date(T0), &json!({"t":"start","id":"a","model":"m","lane":"live","at":rfc3339(T0),"reserved_musd":100}), true).unwrap();
        ledger.write(&date(T0), &json!({"t":"end","id":"a","cost_musd":30,"usage":{"input":12,"output":3,"cache_read":0}}), true).unwrap();
        checkpoint(&dir, T0 + 4 * DAY_MS).unwrap();
        let m = rebuild(&dir, lim(0,0), T0 + 4 * DAY_MS);
        assert_eq!((m.tokens.input, m.tokens.output, m.tokens.complete), (12,3,true));
        std::fs::write(d.join("totals.json"), json!({"upto":date(T0 + DAY_MS),"epoch":1,"total_spent_musd":30}).to_string()).unwrap();
        let m = rebuild(&dir, lim(0,0), T0 + 4 * DAY_MS);
        assert!(!m.tokens.complete, "legacy checkpoint has no token proof");
        std::fs::remove_dir_all(&d).unwrap();
    }

}
