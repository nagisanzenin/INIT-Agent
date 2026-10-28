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
/// split by TTL; OpenAI `cache_write_tokens`) and the output.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    /// Writes at the 5-minute TTL, or a provider's single write class (OpenAI).
    pub cache_write: u64,
    /// Anthropic 1-hour writes, and every Anthropic write the response does not split by TTL (the dearer class).
    pub cache_write_1h: u64,
    pub output: u64,
}

impl Usage {
    /// The cost at the model's peak prices (egress.md §3.4): cache reads at its cache-read price, cache writes at its
    /// write prices, the rest of the input at the input price, rounded up once. Provisional owner decision 2026-10-20
    /// (build-deviations §3.13); a model without a cache-read price bills reads at the input price, and one without
    /// write prices bills writes at twice the input price (classes.rs), so neither under-counts. Each class is
    /// clamped to what is left of the input, so a provider that reports more than its input cannot lower the bill.
    pub fn cost(&self, m: &Model) -> u64 {
        let cr = self.cache_read.min(self.input);
        let w1 = self.cache_write_1h.min(self.input - cr);
        let w5 = self.cache_write.min(self.input - cr - w1);
        musd(&[(self.input - cr - w1 - w5, m.input), (cr, m.cache_read), (w5, m.cache_write), (w1, m.cache_write_1h), (self.output, m.output)])
    }
    /// The `usage` object of a ledger record; the write fields only when there are writes (DeepSeek's records are
    /// unchanged).
    pub fn json(&self) -> Value {
        let mut v = json!({"cache_read": self.cache_read, "input": self.input, "output": self.output});
        if self.cache_write > 0 {
            v["cache_write"] = json!(self.cache_write);
        }
        if self.cache_write_1h > 0 {
            v["cache_write_1h"] = json!(self.cache_write_1h);
        }
        v
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
}

impl Tap {
    pub fn new(family: Family, sse: bool) -> Tap {
        Tap { family, sse, buf: Vec::new(), overflow: false, a: [None; 6], usage: None, finish: None, calls: None }
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

    /// The end of the stream: a non-streamed body is parsed once here.
    pub fn finish(self) -> (Option<Usage>, Option<&'static str>) {
        let (u, f, _) = self.finish_calls();
        (u, f)
    }

    /// `finish`, plus the cap ids of the response's tool calls with their uses (X5; empty without `with_calls`).
    pub fn finish_calls(mut self) -> (Option<Usage>, Option<&'static str>, BTreeMap<String, u32>) {
        if !self.sse && !self.overflow && self.family != Family::None
            && let Ok(v) = serde_json::from_slice::<Value>(&self.buf)
        {
            self.observe(&v);
            if let Some(c) = self.calls.as_mut() {
                c.body(&v);
            }
        }
        (self.usage, self.finish, self.calls.map(|c| c.caps()).unwrap_or_default())
    }

    fn observe(&mut self, v: &Value) {
        let n = |u: &Value, k: &str| u.get(k).and_then(Value::as_u64);
        match self.family {
            Family::Chat => {
                if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                    let hm = n(u, "prompt_cache_hit_tokens").zip(n(u, "prompt_cache_miss_tokens")).map(|(h, m)| h + m);
                    let cache_read = n(u, "prompt_cache_hit_tokens").or_else(|| n(&u["prompt_tokens_details"], "cached_tokens")).unwrap_or(0);
                    // a gateway's cache writes (OpenRouter `prompt_tokens_details.cache_write_tokens`); DeepSeek sends none
                    let cache_write = n(&u["prompt_tokens_details"], "cache_write_tokens").unwrap_or(0);
                    if let Some(input) = n(u, "prompt_tokens").or(hm) {
                        self.usage = Some(Usage { input, cache_read, cache_write, cache_write_1h: 0, output: n(u, "completion_tokens").unwrap_or(0) });
                    }
                }
                if let Some(f) = v["choices"][0]["finish_reason"].as_str() {
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
                if let Some(i) = self.a[0] {
                    let (cache_read, output, created) = (self.a[1].unwrap_or(0), self.a[3].unwrap_or(0), self.a[2].unwrap_or(0));
                    // only writes the response puts in the 5-minute class bill at the 5-minute price; the rest at 1 hour
                    let w5 = self.a[4].unwrap_or(0).min(created);
                    self.usage = Some(Usage { input: i + cache_read + created, cache_read, cache_write: w5, cache_write_1h: created - w5, output });
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
                    let cache_write = n(&u["input_tokens_details"], "cache_write_tokens").unwrap_or(0);
                    self.usage = Some(Usage { input, cache_read, cache_write, cache_write_1h: 0, output: n(u, "output_tokens").unwrap_or(0) });
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
            // 5 written tokens with no TTL split: the dearer 1-hour class (v0.5.0 L3)
            assert_eq!(run(Family::Messages, true, ANTHROPIC, split), (Some(Usage { cache_write_1h: 5, ..u(315, 300, 42) }), Some("stop")), "{split}");
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
        assert_eq!(w(Family::Messages, split), Usage { input: 134, cache_read: 100, cache_write: 20, cache_write_1h: 10, output: 9 });
        // OpenAI Responses, and a Chat gateway's single write class (OpenRouter)
        let resp = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":50,\
            \"input_tokens_details\":{\"cached_tokens\":20,\"cache_write_tokens\":25},\"output_tokens\":3}}}\n\n";
        assert_eq!(w(Family::Responses, resp), Usage { input: 50, cache_read: 20, cache_write: 25, cache_write_1h: 0, output: 3 });
        let chat = "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":50,\
            \"prompt_tokens_details\":{\"cached_tokens\":20,\"cache_write_tokens\":25},\"completion_tokens\":3}}\n\n";
        assert_eq!(w(Family::Chat, chat), Usage { input: 50, cache_read: 20, cache_write: 25, cache_write_1h: 0, output: 3 });
        // each class at its own price; counts beyond the input are clamped to it and can only raise the bill
        let m = Model { min_level: 0, input: 1_000_000, cache_read: 100_000, cache_write: 1_250_000, cache_write_1h: 2_000_000,
            output: 5_000_000, reserve_out: 1 };
        assert_eq!(w(Family::Messages, split).cost(&m), musd(&[(4, 1_000_000), (100, 100_000), (20, 1_250_000), (10, 2_000_000), (9, 5_000_000)]));
        let inflated = Usage { input: 10, cache_read: 4, cache_write: 50, cache_write_1h: 50, output: 0 };
        assert_eq!(inflated.cost(&m), musd(&[(4, 100_000), (6, 2_000_000)]), "the 4 reads, then the 6 tokens left at the dearest write class");
        assert_eq!(w(Family::Responses, resp).json(), json!({"cache_read": 20, "cache_write": 25, "input": 50, "output": 3}));
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
