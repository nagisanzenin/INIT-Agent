//! Rendering a reply into each provider's real streaming wire format (provider.md §3.12 rules).
//!
//! Deterministic: no clocks (`created` is 0), no randomness. Ids derive from `n`, the number of
//! assistant messages already in the request, so they are unique within a conversation and the same
//! request always yields the same bytes.

use super::canon;
use super::script::{Fault, Reply, Stop, UsageSpec};
use super::wire::Family;
use serde_json::{Value, json};

/// Reasoning text the DeepSeek renderer emits when a turn has no `think` and the request did not
/// disable thinking: real DeepSeek always reasons in thinking mode, and `--strict deepseek` rejects
/// a later request whose assistant messages lack `reasoning_content`. `"think": ""` opts out.
pub const DEFAULT_REASONING: &str = "Mock reasoning.";

/// One piece of a streamed response body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// One complete SSE event (`data: …\n\n` or `event: …\ndata: …\n\n`), sent as one HTTP chunk.
    Event(Vec<u8>),
    /// Go silent for this many milliseconds.
    Stall(u64),
    /// Close the connection without the terminating chunk.
    Cut,
}

/// Final usage numbers of a reply (provider-neutral; mapped per wire format).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// All prompt tokens, cached or not (DeepSeek `prompt_tokens`).
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// All completion tokens, reasoning included.
    pub output: u64,
    pub reasoning: u64,
}

/// Everything rendering needs besides the reply.
#[derive(Clone, Debug)]
pub struct RenderCx<'a> {
    pub family: Family,
    pub model: &'a str,
    /// Responder or purpose label, used in the response id `mock-<label>-<n>`.
    pub label: &'a str,
    /// Assistant messages already in the request.
    pub n: usize,
    pub chunk: usize,
    pub thinking_disabled: bool,
}

/// The reasoning text actually rendered.
pub fn effective_think(cx: &RenderCx, reply: &Reply) -> String {
    match &reply.think {
        Some(t) => t.clone(),
        None if cx.family == Family::Chat && !cx.thinking_disabled => DEFAULT_REASONING.to_string(),
        None => String::new(),
    }
}

fn ceil4(n: usize) -> u64 {
    n.div_ceil(4) as u64
}

/// Canonical (sorted-key) arguments string of a call input.
pub fn call_arguments(input: &Value) -> String {
    canon::to_string(input)
}

/// Fill in omitted usage fields: input = body bytes / 4, cache_read = `simulated` or 0,
/// output = ⌈rendered output bytes / 4⌉, reasoning = ⌈reasoning bytes / 4⌉.
pub fn compute_usage(
    spec: &UsageSpec,
    body_len: usize,
    simulated: Option<u64>,
    think: &str,
    reply: &Reply,
) -> Usage {
    let input = spec.input.unwrap_or((body_len / 4) as u64);
    let cache_read = spec.cache_read.or(simulated).unwrap_or(0).min(input);
    let out_bytes = think.len()
        + reply.text.len()
        + reply
            .calls
            .iter()
            .map(|c| c.name.len() + call_arguments(&c.input).len())
            .sum::<usize>();
    Usage {
        input,
        cache_read,
        cache_write: spec.cache_write.unwrap_or(0),
        output: spec.output.unwrap_or(ceil4(out_bytes)),
        reasoning: spec.reasoning.unwrap_or(ceil4(think.len())),
    }
}

/// Cut `s` into pieces of at most `n` characters.
pub fn split_chars(s: &str, n: usize) -> Vec<&str> {
    let n = n.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in s.char_indices() {
        if count == n {
            out.push(&s[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Cut `s` into pieces of at most `n` bytes, never inside a character (a piece holds at least one
/// character even when that character is longer than `n`).
pub fn split_bytes(s: &str, n: usize) -> Vec<&str> {
    let n = n.max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let mut j = (i + n).min(s.len());
        while !s.is_char_boundary(j) {
            j -= 1;
        }
        if j == i {
            j = i + 1;
            while !s.is_char_boundary(j) {
                j += 1;
            }
        }
        out.push(&s[i..j]);
        i = j;
    }
    out
}

fn data_event(v: &Value) -> Vec<u8> {
    format!("data: {}\n\n", canon::to_string(v)).into_bytes()
}

fn named_event(name: &str, v: &Value) -> Vec<u8> {
    format!("event: {name}\ndata: {}\n\n", canon::to_string(v)).into_bytes()
}

/// Render `reply` as the full event list of a successful stream.
pub fn render(cx: &RenderCx, reply: &Reply, usage: &Usage) -> Vec<Vec<u8>> {
    match cx.family {
        Family::Chat => render_chat(cx, reply, usage, None),
        Family::Messages => render_messages(cx, reply, usage),
        Family::Responses => render_responses(cx, reply, usage),
    }
}

/// The number of terminal events at the end of a successful stream (never cut or errored into).
pub fn tail_len(family: Family) -> usize {
    match family {
        Family::Chat => 2,      // the finish chunk with usage, then [DONE]
        Family::Messages => 2,  // message_delta, message_stop
        Family::Responses => 1, // response.completed / response.incomplete
    }
}

/// The reply a stream fault without its own `reply` streams before it strikes.
pub fn filler_reply() -> Reply {
    Reply::text("This is mock filler output for a stream fault; it is cut, stalled or errored.")
}

/// Turn a stream fault into parts. `events` is the full successful event list.
pub fn apply_stream_fault(cx: &RenderCx, events: Vec<Vec<u8>>, fault: &Fault) -> Vec<Part> {
    let body_events = events.len().saturating_sub(tail_len(cx.family));
    match fault {
        Fault::Cut { after_events } => {
            let k = (*after_events).min(events.len().saturating_sub(1));
            let mut parts: Vec<Part> = events.into_iter().take(k).map(Part::Event).collect();
            parts.push(Part::Cut);
            parts
        }
        Fault::Stall { after_events, ms } => {
            let k = (*after_events).min(events.len());
            let mut parts = Vec::with_capacity(events.len() + 1);
            for (i, e) in events.into_iter().enumerate() {
                if i == k {
                    parts.push(Part::Stall(*ms));
                }
                parts.push(Part::Event(e));
            }
            if k == parts.len() {
                parts.push(Part::Stall(*ms));
            }
            parts
        }
        Fault::StreamError {
            after_events,
            error,
        } => {
            let k = (*after_events).min(body_events);
            let mut parts: Vec<Part> = events.into_iter().take(k).map(Part::Event).collect();
            for e in stream_error_events(cx, error, k) {
                parts.push(Part::Event(e));
            }
            parts
        }
        Fault::Http { .. } | Fault::Budget => events.into_iter().map(Part::Event).collect(),
    }
}

fn stream_error_events(cx: &RenderCx, error: &str, seq: usize) -> Vec<Vec<u8>> {
    match cx.family {
        Family::Chat => {
            let r = render_chat(
                cx,
                &Reply::text(""),
                &Usage::default(),
                Some("insufficient_system_resource"),
            );
            // The finish chunk (without usage) and [DONE].
            r[r.len() - 2..].to_vec()
        }
        Family::Messages => vec![named_event(
            "error",
            &json!({"error": {"message": format!("mock stream error: {error}"), "type": error}, "type": "error"}),
        )],
        Family::Responses => vec![named_event(
            "response.failed",
            &json!({
                "response": {
                    "error": {"code": error, "message": format!("mock stream error: {error}")},
                    "id": format!("resp_mock{}", cx.n), "model": cx.model, "object": "response",
                    "output": [], "status": "failed"
                },
                "sequence_number": seq, "type": "response.failed"
            }),
        )],
    }
}

// ---------------------------------------------------------------- DeepSeek Chat Completions

fn chat_finish(stop: Stop) -> &'static str {
    match stop {
        Stop::EndTurn => "stop",
        Stop::ToolUse => "tool_calls",
        Stop::MaxTokens => "length",
        Stop::Refusal => "content_filter",
    }
}

/// DeepSeek usage object (prompt_tokens = hit + miss; usage only on the last chunk).
pub fn chat_usage(u: &Usage) -> Value {
    json!({
        "completion_tokens": u.output,
        "completion_tokens_details": {"reasoning_tokens": u.reasoning},
        "prompt_cache_hit_tokens": u.cache_read,
        "prompt_cache_miss_tokens": u.input.saturating_sub(u.cache_read),
        "prompt_tokens": u.input,
        "prompt_tokens_details": {"cached_tokens": u.cache_read},
        "total_tokens": u.input + u.output
    })
}

fn render_chat(
    cx: &RenderCx,
    reply: &Reply,
    usage: &Usage,
    finish_override: Option<&str>,
) -> Vec<Vec<u8>> {
    let id = format!("mock-{}-{}", cx.label, cx.n);
    let chunk = |delta: Value, finish: Value, usage: Option<Value>| {
        let mut o = json!({
            "choices": [{"delta": delta, "finish_reason": finish, "index": 0, "logprobs": null}],
            "created": 0,
            "id": id,
            "model": cx.model,
            "object": "chat.completion.chunk",
            "system_fingerprint": "fp_mock"
        });
        if let Some(u) = usage {
            o["usage"] = u;
        }
        data_event(&o)
    };
    let think = effective_think(cx, reply);
    let mut ev = Vec::new();
    if think.is_empty() {
        ev.push(chunk(
            json!({"content": "", "role": "assistant"}),
            Value::Null,
            None,
        ));
    } else {
        ev.push(chunk(
            json!({"content": null, "reasoning_content": "", "role": "assistant"}),
            Value::Null,
            None,
        ));
        for p in split_chars(&think, cx.chunk) {
            ev.push(chunk(
                json!({"content": null, "reasoning_content": p}),
                Value::Null,
                None,
            ));
        }
    }
    for p in split_chars(&reply.text, cx.chunk) {
        ev.push(chunk(
            json!({"content": p, "reasoning_content": null}),
            Value::Null,
            None,
        ));
    }
    for (i, c) in reply.calls.iter().enumerate() {
        let call_id = format!("call_{i:02}_mock{}", cx.n);
        ev.push(chunk(
            json!({"tool_calls": [{"function": {"arguments": "", "name": c.name}, "id": call_id, "index": i, "type": "function"}]}),
            Value::Null,
            None,
        ));
        let args = call_arguments(&c.input);
        for frag in split_bytes(&args, cx.chunk) {
            ev.push(chunk(
                json!({"tool_calls": [{"function": {"arguments": frag}, "index": i}]}),
                Value::Null,
                None,
            ));
        }
    }
    match finish_override {
        Some(f) => ev.push(chunk(
            json!({"content": "", "reasoning_content": null}),
            json!(f),
            None,
        )),
        None => ev.push(chunk(
            json!({"content": "", "reasoning_content": null}),
            json!(chat_finish(reply.stop)),
            Some(chat_usage(usage)),
        )),
    }
    ev.push(b"data: [DONE]\n\n".to_vec());
    ev
}

// ---------------------------------------------------------------- Anthropic Messages (stub)

fn render_messages(cx: &RenderCx, reply: &Reply, u: &Usage) -> Vec<Vec<u8>> {
    let mut ev = Vec::new();
    let uncached = u.input.saturating_sub(u.cache_read + u.cache_write);
    ev.push(named_event(
        "message_start",
        &json!({"message": {
            "content": [], "id": format!("msg_mock{}", cx.n), "model": cx.model, "role": "assistant",
            "stop_reason": null, "stop_sequence": null, "type": "message",
            "usage": {"cache_creation_input_tokens": u.cache_write, "cache_read_input_tokens": u.cache_read,
                      "input_tokens": uncached, "output_tokens": 1}
        }, "type": "message_start"}),
    ));
    ev.push(named_event("ping", &json!({"type": "ping"})));
    let mut idx = 0usize;
    if reply.think.is_some() {
        // Thinking block: empty text, signature mock-sig-<n> (provider.md §3.12).
        ev.push(named_event(
            "content_block_start",
            &json!({"content_block": {"signature": "", "thinking": "", "type": "thinking"}, "index": idx, "type": "content_block_start"}),
        ));
        ev.push(named_event(
            "content_block_delta",
            &json!({"delta": {"signature": format!("mock-sig-{}", cx.n), "type": "signature_delta"}, "index": idx, "type": "content_block_delta"}),
        ));
        ev.push(named_event(
            "content_block_stop",
            &json!({"index": idx, "type": "content_block_stop"}),
        ));
        idx += 1;
    }
    if !reply.text.is_empty() {
        ev.push(named_event(
            "content_block_start",
            &json!({"content_block": {"text": "", "type": "text"}, "index": idx, "type": "content_block_start"}),
        ));
        for p in split_chars(&reply.text, cx.chunk) {
            ev.push(named_event(
                "content_block_delta",
                &json!({"delta": {"text": p, "type": "text_delta"}, "index": idx, "type": "content_block_delta"}),
            ));
        }
        ev.push(named_event(
            "content_block_stop",
            &json!({"index": idx, "type": "content_block_stop"}),
        ));
        idx += 1;
    }
    for (i, c) in reply.calls.iter().enumerate() {
        ev.push(named_event(
            "content_block_start",
            &json!({"content_block": {"id": format!("toolu_mock{}_{i}", cx.n), "input": {}, "name": c.name, "type": "tool_use"},
                    "index": idx, "type": "content_block_start"}),
        ));
        for frag in split_bytes(&call_arguments(&c.input), cx.chunk) {
            ev.push(named_event(
                "content_block_delta",
                &json!({"delta": {"partial_json": frag, "type": "input_json_delta"}, "index": idx, "type": "content_block_delta"}),
            ));
        }
        ev.push(named_event(
            "content_block_stop",
            &json!({"index": idx, "type": "content_block_stop"}),
        ));
        idx += 1;
    }
    ev.push(named_event(
        "message_delta",
        &json!({"delta": {"stop_reason": reply.stop.as_str(), "stop_sequence": null}, "type": "message_delta",
                "usage": {"output_tokens": u.output}}),
    ));
    ev.push(named_event(
        "message_stop",
        &json!({"type": "message_stop"}),
    ));
    ev
}

// ---------------------------------------------------------------- OpenAI Responses (stub)

fn render_responses(cx: &RenderCx, reply: &Reply, u: &Usage) -> Vec<Vec<u8>> {
    let n = cx.n;
    let mut ev = Vec::new();
    let mut seq = 0usize;
    let mut push = |ev: &mut Vec<Vec<u8>>, name: &str, mut v: Value| {
        v["sequence_number"] = json!(seq);
        v["type"] = json!(name);
        seq += 1;
        ev.push(named_event(name, &v));
    };
    let resp = |status: &str, output: Vec<Value>| json!({"id": format!("resp_mock{n}"), "model": cx.model, "object": "response", "output": output, "status": status});
    push(
        &mut ev,
        "response.created",
        json!({"response": resp("in_progress", vec![])}),
    );
    let mut output: Vec<Value> = Vec::new();
    if let Some(think) = &reply.think {
        let oi = output.len();
        let id = format!("rs_mock{n}");
        let enc = format!("mock-enc-{n}");
        push(
            &mut ev,
            "response.output_item.added",
            json!({"item": {"encrypted_content": enc, "id": id, "summary": [], "type": "reasoning"}, "output_index": oi}),
        );
        let summary: Vec<Value> = if think.is_empty() {
            vec![]
        } else {
            vec![json!({"text": think, "type": "summary_text"})]
        };
        let item =
            json!({"encrypted_content": enc, "id": id, "summary": summary, "type": "reasoning"});
        push(
            &mut ev,
            "response.output_item.done",
            json!({"item": item, "output_index": oi}),
        );
        output.push(item);
    }
    if !reply.text.is_empty() {
        let oi = output.len();
        let id = format!("msg_mock{n}");
        let refusal = reply.stop == Stop::Refusal;
        let (part_ty, delta_ev, done_ev, key) = if refusal {
            (
                "refusal",
                "response.refusal.delta",
                "response.refusal.done",
                "refusal",
            )
        } else {
            (
                "output_text",
                "response.output_text.delta",
                "response.output_text.done",
                "text",
            )
        };
        push(
            &mut ev,
            "response.output_item.added",
            json!({"item": {"content": [], "id": id, "role": "assistant", "status": "in_progress", "type": "message"}, "output_index": oi}),
        );
        let mut empty_part = json!({"type": part_ty});
        empty_part[key] = json!("");
        if !refusal {
            empty_part["annotations"] = json!([]);
        }
        push(
            &mut ev,
            "response.content_part.added",
            json!({"content_index": 0, "item_id": id, "output_index": oi, "part": empty_part}),
        );
        for p in split_chars(&reply.text, cx.chunk) {
            push(
                &mut ev,
                delta_ev,
                json!({"content_index": 0, "delta": p, "item_id": id, "output_index": oi}),
            );
        }
        let mut done = json!({"content_index": 0, "item_id": id, "output_index": oi});
        done[key] = json!(reply.text);
        push(&mut ev, done_ev, done);
        let mut part = json!({"type": part_ty});
        part[key] = json!(reply.text);
        if !refusal {
            part["annotations"] = json!([]);
        }
        push(
            &mut ev,
            "response.content_part.done",
            json!({"content_index": 0, "item_id": id, "output_index": oi, "part": part}),
        );
        let item = json!({"content": [part], "id": id, "role": "assistant", "status": "completed", "type": "message"});
        push(
            &mut ev,
            "response.output_item.done",
            json!({"item": item, "output_index": oi}),
        );
        output.push(item);
    }
    for (i, c) in reply.calls.iter().enumerate() {
        let oi = output.len();
        let id = format!("fc_mock{n}_{i}");
        let call_id = format!("call_mock{n}_{i}");
        let args = call_arguments(&c.input);
        push(
            &mut ev,
            "response.output_item.added",
            json!({"item": {"arguments": "", "call_id": call_id, "id": id, "name": c.name, "status": "in_progress", "type": "function_call"},
                   "output_index": oi}),
        );
        for frag in split_bytes(&args, cx.chunk) {
            push(
                &mut ev,
                "response.function_call_arguments.delta",
                json!({"delta": frag, "item_id": id, "output_index": oi}),
            );
        }
        push(
            &mut ev,
            "response.function_call_arguments.done",
            json!({"arguments": args, "item_id": id, "output_index": oi}),
        );
        let item = json!({"arguments": args, "call_id": call_id, "id": id, "name": c.name, "status": "completed", "type": "function_call"});
        push(
            &mut ev,
            "response.output_item.done",
            json!({"item": item, "output_index": oi}),
        );
        output.push(item);
    }
    let usage = json!({
        "input_tokens": u.input,
        "input_tokens_details": {"cached_tokens": u.cache_read},
        "output_tokens": u.output,
        "output_tokens_details": {"reasoning_tokens": u.reasoning},
        "total_tokens": u.input + u.output
    });
    if reply.stop == Stop::MaxTokens {
        let mut r = resp("incomplete", output);
        r["incomplete_details"] = json!({"reason": "max_output_tokens"});
        r["usage"] = usage;
        push(&mut ev, "response.incomplete", json!({"response": r}));
    } else {
        let mut r = resp("completed", output);
        r["usage"] = usage;
        push(&mut ev, "response.completed", json!({"response": r}));
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mockllm::script::Call;

    fn cx(family: Family) -> RenderCx<'static> {
        RenderCx {
            family,
            model: "m",
            label: "turn",
            n: 3,
            chunk: 4,
            thinking_disabled: false,
        }
    }

    #[test]
    fn split_chars_counts_characters() {
        assert_eq!(split_chars("héllo wörld", 4), vec!["héll", "o wö", "rld"]);
        assert!(split_chars("", 4).is_empty());
        assert_eq!(split_chars("ab", 1), vec!["a", "b"]);
    }

    #[test]
    fn split_bytes_respects_char_boundaries() {
        let s = "aé😀b";
        for n in 1..8 {
            let parts = split_bytes(s, n);
            assert_eq!(parts.concat(), s);
            for p in &parts {
                assert!(p.len() <= n || p.chars().count() == 1, "n={n} {parts:?}");
            }
        }
    }

    #[test]
    fn chat_stream_shape() {
        let r = Reply {
            think: Some("abc".into()),
            text: "hi".into(),
            calls: vec![Call {
                name: "bash".into(),
                input: json!({"b": 1, "a": "x"}),
            }],
            stop: Stop::ToolUse,
            usage: UsageSpec::default(),
        };
        let u = compute_usage(&r.usage, 400, Some(64), "abc", &r);
        assert_eq!((u.input, u.cache_read), (100, 64));
        let ev = render(&cx(Family::Chat), &r, &u);
        let s: Vec<String> = ev
            .iter()
            .map(|e| String::from_utf8(e.clone()).unwrap())
            .collect();
        assert!(
            s[0].contains(r#""reasoning_content":"","role":"assistant""#),
            "{}",
            s[0]
        );
        assert!(s.iter().any(|e| e.contains(r#""id":"call_00_mock3""#)));
        assert!(
            s.iter().any(|e| e.contains(r#"{"arguments":"{\"a\""#)),
            "{s:?}"
        );
        let last = &s[s.len() - 2];
        assert!(last.contains(r#""finish_reason":"tool_calls""#));
        assert!(last.contains(r#""prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":36"#));
        assert_eq!(s.last().unwrap(), "data: [DONE]\n\n");
        assert!(s[..s.len() - 2].iter().all(|e| !e.contains("usage")));
    }

    #[test]
    fn default_reasoning_only_when_thinking_on() {
        let r = Reply::text("x");
        assert_eq!(effective_think(&cx(Family::Chat), &r), DEFAULT_REASONING);
        let mut off = cx(Family::Chat);
        off.thinking_disabled = true;
        assert_eq!(effective_think(&off, &r), "");
        let mut explicit = r.clone();
        explicit.think = Some(String::new());
        assert_eq!(effective_think(&cx(Family::Chat), &explicit), "");
        assert_eq!(effective_think(&cx(Family::Messages), &r), "");
    }

    #[test]
    fn faults_never_send_the_terminal_events() {
        let r = Reply::text("hello world, this is long");
        let c = cx(Family::Chat);
        let u = Usage::default();
        let ev = render(&c, &r, &u);
        let n = ev.len();
        let cut = apply_stream_fault(&c, ev.clone(), &Fault::Cut { after_events: 1000 });
        assert_eq!(cut.len(), n); // n-1 events + Cut
        assert_eq!(cut.last(), Some(&Part::Cut));
        let se = apply_stream_fault(
            &c,
            ev.clone(),
            &Fault::StreamError {
                after_events: 2,
                error: "x".into(),
            },
        );
        let txt: Vec<u8> = se
            .iter()
            .flat_map(|p| {
                if let Part::Event(e) = p {
                    e.clone()
                } else {
                    vec![]
                }
            })
            .collect();
        let txt = String::from_utf8(txt).unwrap();
        assert!(txt.contains("insufficient_system_resource"));
        assert!(!txt.contains("usage"));
        let st = apply_stream_fault(
            &c,
            ev,
            &Fault::Stall {
                after_events: 2,
                ms: 5,
            },
        );
        assert_eq!(st[2], Part::Stall(5));
        assert_eq!(st.len(), n + 1);
    }
}
