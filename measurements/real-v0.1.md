# Real-model flow batch for v0.1 (S-REAL, 2026-10-20)

Lane S-REAL proves the v0.1 flows on the real model, `deepseek-flash` only, through the full stack: owner → `init-root` → `init-kernel` (Seatbelt `kernel`) → `init-body` (Seatbelt `body`) → kernel proxy → Root egress → api.deepseek.com. The harness is `tests/real/real.sh` plus `tests/real/checks.py`. Redacted artifacts are in `build/real-artifacts/real/<check>/`, and the lane status is `build/status/S-REAL.md`.

## Budget and meters

- **Authorization:** the owner's flow-test key, used sparingly. Hard cap $1.00 Root-metered, enforced by the test home's charter (`money.day_usd = total_usd = "1.00"`). The orchestrator's soft line (no new check after $0.50) applied to home 1. At 14:45 the orchestrator lifted it for the remaining checks, and the hard cap now carries over: home 2's charter cap is $1.00 minus home 1's spend.
- **Meters.** Each figure below names its meter.
  - **Root:** what the charter ceiling counts. The frozen Root (built 12:23) predates R-DEC `cdb49e8`, so cache hits bill at the **input** price. This is the conservative meter the cap is defined on.
  - **Proxy:** the kernel proxy's meter, at DeepSeek's list price for the time window (peak from 13:00 +07), with cache hits at the cache-read price. It is close to the real charge.
  - **Root (cache-read est.):** the same requests at the Root's list prices with hits at $0.006/MTok, which is what a Root at or after `cdb49e8` would meter. It covers agent-visible requests only.
- **Real money:** the DeepSeek balance, read with the free `GET user/balance`, went from $10.62 at 13:34 to $10.41 at 17:06 (see Totals).

## Results (home 1, `target/real-homes/r`, seed `5bf085a`)

| Check | Result | Root µUSD | Proxy µUSD | Root cache-read est. | Requests | Notes |
|---|---|---|---|---|---|---|
| V-DS-1…6 (`vds`) | 5 of 6 as specified; V-DS-2 contradicts the doc (below) | 10,352 | 6,418 | 3,240 | 10 | proxy figure includes ~1.6k per refused 400 |
| V-DS-2 rerun with reasoning (`vds2`) | the doc's expectation is wrong (below) | 5,810 | 1,330 | 1,333 | 5 | |
| V-DS-3 follow-ups (`vds3`) | measured | 1,208 | 3,291 | 79 | 3 | 2 of the 3 were refused (400) |
| A1-real | **pass** | 15,446 | 2,927 | 2,932 | 7 turns | 9.6 s owner line → idle |
| E2E-2 (thinking on) | **pass**, 8/8 criteria | 4,158 | 544 | 546 | 3 turns | 3.5 s |
| E2E-1, try 1 | fail: `gate --quick` E_FETCH (kernel bug) | 4,869 | 1,143 | — | 3 turns | |
| E2E-1, try 2 (`gate --quick main`) | boot part **pass**; gate: G0 reject (`forbid_missing`, seed bug) | 4,700 | 973 | 975 | 3 turns | cold G0 build ~6 min |
| E2E-3, try 2 | promote refused: harness commit message (trailers) | 5,744 | 776 | — | 3 turns | harness bug, fixed |
| E2E-3, try 3 | G0 `test` fails on the seed's own tests in a real install (blocker) | 4,646 | 731 | 733 | 2 turns | |
| A2-real | **pass** (relaxed: tie order) | 245,656 | 31,993 | 21,450 | 26 turns | stopped by the $0.15 per-check cap in the agent's final self-check, after the work was done |
| **Home 1 total** | | **304,074** | **50,126** | | | balance $10.62 → $10.61 |

### V-DS (provider facts, live)

| Check | Doc expectation | Live result |
|---|---|---|
| V-DS-1 streaming tool call with the five real tools | well-formed, args parse, usage, `tool_calls` | **pass**: one `bash` call whose arguments arrived in 13 chunks; JSON ok; usage present; `[DONE]`; the second turn returns `vds-ok`, `stop` |
| V-DS-2 replaying a tool turn without `reasoning_content` | 400 (pass-back rule) | **200, accepted**, both for a turn with empty reasoning and for one with 64 chars of reasoning (32 tokens). `reasoning_content: ""` instead of the text is also accepted (200). |
| V-DS-3 foreign tool turn (`toolu_…` id) without `reasoning_content` | unverified | **400** `The \`reasoning_content\` in the thinking mode must be passed back to the API.` The same 400 comes for a made-up DeepSeek-shaped id (`call_00_…`) with empty content. With `"reasoning_content": ""` added (tool calls kept): **200**, and the model answers `vds-ok` from the foreign result (5 tokens). With the §3.10 flattened history: **200**, but the model **re-runs the tool** (548 output tokens). |
| V-DS-4 prefix cache | hit > 0 on the second request | turn 2: 3,840 / 4,002 (96%); identical repeat: 3,712 / 3,931 (94%); A2's 26 turns: session 967‰ |
| V-DS-5 keep-alive comments | record if any | none in any stream (short streams, < 10 s). The stream is kept as a fixture: `seed/src/provider/testdata/deepseek/real-*.sse` |
| V-DS-6 `max_tokens: 393217` | record the 400 text | 400 `invalid_request_error`: `Invalid max_tokens value, the valid range of max_tokens is [1, 393216]`. Classified `InvalidRequest`, not overflow. The over-window half was not run (it needs the owner's approval). |
| Thinking disabled (`thinking: {type: disabled}`) | E2E-2's second mode | the provider accepts a tool round trip with no `reasoning_content` at all (200, 200). The body cannot run this mode: `thinking` is hard-coded `enabled` in `seed/src/context/render.rs` (no config key). |
| Thinking mode with an empty reasoning | — | common: V-DS-1's tool call came with `reasoning_tokens: 0`. The body omits the field for such turns, and DeepSeek accepts that for its own call ids (V-DS-2). |

Conclusion: DeepSeek enforces the pass-back rule only for tool calls it cannot match to its own call ids. The body's first fallback now adds `"reasoning_content": ""` and keeps the tool calls; the flatten is only the second fallback (`669d5a1`).

### A1-real, E2E-2, A2-real

- **A1-real** (`fixtures/repos/a1-calc`): 7 model turns, 6 `bash` calls; 2 tool errors, both expected: a wrong `cd` path, and the failing suite before the fix. The model fixed `add` and ran `python3 -m unittest -q` (OK). The harness re-ran the suite outside the sandbox: exit 0. It used only `bash`: the model reads and edits with shell and Python rather than the `read` and `edit` tools.
- **E2E-2** (thinking on): (1) streamed `write` arguments assembled from 40 and 19 chunks; (2) tool_call_ids match; (3) every later request carries the earlier `reasoning_content`; (4) the final `stop` contains `INIT OK`; (5) file = `INIT OK\n`; (6) event-log shape; (7) $0.004 ≤ $0.05; (8) the second request hit 4,096 of 4,255 tokens. One note: the model wrote `INIT_HOME/work/work/e2e/hello.txt`. The body's cwd is `work/`, so "work/e2e/…" in the prompt is ambiguous; the harness accepts either location.
- **A2-real**: the agent found and read `body/skills/ext-authoring/SKILL.md` (after one wrong relative path), scaffolded `body/ext/semver-sort` from the template, and wrote the SemVer logic with 8 unit tests (green). It built for `wasm32-wasip2` (`work/target/wasm32-wasip2/debug/semver_sort.wasm`), tried the component with `init-body ext call`, then ran `initctl reload`: the reload event lists `semver-sort`, with `head_hash_before == head_hash_after`. The `ext` call wrote `work/a2/sorted.txt`, a valid SemVer 2.0.0 order of all 40 versions. Its only difference from `fixtures/a2/expected-sorted.txt` is the order of `1.2.3+build.42` and `1.2.3`, which have equal precedence (build metadata is ignored); the fixture keeps input order. So the check now accepts any valid precedence order.

### E2E-1 (partial)

The cold boot reached `ready` in 4.8–7.7 s on every start (≤ 90 s). The boot event has `model deepseek-flash`, `context_window 1000000`, `effective_window 200000`, `mode normal`, `reason cold`, and the boot made no model call. The quick gate's G2 round trip is still pending: see the blockers in `build/status/S-REAL.md`.

## Home 2 (`target/real-homes/r` reinstalled, frozen root after G0-ROOT, kernel after f8c212d/2794924/40600e0)

| Check | Seed | Result | Root µUSD | Proxy µUSD | Notes |
|---|---|---|---|---|---|
| E2E-2 (smoke) | `399a383` (HEAD 3008357) | **pass** 8/8 | 4,150 | 573 | |
| E2E-3 | `399a383` | G0 `test`: 3 awareness tests, socket under the export (fixed by G0-SEED f76e749) | 27,378 | 5,170 | the agent inspected the diff before promoting |
| E2E-2 (smoke) | `7c4b1a9` (HEAD 15:28, G0 green at 3610c17) | **pass** 8/8 | 4,031 | 454 | |
| E2E-3 | `7c4b1a9` | **G0 pass** (live, real install); then G1 `manifest_error` (kernel bug: G1's build-profile OUT excludes the candidate binary) | 4,052 | 1,229 | |
| **Batch total after home 2b** | | | **343,685** | **57,552** | balance $10.62 → $10.58 |

Each home's charter cap is $1.00 minus the spend of the earlier homes: home 2 $0.69, home 2b $0.66.

## Home 2c (kernel `2bb7ffa`, which includes the G1 fix 70f30d3; seed at HEAD 15:53; charter cap $0.65)

| Check | Result | Root µUSD | Proxy µUSD | Notes |
|---|---|---|---|---|
| E2E-2 (smoke) | **pass** 8/8 | 4,121 | 544 | |
| **E2E-3** | **pass** | 45,015 | 6,054 | below |

E2E-3, the real T2 handoff (`initctl promote evo/m4 --wait`: a comment-only change plus nothing else, `Prefix-change: no`):
- G0 passed in 308 s (cold slot build plus metadata, build_config, shape, check, clippy, ext, test). G1 10 s. G2 27 s: both probes did real round trips; continuity pass; prefix hash parent = candidate = `sha256:795cac44…`.
- **G3 (model mode)**: the model, in a fresh probe, made the scripted self-edit (commit `5b147df`), and the inner gate passed (G0, G1 and G2 on that commit).
- Then `awaiting_boundary`, and the overlapped handoff at the next idle: `boot{reason:"promote", sha:4a7f81e}` with the same `prefix_hash` as the cold boots. Promote wall time 1,060 s.
- **Cache across the restart**: the first `turn` request after the boot read 5,888 cached of 6,696 prompt tokens. The head is at most 4,084 tokens, so the hit is ≥ 0.9 × head. **Pass.**
- G4 (`j-000002`, the 2/1/1 stand-in pool, R = 2) ran 13 paired runs and ended `inconclusive: underpowered`. The runs are not valid evidence: each one was `cost_capped` with 0 requests, because the harness's task cap (`max_cost_musd` 30,000) is below one request's reservation (max_tokens 64k at the output price, about $0.08). The harness now uses 150,000. A3 exercises G4 with real runs.
- E2E-1's probe criterion is shown by these G2 probes: real round trips with the tool result sent back, the same G2 procedure as `gate --quick`.

## Home 2d (same binaries; charter cap $0.60): A3-real

| Check | Result | Root µUSD | Proxy µUSD | Root cache-read est. |
|---|---|---|---|---|
| **A3-real** | **pass** | 131,865 | 19,731 (live scope only; the gate lanes are not in it) | 15,703 |

The steps of the real T2 self-edit:
- The agent ran `git -C body worktree add work/wt-doc -b evo/doc-loop origin/main`. Its first `edit` used a guessed path (`/Users/init/...`); the second used the real path, added a `//!` line to `src/loop/mod.rs`, then ran `selfmap --write` (a cold body build in `work/target`, ~5 min) and committed with the four trailers.
- The first `initctl promote` was refused by the trailer validator. The agent amended the message and promoted again.
- **G0** 337 s, **G1** 4 s, **G2** 22 s (prefix unchanged, continuity pass), **G3** model mode 761 s (the probe agent's commit `44e8b39` passed the inner gate). Verdict `accept g0_g3`, then `awaiting_boundary`.
- **Handoff:** `boot{reason:"promote", sha:50e08e1}` on the same session. Asked "Which commit are you running now, and what did your last change do?", the new body answered: running `50e08e1` (provisional; parent LKG `016a684`), and the last change added a one-line `//!` summary to `src/loop/mod.rs`.
- **G4** (`j-000003`, the 2/1/1 stand-in pool, R = 2): 13 paired runs, each with real requests (2–3 per run, 6.9–10.0 s). The pool and holdout runs scored 1000 in both arms, and the canary 0 (no leak). The verdict is `inconclusive: underpowered`, as designed for a pool this small (GAIN_MIN_HOLDOUT = 4; S < 12).
- Owner line → promote result: 1,337 s, 12 model turns.

## Home 2e (cap $0.47): E2E-1's gate half, and the final balance

| Check | Result | Root µUSD | Proxy µUSD |
|---|---|---|---|
| E2E-1 | boot half **pass** (ready 2.5 s, cold, model check ok, no model call). Plain `initctl gate --quick` (default rev HEAD) works live, so the K-FLAKE `f8c212d` fix is verified. G0 passed in ~2 min. G1 then rejected: `self_json_missing`, because the seed has no committed `self.json` (a seed gap; see S-REAL N). E2E-1's probe criterion is shown by the G2 probes of E2E-3 and A3, which use the same procedure. | 4,656 | 446 |
| balance | $0 balance read (no model call) | 0 | 0 |

## Totals

| | Value |
|---|---|
| Root-metered (input-price meter, the cap's meter) | **529,342 µUSD = $0.529** of the $1.00 hard cap: home 1 304,074; home 2 31,528; home 2b 8,083; home 2c 49,136; home 2d 131,865; home 2e 4,656 |
| Proxy (live scope; excludes the gate lanes) | 84,327 µUSD ≈ $0.084 |
| **Real money (DeepSeek balance)** | **$10.62 (13:34) → $10.41 (17:06): $0.21**, at 2-decimal resolution, and billing lags by minutes. Root/real ≈ 2.5× over the batch. |
| Requests | V-DS 18 raw; plus the agent turns, the G2/G3 probes and the G4 runs above |

**Pass list:** V-DS-1, 3, 4, 5, 6 (V-DS-2 contradicts the doc). A1-real. E2E-2 (×4 homes). A2-real. **E2E-3. A3-real.** E2E-1's boot half, and its quick-gate path through G0.

**Not run:** M-3, which would be ~$2.5 Root-metered for 10 trials at A2's rate, over the cap. The over-window half of V-DS-6, which needs the owner's approval. E2E-2 with thinking disabled through the body, which has no config key (provider-level check only). E2E-4 (A1-real × 3): one run only.
