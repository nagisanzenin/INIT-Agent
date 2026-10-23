# ADR-002: The kernel model proxy

```
Status: draft v0.1 (decision: accepted) · 2026-10-19 · Area: kernel
Depends on: spec v0.2 (§2.1 invariants 2–4, §3, §7.2, §9), kernel/proxy.md, kernel/supervise.md (§3.2),
            kernel/sandbox.md, body/provider.md, kernel/gate.md
Defines:    the decision to route every model call through a kernel-owned, key-holding, metering,
            pass-through reverse proxy on Unix sockets; the alternatives rejected and why
```

---

## 1. Purpose and context

Every model call the agent makes is both its most important action and its largest cost. Spec §3 puts one boundary around it, and many requirements hang on that one boundary:

| Requirement | Source |
|---|---|
| API keys never exist inside the sandbox | invariant 3 |
| A spend cap the agent cannot bypass (Ouroboros spent $110.6K over 161 days) | invariant 3, §1 |
| Raw model traces the agent cannot edit (Meta-Harness: raw traces beat summaries; DGM faked logs) | invariant 2 |
| Usage, cost, TTFT, tokens/s and cache hit rate measured from outside the body, for self-signals and the gate | §5, §6.3 |
| The tool list the model was actually sent, so a self-report can be checked | §5 "verify, don't trust" |
| Persistent upstream connections that survive body restarts | §3, §9 rank 2 |
| A body that needs only `std` (no TLS, no async runtime) | §3, CONVENTIONS §5 |
| DeepSeek first, then Anthropic and OpenAI, with each provider's own caching rules intact | CONVENTIONS §5 |

The forces in tension:
- **Transparency.** The cache is the largest cost lever; a DeepSeek miss costs 50× a hit. The proxy must not alter what the body sends.
- **Honesty.** The proxy must see enough to meter and to observe tools.
- **Robustness.** The proxy runs for the life of the kernel, across every body swap.

## 2. Responsibilities / non-goals

This ADR records *what kind* of component the proxy is and why. Every mechanism (routes, caps, the 429 format, parsing, traces, the control API) is specified in kernel/proxy.md. The process topology is kernel/supervise.md's.

## 3. Design: the decision

**INIT routes every model call through a kernel-owned reverse proxy with these properties:**

1. **Unix-socket HTTP/1.1 front, persistent TLS back.**
   - The body speaks HTTP/1.1 + SSE to `run/proxy.sock` with std only.
   - The proxy (hyper 1.12 server; hyper-util 0.1.21 legacy client; hyper-rustls 0.27.10 with rustls 0.23.46 on `ring` and the platform verifier) keeps HTTP/2 connections, or HTTP/1.1 keep-alive pools where h2 is not offered, open to each provider for the life of the kernel.
2. **Pass-through of native wire formats.**
   - Request bodies are forwarded byte for byte, and response bytes are forwarded frame by frame as they arrive.
   - The proxy parses only what it must: model, limits, tools, the leading system content, and usage and tool calls on the side.
   - Normalization across providers stays in the body (body/provider.md), the one model-specific seam.
3. **Attribution by socket, not by claim.**
   - Every request belongs to the **lane** whose socket it arrived on: live, or one per probe or gate run.
   - Headers from the body (`x-init-task`, `x-init-session`) refine attribution only inside the live lane, and their absence cannot bypass a cap.
4. **Keys as 0600 files in `vault/keys/`,** behind the same sandbox rule that already hides the holdout. Not the macOS Keychain (§3.1 F).
5. **Caps with worst-case reservations,** in integer pico-dollars.
   - A request is admitted only if every applicable cap (day, evolution share, task, gate, lane, request) still holds with its worst-case cost reserved.
   - The price table is the model allowlist.
   - Refusals are typed 429s (`init_cap_exceeded`, `x-init-cap`) that the body never retries.
6. **Retries only where they are invisible.**
   - Before any response byte is forwarded, and only for outcomes where no tokens were generated: connect errors, rate limits, overloads, 5xx.
   - Everything else, including all mid-stream failures, goes to the body.
7. **Prefix-deduplicated raw traces in `vault/traces/`.**
   - Content-addressed packs plus message-level chains.
   - The agent gets a read-only view of its own lanes; holdout and canary runs are never visible.
8. **Route allowlists per provider.** A key can be used only for the chat, messages and responses endpoints, plus free metadata endpoints.
9. **In-process, on its own runtime.** The proxy runs inside `init-kernel supervise` (kernel/supervise.md §3.2) on a dedicated tokio runtime. The rest of the kernel stays std-only and controls it through `ProxyHandle`, including `panic()`.
10. **OpenAI WebSocket mode inside the proxy,** presented to the body as ordinary SSE, so the std-only body gets the persistent-socket gain without a WebSocket client.

### 3.1 Alternatives considered

| # | Alternative | Why rejected |
|---|---|---|
| A | **Keys in the body** (env var or file inside the sandbox), calling providers directly | Violates invariant 3 outright. A self-modifying agent could exfiltrate the key, lift the spend cap, and fake its own traces. No honest cost or cache numbers either. |
| B | **Generic forward proxy** (HTTP CONNECT) with TLS interception to meter and inject keys | The body would need TLS, against the std-only goal. MITM certificates add a trust root to the sandbox. Every provider's traffic would need decrypting anyway, so the proxy gains nothing over a reverse proxy and loses simplicity. |
| C | **Normalizing proxy** (one unified API in the kernel, LiteLLM-style) | Moves the model-specific seam from the agent-owned body into the maintainer-owned kernel, so every new provider feature (cache breakpoints, thinking blocks, Responses items) waits for a kernel release. Translation also changes bytes: the cache layout would be the proxy's, not the agent's (§9 rank 1). Spec §2 Article 2 puts the one seam in the body's provider. |
| D | **Off-the-shelf gateway** (Envoy, LiteLLM, a hosted gateway) | Cannot bind attribution to a per-probe socket, cannot dedup traces by prefix, and cannot expose observed tools to the gate without custom extensions. It adds a large runtime (Python for LiteLLM; tens of MB for Envoy) and a third-party trust root that holds the keys. The kernel would still need most of the custom code. |
| E | **Separate proxy process** (supervised child) | Considered first, and viable. It gives crash isolation from the supervisor. Rejected in favour of supervise.md's single-process topology: the panic flag, the lane table and the supervisor state share memory, `panic()` stops spend in < 100 ms without IPC, and the upstream pool survives body restarts either way. The cost, a proxy bug that aborts the whole kernel, is bounded: launchd/systemd restarts it, caps survive in the ledger, and per-task panics are contained by tokio. Revisit if real proxy crashes ever take the supervisor down (§3.2). |
| F | **macOS Keychain for keys** | Protection relies on a second mechanism (Mach IPC to `securityd` plus item ACLs): any same-user process that may run `/usr/bin/security` can read an item created with default ACLs. Locked keychains fail headless restarts. Binary-specific ACLs prompt on every ad-hoc-signed rebuild. Linux has no Keychain. A 0600 file in `vault/` is covered by the existing sandbox rule and test A6. Guards for what files give up: FileVault, Time Machine exclusion. |
| G | **Attribution by headers** the kernel sets in the gate body's environment (evolution.md Q1 proposal) | Anything the body sends, it can forge or omit. A socket per lane is a capability that cannot be forged. |
| H | **No retries in the proxy** (all to the body) / **retry everything** | No retries: the body pays an extra round trip for pure transport hiccups, and two body versions might handle them differently, which biases gate comparisons for reasons unrelated to the change. Retry everything: duplicates billed generations and hides informative errors. The middle rule is transport-only and pre-first-byte. |
| I | **Full logging** (store every request whole) | About 0.5 MB per step (spec §7.2). Over a 1,000-step day that is ~0.5 GB, against a disk budget the owner already finds tight. Message-level dedup stores about 3–5 KB per step. |
| J | **Per-request connections** (no pooling) | Pays DNS, TCP and TLS on every call and after every body restart, exactly the round-trip cost §9 rank 2 says to remove. |

### 3.2 Consequences

**Positive**
- Invariants 2–4 rest on one OS boundary (the sandbox reaches only the proxy socket) plus one kernel component, instead of on body behaviour.
- The gate's cost, time, cache and tool observations come from the provider stream, so a candidate body cannot flatter itself.
- Body restarts are cheap on the network side: no handshake, warm h2 streams, and on DeepSeek a byte-stable history hits the disk cache right away.
- Adding a provider is mostly configuration (preset, prices, key file) plus a usage parser for a new wire family.
- The body stays std-only, which keeps its build small and its code readable in one sitting.

**Negative, with guards**

| Cost | Guard |
|---|---|
| One more long-lived, network-facing component in the kernel, with tokio, hyper and rustls (the largest kernel dependencies) | They are confined to the proxy module; tests P-01…P-17 |
| Parsing three wire families for usage, with providers that add events and fields over time | Unknown events are skipped; missing usage falls back to a conservative estimate flagged `usage_estimated`; parsers are tested on recorded fixtures |
| The proxy shares the kernel's fate | Ledger rebuild on restart; caps survive; launchd/systemd restart |
| Buffering request bodies (up to 32 MiB) for hashing and caps | Local copies only, bounded |
| Plaintext keys at rest | FileVault, backup exclusion, revocable keys with provider-side limits |
| Attribution needs a socket per lane, which the sandbox and supervise must cooperate on | kernel/sandbox.md literal-path connect rules; supervise's `probe()` creates lane sockets |

**What this locks in.** The body → kernel model contract is `POST /<provider_id>/<upstream path>` on a Unix socket, with native bodies and no auth (CONVENTIONS §4). Changing it is a kernel change and a body change together. Everything behind the socket (TLS stack, retry policy, trace format) can change in a kernel release without touching the body.

**Revisit when:**
- a proxy fault ever takes down the supervisor in practice (then split the process: alternative E);
- a provider ships a transport the body cannot reach through HTTP/1.1 + SSE and the proxy cannot translate;
- the reservation estimate refuses legitimate requests near caps often enough to matter (then use the provider's token-count endpoint for the input estimate);
- CloudFront or another edge closes idle h2 connections despite PINGs (measured by `connects` and `reuse_permille`).

## 4. Interfaces

See kernel/proxy.md §4 (body requests, error and 429 formats, `/_init/*`, `ProxyHandle`, CLI, files, provider notes) and kernel/config.md §4.2–4.5 (configuration).

## 5. Failure modes and recovery

See kernel/proxy.md §5. The decision-level failure, the proxy being unavailable, is bounded: the body's provider retries a refused connect (body/provider.md §3.9), and the kernel restarts as a whole within seconds.

## 6. Performance notes (spec §9)

- **Rank 1:** byte transparency keeps the agent's cache layout intact, and per-request `prefix_hash` and cache hit rate make cache breaks visible.
- **Rank 2:** pooled h2 connections that outlive the body, and transparent WebSocket mode for OpenAI.
- **Rank 5:** trace dedup keeps disk growth proportional to new content.
- **Overhead target:** ≤ 1 ms p50 added per request before upstream, ≤ 20 µs per forwarded frame (kernel/proxy.md P-12).

## 7. Invariants and security (spec §2.1)

| Invariant | How this decision serves it |
|---|---|
| #2 | measurements and traces are produced outside the body and kept where the body cannot write |
| #3 | keys exist only in `vault/keys/` and proxy memory; the sandbox reaches the network only through allowlisted routes; spend is capped by reservation |
| #4 | every call has a durable ledger entry and a reconstructable raw trace |

## 8. Test plan

Covered by kernel/proxy.md §8 (P-01…P-17) and the A6 sandbox suite (kernel/sandbox.md §8). The decision-level test is A6: from inside the live sandbox, reading `vault/keys/`, connecting to a probe lane socket, and connecting to any provider host directly all fail.

## 9. Open questions

- Should the proxy ever offer general allowlisted egress (for example `wasi:http` for extensions, ext/wit.md Q-W2)? Not before an observed need. It would be a separate route class with its own caps.

## 10. Spec deltas

| Spec v0.2 | This ADR | Why |
|---|---|---|
| §3 diagram shows `init-kernel proxy` as a subcommand | in-process module of `init-kernel supervise` with its own runtime; `init-kernel proxy --standalone` only for tests | shared panic flag and lane table (kernel/supervise.md §3.2) |
| §3 "allowlisted egress via proxy" | the proxy is a model proxy with per-provider route allowlists, not a general egress proxy | a key-holding general proxy widens the blast radius |

## 11. References

Checked 2026-10-19.
- Spec v0.2 §2.1, §3, §7.2, §9; research_notes/where_performance_matters.md (cache economics, WebSocket mode, TraceLab medians); research_notes/self_improving_agents.md (Ouroboros spend, DGM faked logs, PROCTOR).
- kernel/proxy.md §3.3 (key-storage analysis), §3.9 (retry policy), §3.13 (traces), §4.8 (provider notes and sources).
- kernel/supervise.md §3.2 (process topology).
- hyper 1.12.0 — https://docs.rs/hyper/1.12.0/hyper/ ; hyper-util 0.1.21 — https://docs.rs/hyper-util/0.1.21/hyper_util/ ; hyper-rustls 0.27.10 — https://docs.rs/hyper-rustls/0.27.10/hyper_rustls/ ; rustls 0.23.46 — https://docs.rs/rustls/0.23.46/rustls/
- OpenAI WebSocket mode — https://developers.openai.com/api/docs/guides/websocket-mode
- DeepSeek Context Caching and pricing — https://api-docs.deepseek.com/guides/kv_cache , https://api-docs.deepseek.com/quick_start/pricing/
- `security` CLI keychain ACL behaviour — https://ss64.com/mac/security-password.html
