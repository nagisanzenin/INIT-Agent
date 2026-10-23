# WP2: `init-testkit`: `init-mockllm` (lane T)

Status: **done** (2026-10-20).

## Done
- **Library `init_testkit::mockllm`** (`kernel/crates/init-testkit/src/mockllm/`), std threads only, no async:
  - `script`: parser for the provider.md §3.12 format (`meta`, `turn` with `expect`/`reply`/`fault`/`raw`, cursors per (session, purpose)).
  - `render`: deterministic rendering into DeepSeek Chat Completions SSE (reasoning_content deltas, tool-call deltas keyed by `index` with args cut into `chunk`-byte fragments on char boundaries, final chunk with `finish_reason` + usage incl. `prompt_cache_hit_tokens`/`prompt_cache_miss_tokens`, `data: [DONE]`, `created: 0`). Anthropic Messages and OpenAI Responses as stubs (correct event order, ids, usage).
  - `strict`: `--strict deepseek` checks of testing.md §3.4.2 (reasoning pass-back with tools, tool-call pairing, `stream`/`model`/`messages`, credential headers, `x-init-*` present in socket mode and stripped in upstream mode) → HTTP 400 `{"error":{"message":…,"type":"invalid_request_error"}}`.
  - `responders`: stateless probe responder, G3 responder (parses `<H>`, `<n>`, step 2 lines and step 4 message from gate.md §3.6's instruction), per-task scripts `tasks/<id>.mock.jsonl` with one cursor per session, give-up responder.
  - `cache`: `--simulate-cache` (LCP of raw prompt bytes captured with `RawValue`, `u × ⌊⌈LCP/4⌉/u⌋`, per model).
  - Recording: `<record_dir>/<seq:06>-<purpose>.json` bodies and `mock.log.jsonl` lines (canonical JSON) with `seq, session, purpose, path, responder, status, expect_ok, prefix_sha256, fault, protocol_error, exhausted, cache_read, mock_error, t_recv_ms, t_done_ms`; `read_log`, `summarize`, `prefix_constant` helpers for scenario assertions.
  - `http::MockServer`: loopback TCP or Unix socket, keep-alive, chunked request bodies, `expect: 100-continue`, one HTTP chunk per SSE event, real stalls and cuts. `GET /_init/stats` and `/_init/health` in socket mode.
  - In-process API: `Mock::respond(&MockRequest) -> MockResponse` (no socket).
  - Client-side helpers for tests: `sse::SseParser` / `ChunkedDecoder` (push-based, split-safe), `assemble` (reference reassembly per family; DeepSeek follows provider.md §3.8.1's decoder rules), `client` (blocking HTTP/1.1 over TCP/Unix).
- **Binary `init-mockllm`**: the flags of testing.md §3.4.1 (`--mode upstream|socket`, `--listen`, `--port-file` (atomic write), `--socket`, `--script`, `--tasks-dir` (default `scripts/<name>/tasks/` if present), `--record-dir`, `--record-bodies`, `--strict deepseek`, `--simulate-cache`, `--cache-unit-tokens`, `--no-probe-responder`, `--no-g3-responder`). Exit 64 usage, 66 bad script, 74 bind/IO.
- **Golden SSE files** in `kernel/crates/init-testkit/fixtures/{deepseek,anthropic,openai}/` (10 rendered + 2 stream-fault goldens; regenerate with `INIT_TESTKIT_BLESS=1`), plus hand-made DeepSeek edge cases: `usage-separate-chunk.sse` (testing.md §3.4.2), `split-args.sse` (interleaved calls by index), `keepalive-crlf.sse`.

## Tests
- `CARGO_TARGET_DIR=kernel/target/t cargo test --offline -p init-testkit` (in `kernel/`): **70 passed, 0 failed** (lib 53, bin 2, golden 6, server 5, WP0 stub bins 4). The server tests were also run 15 more times in a row: all green.
  - Split-fuzz: every family × 5 replies (Unicode, emoji, multi-call, empty text, max_tokens, refusal) × chunk sizes 1–64; boundaries at every byte for the SSE layer and for the HTTP chunked framing, plus byte-at-a-time and seeded random multi-splits; all reassemble identically, and reassemble to the scripted reply.
  - Strict mode: missing `reasoning_content` → exact 400 body, no turn consumed, `protocol_error` logged; fine without tools or with thinking disabled; pairing and header rules; the binary returns the same 400 over TCP.
  - Responders, cursors, `expect` (500, not consumed), `prefix` same/hash, all fault kinds, raw byte-exact replay, cache simulation incl. hit after restart, recording files, determinism (same script → same bytes), keep-alive, cut closes the connection without the last chunk, stall is real silence.
- `cargo clippy --offline -p init-testkit --all-targets -- -D warnings`: clean. My files are rustfmt-clean.
- Not run: a full `cargo build --offline` of the kernel workspace in `target/t` (disk at 10 GiB free; wasmtime would add GBs). The only manifest change is turning on serde_json's `raw_value` feature (no new crate; `Cargo.lock` and `vendor/` untouched).

## Deviations
1. **Default DeepSeek reasoning.** A turn without `think`, on a request that does not disable thinking, renders `reasoning_content: "Mock reasoning."`; `"think": ""` opts out. Without this, `--strict deepseek` would reject the next request of the docs' own scripts (the A3 example, the probe responder), because the body replays no `reasoning_content` for a reply that had none. Real DeepSeek always reasons in thinking mode.
2. **Budget fault** sends `x-init-cap: day`, `x-init-error: budget.day` and proxy.md §4.2's body (`type: init_cap_exceeded`, `init{…}`); provider.md §3.12 says `x-init-cap: daily`, but proxy.md owns the cap format.
3. **Ids.** `<n>` = the number of assistant messages already in the request (stateless, unique per conversation). Response id `mock-<label>-<n>` (label = purpose for scripted turns, else the responder). DeepSeek call ids `call_<ii>_mock<n>` (`ii` = index in the reply, DeepSeek's own pattern); Anthropic `toolu_mock<n>_<i>`; OpenAI `call_mock<n>_<i>`, `fc_mock<n>_<i>`. The response `model` echoes the request's (meta `model` is the fallback).
4. **Upstream mode has no `x-init-*` headers** (the proxy strips them), so the session is `_none`, a request is a probe when its latest user message contains `[init:probe]`, and task sessions are keyed by a hash of the `[task <id>]` message.
5. **G3 detection**: the first user message *containing* `[init:g3 v1 nonce=` (testing.md says "first user message starts with"), because a G3 session opens with a probe round trip. Steps are counted from assistant messages after the instruction; commands shell-quote `<H>` (paths may contain spaces); step 4 adds `git rev-parse HEAD` so `G3 DONE <sha>` can quote it.
6. **Task detection**: the first user message having a line that starts `[task <id>]` (a boot block may precede it); ids must match `[A-Za-z0-9._-]{1,128}`.
7. An `expect` failure does not consume the turn (retries meet the same failure); `prefix: "same"` compares with the last consumed request of the cursor and passes on the first.
8. Stream faults (`cut`, `stall`, `stream_error`) may carry a `reply`; otherwise a filler text reply is streamed. A cut never sends the terminal events; a stall resumes the stream afterwards.
9. Usage defaults beyond provider.md's: `output = ⌈output bytes / 4⌉`, `reasoning = ⌈reasoning bytes / 4⌉`.
10. The listener is independent of the dialect: `--socket` works in upstream mode (a `unix:` upstream) and `--listen` in socket mode.
11. The log line is written just before the response's last byte, so a client holding the full response finds its line on disk (`t_done_ms` is that moment).

## Needs from others
- **provider.md §3.12 / testing.md §3.4.4 (docs owner):** record deviations 1–6 (default reasoning, `x-init-cap: day`, id scheme, upstream-mode fallbacks, G3 and task detection wording).
- **WP3:** build the scenario runner and `init-fakeroot` on `mockllm::http::MockServer` (upstream mode), `client`, `read_log`/`summarize`/`prefix_constant`; `fixtures/` already exists for its homes/charters.
- **WP10 (seed provider):** spawn `$INIT_MOCKLLM_BIN --mode socket --socket … --strict deepseek`; copy the golden `.sse` files to `seed/tests/fixtures/` for the decoder tests.
- **WP5 / WP22:** run the mock in upstream mode with `--strict deepseek` to assert that `x-init-*` headers are stripped before the upstream.

## Left
- Strict checks for Anthropic and OpenAI (stubs only render).
- `scripts/` and scenario scripts (WP3).
