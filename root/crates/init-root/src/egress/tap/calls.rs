//! ADR-011 §5.2 (lane X5): the tap's tool-call reader. From the response bytes the egress already relays, it learns
//! which tools the model called and, for `ext`, `read`, `edit`, `write` and `bash`, the one argument that names a
//! capability (`name`, `path`, `command`). Every family delivers the function name whole in the first event of a
//! call; the arguments arrive in pieces and are scanned as they come, so an argument split across deltas is still
//! read, while only the wanted top-level string (≤ 4 KiB) is held, and only until the call's cap ids are known.
//! Read-only: it is fed copies of relayed bytes and never changes them. No argument text leaves this module.

use super::Family;
use serde_json::Value;
use std::collections::BTreeMap;

/// The longest captured argument value (ADR-011 §5.2: 4 KiB); a longer `command` is read as its prefix.
const ARG_MAX: usize = 4096;
/// Calls tracked per response; more are counted by name only.
const CALLS_MAX: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Skip,
    Key,
    Val,
}

/// A streaming scan of one call's argument JSON for the top-level string value of `want`.
#[derive(Debug, Default)]
struct Scan {
    want: &'static str,
    depth: u32,
    in_str: bool,
    esc: bool,
    mode: Mode,
    expect_key: bool,
    colon: bool,
    key: Vec<u8>,
    hit: bool,
    val: Vec<u8>,
    got: bool,
}

impl Scan {
    fn new(tool: &str) -> Scan {
        let want = match tool {
            "ext" => "name",
            "read" | "edit" | "write" => "path",
            "bash" => "command",
            _ => "",
        };
        Scan { want, ..Scan::default() }
    }

    fn push(&mut self, c: u8) {
        match self.mode {
            Mode::Key if self.key.len() <= self.want.len() => self.key.push(c),
            Mode::Val if self.val.len() < ARG_MAX => self.val.push(c),
            _ => {}
        }
    }

    fn feed(&mut self, b: &[u8]) {
        if self.want.is_empty() || self.got {
            return;
        }
        for &c in b {
            if self.in_str {
                if self.esc {
                    self.esc = false;
                    self.push(c);
                } else if c == b'\\' {
                    self.esc = true;
                    self.push(c);
                } else if c == b'"' {
                    self.in_str = false;
                    match self.mode {
                        Mode::Key => self.hit = self.key == self.want.as_bytes(),
                        Mode::Val => {
                            self.got = true;
                            return;
                        }
                        Mode::Skip => {}
                    }
                } else {
                    self.push(c);
                }
                continue;
            }
            match c {
                b'"' => {
                    self.in_str = true;
                    self.mode = match (self.depth == 1, self.expect_key, self.colon && self.hit) {
                        (true, true, _) => {
                            self.key.clear();
                            Mode::Key
                        }
                        (true, false, true) => Mode::Val,
                        _ => Mode::Skip,
                    };
                }
                b'{' | b'[' => {
                    self.depth = self.depth.saturating_add(1);
                    if self.depth == 1 {
                        (self.expect_key, self.colon, self.hit) = (c == b'{', false, false);
                    }
                }
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                b':' if self.depth == 1 => (self.expect_key, self.colon) = (false, true),
                b',' if self.depth == 1 => (self.expect_key, self.colon, self.hit) = (true, false, false),
                _ => {}
            }
        }
    }

    /// The captured value, unescaped; only a closed string counts (a cut stream yields nothing).
    fn value(&self) -> Option<String> {
        self.got.then(|| unescape(&self.val))
    }
}

/// JSON string unescaping of a raw (possibly truncated) value; an escape cut by the 4 KiB bound is dropped.
fn unescape(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        i += 1;
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let Some(&e) = raw.get(i) else { break };
        i += 1;
        let ch = match e {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'u' => {
                let Some(h) = raw.get(i..i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok()) else { break };
                i += 4;
                char::from_u32(h).unwrap_or('\u{fffd}')
            }
            other => other as char,
        };
        let mut b = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug)]
struct Call {
    name: String,
    scan: Scan,
    fed: bool,
}

/// The tool calls of one response, keyed by (choice, call index) in the family's own numbering.
#[derive(Debug)]
pub struct Calls {
    family: Family,
    calls: BTreeMap<(u64, u64), Call>,
    /// Calls beyond `CALLS_MAX`, by name only.
    extra: BTreeMap<(u64, u64), String>,
}

impl Calls {
    pub fn new(family: Family) -> Calls {
        Calls { family, calls: BTreeMap::new(), extra: BTreeMap::new() }
    }

    /// Whether an SSE `data:` payload can carry a call event (a byte search, so text deltas are never parsed).
    pub fn wants(&self, d: &[u8]) -> bool {
        let needles: &[&[u8]] = match self.family {
            Family::Chat => &[b"\"tool_calls\""],
            Family::Messages => &[b"\"tool_use\"", b"input_json_delta"],
            Family::Responses => &[b"function_call"],
            Family::None => &[],
        };
        needles.iter().any(|n| d.windows(n.len()).any(|w| w == *n))
    }

    fn open(&mut self, key: (u64, u64), name: &str) {
        if self.calls.contains_key(&key) || self.extra.contains_key(&key) {
            return;
        }
        if self.calls.len() >= CALLS_MAX {
            self.extra.insert(key, name.to_string());
            return;
        }
        self.calls.insert(key, Call { name: name.to_string(), scan: Scan::new(name), fed: false });
    }

    fn args(&mut self, key: (u64, u64), piece: &str) {
        if let Some(c) = self.calls.get_mut(&key) {
            c.fed |= !piece.is_empty();
            c.scan.feed(piece.as_bytes());
        }
    }

    /// One streamed event (SSE), already parsed by the tap.
    pub fn event(&mut self, v: &Value) {
        let idx = |x: &Value, i: usize| x.as_u64().unwrap_or(i as u64);
        match self.family {
            Family::Chat => {
                for (ci, ch) in v["choices"].as_array().into_iter().flatten().enumerate() {
                    for (ti, tc) in ch["delta"]["tool_calls"].as_array().into_iter().flatten().enumerate() {
                        let key = (idx(&ch["index"], ci), idx(&tc["index"], ti));
                        if let Some(n) = tc["function"]["name"].as_str() {
                            self.open(key, n);
                        }
                        if let Some(a) = tc["function"]["arguments"].as_str() {
                            self.args(key, a);
                        }
                    }
                }
            }
            Family::Messages => {
                let key = (0, idx(&v["index"], 0));
                match v["type"].as_str() {
                    Some("content_block_start") if v["content_block"]["type"] == "tool_use" => {
                        let b = &v["content_block"];
                        self.open(key, b["name"].as_str().unwrap_or(""));
                        if b["input"].as_object().is_some_and(|o| !o.is_empty()) {
                            self.args(key, &b["input"].to_string());
                        }
                    }
                    Some("content_block_delta") if v["delta"]["type"] == "input_json_delta" => {
                        self.args(key, v["delta"]["partial_json"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
            }
            Family::Responses => {
                let key = (0, idx(&v["output_index"], 0));
                match v["type"].as_str() {
                    // added opens the call; done only stands in for an added (or deltas) the stream skipped
                    Some(t @ ("response.output_item.added" | "response.output_item.done")) if v["item"]["type"] == "function_call" => {
                        if t.ends_with("added") || !self.calls.get(&key).is_some_and(|c| c.fed) {
                            self.open(key, v["item"]["name"].as_str().unwrap_or(""));
                            self.args(key, v["item"]["arguments"].as_str().unwrap_or(""));
                        }
                    }
                    Some("response.function_call_arguments.delta") => self.args(key, v["delta"].as_str().unwrap_or("")),
                    Some("response.function_call_arguments.done") if !self.calls.get(&key).is_some_and(|c| c.fed) => {
                        self.args(key, v["arguments"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
            }
            Family::None => {}
        }
    }

    /// A whole non-streamed body.
    pub fn body(&mut self, v: &Value) {
        fn arr(x: &Value) -> &[Value] {
            x.as_array().map(Vec::as_slice).unwrap_or_default()
        }
        match self.family {
            Family::Chat => {
                for (ci, ch) in arr(&v["choices"]).iter().enumerate() {
                    for (ti, tc) in arr(&ch["message"]["tool_calls"]).iter().enumerate() {
                        self.open((ci as u64, ti as u64), tc["function"]["name"].as_str().unwrap_or(""));
                        self.args((ci as u64, ti as u64), tc["function"]["arguments"].as_str().unwrap_or(""));
                    }
                }
            }
            Family::Messages => {
                for (i, b) in arr(&v["content"]).iter().enumerate().filter(|(_, b)| b["type"] == "tool_use") {
                    self.open((0, i as u64), b["name"].as_str().unwrap_or(""));
                    self.args((0, i as u64), &b["input"].to_string());
                }
            }
            Family::Responses => {
                for (i, o) in arr(&v["output"]).iter().enumerate().filter(|(_, o)| o["type"] == "function_call") {
                    self.open((0, i as u64), o["name"].as_str().unwrap_or(""));
                    self.args((0, i as u64), o["arguments"].as_str().unwrap_or(""));
                }
            }
            Family::None => {}
        }
    }

    /// Cap id → uses: one use per call that names the cap (a call naming it twice counts once).
    pub fn caps(&self) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for c in self.calls.values() {
            for id in crate::extend::call_caps(&c.name, c.scan.value().as_deref()) {
                *out.entry(id).or_default() += 1;
            }
        }
        for name in self.extra.values() {
            for id in crate::extend::call_caps(name, None) {
                *out.entry(id).or_default() += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::Tap;
    use super::*;
    use crate::sentinel::Turn;

    /// One turn, five calls (tests/fixtures/x5/gen.py), recorded in each family's wire shape, streamed and not.
    const CHAT: &str = include_str!("../../../tests/fixtures/x5/chat.sse");
    const MSG: &str = include_str!("../../../tests/fixtures/x5/messages.sse");
    const RESP: &str = include_str!("../../../tests/fixtures/x5/responses.sse");
    const CHAT_J: &str = include_str!("../../../tests/fixtures/x5/chat.json");
    const MSG_J: &str = include_str!("../../../tests/fixtures/x5/messages.json");
    const RESP_J: &str = include_str!("../../../tests/fixtures/x5/responses.json");
    const ARGS: &str = include_str!("../../../tests/fixtures/x5/args.txt");
    /// Recordings from seed/src/provider/testdata: a live DeepSeek tool call, and the mock Anthropic and Responses ones.
    const DS_REAL: &str = include_str!("../../../tests/fixtures/x5/deepseek-real.sse");
    const ANTH_MOCK: &str = include_str!("../../../tests/fixtures/x5/anthropic-mock.sse");
    const RESP_MOCK: &str = include_str!("../../../tests/fixtures/x5/responses-mock.sse");

    fn all() -> [(Family, bool, &'static str); 6] {
        [(Family::Chat, true, CHAT), (Family::Messages, true, MSG), (Family::Responses, true, RESP),
            (Family::Chat, false, CHAT_J), (Family::Messages, false, MSG_J), (Family::Responses, false, RESP_J)]
    }

    fn run(f: Family, sse: bool, s: &str, split: usize) -> (BTreeMap<String, u32>, Option<super::super::Usage>, Option<&'static str>) {
        let mut t = Tap::new(f, sse).with_calls();
        s.as_bytes().chunks(split.max(1)).for_each(|c| t.feed(c));
        let (u, fin, caps) = t.finish_calls();
        (caps, u, fin)
    }

    fn want() -> BTreeMap<String, u32> {
        [("agent/reviewer", 1), ("ext/semver-sort", 1), ("skill/release-notes", 3), ("tool/bash", 1), ("tool/edit", 1),
            ("tool/ext", 1), ("tool/read", 1), ("tool/write", 1)].into_iter().map(|(k, n)| (k.to_string(), n)).collect()
    }

    /// RF-19: recorded streams of all three families give identical cap sets, streamed or not; the reader leaves
    /// the usage and finish tap as it was; a Responses call is counted once although `done` and `completed` repeat it.
    #[test]
    fn rf19_three_families_identical_cap_sets() {
        for (f, sse, s) in all() {
            let (caps, u, fin) = run(f, sse, s, 16 * 1024);
            assert_eq!(caps, want(), "{f:?} sse={sse}");
            let mut plain = Tap::new(f, sse);
            plain.feed(s.as_bytes());
            assert_eq!((u, fin), plain.finish(), "{f:?} sse={sse}: usage and finish unchanged");
            assert_eq!(fin, Some("tool_use"), "{f:?} sse={sse}");
        }
        let bash: BTreeMap<String, u32> = [("tool/bash".to_string(), 1)].into();
        for (f, s) in [(Family::Chat, DS_REAL), (Family::Messages, ANTH_MOCK), (Family::Responses, RESP_MOCK)] {
            assert_eq!(run(f, true, s, 7).0, bash, "{f:?} recording");
        }
        assert!(Tap::new(Family::Chat, true).finish_calls().2.is_empty(), "sealed lanes never read calls");
    }

    /// RF-20: the ext name is split across argument deltas (no one event holds it), and every split of the bytes
    /// into reads gives the same caps.
    #[test]
    fn rf20_ext_name_split_across_deltas() {
        for s in [CHAT, MSG, RESP] {
            let deltas = s.lines().filter(|l| l.contains("tool_calls") || l.contains("input_json_delta") || l.contains("arguments.delta"));
            assert!(deltas.clone().count() > 50 && !deltas.clone().any(|l| l.contains("semver-sort")), "the fixture splits the name");
        }
        for split in [1, 2, 3, 5, 13, 64, 511] {
            for (f, sse, s) in all() {
                assert_eq!(run(f, sse, s, split).0, want(), "{f:?} sse={sse} split={split}");
            }
        }
        // a stream cut inside the ext call's arguments: the call counts by name only
        let events: Vec<&str> = CHAT.split_inclusive("\n\n").collect();
        let at = events.iter().position(|l| l.contains("\"name\":\"ext\"")).unwrap();
        assert_eq!(run(Family::Chat, true, &events[..at + 5].concat(), 9).0, [("tool/ext".to_string(), 1)].into());
    }

    fn home(name: &str) -> std::path::PathBuf {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/x5").join(name);
        if d.exists() {
            std::fs::remove_dir_all(&d).unwrap(); // exact path, made by an earlier run of this test
        }
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                out.push(s.clone());
                out.extend(s.split_whitespace().filter(|w| w.starts_with("ARGTEXT-")).map(String::from));
            }
            Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            Value::Object(o) => o.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }

    /// RF-21: the six recorded responses go through the tap into closed turns over three owner days (so digests are
    /// due); no argument text, only cap ids, is found anywhere under `ROOT_HOME/extend/`, in the `cap_use`
    /// digests (mirrored lineage) or in the `caps.json` view.
    #[test]
    fn rf21_no_argument_text_under_extend() {
        let rh = home("rf21");
        let ext = crate::extend::Extend::default();
        let mut digests = Vec::new();
        for (i, (f, sse, s)) in all().into_iter().enumerate() {
            let caps = run(f, sse, s, 97).0.into_iter().map(|(k, n)| (k, (n, "root".to_string()))).collect();
            let end = 1_792_000_000_000 + i as i64 * 40_000_000;
            let t = Turn { seq: i as u64 + 1, kind: "owner".into(), start: end - 60_000, end, caps, provider_errors: u32::from(i == 2), ..Turn::default() };
            digests.extend(ext.turn(&rh, &t, &format!("2026-10-{:02}", 20 + i / 2)).unwrap());
        }
        assert_eq!(digests.len(), 2, "two finished owner days: {digests:?}");
        let mut text = vec![serde_json::to_string(&digests).unwrap(), serde_json::to_string(&ext.view("2026-10-22")).unwrap()];
        let mut stack = vec![rh.join("extend")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                if e.file_type().unwrap().is_dir() { stack.push(e.path()) } else { text.push(std::fs::read_to_string(e.path()).unwrap()) }
            }
        }
        assert!(text.len() >= 3, "usage files were written");
        let mut secrets = Vec::new();
        for l in ARGS.lines() {
            let v: Value = serde_json::from_str(l).unwrap();
            secrets.push(l.to_string());
            strings(&v, &mut secrets);
        }
        secrets.retain(|s| s != "semver-sort" && s.len() >= 6); // the ext name is the cap's identity, kept by design
        assert!(secrets.iter().filter(|s| s.starts_with("ARGTEXT-")).count() >= 5, "{secrets:?}");
        for t in &text {
            for sct in &secrets {
                assert!(!t.contains(sct.as_str()), "argument text {sct:?} under extend/: {t}");
            }
        }
        digests.iter().for_each(|d| ext.apply(&serde_json::json!({"by": "root", "data": d, "kind": "cap_use"})));
        let rows = ext.table();
        let skill = rows.iter().find(|r| r.id == "skill/release-notes").unwrap();
        assert_eq!((skill.turns, skill.uses, skill.harm_turns, skill.state.as_str()), (6, 18, 1, "observed"));
    }

    fn scan(tool: &str, args: &str, split: usize) -> Option<String> {
        let mut s = Scan::new(tool);
        args.as_bytes().chunks(split.max(1)).for_each(|c| s.feed(c));
        s.value()
    }

    #[test]
    fn scan_takes_only_the_top_level_string() {
        for split in [1, 2, 3, 5, 64] {
            assert_eq!(scan("ext", r#"{"input":{"name":"inner"},"name":"semver-sort"}"#, split).as_deref(), Some("semver-sort"));
            assert_eq!(scan("ext", r#"{ "name" : "a\"b\\cA" }"#, split).as_deref(), Some("a\"b\\cA"));
            assert_eq!(scan("write", r#"{"content":"\"path\":\"skills/no/\"","path":"x/skills/yes/f"}"#, split).as_deref(), Some("x/skills/yes/f"));
            assert_eq!(scan("bash", r#"{"timeout":5,"command":"ls"}"#, split).as_deref(), Some("ls"));
            assert_eq!(scan("read", r#"{"path":["a"],"paths":"b"}"#, split), None, "a non-string value, then a longer key");
            assert_eq!(scan("ext", r#"{"name":"cut-off"#, split), None, "an unclosed string counts as nothing");
            assert_eq!(scan("grep", r#"{"path":"skills/a/"}"#, split), None, "no argument is read for other tools");
        }
        let long = format!(r#"{{"command":"cat skills/a/x {} tail"}}"#, "y".repeat(10_000));
        let v = scan("bash", &long, 7).unwrap();
        assert_eq!(v.len(), ARG_MAX, "a long command is kept as its 4 KiB prefix");
        assert!(v.starts_with("cat skills/a/x "));
        assert_eq!(unescape(br#"ab\u00"#), "ab", "an escape cut by the bound is dropped");
    }

    #[test]
    fn calls_beyond_the_bound_count_by_name_once() {
        let mut c = Calls::new(Family::Responses);
        for i in 0..(CALLS_MAX as u64 + 3) {
            for t in ["response.output_item.added", "response.output_item.done"] {
                c.event(&serde_json::json!({"type": t, "output_index": i, "item": {"type": "function_call", "name": "bash", "arguments": "{\"command\":\"ls skills/a/\"}"}}));
            }
        }
        let caps = c.caps();
        assert_eq!((caps["tool/bash"], caps["skill/a"]), (CALLS_MAX as u32 + 3, CALLS_MAX as u32));
    }
}
