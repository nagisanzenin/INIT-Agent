//! Masking, layer 2 (log-context.md §3.11): candidates and placeholder texts. Placeholders are computed
//! once at the mask epoch and stored in the `mask` event (P2).

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::model::{
    Calib, MASK_ERROR_MIN_TOKENS, MASK_INPUT_MIN_TOKENS, MASK_INPUT_STRING_MIN_TOKENS,
    MASK_MIN_TOKENS,
};
use super::project::{Loaded, View, mask_input};
use crate::log::clock::id_hex;
use crate::log::event::{Block, EventBody, MaskEntry, joined_text};
use crate::log::icj;

/// `bash \`{first 80 chars, newlines → ⏎}\` exit={code}` and friends.
pub fn describe(
    name: &str,
    input: Option<&Map<String, Value>>,
    output: &[Block],
    exit_code: Option<i64>,
) -> String {
    let s = |k: &str| {
        input
            .and_then(|i| i.get(k))
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    };
    match name {
        "read" => {
            let (a, b) = read_range(input, output);
            format!("read {} lines {a}-{b}", s("path"))
        }
        "bash" => {
            let cmd: String = s("command")
                .chars()
                .take(80)
                .collect::<String>()
                .replace('\n', "⏎");
            let code = exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into());
            format!("bash `{cmd}` exit={code}")
        }
        "write" | "edit" => format!("{name} {}", s("path")),
        "ext" => format!("ext {}", s("name")),
        other => other.to_string(),
    }
}

/// Lines shown by a `read`: from the `[Showing lines S-E of T…]` trailer, else from `offset` and the
/// number of numbered lines in the output.
fn read_range(input: Option<&Map<String, Value>>, output: &[Block]) -> (u64, u64) {
    let text = joined_text(output, "\n");
    if let Some(p) = text.rfind("[Showing lines ") {
        let rest = &text[p + "[Showing lines ".len()..];
        if let Some((range, _)) = rest.split_once(" of ")
            && let Some((a, b)) = range.split_once('-')
            && let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse())
        {
            return (a, b);
        }
    }
    let a = input
        .and_then(|i| i.get("offset"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let n = text
        .lines()
        .filter(|l| {
            l.split_once('\t')
                .map(|(n, _)| n.trim().parse::<u64>().is_ok())
                .unwrap_or(false)
        })
        .count() as u64;
    (a, a + n.max(1) - 1)
}

/// `[init:masked] {desc} → ~{tokens} tokens, see event {id}{spill}`
pub fn output_placeholder(desc: &str, tokens: u64, id: u64, spill: Option<&str>) -> String {
    let spill = spill
        .map(|p| format!(" (full output: {p})"))
        .unwrap_or_default();
    format!(
        "[init:masked] {desc} → ~{tokens} tokens, see event {}{spill}",
        id_hex(id)
    )
}

/// A mask candidate with its estimated savings.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub entry: MaskEntry,
    pub saved: u64,
}

/// Every candidate in `window[..protect_from]` that is not already masked.
pub fn candidates(v: &View, protect_from: usize, calib: &Calib) -> Vec<Candidate> {
    candidates_in(v, protect_from, None, calib)
}

/// Candidates in `window[..end]`; the assistant message at `keep_assistant` (if any) keeps its inputs.
pub fn candidates_in(
    v: &View,
    end: usize,
    keep_assistant: Option<usize>,
    calib: &Calib,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    // call_id → (name, input) for the latest assistant message before each result.
    let mut calls: HashMap<String, (String, Map<String, Value>)> = HashMap::new();
    for (i, l) in v.window[..end.min(v.window.len())].iter().enumerate() {
        let id = l.id();
        match &l.ev.body {
            EventBody::Message(m) if m.is_assistant() => {
                calls.clear();
                for b in &m.content {
                    if let Block::ToolUse {
                        call_id,
                        name,
                        input,
                        ..
                    } = b
                    {
                        calls.insert(call_id.clone(), (name.clone(), input.clone()));
                        if keep_assistant != Some(i) {
                            input_candidate(v, l, call_id, name, input, calib, &mut out);
                        }
                    }
                }
            }
            EventBody::ToolResult(r) => {
                if v.masked.contains_key(&(id, "output".into(), String::new())) {
                    continue;
                }
                let (name, input) = match calls.get(&r.call_id) {
                    Some((n, i)) => (n.as_str(), Some(i)),
                    None => (r.name.as_str(), None),
                };
                let image_tokens: u64 = r
                    .output
                    .iter()
                    .map(|b| {
                        if let Block::Image { tokens_est, .. } = b {
                            *tokens_est
                        } else {
                            0
                        }
                    })
                    .sum();
                let text_tokens = r.tokens_est.saturating_sub(image_tokens);
                let big = text_tokens >= MASK_MIN_TOKENS
                    && (!r.is_error || text_tokens >= MASK_ERROR_MIN_TOKENS);
                if big {
                    let desc = describe(name, input, &r.output, r.exit_code);
                    let ph = output_placeholder(&desc, r.tokens_est, id, r.spill_path.as_deref());
                    let saved = r.tokens_est.saturating_sub(calib.tokens(ph.len() as u64));
                    out.push(Candidate {
                        entry: MaskEntry {
                            id: id_hex(id),
                            part: "output".into(),
                            tokens: r.tokens_est,
                            placeholder: ph,
                            call_id: None,
                            input: None,
                        },
                        saved,
                    });
                } else if !v.masked.contains_key(&(id, "image".into(), String::new())) {
                    let images: Vec<(&str, u64)> = r
                        .output
                        .iter()
                        .filter_map(|b| match b {
                            Block::Image {
                                media_type,
                                tokens_est,
                                ..
                            } => Some((media_type.as_str(), *tokens_est)),
                            _ => None,
                        })
                        .collect();
                    if !images.is_empty() {
                        let tokens: u64 = images.iter().map(|x| x.1).sum();
                        let ph = images
                            .iter()
                            .map(|(mt, t)| output_placeholder(&format!("image {mt}"), *t, id, None))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let saved = tokens.saturating_sub(calib.tokens(ph.len() as u64));
                        out.push(Candidate {
                            entry: MaskEntry {
                                id: id_hex(id),
                                part: "image".into(),
                                tokens,
                                placeholder: ph,
                                call_id: None,
                                input: None,
                            },
                            saved,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn input_candidate(
    v: &View,
    l: &Loaded,
    call_id: &str,
    name: &str,
    input: &Map<String, Value>,
    calib: &Calib,
    out: &mut Vec<Candidate>,
) {
    if name != "write" && name != "edit" {
        return;
    }
    let id = l.id();
    if v.masked
        .contains_key(&(id, "input".into(), call_id.to_string()))
    {
        return;
    }
    let bytes = icj::to_vec(&Value::Object(input.clone()))
        .map(|b| b.len() as u64)
        .unwrap_or(0);
    let tokens = calib.tokens(bytes);
    if tokens < MASK_INPUT_MIN_TOKENS {
        return;
    }
    let Some(masked) = mask_input(name, input, id, calib, MASK_INPUT_STRING_MIN_TOKENS) else {
        return;
    };
    let after = icj::to_string(&Value::Object(masked.clone())).unwrap_or_default();
    let saved = tokens.saturating_sub(calib.tokens(after.len() as u64));
    out.push(Candidate {
        entry: MaskEntry {
            id: id_hex(id),
            part: "input".into(),
            tokens,
            placeholder: format!("[init:masked ~{tokens} tokens, see event {}]", id_hex(id)),
            call_id: Some(call_id.to_string()),
            input: Some(masked),
        },
        saved,
    });
}
