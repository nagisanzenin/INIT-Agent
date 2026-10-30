//! The Root's usage and finish tap (root/egress.md §3.4): it reads only the lines it needs from the
//! forwarded bytes (kernel/proxy.md §3.10.2 families, §3.12 finish classes), and the peak-price
//! arithmetic in integer micro-USD.

use super::classes::Model;
use serde_json::{Value, json};
use std::collections::BTreeMap;

mod calls; // ADR-011 §5.2 (X5): the tool calls of a live response
pub use calls::Calls;

const LINE_MAX: usize = 1 << 20;
const BODY_MAX: usize = 16 << 20;
const NEEDLES: [&[u8]; 5] = [b"usage", b"finish_reason", b"stop_reason", b"response.", b"error"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Family {
    Chat,
    Messages,
    Responses,
    None,
}

impl Family {
    /// The wire family of a model route (kernel/proxy.md §3.5 table); `None` is unmetered.
    pub fn of(rest: &str) -> Family {
        let p = rest.split('?').next().unwrap_or("");
        if p.ends_with("chat/completions") {
            Family::Chat
        } else if p.ends_with("messages") {
            Family::Messages
        } else if p.ends_with("responses") {
            Family::Responses
        } else {
            Family::None
        }
    }
}

/// Σ tokens × price (µUSD per 1M tokens) in micro-USD, rounded up so the Root never under-counts.
pub fn musd(parts: &[(u64, u64)]) -> u64 {
    let s = parts.iter().fold(0u128, |sum, &(t, p)| sum.saturating_add(u128::from(t) * u128::from(p)));
    u64::try_from(s.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

/// Tokens as the provider reported them: the input total (cache reads and writes included), the cache
/// reads within it (DeepSeek `prompt_cache_hit_tokens`, OpenAI `cached_tokens`, Anthropic
/// `cache_read_input_tokens`), the cache writes within it (v0.5.0 L3: Anthropic `cache_creation_input_tokens`,
/// split by TTL; OpenAI `cache_write_tokens`) and the output (reasoning included: every family counts it there).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    /// Writes at the 5-minute TTL, or a provider's single write class (OpenAI).
    pub cache_write: u64,
    /// Anthropic 1-hour writes.
    pub cache_write_1h: u64,
    /// v0.5.4 (review 4, n1): Anthropic writes the response does not split by TTL (`cache_creation_input_tokens` with no
    /// `cache_creation`). Their lifetime is 5 minutes or 1 hour, so `cost_with` bills them at the dearer of the two write
    /// prices. Not drift: both prices are known (it was the 1-hour class, which a charter may price under 5 minutes).
    pub cache_write_unsplit: u64,
    pub output: u64,
    /// v0.5.3 L6: tokens in usage fields this codec has no price for (a class the provider added, such as a new
    /// cache TTL): the Root books them at the model's dearest price and flags `price_drift` (money.rs `book`).
    pub unknown: u64,
    /// v0.5.4 (the live OpenAI qualify: +302 %): the part of `unknown` the provider reports as a breakdown of its cache
    /// writes (Responses `input_tokens_details.cache_write_tokens_details.*`, Messages `cache_creation.*`), so it is
    /// inside the input and out of `cache_write`/`cache_write_1h`: `cost_with` leaves it out of the priced input
    /// classes, so with `book`'s dearest-price term it is billed once.
    pub unknown_write: u64,
    /// v0.5.3 L6: the stream ended before the provider's final usage (an Anthropic stream cut after
    /// `message_start`, a Chat stream cut before its finish reason): `output` is only what was reported so far, so
    /// the Root books the output at the request's wire cap (money.rs `book`).
    pub partial: bool,
}

impl Usage {
    /// The cost at the model's peak prices (egress.md §3.4): cache reads at its cache-read price, cache writes at its
    /// write prices, the rest of the input at the input price, rounded up once. Provisional owner decision 2026-10-20
    /// (build-deviations §3.13); a model without a cache-read price bills reads at the input price, and one without
    /// write prices bills writes at twice the input price (classes.rs), so neither under-counts. Each class is
    /// clamped to what is left of the input, so a provider that reports more than its input cannot lower the bill.
    /// Classes the Root has no price for (`unknown`) are not in it: money.rs `book` adds them.
    /// v0.5.4: `unknown_write` counts here as writes of the dearer write class (a charter may price 1 hour under 5
    /// minutes), a floor under any write's price.
    pub fn cost(&self, m: &Model) -> u64 {
        Usage { unknown_write: 0, ..self.dearer_write(m, self.unknown_write) }.cost_with(m, &[])
    }

    /// `self` with `n` more writes in the dearer write class (the 1-hour one when the two prices are equal).
    fn dearer_write(&self, m: &Model, n: u64) -> Usage {
        if m.cache_write > m.cache_write_1h {
            Usage { cache_write: self.cache_write.saturating_add(n), ..*self }
        } else {
            Usage { cache_write_1h: self.cache_write_1h.saturating_add(n), ..*self }
        }
    }

    /// `cost` with `extra` (tokens, price) terms, rounded up once over the sum; `unknown_write` is left out of the
    /// input classes (the caller's `extra` prices it), and `cache_write_unsplit` is in the dearer write class.
    pub fn cost_with(&self, m: &Model, extra: &[(u64, u64)]) -> u64 {
        let s = Usage { cache_write_unsplit: 0, ..self.dearer_write(m, self.cache_write_unsplit) };
        let cr = s.cache_read.min(s.input);
        let uw = s.unknown_write.min(s.input - cr);
        let w1 = s.cache_write_1h.min(s.input - cr - uw);
        let w5 = s.cache_write.min(s.input - cr - uw - w1);
        let mut parts = vec![(s.input - cr - uw - w1 - w5, m.input), (cr, m.cache_read), (w5, m.cache_write), (w1, m.cache_write_1h), (s.output, m.output)];
        parts.extend_from_slice(extra);
        musd(&parts)
    }
    /// The `usage` object of a ledger record; the write fields only when there are writes, `unknown` and `partial`
    /// only when set (v0.5.3 L6), so DeepSeek's records are unchanged.
    pub fn json(&self) -> Value {
        let mut v = json!({"cache_read": self.cache_read, "input": self.input, "output": self.output});
        if self.cache_write > 0 {
            v["cache_write"] = json!(self.cache_write);
        }
        if self.cache_write_1h > 0 {
            v["cache_write_1h"] = json!(self.cache_write_1h);
        }
        if self.cache_write_unsplit > 0 {
            v["cache_write_unsplit"] = json!(self.cache_write_unsplit);
        }
        if self.unknown > 0 {
            v["unknown"] = json!(self.unknown);
        }
        if self.unknown_write > 0 {
            v["unknown_write"] = json!(self.unknown_write);
        }
        if self.partial {
            v["partial"] = json!(true);
        }
        v
    }
}

/// v0.5.3 L6: the usage fields each family's codec reads, as paths in the usage object. Totals and subsets of a priced
/// field are listed too (`total_tokens`; reasoning and predicted-output tokens, which every family counts in its
/// output; v0.5.4: Responses' `cache_write_tokens_details.ttl_30m_tokens`, OpenAI's default 30-minute write since
/// 2026-10-20, which is all or part of `cache_write_tokens` at its price). Any other positive integer in a usage object
/// is a class the Root has no price for; one in a family's write breakdown is also taken out of the write total, as far
/// as that total holds it (`observe`, every family).
const KNOWN_CHAT: [&str; 11] = ["prompt_tokens", "completion_tokens", "total_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens",
    "prompt_tokens_details.cached_tokens", "prompt_tokens_details.cache_write_tokens",
    "prompt_tokens_details.cache_write_tokens_details.ttl_30m_tokens", "completion_tokens_details.reasoning_tokens",
    "completion_tokens_details.accepted_prediction_tokens", "completion_tokens_details.rejected_prediction_tokens"];
const KNOWN_MESSAGES: [&str; 6] = ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens",
    "cache_creation.ephemeral_5m_input_tokens", "cache_creation.ephemeral_1h_input_tokens"];
const KNOWN_RESPONSES: [&str; 7] = ["input_tokens", "output_tokens", "total_tokens", "input_tokens_details.cached_tokens",
    "input_tokens_details.cache_write_tokens", "input_tokens_details.cache_write_tokens_details.ttl_30m_tokens",
    "output_tokens_details.reasoning_tokens"];
/// v0.5.4: where Responses breaks its cache writes down by TTL. A field there the codec has no price for (a 12-hour TTL)
/// is drift; the writes are the larger of `cache_write_tokens` and the breakdown's sum; what the breakdown does not
/// explain is drift too ([`UNEXPLAINED`]); every write but the 30-minute ones moves to `unknown_write` (dearest price,
/// once). Messages' `cache_creation.*` and a Chat gateway's breakdown go through the same check in `observe`.
const RESPONSES_SPLIT: &str = "input_tokens_details.cache_write_tokens_details.";
/// v0.5.4 (review 3): the drift name of the write tokens a Responses total has beyond its TTL breakdown.
const UNEXPLAINED: &str = "input_tokens_details.cache_write_tokens_details.(unexplained)";
/// v0.5.4 (review 4): the same for a Chat gateway's breakdown (n6) and for Messages' `cache_creation` split (S1).
const CHAT_SPLIT: &str = "prompt_tokens_details.cache_write_tokens_details.";
const CHAT_UNEXPLAINED: &str = "prompt_tokens_details.cache_write_tokens_details.(unexplained)";
const MESSAGES_UNEXPLAINED: &str = "cache_creation.(unexplained)";

/// v0.5.4 (reviews 3 and 4): a write `total` checked against its TTL breakdown: `known` (the parts with a price) plus
/// the drift fields under `prefix` (TTLs without one). The writes are the larger of the total and the breakdown; a part
/// of the total the breakdown does not explain is drift (`unexplained`). Returns (writes, writes − unknown,
/// unknown parts' count, unknown_write): the unknown TTLs and the unexplained part, billed once at the dearest price.
fn write_split(drift: &mut BTreeMap<String, u64>, total: u64, known: u64, prefix: &str, unexplained: &str) -> (u64, u64, u64, u64) {
    let theirs = drift.iter().filter(|(k, _)| k.starts_with(prefix) && k.as_str() != unexplained).fold(0u64, |s, (_, n)| s.saturating_add(*n));
    let explained = known.saturating_add(theirs);
    let residual = total.saturating_sub(explained);
    if residual > 0 {
        drift.insert(unexplained.into(), residual);
    } else {
        drift.remove(unexplained);
    }
    let writes = total.max(explained);
    let uw = theirs.saturating_add(residual).min(writes - known.min(writes));
    (writes, writes - uw, theirs, uw)
}
/// Unknown fields named in a record or an alert (all of them are counted).
const DRIFT_NAMES: usize = 8;

/// The positive integers of a usage object (nested objects included, three levels deep) at paths `known` lacks.
fn unknown_fields(u: &Value, known: &[&str], prefix: &str, depth: u32, out: &mut BTreeMap<String, u64>) {
    let Some(o) = u.as_object() else { return };
    for (k, v) in o {
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        if v.is_object() && depth < 3 {
            unknown_fields(v, known, &path, depth + 1, out);
        } else if let Some(n) = v.as_u64().filter(|n| *n > 0)
            && !known.contains(&path.as_str())
        {
            let name: String = path.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '?' }).collect();
            *out.entry(name).or_default() = n; // cumulative fields (Anthropic): the latest value, never a sum
        }
    }
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

pub struct Tap {
    family: Family,
    sse: bool,
    buf: Vec<u8>,
    overflow: bool,
    /// Anthropic's cumulative fields: input, cache read, cache write, output, and the cache write's
    /// `cache_creation` split, 5 minutes and 1 hour (overwritten, never summed).
    a: [Option<u64>; 6],
    pub usage: Option<Usage>,
    pub finish: Option<&'static str>,
    /// X5: the tool-call reader, on live forwards only (`with_calls`).
    calls: Option<Calls>,
    /// v0.5.3 L6: the stream reached the event that carries the final usage (Chat: a finish reason; Messages:
    /// `message_delta`; Responses: a finished response). A stream without it that has usage is `partial`.
    done: bool,
    /// v0.5.3 L6: usage fields the codec has no price for, by path, with their latest count.
    drift: BTreeMap<String, u64>,
    /// v0.5.4 (review 4): a Messages usage carried its `cache_creation` split.
    split: bool,
    /// v0.5.4 (review 4, n5): a non-streamed body was parsed (`settle`).
    settled: bool,
}

impl Tap {
    pub fn new(family: Family, sse: bool) -> Tap {
        Tap { family, sse, buf: Vec::new(), overflow: false, a: [None; 6], usage: None, finish: None, calls: None, done: false,
            drift: BTreeMap::new(), split: false, settled: false }
    }

    /// v0.5.3 L6: the usage fields the codec has no price for (at most `DRIFT_NAMES`), for the record and the alert.
    pub fn drift(&self) -> Vec<String> {
        self.drift.keys().take(DRIFT_NAMES).cloned().collect()
    }

    /// X5: also read the response's tool calls (ADR-011 §5.2); the live forwarder only, never sealed lanes.
    pub fn with_calls(mut self) -> Tap {
        self.calls = Some(Calls::new(self.family));
        self
    }

    /// Called with every forwarded slice; never changes it.
    pub fn feed(&mut self, b: &[u8]) {
        if self.family == Family::None || self.overflow {
            return;
        }
        let mut buf = std::mem::take(&mut self.buf);
        buf.extend_from_slice(b);
        if !self.sse {
            self.overflow = buf.len() > BODY_MAX;
            self.buf = if self.overflow { Vec::new() } else { buf };
            return;
        }
        let mut start = 0;
        while let Some(i) = buf[start..].iter().position(|&c| c == b'\n') {
            self.line(&buf[start..start + i]);
            start += i + 1;
        }
        buf.drain(..start);
        self.overflow = buf.len() > LINE_MAX;
        self.buf = if self.overflow { Vec::new() } else { buf };
    }

    fn line(&mut self, l: &[u8]) {
        let l = l.strip_suffix(b"\r").unwrap_or(l);
        let Some(d) = l.strip_prefix(b"data:") else { return };
        let (usage, calls) = (NEEDLES.iter().any(|n| has(d, n)), self.calls.as_ref().is_some_and(|c| c.wants(d)));
        if (usage || calls)
            && let Ok(v) = serde_json::from_slice::<Value>(d)
        {
            if usage {
                self.observe(&v);
            }
            if let Some(c) = self.calls.as_mut().filter(|_| calls) {
                c.event(&v);
            }
        }
    }

    /// v0.5.4 (review 4, n5): parse a non-streamed body now (once), so `drift` lists its fields before `finish_calls`.
    pub fn settle(&mut self) {
        if !self.settled && !self.sse && !self.overflow && self.family != Family::None
            && let Ok(v) = serde_json::from_slice::<Value>(&self.buf)
        {
            self.settled = true;
            self.observe(&v);
            self.done = true; // a whole body: its usage is the final one
            if let Some(c) = self.calls.as_mut() {
                c.body(&v);
            }
        }
    }

    /// The end of the stream: a non-streamed body is parsed once here.
    pub fn finish(self) -> (Option<Usage>, Option<&'static str>) {
        let (u, f, _) = self.finish_calls();
        (u, f)
    }

    /// `finish`, plus the cap ids of the response's tool calls with their uses (X5; empty without `with_calls`).
    pub fn finish_calls(mut self) -> (Option<Usage>, Option<&'static str>, BTreeMap<String, u32>) {
        self.settle();
        let unknown = self.drift.values().fold(0u64, |s, n| s.saturating_add(*n));
        let usage = self.usage.map(|u| Usage { unknown, partial: !self.done, ..u });
        (usage, self.finish, self.calls.map(|c| c.caps()).unwrap_or_default())
    }

    fn observe(&mut self, v: &Value) {
        let n = |u: &Value, k: &str| u.get(k).and_then(Value::as_u64);
        match self.family {
            Family::Chat => {
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    let hm = n(u, "prompt_cache_hit_tokens").zip(n(u, "prompt_cache_miss_tokens")).map(|(h, m)| h.saturating_add(m)); // review 4b: never wraps
                    let cache_read = n(u, "prompt_cache_hit_tokens").or_else(|| n(&u["prompt_tokens_details"], "cached_tokens")).unwrap_or(0);
                    // a gateway's cache writes (OpenRouter `prompt_tokens_details.cache_write_tokens`); DeepSeek sends none
                    let mut cache_write = n(&u["prompt_tokens_details"], "cache_write_tokens").unwrap_or(0);
                    if let Some(input) = n(u, "prompt_tokens").or(hm) {
                        unknown_fields(u, &KNOWN_CHAT, "", 0, &mut self.drift);
                        // v0.5.4 (review 4, n6): a gateway that forwards OpenAI's TTL breakdown, as for Responses
                        let mut unknown_write = 0;
                        if let Some(d) = u["prompt_tokens_details"]["cache_write_tokens_details"].as_object() {
                            let t30 = d.get("ttl_30m_tokens").and_then(Value::as_u64).unwrap_or(0);
                            let (writes, _, _, uw) = write_split(&mut self.drift, cache_write, t30, CHAT_SPLIT, CHAT_UNEXPLAINED);
                            (cache_write, unknown_write) = (writes - uw, uw);
                        }
                        self.usage = Some(Usage { input, cache_read, cache_write, unknown_write, output: n(u, "completion_tokens").unwrap_or(0),
                            ..Usage::default() });
                    }
                }
                if let Some(f) = v["choices"][0]["finish_reason"].as_str() {
                    self.done = true; // the usage chunk is this one (DeepSeek) or follows it (OpenAI)
                    self.finish = Some(match f {
                        "tool_calls" | "function_call" => "tool_use",
                        "stop" => "stop",
                        "length" => "length",
                        _ => "other",
                    });
                }
            }
            Family::Messages => {
                let t = v["type"].as_str().unwrap_or("");
                let u = if t == "message_start" { &v["message"]["usage"] } else { &v["usage"] };
                for (i, k) in ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"].iter().enumerate() {
                    if let Some(x) = n(u, k) {
                        self.a[i] = Some(x);
                    }
                }
                for (i, k) in [(4, "ephemeral_5m_input_tokens"), (5, "ephemeral_1h_input_tokens")] {
                    if let Some(x) = n(&u["cache_creation"], k) {
                        self.a[i] = Some(x);
                    }
                }
                if u.is_object() {
                    self.split |= u["cache_creation"].is_object();
                    unknown_fields(u, &KNOWN_MESSAGES, "", 0, &mut self.drift);
                    // `message_delta` carries the final, cumulative usage; `message_start`'s output is a placeholder
                    self.done |= t == "message_delta" || t == "message";
                }
                if let Some(i) = self.a[0] {
                    let (cache_read, output, created) = (self.a[1].unwrap_or(0), self.a[3].unwrap_or(0), self.a[2].unwrap_or(0));
                    // writes the response does not split by TTL: 5 minutes or 1 hour, at the dearer price (review 4, n1)
                    let (mut writes, mut w5, mut w1, mut uw, mut unsplit) = (created, 0, 0, 0, created);
                    if self.split {
                        // v0.5.4 (review 4): the total is checked against its split, as for Responses: the writes are the
                        // larger of the two, and a part of the total the split does not explain is an unknown write
                        let (c5, c1) = (self.a[4].unwrap_or(0), self.a[5].unwrap_or(0));
                        let (all, _, _, unknown) = write_split(&mut self.drift, created, c5.saturating_add(c1), "cache_creation.", MESSAGES_UNEXPLAINED);
                        (writes, w5, w1, uw, unsplit) = (all, c5, c1, unknown, 0);
                    }
                    self.usage = Some(Usage { input: i.saturating_add(cache_read).saturating_add(writes), cache_read, cache_write: w5,
                        cache_write_1h: w1, cache_write_unsplit: unsplit, unknown_write: uw, output, ..Usage::default() });
                }
                let stop = v["delta"]["stop_reason"].as_str().or(v["stop_reason"].as_str());
                self.finish = match (t, stop) {
                    ("error", _) => Some("error"),
                    (_, Some("tool_use")) => Some("tool_use"),
                    (_, Some("end_turn" | "stop_sequence")) => Some("stop"),
                    (_, Some("max_tokens")) => Some("length"),
                    (_, Some(_)) => Some("other"),
                    _ => self.finish,
                };
            }
            Family::Responses => {
                if v["type"] == "error" {
                    self.finish = Some("error");
                }
                let r = if v["object"] == "response" { v } else { &v["response"] };
                if let Some(u) = r.get("usage").filter(|u| u.is_object())
                    && let Some(input) = n(u, "input_tokens")
                {
                    let cache_read = n(&u["input_tokens_details"], "cached_tokens").unwrap_or(0);
                    let mut cache_write = n(&u["input_tokens_details"], "cache_write_tokens").unwrap_or(0);
                    unknown_fields(u, &KNOWN_RESPONSES, "", 0, &mut self.drift);
                    // v0.5.4 (review 3): the write total is checked against its TTL breakdown, never assumed to hold it.
                    // The writes are the larger of the two; what the breakdown does not explain is an unknown write.
                    let mut unknown_write = 0;
                    if let Some(d) = u["input_tokens_details"]["cache_write_tokens_details"].as_object() {
                        let t30 = d.get("ttl_30m_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let (writes, _, _, uw) = write_split(&mut self.drift, cache_write, t30, RESPONSES_SPLIT, UNEXPLAINED);
                        (cache_write, unknown_write) = (writes, uw);
                    }
                    self.usage = Some(Usage { input, cache_read, cache_write: cache_write - unknown_write, unknown_write,
                        output: n(u, "output_tokens").unwrap_or(0), ..Usage::default() });
                    // usage on a finished response is final; on any other event it is a running count
                    self.done = matches!(r["status"].as_str(), Some("completed" | "incomplete" | "failed" | "cancelled"));
                }
                let call = r["output"].as_array().into_iter().flatten().any(|o| matches!(o["type"].as_str(), Some("function_call" | "custom_tool_call")));
                self.finish = match r["status"].as_str() {
                    Some("completed") if call => Some("tool_use"),
                    Some("completed") => Some("stop"),
                    Some("incomplete") => Some("length"),
                    Some("failed") => Some("error"),
                    _ => self.finish,
                };
            }
            Family::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_tariff_usage_saturates_without_wrapping() {
        assert_eq!(musd(&[(u64::MAX, u64::MAX); 3]), u64::MAX);
        assert_eq!(musd(&[(1, 1), (1, 1)]), 1);
        assert_eq!(musd(&[]), 0);
    }

    const DEEPSEEK: &str = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"index\":0}]}\n\n\
        : keep-alive\r\n\r\n\
        data: {\"choices\":[{\"delta\":{\"content\":\"usage of tools\"},\"index\":0}]}\n\n\
        data: {\"choices\":[{\"delta\":{\"content\":\"\"},\"finish_reason\":\"tool_calls\",\"index\":0}],\"usage\": {\"completion_tokens\":50,\"prompt_cache_hit_tokens\":200,\"prompt_cache_miss_tokens\":800,\"prompt_tokens\":1000}}\n\n\
        data: [DONE]\n\n";
    const ANTHROPIC: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":300,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n\
        event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
        event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n\
        event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    const RESPONSES: &str = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n\
        event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\"}],\"usage\":{\"input_tokens\":900,\"output_tokens\":70}}}\n\n";

    fn run(f: Family, sse: bool, s: &str, split: usize) -> (Option<Usage>, Option<&'static str>) {
        let mut t = Tap::new(f, sse);
        for c in s.as_bytes().chunks(split.max(1)) {
            t.feed(c);
        }
        t.finish()
    }

    #[test]
    fn families_at_every_split() {
        for split in [1, 2, 3, 7, 64, 10_000] {
            assert_eq!(run(Family::Chat, true, DEEPSEEK, split), (Some(u(1000, 200, 50)), Some("tool_use")), "{split}");
            // 5 written tokens with no TTL split: their own class, at the dearer write price (v0.5.4, review 4 n1)
            assert_eq!(run(Family::Messages, true, ANTHROPIC, split), (Some(Usage { cache_write_unsplit: 5, ..u(315, 300, 42) }), Some("stop")), "{split}");
            assert_eq!(run(Family::Responses, true, RESPONSES, split), (Some(u(900, 0, 70)), Some("tool_use")), "{split}");
        }
        let json = r#"{"choices":[{"finish_reason":"length"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#;
        assert_eq!(run(Family::Chat, false, json, 5), (Some(u(12, 0, 3)), Some("length")));
        let openai = r#"{"usage":{"prompt_tokens":12,"prompt_tokens_details":{"cached_tokens":8},"completion_tokens":3}}"#;
        assert_eq!(run(Family::Chat, false, openai, 5).0, Some(u(12, 8, 3)));
        let resp = r#"{"object":"response","usage":{"input_tokens":9,"input_tokens_details":{"cached_tokens":4},"output_tokens":1}}"#;
        assert_eq!(run(Family::Responses, false, resp, 5).0, Some(u(9, 4, 1)));
        let msg = r#"{"type":"message","stop_reason":"max_tokens","usage":{"input_tokens":4,"output_tokens":9}}"#;
        assert_eq!(run(Family::Messages, false, msg, 4), (Some(u(4, 0, 9)), Some("length")));
        assert_eq!(run(Family::None, false, json, 5), (None, None));
    }

    #[test]
    fn cut_streams_and_floods_have_no_usage() {
        let cut = &DEEPSEEK[..DEEPSEEK.find("data: {\"choices\":[{\"delta\":{\"content\":\"\"},\"finish").unwrap()];
        assert_eq!(run(Family::Chat, true, cut, 9), (None, None));
        let mut t = Tap::new(Family::Chat, true);
        t.feed(&vec![b'x'; LINE_MAX + 1]);
        t.feed(DEEPSEEK.as_bytes());
        assert_eq!(t.finish(), (None, None), "an over-long line stops parsing");
        // a usage-looking line the kernel cannot have injected: the tap reads only upstream bytes
        assert_eq!(run(Family::Chat, true, "data: {\"usage\":{\"prompt_tokens\":\"many\"}}\n", 3), (None, None));
    }

    fn u(input: u64, cache_read: u64, output: u64) -> Usage {
        Usage { input, cache_read, output, ..Usage::default() }
    }

    /// One DeepSeek stream's final usage line (prompt = hit + miss), as the S1 run's provider sent it.
    fn ds(hit: u64, miss: u64, out: u64) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\",\"index\":0}}],\"usage\":{{\"completion_tokens\":{out},\
            \"prompt_cache_hit_tokens\":{hit},\"prompt_cache_miss_tokens\":{miss},\"prompt_tokens\":{}}}}}\n\ndata: [DONE]\n\n", hit + miss)
    }

    /// build-deviations §3.13 on S1's real usage (build/status/S1.md): turn 1 had 0 hit / 4,474 miss and 81
    /// output tokens, turn 2 4,480 hit / 190 miss and 2 output. Billing hits at the input price gave the Root's
    /// 2,844 µUSD against the proxy's 762 (off-peak, cache-read priced): 3.7×. At the cache-read price, still
    /// at peak, the Root bills 1,527: 2.0×, which is the peak/off-peak factor the Root keeps on purpose (an
    /// upper bound). A model without a cache-read price keeps the old bill.
    #[test]
    fn s1_usage_at_the_cache_read_price() {
        let flash = Model { min_level: 0, input: 300_000, cache_read: 6_000, cache_write: 600_000, cache_write_1h: 600_000, output: 1_200_000, reserve_out: 65_536 };
        let old = Model { cache_read: flash.input, ..flash.clone() };
        let turns = [run(Family::Chat, true, &ds(0, 4474, 81), 7).0.unwrap(), run(Family::Chat, true, &ds(4480, 190, 2), 7).0.unwrap()];
        assert_eq!(turns[1], u(4670, 4480, 2));
        let bill = |m: &Model| turns.iter().map(|t| t.cost(m)).collect::<Vec<u64>>();
        assert_eq!(bill(&old), [1440, 1404], "the S1 Root bill: 2,844 µUSD");
        assert_eq!(bill(&flash), [1440, 87], "turn 2: 190 × 0.30 + 4,480 × 0.006 + 2 × 1.20 = 86.28, rounded up");
        let (root, proxy) = (bill(&flash).iter().sum::<u64>(), 762u64);
        assert_eq!(root, 1527);
        assert!(root >= proxy, "the Root stays an upper bound of the proxy's figure");
        assert_eq!(root * 100 / proxy, 200, "Root/proxy 2.00× (was 2,844 / 762 = 3.73×)");
        // a provider that reports more cache reads than input cannot make the bill go negative
        assert_eq!(u(10, 99, 0).cost(&flash), 1);
    }

    /// v0.5.0 L3: cache writes by family and TTL, and their price.
    #[test]
    fn cache_writes_by_family_and_ttl() {
        let w = |f: Family, s: &str| run(f, true, s, 5).0.unwrap();
        // Anthropic with the TTL split: the 5-minute and the 1-hour writes apart
        let split = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":4,\"cache_read_input_tokens\":100,\
            \"cache_creation_input_tokens\":30,\"cache_creation\":{\"ephemeral_5m_input_tokens\":20,\"ephemeral_1h_input_tokens\":10},\
            \"output_tokens\":1}}}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n";
        assert_eq!(w(Family::Messages, split), Usage { input: 134, cache_read: 100, cache_write: 20, cache_write_1h: 10, output: 9, ..Usage::default() });
        // OpenAI Responses, and a Chat gateway's single write class (OpenRouter)
        let resp = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":50,\
            \"input_tokens_details\":{\"cached_tokens\":20,\"cache_write_tokens\":25},\"output_tokens\":3}}}\n\n";
        assert_eq!(w(Family::Responses, resp), Usage { input: 50, cache_read: 20, cache_write: 25, output: 3, ..Usage::default() });
        let chat = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":50,\
            \"prompt_tokens_details\":{\"cached_tokens\":20,\"cache_write_tokens\":25},\"completion_tokens\":3}}\n\n";
        assert_eq!(w(Family::Chat, chat), Usage { input: 50, cache_read: 20, cache_write: 25, output: 3, ..Usage::default() });
        // each class at its own price; counts beyond the input are clamped to it and can only raise the bill
        let m = Model { min_level: 0, input: 1_000_000, cache_read: 100_000, cache_write: 1_250_000, cache_write_1h: 2_000_000,
            output: 5_000_000, reserve_out: 1 };
        assert_eq!(w(Family::Messages, split).cost(&m), musd(&[(4, 1_000_000), (100, 100_000), (20, 1_250_000), (10, 2_000_000), (9, 5_000_000)]));
        let inflated = Usage { input: 10, cache_read: 4, cache_write: 50, cache_write_1h: 50, output: 0, ..Usage::default() };
        assert_eq!(inflated.cost(&m), musd(&[(4, 100_000), (6, 2_000_000)]), "the 4 reads, then the 6 tokens left at the dearest write class");
        assert_eq!(w(Family::Responses, resp).json(), json!({"cache_read": 20, "cache_write": 25, "input": 50, "output": 3}));
    }

    /// v0.5.3 L6: a positive count in a usage field the codec has no price for is an unknown class, by path; totals,
    /// reasoning and other subsets of a priced field are not, nor are zeros, strings or floats.
    #[test]
    fn usage_fields_without_a_price_are_unknown_classes() {
        let tap = |f: Family, s: &str| {
            let mut t = Tap::new(f, true);
            t.feed(s.as_bytes());
            let d = t.drift();
            (t.finish().0.unwrap(), d)
        };
        // DeepSeek's real fields (seed/src/provider/testdata/deepseek/real-text-cache-hit.sse) are all known
        let ds = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4002,\"completion_tokens\":5,\"total_tokens\":4007,\
            \"prompt_tokens_details\":{\"cached_tokens\":3840},\"completion_tokens_details\":{\"reasoning_tokens\":2},\"prompt_cache_hit_tokens\":3840,\
            \"prompt_cache_miss_tokens\":162}}\n\n";
        assert_eq!(tap(Family::Chat, ds), (Usage { input: 4002, cache_read: 3840, output: 5, ..Usage::default() }, vec![]));
        // OpenAI Chat: audio and a gateway's float `cost` and zero counts; predicted-output tokens are inside the completion
        let oc = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":9,\"total_tokens\":59,\"cost\":0.0012,\
            \"is_byok\":false,\"prompt_tokens_details\":{\"cached_tokens\":0,\"audio_tokens\":30},\"completion_tokens_details\":{\"reasoning_tokens\":0,\
            \"accepted_prediction_tokens\":3,\"rejected_prediction_tokens\":2,\"audio_tokens\":0},\"cost_details\":{\"upstream_inference_cost\":null}}}\n\n";
        assert_eq!(tap(Family::Chat, oc), (Usage { input: 50, output: 9, unknown: 30, ..Usage::default() }, vec!["prompt_tokens_details.audio_tokens".to_string()]));
        // Anthropic: a 12-hour TTL in the cache_creation split, counted once though both events carry it. v0.5.4: it is part
        // of cache_creation_input_tokens, so it leaves the 1-hour class (it was billed there and again at the dearest price)
        let an = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_read_input_tokens\":0,\
            \"cache_creation_input_tokens\":700,\"cache_creation\":{\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":0,\
            \"ephemeral_12h_input_tokens\":700},\"output_tokens\":1,\"service_tier\":\"standard\"}}}\n\n\
            data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12,\
            \"cache_creation\":{\"ephemeral_12h_input_tokens\":700},\"server_tool_use\":{\"web_search_requests\":0}}}\n\n";
        assert_eq!(tap(Family::Messages, an), (Usage { input: 703, output: 12, unknown: 700, unknown_write: 700, ..Usage::default() },
            vec!["cache_creation.ephemeral_12h_input_tokens".to_string()]));
        // OpenAI Responses: a TTL class next to the known details; reasoning is inside output_tokens
        let rs = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":900,\
            \"input_tokens_details\":{\"cached_tokens\":0,\"cache_write_tokens\":0,\"ttl_12h_tokens\":800},\"output_tokens\":70,\
            \"output_tokens_details\":{\"reasoning_tokens\":64},\"total_tokens\":970}}}\n\n";
        assert_eq!(tap(Family::Responses, rs), (Usage { input: 900, output: 70, unknown: 800, ..Usage::default() },
            vec!["input_tokens_details.ttl_12h_tokens".to_string()]));
        assert_eq!(tap(Family::Responses, rs).0.json()["unknown"], 800, "the ledger keeps the count");
    }

    /// v0.5.4 (the live OpenAI qualify of 2026-10-30: the ledger +302 %): OpenAI's Responses usage breaks its cache writes
    /// down by TTL under `input_tokens_details.cache_write_tokens_details` (the 30-minute default since 2026-10-20). The
    /// 30-minute part is the ordinary write: no drift, billed once at the write price. Another TTL there (12 hours) is a
    /// class without a price: drift, at the dearest price, and out of `cache_write`, so it is billed once, not twice.
    #[test]
    fn a_write_breakdown_is_billed_once() {
        use super::super::money::book;
        let tap = |s: &str| {
            let mut t = Tap::new(Family::Responses, true);
            t.feed(s.as_bytes());
            let d = t.drift();
            (t.finish().0.unwrap(), d)
        };
        let resp = |details: &str| format!("data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"usage\":{{\"input_tokens\":6049,\
            \"input_tokens_details\":{{\"cached_tokens\":0,\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{{{details}}}}},\"output_tokens\":83,\
            \"output_tokens_details\":{{\"reasoning_tokens\":0}},\"total_tokens\":6132}}}}}}\n\n");
        // gpt-6.1-sol as its catalog row compiles: input 2.00, cache read 0.10, writes 2.50, output 10.00 per 1M tokens
        let sol = Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 2_500_000, output: 10_000_000,
            reserve_out: 32_768 };
        // request 1 of the live qualify: the whole write at 30 minutes. v0.5.3 billed 75,939 µUSD (6,001 tokens again at
        // the output price); its list price is 48 × 2.00 + 6,001 × 2.50 + 83 × 10.00 = 15,928.5 µUSD
        let (u, drift) = tap(&resp("\"ttl_30m_tokens\":6001"));
        assert_eq!((u, drift), (Usage { input: 6049, cache_write: 6001, output: 83, ..Usage::default() }, vec![]));
        assert_eq!(book(Some(&u), &sol, u64::MAX, 32_768, true), (15_929, vec![]));
        // 30 minutes and 12 hours: the 12-hour part is drift, at the dearest price once; the 30-minute part at the write price
        let (u, drift) = tap(&resp("\"ttl_30m_tokens\":4000,\"ttl_12h_tokens\":2001"));
        assert_eq!(drift, vec!["input_tokens_details.cache_write_tokens_details.ttl_12h_tokens".to_string()]);
        assert_eq!(u, Usage { input: 6049, cache_write: 4000, unknown: 2001, unknown_write: 2001, output: 83, ..Usage::default() });
        let (bill, flags) = book(Some(&u), &sol, u64::MAX, 32_768, true);
        assert_eq!((bill, flags), (musd(&[(48, 2_000_000), (4000, 2_500_000), (2001, 10_000_000), (83, 10_000_000)]), vec!["price_drift"]));
        assert_eq!(bill, 30_936);
        // never under the list price, with the 12-hour write at OpenAI's 2.5× the input (5.00): 20,930.5 µUSD
        assert!(bill >= musd(&[(48, 2_000_000), (4000, 2_500_000), (2001, 5_000_000), (83, 10_000_000)]));
        // capped by a small hold, the floor still prices the 12-hour part as a write (1-hour class), never at 0
        assert_eq!(book(Some(&u), &sol, 1, 32_768, true).0, musd(&[(48, 2_000_000), (4000, 2_500_000), (2001, 2_500_000), (83, 10_000_000)]));
        assert_eq!(u.json()["unknown_write"], 2001, "the ledger keeps the split");
    }

    /// v0.5.4 (review 3 of the meter fix): a write total is checked against its TTL breakdown. Inconsistent shapes never
    /// bill under the list price with the unknown part at the dearest price; consistent ones keep their numbers.
    #[test]
    fn a_write_total_is_checked_against_its_breakdown() {
        use super::super::money::book;
        let sol = Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 2_500_000, output: 10_000_000,
            reserve_out: 32_768 };
        let son = Model { cache_write_1h: 4_000_000, ..sol.clone() };
        let bill = |f: Family, s: &str, m: &Model| {
            let mut t = Tap::new(f, true);
            t.feed(s.as_bytes());
            let d = t.drift();
            let u = t.finish().0.unwrap();
            (book(Some(&u), m, u64::MAX, 32_768, true).0, d)
        };
        let resp = |det: &str| format!("data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"usage\":{{\"input_tokens\":6049,\
            \"input_tokens_details\":{{\"cached_tokens\":0{det}}},\"output_tokens\":83}}}}}}\n\n");
        let list = musd(&[(48, 2_000_000), (6001, 2_500_000), (83, 10_000_000)]);
        assert_eq!(list, 15_929);
        // 1a: the total missing, the 30-minute breakdown present (v0.5.4 before review 3: 12,928)
        assert_eq!(bill(Family::Responses, &resp(",\"cache_write_tokens_details\":{\"ttl_30m_tokens\":6001}"), &sol), (15_929, vec![]));
        // 1a': a total smaller than its 30-minute part (was 14,928)
        assert_eq!(bill(Family::Responses, &resp(",\"cache_write_tokens\":4000,\"cache_write_tokens_details\":{\"ttl_30m_tokens\":6001}"), &sol).0, 15_929);
        // 1b: a total larger than its breakdown: the 2,001 unexplained are drift at the dearest price (was 15,929, no alert)
        let (b, d) = bill(Family::Responses, &resp(",\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{\"ttl_30m_tokens\":4000}"), &sol);
        assert_eq!((b, d), (musd(&[(48, 2_000_000), (4000, 2_500_000), (2001, 10_000_000), (83, 10_000_000)]), vec![UNEXPLAINED.to_string()]));
        assert_eq!(b, 30_936);
        assert!(b >= musd(&[(48, 2_000_000), (4000, 2_500_000), (2001, 5_000_000), (83, 10_000_000)]), "over a 12-hour list price (20,931)");
        // the consistent shapes keep their numbers
        assert_eq!(bill(Family::Responses, &resp(",\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{\"ttl_30m_tokens\":6001}"), &sol).0, 15_929);
        assert_eq!(bill(Family::Responses, &resp(",\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{\"ttl_30m_tokens\":4000,\"ttl_12h_tokens\":2001}"), &sol).0,
            30_936);
        // 2: Messages, a 12-hour TTL the total does not hold beside its 1-hour part: on top, as in v0.5.3 (was 7,126)
        let msg = |cc: &str| format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":3,\"cache_read_input_tokens\":0,\
            \"cache_creation_input_tokens\":700,\"cache_creation\":{{{cc}}},\"output_tokens\":1}}}}}}\n\ndata: {{\"type\":\"message_delta\",\
            \"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":12}}}}\n\n");
        let over = msg("\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":700,\"ephemeral_12h_input_tokens\":700");
        assert_eq!(bill(Family::Messages, &over, &son).0, musd(&[(3, 2_000_000), (700, 4_000_000), (700, 10_000_000), (12, 10_000_000)]));
        assert_eq!(bill(Family::Messages, &over, &son).0, 9_926);
        // inside the total: moved out of the 1-hour class, billed once
        let inside = msg("\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":0,\"ephemeral_12h_input_tokens\":700");
        assert_eq!(bill(Family::Messages, &inside, &son).0, musd(&[(3, 2_000_000), (700, 10_000_000), (12, 10_000_000)]));
        // 3: the floor (a hold of 1 µUSD) prices an unknown write at the dearer write price, also when a charter puts the
        // 1-hour price under the 5-minute one
        let cheap_1h = Model { cache_write_1h: 1_000_000, ..sol.clone() };
        let mut t = Tap::new(Family::Responses, true);
        t.feed(resp(",\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{\"ttl_12h_tokens\":6001}").as_bytes());
        let u = t.finish().0.unwrap();
        assert_eq!(book(Some(&u), &cheap_1h, 1, 32_768, true).0, 15_929, "6,001 at the 5-minute price (2.50), not 1.00");
    }

    /// v0.5.4 (review 3): write totals, 30-minute parts and unknown TTLs drawn independently (a total absent, smaller or
    /// larger than its breakdown; Messages TTLs outside `created`): the bill is never under the list price with the
    /// unknown part at the dearest price, bounded by the hold as `book` is. Review 4 n1: a Messages write with no split, at
    /// the dearer write price exactly.
    #[test]
    fn write_breakdowns_never_under_count() {
        use super::super::money::{book, dearest};
        let mut x: u64 = 0x5EED_0054_C0DE_0003;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            match x % 6 { 0 => 0, 1 => n, _ => x % (n + 1) }
        };
        let models = [Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 4_000_000, output: 10_000_000, reserve_out: 1 },
            Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 1_000_000, output: 10_000_000, reserve_out: 1 }];
        for m in &models {
            let d = dearest(m);
            for _ in 0..4000 {
                let (w, t30, t12, cr, out) = (next(5000), next(5000), next(5000), next(5000), next(500));
                let hold = if next(1) == 0 { u64::MAX } else { 1 + next(200_000) };
                let absent = next(1) == 0;
                // Responses: the writes are the larger of the total and the breakdown; all but the 30-minute part unknown
                let writes = if absent { t30 + t12 } else { w.max(t30 + t12) };
                let input = cr + writes + next(5000);
                let total = if absent { String::new() } else { format!(",\"cache_write_tokens\":{w}") };
                let s = format!("data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"usage\":{{\"input_tokens\":{input},\
                    \"input_tokens_details\":{{\"cached_tokens\":{cr}{total},\"cache_write_tokens_details\":{{\"ttl_30m_tokens\":{t30},\"ttl_12h_tokens\":{t12}}}}},\
                    \"output_tokens\":{out}}}}}}}\n\n");
                let mut t = Tap::new(Family::Responses, true);
                t.feed(s.as_bytes());
                let u = t.finish().0.unwrap();
                let list = musd(&[(input - cr - writes, m.input), (cr, m.cache_read), (t30, m.cache_write), (writes - t30, d), (out, m.output)]);
                let b = book(Some(&u), m, hold, 32_768, true).0;
                assert!(b >= list.min(hold), "Responses {s}: {u:?} billed {b} under {list} (hold {hold})");
                // Messages: 5 minutes, 1 hour and a 12-hour TTL beside a total that may not hold them
                let (c5, c1, created) = (next(3000), next(3000), next(6000));
                let input_tokens = next(3000);
                let s = format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":{input_tokens},\"cache_read_input_tokens\":{cr},\
                    \"cache_creation_input_tokens\":{created},\"cache_creation\":{{\"ephemeral_5m_input_tokens\":{c5},\"ephemeral_1h_input_tokens\":{c1},\
                    \"ephemeral_12h_input_tokens\":{t12}}},\"output_tokens\":1}}}}}}\n\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\
                    \"usage\":{{\"output_tokens\":{out}}}}}\n\n");
                let mut t = Tap::new(Family::Messages, true);
                t.feed(s.as_bytes());
                let u = t.finish().0.unwrap();
                // the reference (review 4): the reported 5-minute and 1-hour parts at their prices, in full; the writes are the
                // larger of the total and the split; every write but those two parts (the 12-hour TTL, a part of the
                // total the split does not explain) at the dearest price
                let writes = created.max(c5 + c1 + t12);
                let list = musd(&[(input_tokens, m.input), (cr, m.cache_read), (c5, m.cache_write), (c1, m.cache_write_1h), (writes - c5 - c1, d), (out, m.output)]);
                let b = book(Some(&u), m, hold, 32_768, true).0;
                assert!(b >= list.min(hold), "Messages {s}: {u:?} billed {b} under {list} (hold {hold})");
                // v0.5.4 (review 4, n1): a write with no split at all, at the dearer write price on both charters, with and
                // without a class without a price beside it (Anthropic's `server_tool_use` count)
                let (created, beside, wd) = (next(6000), if next(1) == 0 { 0 } else { next(50) }, m.cache_write.max(m.cache_write_1h));
                let s = format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":{input_tokens},\"cache_read_input_tokens\":{cr},\
                    \"cache_creation_input_tokens\":{created},\"output_tokens\":1}}}}}}\n\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\
                    \"usage\":{{\"output_tokens\":{out},\"server_tool_use\":{{\"web_search_requests\":{beside}}}}}}}\n\n");
                let mut t = Tap::new(Family::Messages, true);
                t.feed(s.as_bytes());
                let u = t.finish().0.unwrap();
                let list = musd(&[(input_tokens, m.input), (cr, m.cache_read), (created, wd), (out, m.output)]);
                let at_dearest = musd(&[(input_tokens, m.input), (cr, m.cache_read), (created, wd), (out, m.output), (beside, d)]);
                let b = book(Some(&u), m, hold, 32_768, true).0;
                assert_eq!(b, if beside == 0 { list } else { list.max(at_dearest.min(hold)) }, "Messages unsplit {s}: {u:?} (hold {hold})");
            }
        }
    }

    /// v0.5.4 (review 4 of the meter fix): Messages' total is checked against its `cache_creation` split, as Responses'
    /// is (S1); a Chat gateway's TTL breakdown is read as Responses' is (n6); a non-streamed body's drift is known
    /// before `finish_calls` (n5). Prices: claude-sonnet-5-5 (2.00, 2.50 for 5 min, 4.00 for 1 h, 10.00 out).
    #[test]
    fn messages_and_chat_check_their_write_totals() {
        use super::super::money::book;
        let son = Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 4_000_000, output: 10_000_000,
            reserve_out: 32_768 };
        let bill = |f: Family, s: &str, m: &Model| {
            let mut t = Tap::new(f, true);
            t.feed(s.as_bytes());
            let d = t.drift();
            let u = t.finish().0.unwrap();
            let (b, flags) = book(Some(&u), m, u64::MAX, 32_768, true);
            (b, flags, d)
        };
        let msg = |created: u64, cc: &str| format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":3,\
            \"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":{created},\"cache_creation\":{{{cc}}},\"output_tokens\":1}}}}}}\n\n\
            data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":12}}}}\n\n");
        // (a) a part of the total the split does not explain: drift at the dearest price, as Responses' "(unexplained)"
        let (b, f, d) = bill(Family::Messages, &msg(700, "\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":0"), &son);
        assert_eq!((b, f, d), (7_126, vec!["price_drift"], vec![MESSAGES_UNEXPLAINED.to_string()]), "was 2,926 with no alert");
        assert_eq!(bill(Family::Messages, &msg(700, "\"ephemeral_5m_input_tokens\":300,\"ephemeral_1h_input_tokens\":0"), &son).0, 4_876, "was 2,476");
        // (b) parts larger than the total: the larger of the two, as for Responses
        assert_eq!(bill(Family::Messages, &msg(700, "\"ephemeral_5m_input_tokens\":0,\"ephemeral_1h_input_tokens\":1000"), &son).0, 4_126, "was 2,926");
        assert_eq!(bill(Family::Messages, &msg(700, "\"ephemeral_5m_input_tokens\":1000,\"ephemeral_1h_input_tokens\":0"), &son).0, 2_626, "was 1,876");
        assert_eq!(bill(Family::Messages, &msg(0, "\"ephemeral_5m_input_tokens\":500,\"ephemeral_1h_input_tokens\":0"), &son).0, 1_376, "was 126");
        // consistent splits are unchanged, and a write with no split at all stays at the 1-hour price (the dearer here), with
        // no alert
        assert_eq!(bill(Family::Messages, &msg(700, "\"ephemeral_5m_input_tokens\":300,\"ephemeral_1h_input_tokens\":400"), &son),
            (musd(&[(3, 2_000_000), (300, 2_500_000), (400, 4_000_000), (12, 10_000_000)]), vec![], vec![]));
        assert_eq!(bill(Family::Messages, ANTHROPIC, &son).0, musd(&[(10, 2_000_000), (300, 100_000), (5, 4_000_000), (42, 10_000_000)]));
        // n6: a Chat gateway forwarding OpenAI's breakdown, the live qualify's request 1 (v0.5.3 and before: 75,939)
        let sol = Model { cache_write_1h: 2_500_000, ..son.clone() };
        let chat = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":6049,\"completion_tokens\":83,\
            \"prompt_tokens_details\":{\"cached_tokens\":0,\"cache_write_tokens\":6001,\"cache_write_tokens_details\":{\"ttl_30m_tokens\":6001}}}}\n\n";
        assert_eq!(bill(Family::Chat, chat, &sol), (15_929, vec![], vec![]));
        let chat12 = chat.replace("{\"ttl_30m_tokens\":6001}", "{\"ttl_30m_tokens\":4000,\"ttl_12h_tokens\":2001}");
        assert_eq!(bill(Family::Chat, &chat12, &sol).0, 30_936);
        // n5: a whole body's drift fields are listed once it is settled, before `finish_calls`
        let mut t = Tap::new(Family::Responses, false);
        t.feed(br#"{"object":"response","status":"completed","usage":{"input_tokens":900,"input_tokens_details":{"ttl_12h_tokens":800},"output_tokens":7}}"#);
        assert!(t.drift().is_empty(), "nothing is parsed before the body ends");
        t.settle();
        assert_eq!(t.drift(), vec!["input_tokens_details.ttl_12h_tokens".to_string()]);
        assert_eq!(t.finish().0.unwrap().unknown, 800, "settled once: the usage is the same");
    }

    /// v0.5.4 (review 4, n1): a Messages write with no `cache_creation` split has a lifetime of 5 minutes or 1 hour, so it
    /// bills at the dearer of the two write prices: never under the list price, on a charter that prices 1 hour under 5
    /// minutes too. Not drift (no alert). The streams: the seed's write fixture (anthropic/cache-write.sse, the shape the
    /// providers tests bill at the provider's price; its `message_delta` already has no split) without the split in
    /// `message_start`, and the review's 700-token write.
    #[test]
    fn an_unsplit_write_bills_at_the_dearer_write_price() {
        use super::super::money::book;
        let son = Model { min_level: 0, input: 2_000_000, cache_read: 100_000, cache_write: 2_500_000, cache_write_1h: 4_000_000, output: 10_000_000,
            reserve_out: 32_768 };
        let inverted = Model { cache_write_1h: 1_000_000, ..son.clone() };
        let bill = |s: &str, m: &Model| {
            let mut t = Tap::new(Family::Messages, true);
            for c in s.as_bytes().chunks(13) {
                t.feed(c);
            }
            let d = t.drift();
            let u = t.finish().0.unwrap();
            let (b, flags) = book(Some(&u), m, u64::MAX, 32_768, true);
            (u, b, flags, d)
        };
        let seed = include_str!("../../../../../seed/src/provider/testdata/anthropic/cache-write.sse");
        let split = "\"cache_creation\":{\"ephemeral_1h_input_tokens\":0,\"ephemeral_5m_input_tokens\":21504},";
        assert_eq!(seed.matches(split).count(), 1);
        let unsplit = seed.replace(split, "");
        let (u, b, flags, d) = bill(&unsplit, &inverted);
        assert_eq!(u, Usage { input: 21_507, cache_write_unsplit: 21_504, output: 87, ..Usage::default() });
        // 3 × 2.00 + 21,504 × 2.50 + 87 × 10.00: the provider's 5-minute bill (providers::tests); was 22,380 (21,504 × 1.00)
        assert_eq!((b, flags, d), (54_636, vec![], vec![]));
        assert_eq!(bill(&unsplit, &son).1, musd(&[(3, 2_000_000), (21_504, 4_000_000), (87, 10_000_000)]), "a normal charter: the 1-hour price");
        assert_eq!(bill(&unsplit, &son).1, 86_892, "unchanged");
        // the fixture with its split keeps its 5-minute bill on both charters (the split arrives first and holds)
        assert_eq!(bill(seed, &inverted).0, Usage { input: 21_507, cache_write: 21_504, output: 87, ..Usage::default() });
        assert_eq!((bill(seed, &inverted).1, bill(seed, &son).1), (54_636, 54_636));
        // the review's shape: 700 written, no split
        let msg = |extra: &str| format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":3,\"cache_read_input_tokens\":0,\
            \"cache_creation_input_tokens\":700,\"output_tokens\":1}}}}}}\n\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\
            \"usage\":{{\"output_tokens\":12{extra}}}}}\n\n");
        let (u, b, flags, _) = bill(&msg(""), &inverted);
        assert_eq!((b, flags), (1_876, vec![]), "700 × 2.50, was 826 (700 × 1.00)");
        assert_eq!(u.json(), json!({"cache_read": 0, "cache_write_unsplit": 700, "input": 703, "output": 12}), "the ledger says what was reported");
        assert_eq!(bill(&msg(""), &son).1, 2_926, "a normal charter: unchanged");
        // with a class without a price beside it, the dearest-price bill keeps the write at the dearer write price too
        let (_, b, flags, d) = bill(&msg(",\"server_tool_use\":{\"web_search_requests\":2}"), &inverted);
        assert_eq!((b, flags, d), (1_896, vec!["price_drift"], vec!["server_tool_use.web_search_requests".to_string()]), "1,876 + 2 × 10.00");
        // a hold too small for the drift: the floor still bills the write at 2.50
        let mut t = Tap::new(Family::Messages, true);
        t.feed(msg(",\"server_tool_use\":{\"web_search_requests\":2}").as_bytes());
        assert_eq!(book(t.finish().0.as_ref(), &inverted, 1, 32_768, true).0, 1_876);
    }

    /// v0.5.4 (review 4b): hit + miss saturates; a wrapped sum billed 51 µUSD for an absurd count.
    #[test]
    fn a_huge_hit_and_miss_sum_saturates() {
        let s = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_cache_hit_tokens\":18446744073709551615,\
            \"prompt_cache_miss_tokens\":2,\"completion_tokens\":5}}\n\n";
        let u = run(Family::Chat, true, s, 7).0.unwrap();
        assert_eq!(u.input, u64::MAX);
        let flash = Model { min_level: 0, input: 300_000, cache_read: 6_000, cache_write: 600_000, cache_write_1h: 600_000, output: 1_200_000, reserve_out: 1 };
        assert!(u.cost(&flash) > 1_000_000_000, "{}", u.cost(&flash));
    }

    /// v0.5.3 L6: usage seen before the event that carries the final count is partial: an Anthropic stream cut after
    /// `message_start` (whose `output_tokens` is a placeholder), a Chat stream with running usage cut before its finish
    /// reason, a Responses usage on an unfinished response. Complete streams and whole bodies are not.
    #[test]
    fn usage_before_the_final_event_is_partial() {
        let start = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":300,\
            \"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\n";
        assert_eq!(run(Family::Messages, true, start, 7).0, Some(Usage { cache_write_unsplit: 5, partial: true, ..u(315, 300, 1) }));
        assert_eq!(run(Family::Messages, true, ANTHROPIC, 7).0.map(|u| u.partial), Some(false));
        let error = format!("{start}data: {{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\"}}}}\n\n");
        assert_eq!(run(Family::Messages, true, &error, 7), (Some(Usage { cache_write_unsplit: 5, partial: true, ..u(315, 300, 1) }), Some("error")));
        // a server that sends usage on every chunk (vLLM's continuous usage), cut before the finish reason
        let running = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}],\"usage\":{\"prompt_tokens\":40,\"completion_tokens\":3}}\n\n";
        assert_eq!(run(Family::Chat, true, running, 5).0, Some(Usage { partial: true, ..u(40, 0, 3) }));
        assert_eq!(run(Family::Chat, true, DEEPSEEK, 5).0.map(|u| u.partial), Some(false));
        let early = "data: {\"type\":\"response.in_progress\",\"response\":{\"status\":\"in_progress\",\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n";
        assert_eq!(run(Family::Responses, true, early, 5).0.map(|u| u.partial), Some(true));
        assert_eq!(run(Family::Responses, true, RESPONSES, 5).0.map(|u| u.partial), Some(false));
        let body = r#"{"type":"message","stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":9}}"#;
        assert_eq!(run(Family::Messages, false, body, 3).0.map(|u| u.partial), Some(false), "a whole body is final");
        assert_eq!(Usage { partial: true, ..u(1, 0, 1) }.json()["partial"], true);
    }

    #[test]
    fn integer_peak_prices_round_up() {
        assert_eq!(musd(&[(1000, 300_000), (50, 1_200_000)]), 360);
        assert_eq!(musd(&[(1, 300_000)]), 1);
        assert_eq!(musd(&[(0, 300_000)]), 0);
        assert_eq!(musd(&[(u64::MAX, u64::MAX)]), u64::MAX);
        assert_eq!(Family::of("anthropic/v1/messages"), Family::Messages);
        assert_eq!(Family::of("v1/messages/count_tokens"), Family::None);
        assert_eq!(Family::of("v1/responses"), Family::Responses);
        assert_eq!(Family::of("user/balance"), Family::None);
    }
}
