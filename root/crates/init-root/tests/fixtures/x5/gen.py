#!/usr/bin/env python3
"""X5 (ADR-011 §5.2) fixtures: one model turn with five tool calls, in the wire shape of each provider family as
recorded by seed/src/provider/testdata (DeepSeek chat chunks, Anthropic Messages events, OpenAI Responses events),
streamed with token-sized argument deltas, and as non-streamed bodies. Regenerate: python3 -I gen.py <this dir>.
Every argument holds an ARGTEXT marker so RF-21 can prove no argument text reaches ROOT_HOME/extend/."""
import json, os, sys

CALLS = [
    ("ext", {"name": "semver-sort", "input": {"list": ["1.2.0", "1.10.0"], "note": "ARGTEXT-ext-7f3a"}}),
    ("read", {"path": "/tmp/init-home/body/skills/release-notes/SKILL.md"}),
    ("bash", {"command": "cat agents/reviewer/AGENT.md && ls skills/release-notes/ && echo ARGTEXT-bash-91c2 skills/x", "timeout": 30}),
    ("write", {"content": "# Notes\nsee skills/fake-in-content/ ARGTEXT-write-c0de\n", "path": "notes/out.md"}),
    ("edit", {"edits": [{"new_text": "ARGTEXT-edit-new", "old_text": "ARGTEXT-edit-old"}], "path": "skills/release-notes/SKILL.md"}),
]

def args(i):
    return json.dumps(CALLS[i][1], separators=(",", ":"), sort_keys=False)

def pieces(s):
    """Token-sized deltas (2-4 chars), as DeepSeek streams them (real-reasoning-tool-call.sse)."""
    out, i, k = [], 0, 0
    while i < len(s):
        n = 2 + (k % 3)
        out.append(s[i:i + n]); i += n; k += 1
    return out

def d(o):
    return json.dumps(o, separators=(",", ":"), sort_keys=True)

def chat():
    base = {"created": 1792478155, "id": "x5-chat", "model": "deepseek-flash", "object": "chat.completion.chunk", "system_fingerprint": "fp_x5"}
    ev = [dict(base, choices=[{"delta": {"content": None, "reasoning_content": "", "role": "assistant"}, "finish_reason": None, "index": 0, "logprobs": None}])]
    ev.append(dict(base, choices=[{"delta": {"content": "Working on it.", "reasoning_content": None}, "finish_reason": None, "index": 0, "logprobs": None}]))
    for i, (name, _) in enumerate(CALLS):
        ev.append(dict(base, choices=[{"delta": {"tool_calls": [{"function": {"arguments": "", "name": name}, "id": f"call_0{i}_x5", "index": i, "type": "function"}]}, "finish_reason": None, "index": 0, "logprobs": None}]))
        for p in pieces(args(i)):
            ev.append(dict(base, choices=[{"delta": {"tool_calls": [{"function": {"arguments": p}, "index": i}]}, "finish_reason": None, "index": 0, "logprobs": None}]))
    ev.append(dict(base, choices=[{"delta": {"content": ""}, "finish_reason": "tool_calls", "index": 0, "logprobs": None}],
                   usage={"completion_tokens": 120, "prompt_cache_hit_tokens": 4000, "prompt_cache_miss_tokens": 300, "prompt_tokens": 4300, "total_tokens": 4420}))
    return "".join(f"data: {d(e)}\n\n" for e in ev) + "data: [DONE]\n\n"

def messages():
    ev = [("message_start", {"message": {"content": [], "id": "msg_x5", "model": "claude-x5", "role": "assistant", "stop_reason": None, "stop_sequence": None, "type": "message",
           "usage": {"cache_creation_input_tokens": 0, "cache_read_input_tokens": 4000, "input_tokens": 300, "output_tokens": 1}}, "type": "message_start"}),
          ("ping", {"type": "ping"}),
          ("content_block_start", {"content_block": {"text": "", "type": "text"}, "index": 0, "type": "content_block_start"}),
          ("content_block_delta", {"delta": {"text": "Working on it.", "type": "text_delta"}, "index": 0, "type": "content_block_delta"}),
          ("content_block_stop", {"index": 0, "type": "content_block_stop"})]
    for i, (name, _) in enumerate(CALLS):
        ix = i + 1
        ev.append(("content_block_start", {"content_block": {"id": f"toolu_x5_{i}", "input": {}, "name": name, "type": "tool_use"}, "index": ix, "type": "content_block_start"}))
        for p in pieces(args(i)):
            ev.append(("content_block_delta", {"delta": {"partial_json": p, "type": "input_json_delta"}, "index": ix, "type": "content_block_delta"}))
        ev.append(("content_block_stop", {"index": ix, "type": "content_block_stop"}))
    ev.append(("message_delta", {"delta": {"stop_reason": "tool_use", "stop_sequence": None}, "type": "message_delta", "usage": {"output_tokens": 120}}))
    ev.append(("message_stop", {"type": "message_stop"}))
    return "".join(f"event: {t}\ndata: {d(o)}\n\n" for t, o in ev)

def responses():
    seq = [0]
    ev = []
    def e(t, **o):
        o.update(type=t, sequence_number=seq[0]); seq[0] += 1; ev.append((t, o))
    e("response.created", response={"id": "resp_x5", "model": "gpt-x5", "object": "response", "output": [], "status": "in_progress"})
    e("response.output_item.added", item={"content": [], "id": "msg_x5", "role": "assistant", "status": "in_progress", "type": "message"}, output_index=0)
    e("response.output_text.delta", content_index=0, delta="Working on it.", item_id="msg_x5", output_index=0)
    e("response.output_item.done", item={"content": [{"annotations": [], "text": "Working on it.", "type": "output_text"}], "id": "msg_x5", "role": "assistant", "status": "completed", "type": "message"}, output_index=0)
    out = [{"content": [{"annotations": [], "text": "Working on it.", "type": "output_text"}], "id": "msg_x5", "role": "assistant", "status": "completed", "type": "message"}]
    for i, (name, _) in enumerate(CALLS):
        ix, fid = i + 1, f"fc_x5_{i}"
        e("response.output_item.added", item={"arguments": "", "call_id": f"call_x5_{i}", "id": fid, "name": name, "status": "in_progress", "type": "function_call"}, output_index=ix)
        for p in pieces(args(i)):
            e("response.function_call_arguments.delta", delta=p, item_id=fid, output_index=ix)
        e("response.function_call_arguments.done", arguments=args(i), item_id=fid, output_index=ix)
        item = {"arguments": args(i), "call_id": f"call_x5_{i}", "id": fid, "name": name, "status": "completed", "type": "function_call"}
        e("response.output_item.done", item=item, output_index=ix)
        out.append(item)
    e("response.completed", response={"id": "resp_x5", "model": "gpt-x5", "object": "response", "output": out, "status": "completed",
      "usage": {"input_tokens": 4300, "input_tokens_details": {"cached_tokens": 4000}, "output_tokens": 120, "total_tokens": 4420}})
    return "".join(f"event: {t}\ndata: {d(o)}\n\n" for t, o in ev)

def bodies():
    chat = {"choices": [{"finish_reason": "tool_calls", "index": 0, "message": {"content": None, "role": "assistant",
            "tool_calls": [{"function": {"arguments": args(i), "name": n}, "id": f"call_0{i}_x5", "type": "function"} for i, (n, _) in enumerate(CALLS)]}}],
            "id": "x5-chat", "object": "chat.completion", "usage": {"completion_tokens": 120, "prompt_tokens": 4300}}
    msg = {"content": [{"text": "Working on it.", "type": "text"}] + [{"id": f"toolu_x5_{i}", "input": a, "name": n, "type": "tool_use"} for i, (n, a) in enumerate(CALLS)],
           "id": "msg_x5", "role": "assistant", "stop_reason": "tool_use", "type": "message", "usage": {"input_tokens": 300, "output_tokens": 120}}
    resp = {"id": "resp_x5", "object": "response", "output": [{"arguments": args(i), "call_id": f"call_x5_{i}", "id": f"fc_x5_{i}", "name": n, "status": "completed",
            "type": "function_call"} for i, (n, _) in enumerate(CALLS)], "status": "completed", "usage": {"input_tokens": 4300, "output_tokens": 120}}
    return d(chat) + "\n", d(msg) + "\n", d(resp) + "\n"

if __name__ == "__main__":
    out = sys.argv[1]
    files = {"chat.sse": chat(), "messages.sse": messages(), "responses.sse": responses()}
    files.update(zip(["chat.json", "messages.json", "responses.json"], bodies()))
    files["args.txt"] = "".join(args(i) + "\n" for i in range(len(CALLS)))
    for n, t in files.items():
        with open(os.path.join(out, n), "w") as f:
            f.write(t)
