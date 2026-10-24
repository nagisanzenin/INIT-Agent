# Kernel model proxy

```
Status: v0.3 + build sync 2026-10-20 · Area: kernel
Depends on: CONVENTIONS.md, INIT Agent spec.md (v0.2 §2.1, §3, §5, §7.2, §9; v0.3 §12–§19),
            adr/007-root-and-autonomy-ladder.md, root/egress.md (R2/R3: egress socket, keys, hard
            ceilings), root/root.md (R5 charter, R7 level), root/sealed-eval.md (R6), kernel/supervise.md,
            kernel/sandbox.md, kernel/config.md, kernel/gate.md, evolution.md, body/provider.md
Defines:    kernel-side routing and route allowlists; L1 model routing (per-subtask model choice) and
            its attribution; request admission, reservations, the kernel's soft budgets under the Root's
            ceilings (and the guard on the Root's own spend), the mapping of Root refusals, and the 429 format; proxy error format; retry policy; usage extraction and
            normalization per wire family; measurement cost and the price semantics of config.toml
            [providers.*]; scopes and scope sockets; the kernel's client side of the Root egress socket;
            the proxy control API (ProxyHandle); GET /_init/* endpoints (stats, health, models, traces);
            proxy-side recomputation of prefix_hash (algorithm owned by body/log-context.md) plus the
            head_bytes_hash and tools_sha256 algorithms; the trace store format under vault/traces/;
            the observed-tools, tool-call and per-model records consumed by gate.md, selfmap.md and
            evolution.md. No longer defines (v0.3): key storage, auth injection, hard money ceilings,
            upstream TLS (all root/egress.md).
```

---

## 1. Purpose

`init-kernel proxy` is the body's only path to a model. In v0.3 it sits between the body and the **Root's egress socket** (root/egress.md, spec v0.3 §13–§14 R2). The Root holds the provider keys, terminates TLS, keeps the persistent provider connections and enforces the hard money ceilings. The kernel proxy holds none of those and has no network access of its own. It does five jobs:

1. **Forward** model requests from the body's lane sockets to the Root egress socket, unchanged.
2. **Route.** Check each request against the provider route allowlist and the model-routing policy of the current autonomy level. At L0 that is the default model only. From L1 it is a per-subtask choice among the models the Root allows, attributed in the traces (§3.5a).
3. **Meter** every request: usage, dollars, time to first token, tokens per second and cache hit rate. These feed the self-signals (spec §5), the gate (kernel/gate.md) and the capability self-model (kernel/selfmap.md §3.12).
4. **Budget** the money the Root allows: enforce soft budgets per day, task, gate, scope and request, and return 429 before a request could reach a Root ceiling (§3.8). Soft budgets are a stability mechanism. The Root enforces its ceilings whatever the kernel does.
5. **Record** raw traces, deduplicated by prefix, and serve the agent a filtered read-only view.

It never decides what the agent does. It never rewrites a request body. From L3 the proxy features are agent-evolvable through the gate-of-gates (spec v0.3 §15–§16). Keys and ceilings stay out of reach at every level, because they live in the Root.

## 2. Responsibilities / non-goals

**Responsibilities**
- HTTP/1.1 server on `$INIT_HOME/run/proxy.sock` (the live lane) and on per-probe lane sockets that supervise and the gate open (§3.4).
- Route `POST /<provider_id>/<upstream path>` to configured providers, only on allowlisted (method, path) pairs, and forward it to the Root egress socket (§3.7).
- Apply the level's model-routing policy and attribute every request to its model, task and subtask (§3.5a).
- Read the Root's published state (level, ceilings and the Root's own spend, day boundary, forwarded models) at start and when it changes (§3.1).
- Stream responses back without buffering, while extracting usage, timing and tool calls on the side.
- Account measurement cost in integer pico-dollars using the price table in `kernel/config.toml`.
- Enforce the soft budgets with reservations, so that concurrent requests cannot overshoot them, and keep every request below the Root's ceilings by a guard checked against the Root's own spend (§3.8).
- Expose `GET /_init/stats`, `GET /_init/health`, `GET /_init/models`, `GET /_init/traces…`.
- Expose an in-process control API (`ProxyHandle`) to supervise and the gate. Panic is not part of it: it belongs to the Root (root/root.md X4).
- Write the trace store and the kernel's spend ledger (`vault/traces/v1/`).
- Report the tool list each scope actually sent to the model, the tool calls the model actually made, and the per-model usage of each scope.

**Non-goals**
- **No keys, no auth injection, no TLS, no hard ceilings.** These belong to the Root egress (root/egress.md). The kernel proxy drops any auth header the body sends and never adds one (§3.3).
- **No normalization of wire formats.** The body speaks each provider's native format (body/provider.md). The proxy parses only what it needs: `model`, `stream`, `max_tokens`, tools, leading system content, usage and tool calls.
- **No general egress proxy.** It forwards only model-provider routes, and only to the Root egress socket. The Root's external-action classes (R3) govern every other kind of egress.
- **No prompt rewriting, no injected `cache_control`, no injected `stream_options`.** Byte transparency is what makes the trace honest and the cache behaviour the body's own (spec §9 rank 1).
- **No response cache.** "Cache" in this doc always means prompt-cache accounting: hit rates, `prefix_hash`, head hashes. The proxy never answers a model request from stored responses. That would be the "cache answer keys" mutant class of the gate-of-gates (spec v0.3 §16).
- **No retries after the first response byte has been forwarded.** Mid-stream failures go to the body (§3.9).
- **No model choice of its own.** The body chooses the model for each request. The proxy enforces the level's routing policy and records the choice (§3.5a).
- **No sealed traffic (v03-decisions D5).** Sealed audits and gate-of-gates runs never pass through this proxy, or through any agent-zone process that can persist data. The Root launches an isolated reference body and evaluator on its own egress lane, and no trace of S is written anywhere the agent zone can read. The one place kernel proxy code meets sealed material is `gate judge` (kernel/gate.md §4.1a). That runs only inside the Root's isolated environment, with no trace store and no ledger file (`judge` mode: observations in memory only).

## 3. Design

### 3.1 Process model and lifecycle

```
 init-kernel supervise  (one process; kernel/supervise.md §3.2)
 ├── std threads: supervisor, initctl server, owner channel, watcher, gate jobs …
 │        │ in-process calls: ProxyHandle (§4.4)
 │        ▼
 └── proxy: tokio runtime, 2 worker threads ─────────────────────────────────────────────┐
     listeners: run/proxy.sock (lane "live")   probe/<id>/run/proxy.sock (one per probe)  │
     per connection: hyper http1 server ─► router ─► routing ─► admission (soft budgets)  │
                                            ▼                                             │
                  egress client: HTTP/1.1 keep-alive pool over the Root egress socket     │
                  (Unix; hyper client::conn::http1), WebSocket upgrades passed through    │
                                            ▼  ── ROOT (root/egress.md): keys, TLS, h2 pool, ceilings ──► providers
                                            ▼                                             │
                  MeteredBody: forwards frames unchanged, taps bytes into a parser        │
                  accounting (Mutex) ─► stats ─► GET /_init/stats                         │
                  trace writer (std thread) ─► vault/traces/v1/{req,packs}                │
 ─────────────────────────────────────────────────────────────────────────────────────────┘
```

- **In-process, own runtime.** The proxy runs inside `init-kernel supervise` on its own tokio runtime (kernel/supervise.md §3.2 owns the topology). The rest of the kernel stays std-only and talks to the proxy only through `ProxyHandle` (§4.4), a cheap `Clone` handle over channels and `Arc` state. A panic inside a proxy task is contained by tokio (the task's `JoinHandle` returns an error and the connection is dropped). The proxy code itself has no `unwrap()` on input-derived data. See adr/002 for why the proxy is not a separate process.
- **Where the persistent connections live (v0.3).** The persistent TLS and h2 connections to providers belong to the Root egress (root/egress.md). The Root outlives every body restart *and* every kernel restart, so spec §3's "persistent upstream connections" now also survive a kernel upgrade at L3. The kernel keeps only a keep-alive pool of local connections to the egress socket (§3.7).
- **Inside the agent zone.** The kernel runs in the agent zone (spec v0.3 §13). Its only network path is the Root egress socket. root/root.md decides the zone's OS user and sandbox.
- **No standalone mode.** Kernel-only tests run the whole kernel under `init-fakeroot` (testing.md §3.5b), which implements the Root's kernel-facing contract and a loopback egress socket that forwards to `init-mockllm`. The kernel never talks to a provider directly, not even in tests (roadmap.md §3.7 item 7).
- **Runtime.** `tokio` 1.53.3 multi-thread runtime, `worker_threads = proxy.worker_threads` (default 2), thread names `init-proxy-*`. One `std::thread` runs the trace writer (blocking file I/O).
- **Start sequence** (`Proxy::start(cfg, alerts: AlertSink) -> ProxyHandle`, called by supervise before the first body spawn):
  1. Validate the `[model]`, `[proxy]`, `[budget]` and `[providers.*]` sections (kernel/config.md). On an error, supervise halts with `E_CONFIG`.
  2. Open the Root egress socket at the path the Root gives in its boot block (`hello` result `paths.egress_sock`, `ROOT_HOME/run/egress.sock`; root/root.md §4.1.3), which supervise passes to `Proxy::start`. Then take the **Root state** (`RootState`, §4.4): supervise relays the result of `ceilings.get` and every later `ceilings` and `level` event from the Root channel (root/root.md §4.1.3–§4.1.4); between events the proxy also reads `GET /_root/ceilings` on the egress socket (root/egress.md §4.3) for fresh spend. If the socket cannot be reached, the proxy still starts. Every metered request then gets 503 `egress.unavailable`, and the proxy reconnects with backoff from 1 s to 30 s.
  3. No TLS setup. TLS terminates in the Root, so the v0.2 rustls crypto-provider install is gone from the kernel.
  4. Rebuild today's soft-budget totals from `vault/traces/v1/req/` (§3.10.4).
  5. Bind `run/proxy.sock` (supervise unlinks stale sockets at start under its single-instance lock; initctl.md §3.3).
  6. Open one connection to the egress socket. Warming provider connections is the egress's job.
  7. Return the handle. Readiness is the return of `start`.
- **Root state refresh.** The proxy re-reads `GET /_root/ceilings` every `proxy.root_poll_s` (10 s) and after every completed request, so the guard of §3.8 tracks the Root's own metering. Supervise calls `ProxyHandle::set_level` when the Root announces a level change (root/root.md). A level change applies to the next admission. A gate keeps the level it opened with (§3.5a).
- **Shutdown** (`ProxyHandle::shutdown(grace)`, called by supervise on stop): stop accepting, wait up to `proxy.shutdown_grace_s` (30 s) for in-flight streams, flush and fsync the trace writer, remove the lane socket files.
- **Panic.** The Root owns panic (R1; root/root.md X4 moved `ProxyHandle::panic` there). A panic stops the agent zone and closes the egress, whatever the kernel does. In-flight requests then fail with a broken egress connection (`upstream.transport`) and are settled at their estimate (§3.10.3). The proxy has no panic or resume call and no `kernel.panicked` code.
- **Reload** (`ProxyHandle::reload(Reload::Config | RootState | All)`): re-reads config, Root state or both. In-flight requests finish under the old values.

### 3.2 The HTTP/1.1 server on Unix sockets

```rust
// crates/init-kernel/src/proxy/server.rs — signatures and key logic.
// hyper 1.12.0, hyper-util 0.1.21, http-body-util 0.1.5, tokio 1.53.3
use hyper::{body::Incoming, server::conn::http1, service::service_fn, Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use http_body_util::combinators::BoxBody;
use bytes::Bytes;
use std::{sync::Arc, time::Duration};
use tokio::net::UnixListener;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type ProxyBody = BoxBody<Bytes, BoxError>;

pub async fn serve_scope(listener: UnixListener, scope: Arc<Scope>, st: Arc<ProxyState>) {
    loop {
        let (stream, _addr) = match listener.accept().await { Ok(x) => x, Err(_) => continue };
        // Defence in depth: the socket is 0600, but also refuse other uids.
        match stream.peer_cred() {
            Ok(c) if c.uid() == st.own_uid => {}
            _ => continue, // drop the connection silently
        }
        let (scope, st) = (scope.clone(), st.clone());
        tokio::spawn(async move {
            let svc = service_fn(move |req: Request<Incoming>| handle(req, scope.clone(), st.clone()));
            let conn = http1::Builder::new()
                .keep_alive(true)
                .timer(TokioTimer::new())                       // required by header_read_timeout
                .header_read_timeout(Duration::from_secs(st.cfg.proxy.header_read_timeout_s))
                .max_headers(64)
                .serve_connection(TokioIo::new(stream), svc);
            let _ = conn.await;
        });
    }
}

async fn handle(req: Request<Incoming>, scope: Arc<Scope>, st: Arc<ProxyState>)
    -> Result<Response<ProxyBody>, BoxError>;
```

- Socket file mode `0600`, owner = kernel uid, created with `umask(0o077)` before `bind`. `run/proxy.sock` path is fixed by CONVENTIONS §4.
- One request per connection at a time (HTTP/1.1 without pipelining). The body may hold several connections for parallel requests.
- Responses are chunked (`transfer-encoding: chunked`) when the upstream body has no length; hyper does this automatically.
- `GET /_init/*` is answered locally (§4.3). Everything else goes to the router.

### 3.3 Keys: none in the kernel (v0.3)

**Decision (spec v0.3 §14 R2, §19):** the kernel holds no provider keys. The Root holds them, injects them, and owns their storage, setup command and file-safety rules (root/egress.md). The owner stores them once at install with `init-root keys set` (spec v0.3 §18).

What stays in the kernel proxy:
- **Strip, never add.** The body's `authorization`, `x-api-key`, `api-key` and `cookie` headers are dropped (§3.6). The proxy adds no credential, so nothing in the agent zone ever holds a key, at any level.
- **Defence in depth on upstream auth errors.** An upstream 401 or 403 that reaches the kernel is still rewritten to 401 `upstream.auth`, with the body withheld (§3.9). The Root should already have rewritten it (§9 cross-doc).

The v0.2 decision (plain files in `vault/keys/`, mode 0600, rather than the macOS Keychain) and its reasons are handed to root/egress.md as input. `vault/keys/`, `init-kernel keys set` and `init-kernel keys check` no longer exist in the kernel.

### 3.4 Scopes (lanes) and their sockets

A **scope** is the unit of attribution and budgeting. It is the proxy side of what kernel/supervise.md and kernel/initctl.md call a **lane**: one lane socket is one scope. Every request belongs to exactly one scope, and the scope is decided by the socket the request arrived on, never by anything the body says.

| Scope kind | Socket (path chosen by supervise) | Opened by | Visibility of traces | Budget (§3.8) |
|---|---|---|---|---|
| `live` | `run/proxy.sock` | proxy start | `agent` | the soft day budget, the guard on the Root's day and total ceilings, plus per-task budgets |
| `member` (L2) | `run/fleet/<bid>/proxy.sock` (kernel/supervise.md §3.14) | supervise, when it starts fleet member `<bid>` | `agent` | the member's daily share: `budget_share_musd_day` from `initctl fleet spawn`, else `fleet.default_share_musd_day` ($0.50); plus the soft day and the Root guard |
| `probe` | `probe/<probe_id>/run/proxy.sock` | gate via supervise `probe()` (G2 arms, G3) | `agent` | `budget.probe_usd` and the gate budget |
| `run` | `probe/<probe_id>/run/proxy.sock` | gate (one G4 task run) | `agent` for pool tasks; `kernel` for the gate's own holdout and canary runs (agent-visible from L3, kernel/gate.md §3.7.1a) | the task's `max_cost_musd` and the gate budget |
| `quick` | `probe/<probe_id>/run/proxy.sock` | gate (`gate --quick` probes) | `agent` | `budget.quick_gate_usd` |

- The caller passes the socket path to `open_scope`. It must lie under `$INIT_HOME/probe/` or `$INIT_HOME/run/fleet/` (member lanes), or be `$INIT_HOME/run/proxy.sock`, and its byte length must be ≤ 103. With `INIT_HOME` ≤ 64 bytes (kernel/supervise.md §3.1) and a 12-hex probe id, the longest path is 64 + 33 = 97 bytes.
- `probe/` is invisible to the live sandbox, and each probe sandbox can reach only its own root (kernel/supervise.md §3.6.3, kernel/sandbox.md). So no body can connect to another lane's socket and spend under its name. kernel/sandbox.md §3.3.4 allows kernel sockets only as literal paths.
- Scope ids: `live`, or `<job_id>/<name>`, where `<job_id>` is the initctl job id of the gate (`j-000043`, kernel/initctl.md §3.6) and `name` ∈ `probe-c`, `probe-p`, `g3`, `run/<run_id>`, or `member:<bid>` for a fleet member. Grammar: `[a-zA-Z0-9][a-zA-Z0-9._/:-]{0,127}`.
- A scope carries a `label` (`{sha, boot_reason, model, level}`), set by supervise (`set_label`) for `live` at every body boot and by the gate at `open_scope` for the others. `model` is the **default model** (`model.id`, the routing default), not a restriction from L1 on (§3.5a). `level` is the autonomy level the scope runs under. Labels go into every request record.

**Task attribution inside `live`** (answer to evolution.md Q1). The body may send `x-init-task: <task_id>` (grammar `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}`) on each request. The proxy records it and applies the per-task budget to it. Requests without the header are attributed to the task id `_none`, which has the same budget, so omitting the header cannot bypass the per-task budget. The header is ignored in gate scopes, where the scope itself is the attribution.

```rust
pub struct Scope {
    pub id: String,                       // "live" or "<job_id>/<name>"
    pub kind: ScopeKind,                  // Live | Member(bid) | Probe | G3 | Run | Quick
    pub job_id: Option<String>,           // the gate job (initctl job id), None for live
    pub visibility: Visibility,           // Agent | Kernel
    pub cap_pusd: Option<u64>,            // scope budget (None for live: the soft day and the Root guard apply)
    pub label: std::sync::Mutex<Label>,   // {sha, boot_reason, model (default), level}
    pub socket: Option<std::path::PathBuf>,
    pub state: std::sync::Mutex<ScopeState>, // counters, observations (§3.12)
}
```

### 3.5 Routing and route allowlists

`POST /<provider_id>/<upstream path>[?query]`, with `provider_id` matching `[a-z][a-z0-9_-]{0,31}` and present in config. The request is forwarded to the Root egress socket as `<method> /<provider_id>/<upstream path>` if and only if `(method, upstream path)` is in that provider's route table and the Root forwards that provider (`RootState.models`). The Root maps the provider id to its base URL and enforces its own allowlist (root/egress.md). Each kernel route also names the **wire family** the proxy uses to parse it.

The route allowlist exists because a provider key can do far more than chat. Examples: OpenAI fine-tuning and batch jobs, file deletion, admin endpoints; DeepSeek's billed Search and Fetch APIs ($0.75 per 1,000 calls, checked 2026-10-19). None of these would be visible to token-based accounting. In v0.3 the Root's external-action classes (R3) are the authoritative allowlist. The kernel table is a stricter subset that also gives the parse family, and it fails fast with a clear error.

**Built-in route tables** (`preset`; the owner can add routes in config, kernel/config.md):

| Preset | Method | Upstream path | Family | Metered | Auth header |
|---|---|---|---|---|---|
| `deepseek` | POST | `chat/completions` | `openai-chat` | yes | `Authorization: Bearer` |
| | POST | `beta/chat/completions` | `openai-chat` | yes | Bearer |
| | POST | `responses` | `openai-responses` | yes | Bearer |
| | POST | `anthropic/v1/messages` | `anthropic` | yes | `x-api-key` |
| | GET | `models` | `none` | no | Bearer |
| | GET | `user/balance` | `none` | no | Bearer |
| `anthropic` | POST | `v1/messages` | `anthropic` | yes | `x-api-key` |
| | POST | `v1/messages/count_tokens` | `none` | no | `x-api-key` |
| | GET | `v1/models` | `none` | no | `x-api-key` |
| `openai` | POST | `v1/responses` | `openai-responses` | yes | Bearer |
| | POST | `v1/chat/completions` | `openai-chat` | yes | Bearer |
| | GET | `v1/models` | `none` | no | Bearer |

Routes marked `Metered: no` (`GET models`, `user/balance`, `count_tokens`) are forwarded and passed through with no budget check and no trace record.

The "Auth header" column is informational in v0.3: it records how the Root authenticates each route (root/egress.md). Base URLs are the Root's too: DeepSeek `https://api.deepseek.com` (the Anthropic-format endpoint is the same host with path prefix `anthropic`); Anthropic `https://api.anthropic.com`; OpenAI `https://api.openai.com`.

**Routing algorithm**

```text
fn route(method, path, query) -> Result<Route, ProxyError>:
    if path starts with "/_init/": return Local
    (pid, rest) = split_once(path.trim_start('/'), '/')         else 404 route.unknown
    provider = cfg.providers[pid]                                 else 404 route.unknown_provider
    if !provider.enabled or pid not in root_state.models:              503 provider.disabled
    if rest contains "..", "//", "%2e", "%2f" (case-insensitive):      400 route.bad_path    // before the route lookup, so ".." is a 400, not a 403
    r = provider.routes.find(method, rest)                        else 403 route.not_allowed
    if query non-empty and method == POST:                             400 route.query_not_allowed
    return Upstream { egress_path: "/" + pid + "/" + rest + query?, family: r.family }
    // there is no direct upstream, not even in tests: init-fakeroot provides a loopback egress (§4.1)
```

### 3.5a Model routing by the agent (L1+)

Spec v0.3 §15 gives L1 the right to "route models per subtask within the ceiling". The body makes the choice; the proxy enforces the level's policy and attributes it. The proxy never substitutes a model.

**Policy by level.** The level comes from the Root's autonomy ledger (R7, root/root.md) through `RootState` and `ProxyHandle::set_level` (§3.1).

| Level | `model` allowed on a metered route | Refusal |
|---|---|---|
| L0 | the scope label's default model (`model.id`, kernel/config.md §4.1) only | 400 `request.model_not_routed` (routing unlocks at L1) |
| L1 and up | any model in the **routable set**: priced in `[providers.<id>.models]` (kernel/config.md §4.5), forwarded by the Root (`RootState.models`, which reflects the charter), and listed in `model.routable` if that list is non-empty | 400 `request.model_not_allowed` |

- **No `models` table.** If `GET /_root/ceilings` has no `models` table, the proxy treats the Root as forwarding every priced model; the Root's own allowlist still decides.
- **Per subtask.** The body may label a request with `x-init-subtask: <id>`, using the same grammar as `x-init-task` (§3.6). The label is free text chosen by the body: a plan step or a sub-agent, for example. It is recorded, never enforced. Model choice is per request, so a subtask can mix models.
- **Within the Root's ceilings.** The reservation uses the chosen model's prices (§3.6), so an expensive model uses up the soft budgets sooner. The Root's day and total ceilings bound the sum (§3.8), and the Root may refuse a model outright under the charter. A Root refusal is mapped to the kernel's error shape and never retried (§3.9, §4.2).
- **Gate scopes.** A gate snapshots the level and the routable set at `open_gate`, and every scope of that gate uses the snapshot. Both G4 arms therefore route under identical rules. Routing is then part of the body under test, and its cost shows up in the arms' reference cost (kernel/gate.md §3.7.4).
- **Attribution.** Every request record carries `model` (what the request named), `default_model` (the scope label's), `routed` (`model ≠ default_model`), `subtask` and `level` (§3.13). `/_init/stats` reports today's requests and spend per model and per (task, model) (§4.3). `ScopeSummary.by_model` gives per-model requests and reference cost, which the gate copies into each RunRecord (kernel/gate.md §4.4) and selfmap uses for per-model capability (kernel/selfmap.md §3.12).
- **What stays keyed to the default model:** the thinning baseline (evolution.md §3.12, only on a `model.id` change), the boot-time model check (body/boot.md), and the gate's noise history (`vault/gate/noise.json`, keyed by default model).
- **Budgets at L1.** At L1 the agent also allocates its daily budget (spec v0.3 §15): it sets the soft budgets within the bounds of §3.8.

### 3.6 Request handling: headers, parsing, admission

**Attribution headers read and stripped** (sent by the body, body/provider.md §3.6; never forwarded):

| Header | Grammar | Use |
|---|---|---|
| `x-init-session` | `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}` | trace chain key, `gap_ms` and `session_permille`; absent → `_none` |
| `x-init-purpose` | `turn` \| `probe` \| `compaction` | recorded; `turns` metrics count only `turn` |
| `x-init-attempt` | `1`..`99` | recorded; lets traces tell a body retry from a new request |
| `x-init-task` | `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}` | per-task budget and attribution in `live` (§3.4) |
| `x-init-subtask` | `[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}` | model-routing attribution (§3.5a); recorded in every scope; absent → `null` |

Malformed values are treated as absent; they never fail the request.

**Headers forwarded to the egress (allowlist).** `content-type`, `accept`, `anthropic-version`, `anthropic-beta`, `openai-beta`, `idempotency-key`. Everything else from the body is dropped, including any `authorization`, `x-api-key`, `api-key`, `cookie`, `x-init-*` and hop-by-hop headers. The proxy then sets:
- no auth header. The Root injects the key (root/egress.md);
- `anthropic-version: 2023-06-01` if the route family is `anthropic` and the body sent none (Anthropic's only current version, checked 2026-10-19);
- `user-agent: init-kernel/<version>`. It carries nothing about the owner. The Root may replace it;
- `accept-encoding: identity`, so the response stream can be parsed without decompression;
- the guard report `x-egress-guard` (§3.8, D6);
- the attribution headers of root/egress.md §4.1: `x-egress-scope: live|gate|member`, `x-egress-body: <body instance id>` (the body's Root instance id, root/egress.md §4.1, not its boot id; for gate scopes the probe id) and `x-egress-reservation: <req_id>` (§3.13), so that the Root's lineage and metering records can be joined with kernel traces. The Root uses them only for labels, never for a ceiling decision;
- the sub-task label `x-egress-subtask` (v0.3.0, body/tools.md §3.7): a copy of the body's `x-init-subtask`, forwarded verbatim only when it matches the grammar above (≤ 64 bytes); a malformed or absent label forwards nothing. The Root's sentinel counts these requests apart from the main loop (ADR-011 §3.1 `sub_requests`, so H10 and H5b are not raised by `delegate`) and never sends the header to the provider. X5's `x-egress-caps` (a copy of `x-init-caps` up to 1 KiB, ADR-011 §5.2) travels the same way.

Account headers (`OpenAI-Organization`, `OpenAI-Project`) belong with the key and are set by the Root (root/egress.md). They are no longer kernel config.

**Body read.** `http_body_util::Limited::new(req.into_body(), proxy.max_request_bytes)` (32 MiB, which matches Anthropic's Messages limit), then `collect().await?.to_bytes()`. Over the limit: 413 `request.too_large`. Request bodies are buffered on purpose: they are needed whole for hashing, caps and the trace, and the copy is local and fast.

**Minimal parse.** One pass of `serde_json` (1.0.154, feature `raw_value`) into a borrowed view; nothing is re-serialized.

```rust
use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
pub struct ReqView<'a> {
    pub model: Option<&'a str>,
    #[serde(default)] pub stream: bool,
    pub max_tokens: Option<u64>,                    // openai-chat, anthropic
    pub max_completion_tokens: Option<u64>,         // openai-chat
    pub max_output_tokens: Option<u64>,             // openai-responses
    #[serde(borrow)] pub tools: Option<&'a RawValue>,
    #[serde(borrow)] pub system: Option<&'a RawValue>,        // anthropic
    #[serde(borrow)] pub instructions: Option<&'a RawValue>,  // openai-responses
    #[serde(borrow)] pub messages: Option<Vec<&'a RawValue>>, // openai-chat, anthropic
    #[serde(borrow)] pub input: Option<&'a RawValue>,         // openai-responses (string or array)
    pub stream_options: Option<StreamOptions>,
    pub service_tier: Option<&'a str>,
    pub speed: Option<&'a str>,                      // anthropic fast mode
}
#[derive(Deserialize)] pub struct StreamOptions { pub include_usage: Option<bool> }
```

The byte offset of each `&RawValue` inside the request buffer is `raw.get().as_ptr() as usize - buf.as_ptr() as usize`. The trace store uses these offsets to cut the request into a head and messages without re-serializing (§3.13).

**Pre-flight rejections (400, no upstream call):**

| Condition | Code |
|---|---|
| metered route and the body is not valid JSON | `request.bad_json` |
| metered route and `model` missing, or not in the routable set (§3.5a: priced in kernel config and forwarded by the Root) | `request.model_not_allowed`. This is the kernel-side model allowlist (body/boot.md Q-B5). The body's own check is agent-editable. At L0–L2 this one is not; from L3 it is agent-evolvable, and the Root's allowlist (R2/R3) is the one the agent can never change. Frontier-class only: the charter and the Root decide which models are forwarded, and the kernel prices only those. |
| L0 and `model` ≠ the scope label's default model | `request.model_not_routed` (§3.5a) |
| `service_tier` or `speed` set to a value without a configured multiplier (§3.10.2). The neutral values `default`, `auto` and `standard` are priced at ×1 and not refused | `request.tier_not_allowed` |
| family `openai-chat` on the `openai` preset, `stream: true`, and `stream_options.include_usage` is not `true` | `request.usage_required` (OpenAI only sends stream usage when asked; DeepSeek always sends it, §4.8.1) |

**Admission and reservation.** Before forwarding, the proxy reserves the worst-case cost of the request, at the chosen model's prices, and admits it only if every applicable soft budget and the guard on the Root's ceilings still hold with the reservation added (§3.8). The reservation is released and replaced by the actual cost when the response completes.

```text
reserve_pusd(req) =
      ceil(len(request_bytes) / budget.reserve_bytes_per_token) × price.input        // as if all uncached
    + (max_tokens | max_completion_tokens | max_output_tokens | model.reserve_output_tokens) × price.output
  using the price window in force at admission time, × any tier multiplier
```

`reserve_bytes_per_token` defaults to 3. Typical JSON-wrapped English or code runs 3.5–4.5 bytes per token, so dividing by 3 over-estimates the input; base64 images over-estimate it by far more, which only makes the reservation safer.

### 3.7 Upstream: the Root egress socket

In v0.3 the kernel proxy has exactly one upstream: the Root's egress socket (root/egress.md). The socket is a Unix socket, `ROOT_HOME/run/egress.sock`, whose path the Root gives in its boot block (§3.1; root/root.md §4.1.3). The Root authenticates, applies its ceilings and external-action classes, and forwards to the provider over its own persistent TLS and h2 connections. It also owns the connect-phase retries to the provider (root/egress.md §4.1). The kernel speaks plain HTTP/1.1 to it, keeps connections alive, and needs no TLS stack.

```rust
// hyper 1.12.0 (features "client", "http1"), hyper-util 0.1.21 (TokioIo), http-body-util 0.1.5, tokio 1.53.3.
// Shapes follow hyper 1.x client::conn::http1; not re-checked on docs.rs for 1.12.0 in this revision: UNVERIFIED.
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use bytes::Bytes;
use tokio::net::UnixStream;

pub type EgressSender = http1::SendRequest<Full<Bytes>>;

pub async fn egress_connect(path: &std::path::Path) -> Result<EgressSender, BoxError> {
    let io = TokioIo::new(UnixStream::connect(path).await?);
    let (sender, conn) = http1::Builder::new().handshake::<_, Full<Bytes>>(io).await?;
    // with_upgrades: lets a WebSocket upgrade pass through the same connection (§3.15)
    tokio::spawn(async move { let _ = conn.with_upgrades().await; });
    Ok(sender)
}
```

- **Pool.** `EgressPool` keeps up to `proxy.egress_pool_max_idle` (16) idle senders. Checkout pops an idle sender that is not closed and awaits `ready()`, or else connects a new one. After a response has been fully forwarded, the sender goes back to the pool if `is_ready()`. HTTP/1.1 carries one request per connection at a time, so N concurrent requests use N local connections. A local Unix connect costs tens of microseconds, so the pool exists for tidiness, not speed.
- **Counting.** `egress_connects` and `egress_requests` feed `/_init/stats` (`egress.reuse_permille`). Provider-side connection reuse is reported by the Root, if at all (root/egress.md).
- **Tests.** There is no direct path to a provider. Kernel-only tests reach the mock provider through `init-fakeroot`'s loopback egress socket (testing.md §3.5b), full-stack tests through the real Root with `root.test_mode = true`. `proxy.test_mode` only relaxes config checks for test homes (`_mib` disk thresholds, a provider named `mock`; kernel/config.md §3.1). It changes nothing about the network path, and the agent zone's network boundary is the Root's sandbox (root/root.md), not this flag.
- **Moved to the Root:** provider TLS (rustls, the platform verifier), HTTP/2 negotiation and multiplexing, h2 keep-alive PINGs, the provider-side connection pool and its warm-up. v0.2's evidence for these (DeepSeek accepts h2 through CloudFront, observed 2026-10-19; the `ring` crypto provider choice) is handed to root/egress.md.

### 3.8 Soft budgets under the Root's ceilings

All money is integer **pico-dollars** (`pusd`, 10⁻¹² USD) in `u64` (max ≈ $18.4M). Config prices are decimal strings in USD per million tokens. They are parsed exactly into micro-dollars per million tokens, which equals pusd per token. For example `"0.003"` becomes 3,000 pusd per token and `"1.20"` becomes 1,200,000. No floats are ever used for money.

**Two layers** (spec v0.3 §14 R2, §15; evolution.md §3.14 owns the policy, root/egress.md §3.3 the ceilings):

| Layer | What | Set by | Enforced by | On exceed |
|---|---|---|---|---|
| **Hard ceilings** | money per day (`ceilings.day_musd`) and in total (`ceilings.total_musd`); compute, disk and parallel-body quotas | the owner, in the charter (R5, root/root.md) | the Root egress (R2, root/egress.md), whatever the kernel does | the Root refuses; the attempt is a demotion trigger (spec v0.3 §15) |
| **Soft budgets** | day, task, gate, quick gate, probe, scope, request, evolution share | maintainer defaults in kernel config at L0 (kernel/config.md §4.3); the agent from L1 (below) | this proxy | a typed 429 `budget.*`; never a demotion trigger |

| Limit | Value | Applies to | Window | 429 `error.code` |
|---|---|---|---|---|
| Day (soft) | `budget.day_usd`, clamped to at most the Root's day ceiling − `budget.request_max_usd` (and `evolution.soft_day_fraction` × the ceiling) | all scopes | the Root's day | `budget.day` |
| Total (soft) | the Root's total ceiling − `budget.request_max_usd` (no config key), checked against the Root's lifetime spend figure because the kernel keeps no lifetime ledger | all scopes | lifetime | `budget.total` |
| Root day headroom (guard) | the Root's day ceiling × (1 − guard), against the **Root's** day spend | all scopes | the Root's day | `budget.root_day` |
| Root total headroom (guard) | the Root's total ceiling × (1 − guard), against the Root's total spend | all scopes | lifetime | `budget.root_total` |
| Evolution share | `budget.evolution_share` × the soft day | all gate scopes together | the Root's day | `budget.evolution` |
| Task | `budget.task_usd` | one `x-init-task` id (or `_none`) in `live` or a `member` lane | the Root's day | `budget.task` |
| Member share (L2) | the member's daily share (§3.4); the sum of open members' shares may not exceed the soft day minus the evolution share, and `open_scope` for a member is refused (`budget.member`) beyond it | one fleet member's lane | the Root's day | `budget.member` |
| Gate | `budget.gate_usd` (promote) / `budget.quick_gate_usd` (quick) | all scopes with one gate job id | gate lifetime | `budget.gate` |
| Scope | per scope at `open_scope` | one gate scope | scope lifetime | `budget.scope` |
| Request | `budget.request_max_usd` | one request's reservation | — | `budget.request` |

**Why the guard checks the Root's own number.** The Root settles every request at its coarse **peak** price (root/egress.md §3.4), while the kernel measures at the real window price. On DeepSeek, peak is 2× off-peak, so on an off-peak day the Root's day spend can be about twice the kernel's. A soft day at 0.9 of the ceiling, checked only against the kernel's own ledger, could then let the Root's ceiling be reached and refused, which is a demotion trigger. So the kernel also keeps the Root's figure in view:

```text
guard          = budget.root_guard_permille                     // default 20 (2%)
spent_view(c)  = root_spent(c) + root-priced estimate of kernel-settled requests since that Root report
                 // root_spent: day_spent + day_reserved of the last GET /_root/ceilings figure (root/egress.md §4.3),
                 // refreshed every proxy.root_poll_s and after every response; the estimate prices usage at the Root's peak table when the
                 // Root publishes it, else at the model's highest configured window (over-counts: safe)
admit only if    spent_view(c) + c.reserved_root + r_root  ≤  c.ceiling × (1000 − guard) / 1000   for c ∈ {day, total}
```

Here `r_root` is the reservation recomputed at peak prices. Normally the soft day budget refuses first. The guard catches the cases where peak-priced settlement or a stale soft value would otherwise let a request reach the Root's ceiling. Under D6 such ordinary ceiling hits only get logged, but avoiding them keeps the logs meaningful. The estimate counts requests settled after the figure was requested, so it also over-counts requests still in flight, which is the safe direction.

**What demotes (D6), and the guard report.** A Root refusal demotes only in three cases: an action-class or policy violation; a level-gated request the Root refuses; or a ceiling refusal at a moment when this proxy's guard reported headroom, that is, the guard was bypassed. So that the Root can tell the third case from an ordinary hit, every forwarded request carries the guard's view:

```text
x-egress-guard: epoch=<E>
```

`E` is the money `epoch` of the Root figure the guard used (`GET /_root/ceilings`). A correct proxy never forwards a request that its own view shows over the guarded limit. root/egress.md §3.6 owns the classification, and the Root makes it from the epoch alone: a ceiling refusal counts as a bypass, and demotes, if the header is missing or malformed, if `E` has left the Root's ring of recent epochs, if the epoch after `E` settled more than 30 s before the admission, or if by the Root's own spend at epoch `E` the request already did not fit (§9 Q-EG4). Otherwise it is metering lag, an ordinary hit. The proxy only guarantees the header is true. The older form `day=…;total=…;seen=…;r=…` is dropped.

**Admission rule** (under the accounting mutex):

```text
admit(req, scope):
    r = reserve_pusd(req)                                 // at the chosen model's prices (§3.6, §3.5a)
    if r > request_max:                                  reject budget.request (no retry-after)
    for each limit c applicable to (scope, task, gate), including the soft day and the Root guards:
        if c.spent_view + c.reserved + r_c > c.limit:    reject c.code (retry-after = c.reset if any)
    for each c: c.reserved += r_c
    return Reservation{r, limits}
settle(res, actual):            // on completion, error, or client abort
    for each c in res.limits: c.reserved -= res.r_c; c.spent += actual_c
```

**Bound.** Kernel spend can exceed a soft budget only by the amount that actual usage exceeds the reservations of requests already in flight. Because the reservation assumes zero cache hits and the full output limit, that needs a wrong input estimate, for example a request whose text runs below 3 bytes per token. Once spend reaches a limit, every new request on it is refused. Total money can never exceed the Root's ceilings, whatever the kernel does.

**Who sets the soft budgets** (evolution.md §3.14):
- **L0:** maintainer defaults from kernel config, fixed: `day_usd` $5.00, `task_usd` $0.50, `gate_usd` $3.00, `evolution_share` 0.6, `quick_gate_usd` $0.50, `probe_usd` $0.25, `request_max_usd` $1.00.
- **L1–L2:** the agent, with `initctl budget set <key>=<value>…` (kernel/initctl.md, proposed by evolution.md §4.1), which calls `ProxyHandle::set_budgets`. Values are refused with `E_BUDGET` (the proxy's `ProxyError::BudgetInvalid`) when:
  - `day_usd` above the clamp bound (the Root's day ceiling − `request_max_usd`, and `soft_day_fraction` × the ceiling);
  - `evolution_share` > 0.8 (tasks always keep at least 20% of the day);
  - `gate_usd` > `evolution_share × day_usd`;
  - `task_usd` > `day_usd`;
  - any value < $0.01.
- **L3 and up:** the agent owns this code, so anything under the ceilings is possible. The Root's ceilings are the only hard limit.
- **Persistence.** Effective values are written to `state/kernel/budget.json` (kernel-written, agent-readable; `source: "config"|"agent"`). They are reported to Root lineage as a `budget` record (proposed). A new gate budget applies to gates opened afterwards; a running gate keeps its own. On a demotion the soft budgets reset to the new level's defaults (evolution.md §3.15.3).
- **Clamping, never refusing (D6; root/root.md X4).** The effective soft day is `min(day_usd, ceiling_day − request_max_usd, soft_day_fraction × ceiling_day)`; the effective soft total is `ceiling_total − request_max_usd`. Both are recomputed on every `ceilings` event. When a ceiling is lowered, the soft budgets shrink at once and the alert `soft_day_clamped` is raised. Nothing refuses to start. Subtracting `request_max_usd` means that one maximal request admitted at the soft limit still fits under the ceiling, so the kernel refuses before the Root, by its own count.
- L2's "budget share grows with value delivered" is the Root raising the bound it grants, up to the charter maximum (R7, evolution.md §3.14). The kernel just reads the new ceiling.

**Day boundary.** The Root's day (`money.day_resets_at` in `GET /_root/ceilings`; the charter's `ceilings.day_starts_at_utc_offset`, root/egress.md §3.3) defines every daily soft budget too, so the two layers never disagree about what "today" is. The v0.2 key `budget.day_starts_at_utc_offset` is gone from kernel config (kernel/config.md). The ledger rebuild at start sums the records whose `ts` falls in `[day_start, now)`.

### 3.9 Failure handling and retry policy

The proxy retries **only** when both of these hold:
- no byte of the response has been forwarded to the body yet;
- the provider cannot have generated tokens for the request. That means either the request never left the agent zone (an egress-socket connect error), or the HTTP status means "rejected before generation". Connect-phase failures towards the provider are retried inside the Root (root/egress.md §4.1, reported as `x-egress-retries`), not here.

Everything else is passed through untouched, so the body (the agent) sees and owns it. Every retry is a new request to the Root, metered by the Root again.

| Outcome seen by the kernel | Proxy action |
|---|---|
| Egress socket connect error (socket missing, refused) | retry with backoff. After the last attempt: 503 `egress.unavailable`; alert `egress_down` |
| **Root refusal** (`x-init-egress-error`: `ceiling.*`, `quota.*`, `class.*`; root/egress.md §3.8) | **never retry.** Map it to the kernel's shape (§4.2) with `outcome = "root_refused"`; alert `root_refused`. This should not happen in normal operation, because §3.8 refuses first. |
| Error after the request was written (reset, broken connection) | **no retry**: 502 `upstream.transport` to the body; the reservation is charged as the cost (it may have been billed) |
| 429 with a rate or concurrency meaning: DeepSeek 429; Anthropic `rate_limit_error` without `error.details.error_code = "enforced_spend_limit_reached"`; OpenAI 429 whose code is not `insufficient_quota`, `credit_balance_exhausted`, `*_spend_limit_exceeded` or `organization_usage_limit_exceeded` | retry with backoff, honouring `retry-after-ms` then `retry-after` |
| 500, 502, 503, 504, Anthropic 529 `overloaded_error` (from the provider, as relayed by the Root) | retry with backoff, same header rules |
| Response header `x-should-retry: false` | never retry (the official SDKs obey it; whether servers send it is UNVERIFIED) |
| 400, 402, 404, 409, 413, 422, and the spend/quota 429s above | pass through, no retry |
| 401, 403 | **rewrite** to 401 `upstream.auth` (a status the body never retries, body/provider.md §3.9) with the original body withheld, because it can echo a masked key fragment (e.g. DeepSeek's `"Your api key: ****abcd is invalid"`, observed 2026-10-19); write alert `upstream_auth`. The Root should rewrite first (§3.3); this is defence in depth. |
| 402 (DeepSeek "Insufficient Balance") | pass through; write alert `provider_balance` |
| Error after streaming started (Anthropic `event: error`, OpenAI `response.failed`, DeepSeek `finish_reason: "insufficient_system_resource"`, broken stream) | forward as is; record `outcome = "stream_error"`; no retry |

**Backoff.** Attempt n (n = 1, 2) waits `min(8 s, 0.5 s × 2^(n−1)) × U(0.75, 1.0)`, the same shape as the official SDKs (checked 2026-10-19). A server-provided delay replaces the computed one. If the server asks for more than `proxy.retry_max_wait_s` (20 s), the proxy does not wait: it passes the response through at once so the body can decide. `proxy.retry_max` = 2. Total waiting per request is capped by `proxy.retry_budget_s` = 30 s. Each retry is admitted again against the soft budgets; it reuses the reservation, so it cannot overshoot.

After the last attempt, the final response is passed through with `x-init-retries: <n>`.

**Why the proxy retries at all.** These retries are pure transport mechanics. They are invisible in the token stream and identical for every body version, which keeps gate comparisons fair. Retrying in the proxy also respects provider `retry-after` across body restarts. The decision about any error that carries information (bad request, mid-stream failure, length stop) stays with the agent. Whether the Root also retries internally is root/egress.md's choice. If it does, the kernel's attempts multiply with the Root's, so the two should agree on one owner (§9 cross-doc).

**Timeouts** (defaults; kernel/config.md). They now measure the egress-plus-provider path.

| Timeout | Default | Why |
|---|---|---|
| connect (egress socket) | 10 s | a local socket; anything slower means the Root is stalled |
| response headers | 660 s | DeepSeek closes a request that has not started inference after 10 minutes; streaming responses usually send headers at once and then `: keep-alive` comments |
| idle between response frames | 330 s | DeepSeek sends keep-alive comments while queued, so silence of 5 minutes means a dead stream. Set 30 s above the body's own 300 s idle timeout (body/provider.md §3.9) so the body's timeout always fires first and the body owns the retry |
| total per request | 3,600 s | 384K output tokens at 150 tok/s is about 43 minutes |
| body → proxy header read | 30 s | hyper default |

On a timeout after streaming began, the proxy ends the response body with an error (the body sees a truncated chunked stream), records `outcome = "timeout"`, and charges an estimate (§3.10.3). A request whose handler is cancelled before it completes (the body disconnected while the proxy was still waiting for headers) is settled at its full reservation and writes no `end` record.

### 3.10 Usage extraction and spend accounting

#### 3.10.1 The metered body: forwarding without buffering

```rust
use http_body::{Body, Frame, SizeHint};
use std::{pin::Pin, task::{Context, Poll}};

pub struct MeteredBody {
    inner: Incoming,                 // hyper::body::Incoming: Send + Sync + Unpin (docs.rs, checked)
    tap: Tap,                        // per-family incremental parser + timers
    idle: Pin<Box<tokio::time::Sleep>>, // reset on every frame
    done: bool,
}

impl Body for MeteredBody {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<Option<Result<Frame<Bytes>, BoxError>>>
    {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.tap.on_bytes(data);       // O(len); never blocks; Bytes clone is a refcount
                }
                this.idle.as_mut().reset(tokio::time::Instant::now() + this.tap.idle_timeout);
                Poll::Ready(Some(Ok(frame)))       // forwarded unchanged, immediately
            }
            Poll::Ready(Some(Err(e))) => { this.tap.on_error(&e); this.done = true; Poll::Ready(Some(Err(e.into()))) }
            Poll::Ready(None) => { this.tap.on_end(); this.done = true; Poll::Ready(None) }
            Poll::Pending => {
                if this.idle.as_mut().poll(cx).is_ready() {
                    this.tap.on_timeout(); this.done = true;
                    return Poll::Ready(Some(Err("upstream idle timeout".into())));
                }
                Poll::Pending
            }
        }
    }
    fn is_end_stream(&self) -> bool { self.inner.is_end_stream() }
    fn size_hint(&self) -> SizeHint { self.inner.size_hint() }
}

impl Drop for MeteredBody {
    fn drop(&mut self) { if !self.done { self.tap.on_abort(); } } // body disconnected mid-stream
}
```

`Tap` holds:
- a line splitter with a bounded carry-over buffer (`proxy.max_sse_line_bytes`, 1 MiB; past that it stops parsing and flags `usage_missing`);
- the family parser (§3.10.2);
- timestamps: `t_sent` (request handed to the client), `t_headers`, `t_first_token`, `t_first_content`, `t_last_token`;
- a trace accumulator: a `Vec<Bytes>` of the raw frames, capped at `traces.max_response_bytes` (16 MiB; beyond that it records `resp_truncated`).

When the tap finishes (end, error, timeout or abort) it sends one `Completion` message over an unbounded `tokio::sync::mpsc` channel to the accounting task. Sending never blocks the stream.

**Cheap pre-filter.** The parser JSON-decodes an SSE `data:` line only when it might matter:
- before the first content token, every data line, to detect it;
- after that, only lines that contain a usage object, a finish or stop reason, an error, or a tool-call start or delta.

The test is a byte search for `"usage"` (followed by optional whitespace, `:`, optional whitespace, `{`), `"finish_reason"`, `"stop_reason"`, `"tool_calls"`, `"tool_use"`, `function_call`, `"type":"error"` or `event: error`. DeepSeek puts a space after the colon (`"usage": {`, observed in its docs' stream example), so the matcher skips whitespace. Text deltas after the first token are never parsed. The tap helper's edge rules (the line bound, which stops parsing for the rest of the response) are recorded as doc comments in `tap.rs`.

#### 3.10.2 Per-family parsing and normalization

```rust
#[derive(Default, Clone, Copy, serde::Serialize)]
pub struct Usage {                 // normalized; all counts are tokens
    pub input_uncached: u64,       // billed at price.input
    pub input_cache_read: u64,     // billed at price.cache_read
    pub input_cache_write: u64,    // billed at price.cache_write (Anthropic 5m, OpenAI ≥5.6)
    pub input_cache_write_1h: u64, // billed at price.cache_write_1h (Anthropic)
    pub output: u64,               // billed at price.output; includes reasoning
    pub reasoning: u64,            // informational subset of output
}
impl Usage {
    pub fn input_total(&self) -> u64 { self.input_uncached + self.input_cache_read + self.input_cache_write + self.input_cache_write_1h }
    pub fn cache_permille(&self) -> Option<u32> {      // integer only (ICJ): floor(read × 1000 / total)
        let t = self.input_total(); (t > 0).then(|| (self.input_cache_read * 1000 / t) as u32)
    }
}
```

**`openai-chat`** (DeepSeek Chat Completions, OpenAI Chat Completions)
- Stream: `data: {json}\n\n` lines, terminated by `data: [DONE]`; comment lines (`: keep-alive`) are ignored.
- First token: the first chunk where `choices[0].delta` has a non-empty `content` or `reasoning_content`, or any `tool_calls`. DeepSeek's first chunk carries `"content": ""` with `role`; that is not a token. First content: the first non-empty `content` or `tool_calls`.
- Usage: the `usage` object of the last chunk. DeepSeek always sends it, whether or not `stream_options.include_usage` is set. It rides on the last content chunk, whose single choice has a non-null `finish_reason`; there is no separate usage-only chunk (DeepSeek API reference, checked 2026-10-19).
- Normalization:
  - DeepSeek: `input_cache_read = prompt_cache_hit_tokens`, `input_uncached = prompt_cache_miss_tokens` (and `prompt_tokens = hit + miss`), `output = completion_tokens`, `reasoning = completion_tokens_details.reasoning_tokens`.
  - OpenAI chat: `input_cache_read = prompt_tokens_details.cached_tokens`, `input_uncached = prompt_tokens − cached`, `output = completion_tokens`.
- Tool calls: `choices[0].delta.tool_calls[]` by `index`. The first chunk of each call carries `id`, `type` and `function.name`; later chunks carry only `function.arguments` fragments, which are concatenated.
- Finish: `choices[0].finish_reason` ∈ `stop`, `length`, `content_filter`, `tool_calls`, `insufficient_system_resource`, `aborted` (DeepSeek list).

**`anthropic`** (Anthropic Messages; DeepSeek's `/anthropic` endpoint)
- Stream: `event: <name>` + `data: {json}`. Events: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`, `error`. Unknown event types are skipped (Anthropic versioning policy allows new ones).
- Usage: start from `message_start.message.usage`; on every `message_delta`, overwrite each field that is present and non-null. Never sum: `message_delta` counts are cumulative and may restate input and cache counts (Anthropic streaming docs, checked 2026-10-19).
- Normalization: `input_uncached = input_tokens` (this excludes cached tokens), `input_cache_read = cache_read_input_tokens`. Cache writes come from `cache_creation.ephemeral_5m_input_tokens` (5m) and `cache_creation.ephemeral_1h_input_tokens` (1h); if the sub-object is absent, all of `cache_creation_input_tokens` counts as 5m. `output = output_tokens`, `reasoning = output_tokens_details.thinking_tokens` if present.
- Pricing modifiers from usage: `speed` (`"fast"`) and `inference_geo` (`"us"` → ×1.1 on 4.6+ models). These go through the multiplier table (§3.10.3).
- First token: the first `content_block_delta` of any type (`text_delta`, `thinking_delta`, `input_json_delta`). First content: the first `text_delta` or `input_json_delta`.
- Tool calls: `content_block_start` with `content_block.type == "tool_use"` gives `id` and `name`; `input_json_delta.partial_json` fragments are concatenated.
- Error mid-stream: `event: error`, `data: {"type":"error","error":{"type":"overloaded_error",…}}`.
- DeepSeek's `/anthropic` endpoint ignores `cache_control` and caches automatically. Which usage fields it returns is UNVERIFIED; the parser takes whatever subset arrives and flags `usage_partial` if `input_tokens` is missing.

**`openai-responses`** (OpenAI Responses over HTTP or WebSocket; DeepSeek `/responses`)
- Stream: SSE with `event:` and `data:` lines; the parser dispatches on the JSON `type` field, not the event line. No `[DONE]`: the stream ends with `response.completed`, `response.incomplete` or `response.failed`, each carrying the full `response`, whose `usage` may be null on failure.
- Normalization: `input_cache_read = input_tokens_details.cached_tokens`; `input_cache_write = input_tokens_details.cache_write_tokens` (GPT-5.6 and later; 0 if absent); `input_uncached = input_tokens − cached − cache_write` (both are subsets of `input_tokens`); `output = output_tokens`; `reasoning = output_tokens_details.reasoning_tokens`.
- Pricing modifier: `response.service_tier` (`default`, `flex`, `scale`, `priority`, `fast`, `ultrafast`).
- First token: the first of `response.output_text.delta`, `response.reasoning_text.delta`, `response.reasoning_summary_text.delta`, `response.function_call_arguments.delta`, `response.custom_tool_call_input.delta`. First content excludes the two reasoning types.
- Tool calls: `response.output_item.added` with `item.type ∈ {function_call, custom_tool_call}` gives `call_id` and `name`; argument deltas are concatenated per `item_id`; `.done` events carry the full arguments and are preferred when present.

**Non-streaming responses** (all families): the body is accumulated by the tap (it is forwarded unchanged frame by frame as well) and parsed once at end; usage sits in the top-level `usage` (Responses: `usage` of the response object).

**Tool-call records.** For each model-issued call the tap emits `{id, name, args}`, with `args` cut at 4 KiB. For a call to the `ext` tool it also parses `args` as JSON and records `ext_name = args.name` when that is a string. These records are **trusted**: they come from the provider's stream, not from the body. evolution.md §3.11.1 counts usage from them.

#### 3.10.3 Cost

```text
cost_pusd = Σ_class tokens[class] × price[window][class] × Π multipliers
classes   = input_uncached, input_cache_read, input_cache_write, input_cache_write_1h, output
window    = price window in force at t_admit (request admission time, UTC)
```

- **Time-of-day windows (DeepSeek).** DeepSeek prices peak and off-peak differently (§4.8.1). Which instant DeepSeek uses to classify a request is UNVERIFIED; the proxy uses admission time. Chinese public holidays are off-peak, but the proxy has no holiday calendar: unknown holidays are priced as peak, which over-estimates. The owner may list holiday dates in config (kernel/config.md, `schedule.default_dates`).
- **Long-context tiers.** If the model config has `tiers` with `above_input_tokens`, the tier is chosen from `input_total()` of the request, per request.
- **Multipliers.** `service_tier:<v>`, `speed:<v>` and `inference_geo:<v>` keys in `providers.<id>.multipliers`. A request that sets a tier without a configured multiplier is refused up front (§3.6), except the neutral values `default`, `auto` and `standard`, which price at ×1. In responses, `global` and `not_available` are neutral too. A response that reports one without a multiplier is charged at the largest configured multiplier, with flag `unpriced_modifier`.
- **Reference cost.** Every request also gets `cost_ref_pusd`: the same tokens priced at the model's `reference_window` (default `peak`) with no time dependence. The gate compares arms on reference cost, so a candidate that happens to run off-peak does not look cheaper (kernel/gate.md).
- **No usage.** On abort, timeout or a stream error without a usage object, the cost is an estimate: `input = ceil(request_bytes / 3)` at `price.input`, plus `output = ceil(content_bytes_seen / 3)` at `price.output`, capped at the reservation, with flag `usage_estimated`. `content_bytes_seen` is the number of response body bytes forwarded so far. Whether providers bill tokens generated before a client cancels is UNVERIFIED; the estimate assumes they do.
- **Measurement price, not enforcement price (v0.3).** This cost is the kernel's *measurement*: it feeds self-signals, soft budgets, `cost_ref` for the gate, and the capability self-model. The money that counts against the ceilings is metered by the Root at its coarse peak prices (root/egress.md §3.4), so the Root's figure is normally **higher** than the kernel's off-peak cost; that is why the §3.8 guard reads the Root's spend rather than recomputing it. If the Root reports a per-request cost (proposed, §9), the `end` record stores it as `root_cost_musd`. A Root cost *below* the kernel's measured cost by more than 1% means a price table is wrong; it is flagged `cost_mismatch` and alerted once per hour. Because the gate compares both arms under one price table, a stale kernel price shifts both arms alike. It cannot by itself turn a worse candidate into a better one.

#### 3.10.4 The spend ledger

The kernel's spend ledger is the request log of the trace store (§3.13): two records per request in `vault/traces/v1/req/<YYYY-MM-DD>.jsonl` (UTC date). It is authoritative for the kernel's soft budgets only; the Root's metering is authoritative for money (R2).
- A `start` record is written at admission. It carries `reserved_pusd`.
- An `end` record is written at completion. It carries `cost_pusd`, `cost_ref_pusd`, `root_cost_musd` (when the Root sent one), usage and timings.

At start the proxy rebuilds today's task, gate, evolution and soft-day totals from these files. The guard of §3.8 starts from the Root's figures (`GET /_root/ceilings`). A `start` without an `end` (proxy crash) is charged at its reservation. Records are appended by the trace writer and `fsync`ed every `traces.fsync_interval_ms` (1,000 ms) and at shutdown. A crash can therefore lose at most one second of records; to make that loss safe, the in-memory cap check always counts `reserved` for requests whose `start` has not been fsynced yet.

### 3.11 Timing metrics

| Metric | Definition |
|---|---|
| `queue_ms` | body request fully read → upstream request handed to the client (admission + pool checkout) |
| `ttfb_ms` | handed to client → response headers |
| `ttft_ms` | handed to client → first generated token (any kind, including reasoning) |
| `ttfc_ms` | handed to client → first non-reasoning content or tool-call token |
| `gen_ms` | first token → last token |
| `tps` | `floor(usage.output × 1000 / gen_ms)`, an integer; null if `gen_ms` < 50 or no usage |
| `duration_ms` | body request read → last byte forwarded |
| `gap_ms` | previous response in the same scope and the same `x-init-session` ended with tool calls → this request arrived. Null otherwise. This is tool time plus harness overhead seen from outside the body (evolution.md metric `gap_ms_p50`). |

Retries add to `ttfb_ms`, `ttft_ms` and `duration_ms`; `retries` is reported separately.

Rolling statistics: per scope and per (provider, model), a ring of the last `proxy.stats_ring` (256) samples of `ttft_ms`, `ttfc_ms`, `tps` and `gap_ms`. p50 and p90 are computed on demand by sorting a copy (256 values, microseconds).

### 3.12 Observations for the gate

For every scope the proxy keeps:

```rust
pub struct Observations {
    pub first: Option<RequestObs>,          // the first metered request in the scope
    pub prefix_hashes: Vec<(String, u64)>,  // distinct prefix_hash values with counts, in first-seen order
    pub tools_sha256: Vec<(String, u64)>,   // distinct tools_sha256 values with counts
    pub models: std::collections::BTreeSet<String>,
    pub by_model: std::collections::BTreeMap<String, ModelUse>,  // "<provider>/<model>" → use (§3.5a)
    pub last_req_by_session: std::collections::BTreeMap<String, String>, // x-init-session → last req_id
    pub emitted_call_ids: std::collections::VecDeque<String>, // last 1024 tool-call ids seen in responses
    pub tool_round_trip: bool,              // a later request carried a result for an emitted call id
    pub tool_calls: std::collections::BTreeMap<String, u64>,  // "tool/<name>", "ext/<name>" → count
    pub last_finish: Option<(String, std::time::SystemTime)>, // normalized finish reason, time
    pub inflight: u32,
}
pub struct RequestObs {
    pub req_id: String, pub model: String, pub family: Family,
    pub prefix_hash: String, pub head_bytes_hash: String, pub tools_sha256: String,
    pub session: Option<String>,
    pub tools: Vec<ToolObs>,               // normalized, in request order
}
pub struct ToolObs { pub name: String, pub schema_sha256: String }
pub struct ModelUse { pub requests: u64, pub routed: u64, pub spent_pusd: u64, pub cost_ref_pusd: u64 }
```

**`tool_round_trip`.** Set when a request in the scope contains a tool-result item whose call id is in `emitted_call_ids`. The item is a `tool` message's `tool_call_id` (openai-chat), a `tool_result` block's `tool_use_id` (anthropic), or a `function_call_output` / `custom_tool_call_output` item's `call_id` (openai-responses). The id must have been emitted by the provider, so a body cannot fake the round trip by inventing an assistant message. G2 relies on this (kernel/gate.md).

**Normalized finish reason** (used by the gate to detect the end of a task turn): `tool_use` if the response ended with tool calls (openai-chat `tool_calls`; anthropic `stop_reason = "tool_use"`; responses with a `function_call` or `custom_tool_call` output item), `stop` for a natural end (`stop`, `end_turn`, `completed` without tool calls), `length`, `error` or `other`.

#### 3.12.1 `prefix_hash` and `head_bytes_hash`

The cacheable head of a request is its tool definitions, its leading system content and the parameters that change the rendered prompt. Changing any of it is a full cache miss on every provider (Anthropic: tools → system → messages; DeepSeek: prefix units must match fully; OpenAI: exact prefix).

**`prefix_hash` is defined by body/log-context.md §3.10.4** (the head hash: `"sha256:" + hex(SHA-256(ICJ(head_object)))` with `head_object = {v, provider, model, params, system, tools}`, cache markers removed, ICJ = INIT canonical JSON with byte-sorted keys). The proxy recomputes it independently from the bytes it forwards, on every metered request, so the body's self-reported value can be checked against it. The proxy does not redefine it. Implementation notes:
- The extraction row is chosen by the route's **family**: log-context.md §3.10.4 keys its table by wire format, `chat_completions` / `messages` / `responses`, which are the proxy's `openai-chat` / `anthropic` / `openai-responses`. The head object's `provider` is the provider id from the path (`deepseek`, `anthropic`, `openai`, or `mock` in test mode).
- The proxy builds the head object as a `serde_json::Value` from the parsed `RawValue`s, deletes `cache_control` and `prompt_cache_breakpoint` keys at any depth, and serializes with `serde_json::to_vec`. The kernel crate must not enable serde_json's `preserve_order` feature, so `Map` is a `BTreeMap` and keys come out byte-sorted. Test P-13 checks this with log-context's golden vectors.
- The value is returned to the body in the response header `x-init-head-hash`, recorded per request, and exposed as `last_head_hash` in `/_init/stats` (log-context.md X3).

**`head_bytes_hash` (proxy-owned diagnostic, algorithm `init-headbytes-v1`).** ICJ sorts keys, so `prefix_hash` cannot see a change in key order or escape spelling that a provider would still render differently: providers re-render schemas in the order received. The proxy therefore also computes:

```text
head_bytes_hash = "sha256:" + hex(sha256(
      "init-headbytes-v1" 0x00 family 0x00 model 0x00 minify(tools_raw or "") 0x00
      for each leading system item s: minify(s) 0x00 ))
leading system items: openai-chat messages[0..k] with role "system"; anthropic top-level "system";
                      openai-responses "instructions" then leading input items with role "system"/"developer"
```

`minify` deletes JSON whitespace (0x20, 0x09, 0x0A, 0x0D) outside string literals and nothing else. When `prefix_hash` is unchanged but `head_bytes_hash` changed, G2 reports a warning `head_bytes_changed`, not a failure (kernel/gate.md).

#### 3.12.2 `tools_sha256` (algorithm `init-tools-v1`)

Each tool definition in a request is normalized to the body's registry shape (body/tools.md §4.1), `{"description": d, "input_schema": s, "name": n}`:
- openai-chat: `function.{name, description, parameters}`;
- anthropic: `{name, description, input_schema}` (any `cache_control` key dropped);
- openai-responses: `{name, description, parameters}` for `type: "function"`, and `{name, description, "input_schema": {"custom": format}}` for `type: "custom"`.

A missing description is `""` and a missing schema is `{}`. Then:

```text
schema_sha256(tool) = "sha256:" + hex(SHA-256(ICJ(tool)))
tools_sha256        = "sha256:" + hex(SHA-256(ICJ([normalized tools sorted byte-wise by name])))
```

ICJ is body/log-context.md §3.8.1 (sorted keys, no whitespace). Sorting by name makes the value identify the **set** of tools and their exact text. Order changes are a cache matter and show in `prefix_hash` and `head_bytes_hash`. The same function is applied by kernel/selfmap.md to the body's `--manifest` output, and body/boot.md §4.3 computes its manifest `tools_sha256` the same way, so all three values are directly comparable.

### 3.13 The trace store (`vault/traces/v1/`)

Full logging would cost about 0.5 MB per step (spec §7.2). Agent conversations are append-only, so consecutive requests share almost every byte. The store keeps each distinct blob once and describes each request as "the message list of an earlier request, truncated to `keep` items, plus these new items".

**Layout**

```
vault/traces/v1/
  req/<YYYY-MM-DD>.jsonl          request records: "start" and "end" (UTC date of ts)
  packs/<YYYYMM>/<NNNNNN>.pack    blob packs, append-only; rotate at traces.pack_max_bytes (256 MiB) or month change
  scopes.jsonl                    scope open/close/label records (one JSON line each)
```

**Pack record** (binary, little-endian):

```
magic   u32  = 0x31424e49  ("INB1")
kind    u8   1 head · 2 message · 3 response · 4 system · 5 tools
len     u32  payload length (≤ 64 MiB)
sha256  [u8; 32]  sha256(payload)
payload [u8; len]
```

A blob is addressed by `{"h": "<64 hex sha256>", "p": "<YYYYMM>/<NNNNNN>", "o": <offset of the record>, "n": <len>}`. Request records store the address, so reads need no index.

**Dedup index.** In memory, a `HashMap<[u8; 32], BlobLoc>` holds the blobs of the current month. It is rebuilt at start by scanning the current month's packs (headers only, seeking over payloads; about 50 ms for 90k records). A torn tail record (short read, bad magic, or hash mismatch) is truncated away. Blobs from earlier months are not indexed, so a blob first seen last month (typically a system head) is written once more per month. Dedup is an optimization: correctness never depends on the index being complete.

**Splitting a request** (from the offsets in `ReqView`):
- `messages` (openai-chat, anthropic) or `input` when it is an array (openai-responses) is the **message list**. Each element's raw bytes are one `message` blob.
- The **head** is the request bytes with the message-list value replaced by the 2-byte placeholder `[]`. It is one `head` blob, plus the byte offset where the list goes (`list_at`).
- Reconstruction: `head[..list_at] + "[" + join(messages, ",") + "]" + head[list_at + 2..]`. This is byte-exact except for whitespace between array elements.

**Chain encoding.** Per (scope, `x-init-session`), the proxy keeps the message hash list of the previous request. For the new request it computes the longest common prefix length `keep`:

```json
"msgs": {"base": "<req_id of previous>", "keep": 41, "add": [{"h": "…", "p": "202610/000003", "o": 1048576, "n": 812}]}
```

`base: null` means a full list. A full list is also written every `traces.checkpoint_every` (64) requests in a chain, so reconstructing any request reads at most 64 records. Editing history (masking, compaction) simply shortens `keep`; nothing breaks.

**The response** is stored as one `response` blob: the raw upstream bytes (SSE text or JSON), up to 16 MiB.

**Request records** (one JSON object per line, sorted keys, written by the trace writer):

```json
{"t":"start","req_id":"019a1c2e3f4b0007","ts":"2026-10-19T13:24:07.123Z","scope":"live","vis":"agent",
 "job_id":null,"task":"2026-10-20-03","subtask":"plan-2","session":"20261019-132407-a1b2c3","purpose":"turn","attempt":1,
 "label":{"boot_reason":"promote","level":"L1","model":"deepseek-flash","sha":"0e786c7"},
 "provider":"deepseek","route":"POST chat/completions","family":"openai-chat","model":"deepseek-v4-pro",
 "default_model":"deepseek-flash","routed":true,"level":"L1",
 "stream":true,"req_bytes":412233,"reserved_pusd":700427640000,"window":"peak",
 "prefix_hash":"sha256:9f2c…","head_bytes_hash":"sha256:77d0…","tools_sha256":"sha256:51ab…",
 "head":{"h":"…","p":"202610/000003","o":0,"n":5120,"list_at":4870},
 "msgs":{"base":"019a1c2e3f4a0006","keep":41,"add":[{"h":"…","p":"202610/000003","o":5160,"n":812}]}}
{"t":"end","req_id":"019a1c2e3f4b0007","ts":"2026-10-19T13:24:16.401Z","status":200,"outcome":"ok",
 "retries":0,"usage":{"input_cache_read":118016,"input_cache_write":0,"input_cache_write_1h":0,
 "input_uncached":875,"output":214,"reasoning":96},"usage_flags":[],
 "cost_pusd":7195144000,"cost_ref_pusd":7195144000,"root_cost_musd":7195,"cache_permille":992,
 "queue_ms":1,"ttfb_ms":402,"ttft_ms":805,"ttfc_ms":2210,"gen_ms":8410,"tps":25,"duration_ms":9278,"gap_ms":3120,
 "finish":"tool_use","tool_calls":[{"args":"{\"command\":\"cargo test\"}","id":"call_00_kw66","name":"bash"}],
 "upstream_request_id":"x-ds-trace-id:5223a4b5d76d2d6f5a571181092b2656","egress_req_id":"…","egress_retries":0,
 "resp":{"h":"…","p":"202610/000003","o":5990,"n":20480},"resp_truncated":false,"resp_type":"text/event-stream"}
```

`resp_type` is the response content type, which `GET /_init/traces/<id>/response` serves. The trace helper's edge rules (chain restarts, how the agent view merges a `start` and its `end`) are recorded as doc comments in `trace.rs`.

`req_id` is 16 lowercase hex characters: 12 hex digits of Unix milliseconds followed by a 4-hex per-process counter. It is time-sortable and has the same shape as event ids (CONVENTIONS §4).

**Writer.** One `std::thread` owns all files. It receives `TraceJob`s over a bounded channel (`traces.writer_queue` = 4,096). Two rules keep it honest without ever stalling a stream:
- **Ledger records** (`start` and the `end` fields used for spend) are sent with `blocking_send`, which back-pressures the completion path (not the forwarding path) if the disk stalls.
- **Blob payloads** are sent with `try_send`. If the queue is full, the blob is dropped and the request record says `"trace":"dropped"`, counted in stats as `trace_dropped`.

**Kernel reader.** The trace module exposes `pub fn reconstruct(root: &Path, req_id: &str) -> io::Result<(Vec<u8>, Vec<Vec<u8>>)>` (request bytes and the message-list element bytes). The gate links it directly (same binary) to compare a probe's first request with the running body's last request (kernel/gate.md, G2 continuity); it is read-only and never goes through the agent view filter.

**Retention.** `traces.retention_days` (default 0 = keep forever). When set, the kernel deletes whole `req/<date>.jsonl` files and whole month directories of packs older than the window, by exact path, only when no newer request chains reference them (checked by scanning `base` pointers of the last 64 records of the following day).

### 3.14 The agent's redacted read-only trace view

The agent may read its own raw traces (Meta-Harness found raw traces beat summaries) through `GET /_init/traces…` on `run/proxy.sock` (§4.3). The view is restricted as follows:
- **Only `vis: "agent"` records.** Runs of the gate's own holdout and canaries are `vis: "kernel"`, so their prompts, workspaces and model outputs do not reach the body. Requests are served by `req_id` only; blobs cannot be fetched by hash, so a blob shared between a hidden and a visible request can only be reached through the visible one.
- **No credentials anywhere.** Request headers are never stored. Response headers are not stored either, except `upstream_request_id`. In v0.3 no credential ever passes through the kernel anyway (§3.3).
- **Reconstructed request bodies are what the body itself sent.** Nothing is added.
- **Read-only.** The handler only reads files under `vault/traces/v1/`, with paths built from validated `req_id`s.

**What `vis: "kernel"` means by level** (spec v0.3 §13: the kernel–body line is a stability boundary, not a security one):
- **L0–L2.** The kernel is maintainer code, and `vault/` is hidden from the body sandbox, so kernel-visibility traces stay out of the agent's reach.
- **L3 and up.** The agent can change the kernel, so it can reach everything the kernel stores, including these traces. That is why kernel/gate.md treats the gate's own holdout as agent-visible from L3. Secrecy that must hold at every level lives only in the Root: the sealed set S never passes through this proxy (§2 non-goals).

The owner can use `init-kernel traces show <req_id>` from a terminal, with no filter.

### 3.15 OpenAI WebSocket mode

OpenAI's Responses API has a WebSocket mode: one persistent connection to `/v1/responses` over which the client sends `{"type":"response.create", …}` events and continues each turn with `previous_response_id` plus only the new input items. OpenAI reports it is up to about 40% faster end to end for rollouts with 20+ tool calls (spec §7.2; OpenAI guide, checked 2026-10-19). The proxy implements it **transparently**, so the std-only body never speaks WebSocket.

- **Through the Root (v0.3).** The WebSocket upgrade passes through the egress socket as an HTTP/1.1 `Upgrade` on the kernel's Unix connection (§3.7, `with_upgrades`). The Root opens the `wss://` connection to the provider with the key. Until root/egress.md confirms that the egress forwards upgrades, `providers.openai.websocket = true` is a validation error (kernel/config.md; §9 cross-doc). The client details below (tokio-tungstenite over rustls) describe what the Root's side needs. The kernel side speaks plain `ws` over the Unix socket.
- **Enabled by** `providers.openai.websocket = true` (default `false` until test P-11 passes and the egress supports upgrades).
- **Mapping.** A body request `POST /openai/v1/responses` with `"stream": true` on a scope with an open session is sent over the WebSocket as one text message: the request JSON object with `"type": "response.create"` added and `stream` and `background` removed. This is the only case where the proxy edits a payload. It applies to the transport envelope only, and the trace stores the body's original bytes. Server events are written back to the body as SSE (`event: <type>\ndata: <json>\n\n`) until `response.completed`, `response.incomplete`, `response.failed` or `error`. The payloads are the same as over HTTP streaming (OpenAI guide).
- **Session per scope.** One WebSocket connection per (scope, provider), opened lazily, on the default lane only. Requests are FIFO, one in flight (the body's loop is sequential; OpenAI allows 16 in flight and 32 named lanes, not used at seed).
- **URL and auth.** `wss://api.openai.com/v1/responses` with `Authorization: Bearer <key>` on the upgrade request. Both are taken from the official SDK source; the guide does not spell them out (UNVERIFIED in docs). Client: `tokio-tungstenite` 0.30.0 `client_async_tls_with_config` over a TCP stream, with `Connector::Rustls` and a separate rustls `ClientConfig` whose ALPN is `http/1.1` only (an h2 ALPN would break the upgrade).
- **Lifetime.** Connections last at most 60 minutes (OpenAI). The proxy retires a session at 55 minutes: it lets the current response finish and opens a new connection for the next request. Every response carries `x-init-ws-conn: <scope-local connection number>`. When that number changes, connection-local state is gone: with `store=false` a `previous_response_id` from the old connection fails with `previous_response_not_found`, and the body must resend full context (body/provider.md §3.8.3).
- **Errors.** An `error` event that arrives before any other event of the request becomes an HTTP response with the event's `status` and the body `{"error": <event.error>}`, so the body handles it like an HTTP error. An `error` after other events is forwarded as an SSE `event: error` frame. On connect failure, the request falls back to HTTPS streaming with `x-init-transport: https`.
- `x-init-transport: ws | https` is set on every Responses-route reply.

## 4. Interfaces

### 4.1 Model requests from the body

```
POST /<provider_id>/<upstream path> HTTP/1.1
host: init-proxy                              (ignored)
content-type: application/json
accept: text/event-stream
connection: close                             (the body uses one connection per request; keep-alive also works)
x-init-session: 20261019-132407-a1b2c3               (optional)
x-init-purpose: turn                          (optional)
x-init-attempt: 1                             (optional)
x-init-task: 2026-10-20-03                    (optional; live scope only)
x-init-subtask: plan-2                        (optional; routing attribution, §3.5a)
<provider-native JSON body, no auth>
```

Response: the provider's status and body, streamed. Forwarded response headers (allowlist): `content-type`, `retry-after`, `retry-after-ms`, `request-id`, `x-request-id`, `x-ds-trace-id`, `openai-processing-ms`, `anthropic-ratelimit-*`, `x-ratelimit-*`. The egress's `x-egress-req-id` and `x-egress-retries` are recorded (§3.13) and not forwarded; `x-init-egress-error` is mapped (§4.2). Added: `x-init-req-id`, `x-init-retries`, `x-init-head-hash` (metered routes), `x-init-routed: 0|1` (metered routes, §3.5a), and `x-init-transport` / `x-init-ws-conn` on Responses routes.

**Test upstreams.** The mock provider of testing.md is configured as an ordinary provider entry, `[providers.mock] preset = "deepseek"`. Its route families, and so the head-hash extraction (§3.12.1), are DeepSeek's, while the head object's `provider` is `"mock"`, the id in the path (body/provider.md §3.12). There are two ways to reach it:
- **Through `init-fakeroot`** (kernel-only tests, testing.md §3.5b): its loopback egress socket mimics the Root's interface (root/egress.md §3.8, §4.1, §4.3): `GET /_root/ceilings`, typed refusals with `x-init-egress-error`, the `x-egress-*` headers. It forwards to `init-mockllm`, and reports ceilings and the level from its charter fixture (`FAKEROOT_MODE`).
- **Through the real Root** (full-stack tests): `root.test_mode = true` lets the Root forward to a loopback mock and refuse real key files (root/root.md §3.12.1; testing.md upstream mode). The kernel's configuration is the same in both cases. A provider entry has no `base_url`.

### 4.2 Proxy-generated errors and the 429 cap format

Every response the proxy generates itself has the header `x-init-error: <code>`, and cap refusals also have `x-init-cap: <cap name>`. The body shape follows the route family, so a body adapter can parse it with its normal error path. Both shapes carry a top-level `init` object with the same fields.

**Cap refusal on an OpenAI-shaped route** (`openai-chat`, `openai-responses`, `none` on the deepseek/openai presets):

```http
HTTP/1.1 429 Too Many Requests
content-type: application/json
retry-after: 38521
x-init-error: budget.day
x-init-cap: day

{"error":{"code":"budget.day","message":"daily budget reached: spent 5000412 of 5000000 micro-USD; resets 2026-10-19T17:00:00Z","param":null,"type":"init_cap_exceeded"},
 "init":{"cap":"day","limit_musd":5000000,"reserved_musd":0,"resets_at":"2026-10-19T17:00:00Z","retryable":true,"scope":"live","spent_musd":5000412,"task":null}}
```

**Cap refusal on an Anthropic-shaped route:**

```json
{"type":"error","error":{"type":"init_cap_exceeded","message":"…"},
 "init":{"cap":"day","limit_musd":5000000,"reserved_musd":0,"resets_at":"2026-10-19T17:00:00Z","retryable":true,"scope":"live","spent_musd":5000412,"task":null}}
```

**Guard refusal** (the Root's own spend near its ceiling, §3.8), same shapes: `x-init-error: budget.root_day`, `x-init-cap: root_day`, and `"init":{"cap":"root_day","ceiling_musd":10000000,"limit_musd":9800000,"spent_musd":9800120,…}`, where `spent_musd` is the Root's figure.

Rules:
- `retry-after` (seconds until `resets_at`) is present only for `budget.day`, `budget.root_day`, `budget.task`, `budget.member` and `budget.evolution`. `budget.total`, `budget.root_total`, `budget.gate`, `budget.scope` and `budget.request` never reset by waiting: `retryable: false`. The seed body never retries any cap refusal (body/provider.md §3.9).
- `ceiling_musd` is present for `budget.root_day` and `budget.root_total` and gives the Root's ceiling; `limit_musd` is the guarded value the kernel enforces, and `spent_musd` is the Root's own figure (§3.8). For the soft budgets `ceiling_musd` is absent.
- **Root refusals** (root/egress.md §3.8) are mapped, not passed through raw, so that the body keeps one error path. The mapping follows root/egress.md's proposal: `ceiling.day` → 429 `budget.root_day`; `ceiling.total` → 429 `budget.root_total`; `quota.<q>` → 429 `budget.root_<q>` (`x-init-cap: root_<name>` in each case); `class.<x>` → 403 `egress.<x>`. The Root's original `init` object is kept as `init.root`. A mapped Root refusal is never retried, and its outcome is recorded as `root_refused`.
- **Root refusals are not proxy-generated.** A refusal from the Root egress (§3.9) reaches the body unchanged, in the format root/egress.md defines. The body tells it apart from a kernel cap by its type and headers (§9 cross-doc to body/provider.md).
- Everything the body may log or render is integer-only (ICJ, body/log-context.md §3.8.1): money in micro-USD (`*_musd`, truncated toward zero from the internal pico-USD), ratios in permille, durations in ms, timestamps as RFC 3339 UTC strings.
- `type: "init_cap_exceeded"` and the `x-init-cap` header distinguish a kernel cap from an upstream rate limit (`rate_limit_error`, HTTP 429 from the provider).

**Other proxy errors** use the same shapes with `type: "init_proxy_error"`:

| HTTP | `code` | When |
|---|---|---|
| 400 | `request.bad_json`, `request.model_not_allowed`, `request.model_not_routed`, `request.tier_not_allowed`, `request.usage_required`, `route.query_not_allowed`, `route.bad_path` | pre-flight (§3.5, §3.5a, §3.6) |
| 403 | `route.not_allowed` | (method, path) not in the provider's route table |
| 404 | `route.unknown`, `route.unknown_provider`, `traces.not_found` | |
| 413 | `request.too_large` | > `proxy.max_request_bytes` |
| 401 | `upstream.auth` | the provider rejected the Root's key (§3.9); never retried by the body |
| 502 | `upstream.transport` | §3.9 |
| 503 | `provider.disabled` | provider disabled in kernel config, or not forwarded by the Root (`RootState.models`) |
| 503 | `egress.unavailable` | the Root egress socket cannot be reached (§3.1, §3.9) |
| 403 | `egress.<x>` | a Root `class.<x>` refusal, mapped (above) |
| 504 | `upstream.timeout` | header timeout before any byte was forwarded |

### 4.3 `GET /_init/*` on any proxy socket

| Endpoint | Returns |
|---|---|
| `GET /_init/stats` | stats JSON for the socket's scope (below) |
| `GET /_init/health` | `{"v":1,"ok":true,"egress":{"connected":true,"root_state_at":"<RFC3339>"},"level":"L1","providers":{"deepseek":{"enabled":true,"forwarded":true}},"test_mode":false}` (`test_mode` only marks a test home) |
| `GET /_init/models` | the routing view (§3.5a): `{"v":1,"level":"L1","default":"deepseek/deepseek-flash","routable":[{"id":"deepseek/deepseek-v4-pro","prices":{…},"reference_window":"peak"},…]}`, with the measurement prices of every model in the routable set (at L0 only the default) |
| `GET /_init/traces?after=<req_id>&limit=<1..500>&task=<id>` | `application/x-ndjson`, one summary per visible request (the `end` fields merged into `start`, minus blob addresses), oldest first. ICJ: money is converted to `*_musd` (truncated); the `*_pusd` ledger fields never leave the kernel |
| `GET /_init/traces/<req_id>/request` | the reconstructed request JSON (`application/json`) |
| `GET /_init/traces/<req_id>/response` | the raw response bytes with the original content type |

Traces endpoints answer only on `run/proxy.sock` and only for `vis: "agent"` records. On gate scope sockets they return 404.

**`GET /_init/stats` schema.** ICJ: sorted keys, **integers only** (body/log-context.md §3.8.1), because the body renders these numbers into its self-signals. Money is micro-USD (`*_musd`), ratios are permille (token-weighted `read / (read + write + uncached)`; a request without usage is excluded), durations are ms, throughput is output tokens per second rounded down, `null` when unknown. The fields awareness.md §4.4 reads (`cache.*_permille`, `last_head_hash`, `requests_today`, `spend.*`) are part of this contract and stay stable across versions; everything else may grow.

```json
{
  "v": 1,
  "ts": "2026-10-19T13:24:16.500Z",
  "spend": {"today_musd": 1234567, "cap_today_musd": 5000000, "reserved_musd": 0,
            "day_starts_at": "2026-10-18T17:00:00Z", "resets_at": "2026-10-19T17:00:00Z",
            "evolution_today_musd": 300000, "evolution_cap_musd": 3000000,
            "budgets": {"evolution_share_permille": 600, "gate_musd": 3000000, "task_musd": 500000, "source": "config"},
            "by_model": {"deepseek/deepseek-flash": {"requests": 1201, "today_musd": 987000},
                         "deepseek/deepseek-v4-pro": {"requests": 89, "today_musd": 247567}},
            "task": {"id": "2026-10-20-03", "today_musd": 120000, "cap_musd": 500000,
                     "by_model": {"deepseek/deepseek-flash": 80000, "deepseek/deepseek-v4-pro": 40000}}},
  "routing": {"level": "L1", "default": "deepseek/deepseek-flash", "routed_today": 89},
  "root": {"v": 1, "ts": "2026-10-19T13:24:10Z", "level": "L1",
           "money": {"day_spent_musd": 2310400, "day_limit_musd": 10000000, "day_resets_at": "2026-10-19T17:00:00Z",
                     "total_spent_musd": 8120400, "total_limit_musd": 25000000},
           "…": "the rest of GET /_root/ceilings, verbatim (root/egress.md §4.3)"},
  "cache": {"last_permille": 992, "session_permille": 930, "today_permille": 912, "window_1h_permille": 950},
  "last_head_hash": "sha256:9b1e04c2d7aa…",
  "requests_today": 1290,
  "scope": {"id": "live", "kind": "live", "job_id": null,
            "label": {"boot_reason": "promote", "level": "L1", "model": "deepseek-flash", "sha": "0e786c7"},
            "opened": "2026-10-19T08:00:00.002Z", "requests": 412, "inflight": 0, "errors": 3, "retries": 1,
            "spent_musd": 412345, "cap_musd": null},
  "last": {"req_id": "019a1c2e3f4b0007", "provider": "deepseek", "model": "deepseek-flash",
           "status": 200, "outcome": "ok", "finish": "tool_use", "retries": 0, "window": "peak",
           "usage": {"input_cache_read": 118016, "input_cache_write": 0, "input_cache_write_1h": 0,
                     "input_uncached": 875, "output": 214, "reasoning": 96},
           "cost_musd": 1227, "cache_permille": 992,
           "ttft_ms": 805, "ttfc_ms": 2210, "tps": 25, "duration_ms": 9278, "gap_ms": 3120},
  "latency_1h": {"requests": 88, "ttft_ms_p50": 790, "ttft_ms_p90": 1900, "tps_p50": 40, "gap_ms_p50": 2600},
  "egress": {"connects": 3, "requests": 1290, "reuse_permille": 997, "root_refusals": 0, "cost_mismatch": 0},
  "providers": {"deepseek": {"enabled": true, "requests": 1290,
                             "errors_upstream": 2, "errors_proxy": 1, "retries": 1, "ws": null,
                             "balance": {"checked": "2026-10-19T13:20:00.000Z", "is_available": true,
                                         "currency": "USD", "total": "41.20"}}},
  "trace_dropped": 0
}
```

- `spend.cap_today_musd` is the soft day budget (as in v0.2), and `spend.today_musd` the kernel-measured spend against it. The names keep their v0.2 meaning for awareness.md ("budget left today"). The Root's own numbers are under `root`: `GET /_root/ceilings` folded in verbatim, as root/egress.md §4.3 asks.
- `spend.budgets.source` is `config` while the maintainer defaults apply, and `agent` after an L1+ `budget set`.
- `spend.task` is present only on `live` when the request context has a task id (the last request's `x-init-task`).
- `cache.session_permille` is for the `x-init-session` of the last request.
- `providers.deepseek.balance` comes from an unmetered `GET /user/balance` through the egress every `proxy.balance_poll_s` (600 s), if the Root forwards that route (§9 cross-doc). Its `total` stays a string exactly as DeepSeek returns it. It is `null` for providers without such an endpoint, or when the Root does not forward it.
- The body reads this after each tool batch for its self-signals (spec §5). It is a local, lock-light read: the cost is one mutex lock and a 256-value sort per percentile.

### 4.4 `ProxyHandle`: the in-process control API

Supervise and the gate (both in `init-kernel supervise`) control the proxy through this handle. Tests drive it through the kernel under `init-fakeroot` (§3.1); there is no separate control socket.

```rust
pub struct ProxyHandle { /* Arc<Shared> + mpsc senders; Clone + Send + Sync */ }

impl ProxyHandle {
    pub fn open_scope(&self, req: OpenScope) -> Result<(), ProxyError>;      // binds req.socket
    pub fn close_scope(&self, scope_id: &str) -> Result<ScopeSummary, ProxyError>; // unbinds, removes socket
    pub fn scope_summary(&self, scope_id: &str) -> Result<ScopeSummary, ProxyError>;
    pub fn set_label(&self, scope_id: &str, label: Label) -> Result<(), ProxyError>;
    pub fn open_gate(&self, job_id: &str, kind: GateKind, cap_musd: u64) -> Result<(), ProxyError>; // budget.evolution check
    pub fn close_gate(&self, job_id: &str) -> Result<GateSpend, ProxyError>;
    pub fn status(&self) -> ProxyStatus;                   // for initctl `status` (budget, cache_hit_permille)
    pub fn tools_usage(&self, since: SystemTime, vis: Visibility) -> BTreeMap<String, u64>; // "tool/bash", "ext/pty"
    pub fn reload(&self, what: Reload) -> Result<Vec<String>, ProxyError>; // Config | RootState | All; enabled providers
    pub fn root_state(&self) -> RootState;                  // the last Root state read (§3.1)
    pub fn set_level(&self, level: Level);                  // supervise, on a Root level change (§3.5a)
    pub fn set_budgets(&self, b: SoftBudgets) -> Result<SoftBudgets, ProxyError>; // L1+ only (§3.8); BudgetInvalid → initctl E_BUDGET
    pub fn set_ceilings(&self, c: serde_json::Value);      // supervise relays ceilings.get / `ceilings` events (§3.1)
    pub fn shutdown(&self, grace: Duration);
}

pub struct OpenScope {
    pub scope_id: String,           // "<job_id>/probe-c", "<job_id>/run/c-2026-10-20-03-r1", …
    pub kind: ScopeKind,            // Probe | Run | Quick | Member
    pub job_id: Option<String>,     // the gate job; its gate budget applies. None for a fleet member
    pub visibility: Visibility,     // Agent | Kernel
    pub cap_musd: u64,               // a member's share for a Member scope
    pub label: Label,               // {sha, model (default), boot_reason, level}
    pub socket: PathBuf,            // e.g. $INIT_HOME/probe/<probe_id>/run/proxy.sock
}
pub struct GateSpend { pub spent_pusd: u64, pub cost_ref_pusd: u64 }
pub struct ProxyStatus {            // initctl.md `status` fields "budget" and "cache_hit_permille"
    pub day_cap_musd: u64, pub day_spent_musd: u64,                     // the soft day budget and kernel-measured spend
    pub root_day_limit_musd: u64, pub root_day_spent_musd: u64,         // the Root's day ceiling and its own figure
    pub root_total_limit_musd: u64, pub root_total_spent_musd: u64,
    pub gate_cap_musd: Option<u64>, pub gate_spent_musd: Option<u64>,   // the running gate, if any
    pub cache_hit_permille_last50: Option<u32>,
    pub level: Level,
}
pub enum Level { L0, L1, L2, L3, L4 }
pub struct SoftBudgets { pub day_musd: u64, pub task_musd: u64, pub gate_musd: u64, pub quick_gate_musd: u64,
                         pub probe_musd: u64, pub request_max_musd: u64, pub evolution_share_permille: u32 }
pub struct RootState {              // the kernel's view of the Root (§3.1)
    pub ceilings: serde_json::Value,                    // GET /_root/ceilings verbatim (root/egress.md §4.3): level,
                                                        // money {day_spent_musd, day_limit_musd, day_resets_at,
                                                        // total_spent_musd, total_limit_musd}, quotas, classes
    pub level: Level,                                   // ceilings.level (R7, root/root.md)
    pub models: BTreeMap<String, BTreeSet<String>>,     // provider id → models the Root forwards. PROPOSED: the
                                                        // Root's ceiling price table (root/egress.md §3.4), §9 Q-EG2
    pub at: SystemTime,
}
```

`open_gate` is refused with `budget.evolution` when the day's evolution share (`budget.evolution_share` × the soft day) is used up, and `open_scope` is refused with `budget.gate` when the gate's budget is used up. `open_gate` also snapshots the level and the routable set for the gate (§3.5a). `ProxyStatus` gives integers (micro-USD, permille), and kernel/initctl.md §4.1 `status` shows them as such (`*_musd`, `cache_hit_permille`).

**`ScopeSummary`:**

```json
{"scope_id":"j-000043/probe-c","kind":"probe","job_id":"j-000043","visibility":"agent",
 "opened":"…","closed":null,"requests":2,"inflight":0,"errors":0,
 "spent_pusd":1200000000,"cost_ref_pusd":1200000000,
 "usage":{"input_cache_read":0,"input_cache_write":0,"input_cache_write_1h":0,"input_uncached":9120,"output":388,"reasoning":120},
 "cache_permille":0,"ttft_ms_p50":910,"tps_p50":38,"gap_ms_p50":140,
 "first":{"req_id":"…","model":"deepseek-flash","family":"openai-chat","session":"20261019-132407-a1b2c3",
          "prefix_hash":"sha256:…","head_bytes_hash":"sha256:…","tools_sha256":"sha256:…",
          "tools":[{"name":"bash","schema_sha256":"…"},{"name":"edit","schema_sha256":"…"}]},
 "prefix_hashes":[["sha256:…",2]],"tools_sha256s":[["…",2]],"models":["deepseek-flash"],
 "by_model":{"deepseek/deepseek-flash":{"cost_ref_pusd":1200000000,"requests":2,"routed":0,"spent_pusd":1200000000}},
 "tool_round_trip":true,"tool_calls":{"tool/read":1},"last_req_by_session":{"20261019-132407-a1b2c3":"019a1c2e3f4b0009"},
 "last_finish":{"reason":"stop","at":"…"},"capped":false}
```

### 4.5 CLI

```
init-kernel traces show <req_id> [--part request|response]   owner view, unfiltered
init-kernel traces stats [--day YYYY-MM-DD]     kernel-measured spend, per-model spend and cache summary from the ledger
```

Exit codes: 0 ok, 1 runtime error, 2 usage or config error.

`init-kernel keys set|check` are gone. Keys are stored with `init-root keys set` (root/egress.md, spec v0.3 §18).

### 4.5a Fault-injection point

`proxy.stream.mid` (testing.md §3.8.1; `fault-injection` feature only) is placed in `MeteredBody::poll_frame` (§3.10.1). It fires right after the first data frame that follows the first-token frame has been forwarded to the body. By then the stream is mid-flight: the body holds partial output, the upstream request is still open, and the reservation is unsettled.

### 4.6 Files

| Path | Mode | Writer | Content |
|---|---|---|---|
| `vault/traces/v1/…` | 0600 files, 0700 dirs | proxy | §3.13 |
| `run/proxy.sock` | 0600 | proxy | live lane |
| `probe/<probe_id>/run/proxy.sock` | 0600 | proxy (path chosen by supervise) | probe, run and quick lanes |
| `state/kernel/budget.json` | 0644 | proxy | effective soft budgets `{"v":1,"budgets":{…},"source":"config"\|"agent","since":"<RFC3339>"}` (§3.8); new file in supervise.md's layout (§9) |

`vault/keys/` no longer exists (§3.3).

### 4.7 Alerts

The proxy reports alerts through the `AlertSink` it receives at start. Supervise writes them to `state/kernel/alerts.jsonl` and decides how they reach the owner (kernel/supervise.md §3.7.1). In v0.3 reading them is optional for the owner (spec v0.3 §18); nothing waits on them. The record is `{"v":1,"ts","source":"proxy","code","message","provider"?,"scope"?}`, the same shape the gate uses. Codes:
- `upstream_auth`
- `provider_balance`
- `budget_day_reached`
- `budget_root_guard` (the guard on the Root's day or total ceiling refused a request, §3.8)
- `soft_day_clamped` (a lowered Root ceiling forced the soft day down, §3.8)
- `budget_evolution_reached`
- `root_refused` (a Root refusal reached the kernel; §3.9)
- `egress_down` (the egress socket stayed unreachable for 30 s)
- `cost_mismatch` (kernel and Root cost differ by more than 1%; first occurrence per hour)
- `trace_dropped` (first occurrence per hour)

`key_file_insecure` is gone with the keys (root/egress.md owns key-file checks). Alert text never contains upstream error bodies.

### 4.8 Upstream provider notes (checked 2026-10-19)

These notes are what the kernel needs for parsing, pricing and retry decisions. In v0.3 the base URLs, auth styles and transport rows describe the Root's side of the connection (root/egress.md). They are kept here because the parse families and the usage normalization depend on them.

#### 4.8.1 DeepSeek (first real provider, Phase 0)

| Item | Value |
|---|---|
| Base URL (OpenAI format) | `https://api.deepseek.com` → `POST /chat/completions` |
| Base URL (Anthropic format) | `https://api.deepseek.com/anthropic` → `POST /anthropic/v1/messages` |
| Responses API | `POST https://api.deepseek.com/responses` (stateless: `previous_response_id`, `store`, `prompt_cache_key` and `prompt_cache_retention` are not supported and are silently ignored) |
| Beta features | base `https://api.deepseek.com/beta` (strict tool mode, chat prefix completion) |
| Auth | `Authorization: Bearer <key>` (OpenAI format and Responses); `x-api-key` on the Anthropic format (`anthropic-version` is ignored there) |
| Transport | HTTP/2 accepted via ALPN; served through CloudFront (`x-cache`, `x-amz-cf-*` headers); every response has `x-ds-trace-id` (observed) |
| Models | `deepseek-flash` = DeepSeek-V4.1-Flash-1017 (released 2026-10-17); `deepseek-v4-pro` = DeepSeek-V4-Pro-0813. Legacy names `deepseek-v4-flash` and `deepseek-v4-flash-vision-exp` still route to Flash at the Flash price. `deepseek-chat` and `deepseek-reasoner` were discontinued 2026-07-24. On the Anthropic endpoint, `claude-opus*` names map to `deepseek-v4-pro` and other `claude*` names to `deepseek-flash`. |
| Context / output | 1M context; `max_tokens` 1..393,216 (384K). If unset: 8K non-thinking, 64K thinking, 128K with `reasoning_effort: "max"`. Whether "1M" is 1,000,000 or 1,048,576 is UNVERIFIED; `GET /models` returns `context_window` and `max_output_tokens`, and the proxy logs them at start. |
| Thinking | On by default with effort `high`. `{"thinking":{"type":"enabled"\|"disabled"}}`; `reasoning_effort` ∈ `none`, `low`, `high`, `max` (`minimal`→low, `medium`/`xhigh`→high). `temperature` has no effect in thinking mode. |
| Tools | `tools[].function.{name, description, parameters, strict}`; names `[a-zA-Z0-9_-]{1,128}`; `tool_choice` `none`/`auto`/`required`/named, but `required` and named are rejected (400) in thinking mode. Strict mode needs the `/beta` base and `strict: true` on every function. Chat Completions cannot take tool calls inserted mid-conversation (the Anthropic and Responses formats can). |
| Thinking + tools | When a request carries `tools`, the `reasoning_content` of **every** earlier assistant turn must be sent back, or the API returns 400. Without `tools`, it is ignored. This is part of the cached prefix too. (body/provider.md) |
| Streaming | `data: {chunk}` lines, `data: [DONE]` at the end; `: keep-alive` SSE comments while waiting (empty lines for non-streaming); the server closes a request that has not started inference after 10 minutes. `stream_options.include_usage` only makes `"usage": null` appear on every chunk; the usage object is always on the last chunk before `[DONE]`. |
| Usage fields | `prompt_tokens` (= hit + miss), `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`, `prompt_tokens_details.cached_tokens` (= hit), `completion_tokens`, `completion_tokens_details.reasoning_tokens`, `total_tokens`. Responses format: `input_tokens`, `input_tokens_details.cached_tokens`, `output_tokens`, `output_tokens_details.reasoning_tokens`. |
| `finish_reason` | `stop`, `length`, `content_filter`, `tool_calls`, `insufficient_system_resource`, `aborted` |
| Errors | 400 invalid format, 401 auth, 402 insufficient balance, 422 invalid parameters, 429 rate/concurrency, 500 server error, 503 overloaded. Body `{"error":{"message","type","param","code"}}`; observed for a bad key: `"type":"authentication_error","code":"invalid_request_error"`. With no auth header at all, the body is plain text `Authentication Fails (governor)` (observed). No `retry-after` is documented. |
| Concurrency | per account: 2,500 in flight for `deepseek-flash`, 500 for `deepseek-v4-pro`; above that, 429 |
| `user_id` | optional, `[a-zA-Z0-9_-]{1,512}`. It also **isolates the KV cache** per value, so a body that varies it loses cache hits. |
| Free endpoints | `GET /models` (context window, output limit, effort levels, capabilities); `GET /user/balance` (`is_available`, balances per currency) |

**Automatic context caching ("Context Caching on Disk").** It is on for every account and needs no request changes. Each request builds a disk cache, and an overlapping prefix in a later request is served from it.
- **Prefix units.** Each cached prefix is an independent unit, and a request hits only if it fully matches a unit. Units are persisted:
  - at request boundaries: one at the end of the user input and one at the end of the model output;
  - when the system detects a common prefix across requests (so two requests `A+B`, `A+C` miss, and a third `A+D` hits `A`);
  - at fixed token intervals inside long inputs and outputs.
- **Best effort.** Building a unit takes seconds. Units are evicted hours to days after last use. There is no cache write charge and no TTL parameter.
- **What this means for INIT:**
  - an append-only history hits up to the end of the previous request;
  - editing anything inside the history (masking, compaction) misses from that point;
  - a new session's first request misses;
  - parallel gate runs that share a system prefix miss on their first requests until common-prefix detection persists the shared head.

**Prices** (USD per 1M tokens):

| Model | Window | Cache hit | Cache miss | Output |
|---|---|---|---|---|
| `deepseek-flash` | off-peak | 0.003 | 0.15 | 0.60 |
| | peak | 0.006 | 0.30 | 1.20 |
| `deepseek-v4-pro` | off-peak | 0.022 | 0.66 | 1.98 |
| | peak | 0.044 | 1.32 | 3.96 |

- Peak hours are 01:00–04:00 and 06:00–10:00 UTC, Monday to Friday, excluding Chinese public holidays. Everything else is off-peak, including weekends and holidays.
- A miss costs **50×** a hit on input, against 10–40× on Anthropic and OpenAI. That makes the byte-stable prefix (spec §9 rank 1) worth even more on DeepSeek.
- Search and Fetch APIs cost $0.75 per 1,000 calls each and are not routed (§3.5). They are free through DeepSeek Harness, Claude Code and Codex server-side tools until billing starts on 2026-10-31 16:00 UTC.

**Retry mapping for DeepSeek:** 429 and 500/503 are retried with computed backoff (there is no documented `retry-after`); 402 is an alert; 401 is rewritten (§3.9).

#### 4.8.2 Anthropic Messages

- `POST https://api.anthropic.com/v1/messages`, `anthropic-version: 2023-06-01`. Auth: `Authorization: Bearer` is listed first by the docs, and `x-api-key` is a "legacy fallback, still supported". The proxy uses `x-api-key` because DeepSeek's Anthropic endpoint documents only that.
- **Caching.**
  - `cache_control: {"type":"ephemeral"}` or `{"type":"ephemeral","ttl":"1h"}`; the 1h TTL is GA, with no beta header.
  - Up to 4 breakpoints. Top-level `cache_control` turns on automatic caching (it uses one slot and moves forward with the conversation).
  - Prefix order is tools → system → messages, and the lookback is 20 positions per breakpoint.
  - Minimum cacheable tokens is 512 on the 5.x models, and 1,024–4,096 on older ones.
  - Changing tool definitions invalidates everything; `tool_choice`, images, thinking and effort changes invalidate messages.
  - The request field `diagnostics: {previous_message_id}` returns `cache_miss_reason` (whether a beta header is needed is UNVERIFIED).
- **Usage.** `input_tokens` (after the last breakpoint, excluding cached), `cache_creation_input_tokens`, `cache_read_input_tokens`, `cache_creation.{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`, `output_tokens`, `output_tokens_details.thinking_tokens`, `server_tool_use`, `service_tier`, `speed`, `inference_geo`. Total input = input + creation + read.
- **Errors.**
  - Types: 400 `invalid_request_error`, 401, 402 `billing_error`, 403, 404, 409, 413 `request_too_large`, 429 `rate_limit_error`, 500 `api_error`, 504 `timeout_error`, 529 `overloaded_error`.
  - Body `{"type":"error","error":{"type","message"},"request_id"}`.
  - A spend-limit 429 (`error.details.error_code = "enforced_spend_limit_reached"`) must not be retried.
  - `retry-after` is on some 429s; on 529 it is UNVERIFIED.
- **Prices (USD/MTok: input / 5m write / 1h write / cache read / output).**
  - `claude-opus-5-5`: 4 / 5 / 8 / 0.20 / 20.
  - `claude-sonnet-5-5`: 2 / 2.50 / 4 / 0.10 / 10.
  - `claude-haiku-5-5`: 0.10 / 0.125 / 0.20 / 0.01 / 0.50 (≤100K input; above that 0.50 / 0.625 / 1 / 0.05 / 2.50, a `tiers` entry).
  - `claude-fable-5-1`: 10 / 12.50 / 20 / 0.25 / 50.
  - Fast mode on Opus 5.5 is $8/$40 (`speed:fast` multiplier 2.0); `inference_geo:us` ×1.1.

#### 4.8.3 OpenAI Responses (HTTP and WebSocket)

- `POST https://api.openai.com/v1/responses`, `Authorization: Bearer`. Response headers: `x-request-id`, `openai-processing-ms`. SSE events carry `type` and `sequence_number`; usage is in `response.completed` (and nullable in `response.failed`/`response.incomplete`).
- **Usage.** `input_tokens`, `input_tokens_details.{cached_tokens, cache_write_tokens}` (both subsets of input), `output_tokens`, `output_tokens_details.reasoning_tokens`, `total_tokens`.
- **Caching.**
  - Automatic for prefixes of 1,024 tokens and up.
  - GPT-5.6 and later: writes cost 1.25×, reads 0.1× (0.05× on `gpt-6.1-sol`); `prompt_cache_options.ttl` accepts only `"30m"`; `prompt_cache_options.mode` is `implicit` or `explicit` (with `prompt_cache_breakpoint` on content blocks, at most 4 writes per request); `prompt_cache_key` is "not needed to optimize caching" and only separates accounting.
  - Earlier models: `prompt_cache_retention` (`in_memory` or `24h`, now deprecated) and `prompt_cache_key` for routing affinity (about 15 requests per minute per key and prefix before overflow routing).
- **WebSocket mode.** §3.15. 16 in-flight responses per connection, 32 named `stream_id` lanes, 60-minute connections. Error codes: `previous_response_not_found`, `invalid_stream_id`, `websocket_stream_limit_reached`, `websocket_connection_limit_reached`.
- **Errors.**
  - Body `{"error":{"message","type","param","code"}}`.
  - Retryable 429s: rate-limit types and `slow_down`. Not retryable: `insufficient_quota`, `credit_balance_exhausted`, `*_spend_limit_exceeded`, `organization_usage_limit_exceeded`.
  - 503 `server_is_overloaded`. `Retry-After` may be present on 429 and 503.
- **Prices (USD/MTok: input / cached / cache write / output, ≤272K input).**
  - `gpt-6-astra`: 10 / 1 / 12.50 / 50.
  - `gpt-6.1-sol`: 2 / 0.10 / 2.50 / 10.
  - `gpt-6-luna`: 0.10 / 0.01 / 0.125 / 0.50.
  - Long context above 272K uses a `tiers` entry.
  - Service tiers: flex 0.5×, fast 2×, ultrafast 6×.

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| Proxy crashes | in-flight streams break; the body sees a truncated response | supervise restarts it (backoff 1 s → 60 s); the ledger charges reservations of unfinished requests; the body retries per body/provider.md |
| Proxy restarts mid-day | soft budgets must survive | today's totals rebuilt from `req/` files, and `/_root/ceilings` re-read, before the socket is bound |
| Root egress socket down or restarting | no model access | 503 `egress.unavailable` to the body; reconnect with backoff 1 s → 30 s; alert `egress_down` after 30 s. The kernel has no other network path, by design |
| Root state stale (no successful `/_root/ceilings` read for 60 s) | the guard may be out of date | keep admitting on the last Root figure plus the kernel's own settled costs priced at peak (§3.8, which only over-counts); flag `root_state_stale` in `/_init/health` |
| Root refuses a request (ceiling, quota, class, level) | request fails | mapped (§4.2), never retried; alert `root_refused`. Under D6 only an action-class or policy violation, a level-gated refusal, or a ceiling refusal while this proxy's guard reported headroom (§3.8) counts as an attempt to exceed and demotes; ordinary ceiling hits are logged |
| Key revoked or wrong | provider 401/403 | the Root handles keys (root/egress.md); a 401/403 that reaches the kernel is rewritten to 401 `upstream.auth` (body withheld); alert; no retry |
| Level changes (promotion or demotion by R7) | the routing policy changes | applies to the next admission; running gates keep their snapshot (§3.5a). A demotion to L0 refuses routed models with `request.model_not_routed` from the next request |
| Root cost below kernel cost | a price table is wrong | `cost_mismatch` flag and alert; the Root's figure still drives the guard (§3.8) |
| Provider balance exhausted (DeepSeek 402) | requests fail | passed through; alert `provider_balance`; balance poll shows `is_available: false` |
| Upstream overloaded (503/529) storm | retries exhausted | final error passed through with `x-init-retries: 2`; the body decides |
| Disk full or stalled | trace writer stalls | blob payloads dropped (`trace_dropped`), ledger records back-pressure completion handling; streams continue; alert |
| Stale socket file after crash | bind fails | connect test, unlink exact path, bind |
| Clock jump | day window computed wrong | day boundaries use wall clock; timings use `Instant`; a backward jump > 60 s is logged |
| Kernel restarts mid-gate | lane sockets under `probe/` linger | lanes are in memory only, so nothing is recreated; supervise deletes probe roots by exact path at start (kernel/supervise.md) |
| WebSocket dropped mid-response | stream error | forwarded as SSE `error`; next request opens a new connection (`x-init-ws-conn` changes) |
| Pathological SSE line > 1 MiB | usage not parsed | forwarded unchanged; cost estimated, `usage_missing` flag |

## 6. Performance notes (spec §9)

- **Rank 1, prompt-cache stability.** The proxy never edits request bytes, so the body's byte-stable prefix reaches the provider intact. `prefix_hash` makes cache breaks visible per request (G2 uses it). The cache hit rate (permille) is reported per request, per hour and per scope. DeepSeek's 50× miss/hit ratio makes this the largest cost lever by far.
- **Rank 2, fewer and faster round trips.**
  - Persistent provider connections live in the Root egress (root/egress.md), so they survive body restarts and, from L3, kernel restarts too: no TCP or TLS handshake (typically one to three RTTs to the CloudFront edge).
  - The kernel–Root hop is a local Unix socket with keep-alive: tens of microseconds per request. Budget ≤ 100 µs p50 added per request for the hop, measured by P-12.
  - Gate runs can multiplex upstream if the egress uses h2 (root/egress.md).
  - OpenAI WebSocket mode passes through the egress once root/egress.md supports upgrades (§3.15).
  - Retries happen inside the proxy without an extra body round trip.
- **L1 routing as a cost lever.** Routing cheap subtasks to a cheaper model is the agent's lever, not the proxy's. The proxy's part is to make the trade measurable: per-model spend in `/_init/stats` and per-model `cost_ref` in every gate run (§3.5a).
- **Rank 5, footprint × N.** Trace dedup stores only new messages per step, about 3–5 KB at the TraceLab median append of 875 tokens, against about 0.5 MB for full logging. The proxy targets < 30 MiB RSS idle with 2 worker threads.
- **Rank 7, per-turn CPU.** The pre-filter parses JSON only for lines that can matter. Request hashing is sha256 over new messages only; the LCP check compares stored hashes. Target overhead: ≤ 1 ms p50 added before the upstream request (excluding pool checkout), ≤ 20 µs per forwarded frame. Test P-12 measures both.

## 7. Invariants and security (spec §2.1)

| Invariant | How the proxy keeps it |
|---|---|
Spec v0.3 §19 maps the invariants onto the Root. The proxy's share:

| Invariant | Anchor in the Root | How the proxy contributes |
|---|---|---|
| #3 Blast radius: keys never in the agent zone | R2 (root/egress.md) | the kernel has no keys at any level; it drops body auth headers, adds none, and withholds 401/403 bodies as defence in depth; its route table is a stricter subset of the Root's allowlist |
| #3 Spend the agent cannot bypass | R2 hard ceilings | the Root enforces money whatever the kernel does. The kernel's soft budgets (scope = socket, not a header; untagged live requests share the `_none` task budget) keep any one task or gate from starving the rest, and the guard on the Root's own spend keeps the system off the ceilings, so that no demotion is triggered |
| #2 Honest measuring stick | R6 (sealed set S, mutation suite M, sealed audit) | usage, timing and tool calls come from the provider stream, not the body; gate scopes are reachable only from their own sandbox; the gate's own holdout and canary traces are `vis: kernel` (hidden from the body below L3); the ledger survives restarts; the Root meters money independently (`root_cost_musd`). Sealed traffic never passes through this proxy |
| #4 Lineage | R4 | every request has a durable `start`/`end` record and a reconstructable raw trace; soft-budget changes are reported to Root lineage (§3.8) |
| #1 Recoverability | R1 | the proxy keeps running across body swaps and rollbacks; the Root's panic closes the egress whatever the proxy does |

**From L3** the proxy is agent-evolvable (spec v0.3 §15). The guarantees above that rest on the Root still hold at L3. Those that rest on proxy code, such as `vis: kernel` and tool-call observations, hold only as far as the gate-of-gates and the sealed audit anchor them (root/sealed-eval.md). A candidate proxy must keep the contracts other docs read: the `/_init/stats` fields awareness.md reads, the `Observations` the gate uses, `ScopeSummary.by_model`, and the trace-record fields of §3.13.

Further rules:
- The proxy runs inside the agent zone. Its only network path is the Root egress socket (spec v0.3 §13). Below L3 it is maintainer code that never executes agent code.
- No request body is ever rewritten (the WebSocket envelope in §3.15 is the one documented transport exception). This keeps the trace an exact record of what the agent sent.
- Untrusted input parsing is bounded: request ≤ 32 MiB, SSE line ≤ 1 MiB, tool-call args ≤ 4 KiB recorded, response trace ≤ 16 MiB, and `serde_json`'s default recursion limit (128).

## 8. Test plan

Unit tests run offline against fixtures. Integration tests run the kernel under `init-fakeroot` and its loopback egress (§4.1; testing.md §3.5b); every test that kills or signals processes runs in a disposable environment (CONVENTIONS §1). P-tests marked "live" need the Root running with the owner's DeepSeek key and a $0.05 ceiling.

| # | Case | Expectation |
|---|---|---|
| P-01 | Routing table: every preset route; unknown provider; `GET` on a POST-only route; `..`, `%2e%2e` in path; query on POST | exact status and `code` per §4.2 |
| P-02 | Header filtering: body sends `Authorization`, `x-api-key`, `cookie`, `accept-encoding: gzip` | none reach the egress; **no** auth header added; `x-egress-scope`, `x-egress-body`, `x-egress-reservation` present; `accept-encoding: identity` |
| P-03 | Egress keep-alive: 100 sequential requests through `init-fakeroot`'s egress | `egress.connects` = 1; a killed egress connection is replaced on the next checkout. (Provider h2 negotiation moved to root/egress.md's tests.) |
| P-04 | DeepSeek stream parser on the docs' example stream (incl. `"usage": {` with a space, empty first `content`) | usage normalized exactly; TTFT at the `"Hello"` chunk; `finish = stop` |
| P-05 | Anthropic parser: `message_start` then two `message_delta`s restating input and cache counts | final usage = last values, not sums |
| P-06 | Responses parser: `response.completed` with `cached_tokens` and `cache_write_tokens` | uncached = input − cached − write |
| P-07 | Cost: flash at Mon 00:59:59Z (off-peak), 01:00:00Z (peak), 03:59:59Z (peak), 04:00:00Z (off-peak), Sat 02:00Z (off-peak), listed holiday (off-peak) | exact pusd; `cost_ref` always peak |
| P-08 | Soft day and Root guard: (a) soft day $5.00, 20 concurrent requests whose reservations exceed it; (b) `init-fakeroot` reports a $10.00 day ceiling with $9.70 spent by its own peak-priced count while the kernel ledger shows $4.00 | (a) admitted set never exceeds $5.00; the rest get 429 `budget.day` with `retry-after`; (b) requests whose peak-priced reservation would pass $9.80 get 429 `budget.root_day` with `ceiling_musd` 10,000,000 and `limit_musd` 9,800,000; the egress sees **no** request that would cross its ceiling; every forwarded request carries a true `x-egress-guard` |
| P-09 | Retry matrix through `init-fakeroot`'s egress: egress socket missing, 429 w/ and w/o `retry-after`, `retry-after: 3600`, 503, 529, 401, 402, reset after write, `x-should-retry: false`, Root refusals `ceiling.day`, `ceiling.total`, `quota.parallel_bodies`, `class.path` | matches §3.9 exactly, incl. 401 → 401 `upstream.auth` with no upstream body; each Root refusal sent once, never retried, mapped per §4.2 (`budget.root_day`, `budget.root_total`, `budget.root_parallel_bodies`, 403 `egress.path`) with `init.root` kept and `outcome = "root_refused"` |
| P-10 | Trace round trip: 200-request synthetic conversation with a compaction at step 120 | every request reconstructs byte-exact modulo inter-element whitespace; chain depth ≤ 64; disk < 3% of naive |
| P-11 | WebSocket mode against a local mock WS server: incremental turn, 55-min rotation (simulated clock), early `error` event | SSE out matches; `x-init-ws-conn` increments; early error → HTTP 400 |
| P-12 | Overhead benchmark: 1,000 requests of 400 KB against a local mock | added p50 ≤ 1 ms; per-frame ≤ 20 µs |
| P-13 | `prefix_hash` on body/log-context.md's golden head vectors (T-CTX-2) for each family; whitespace-only change in tools (both hashes same); key reorder inside a schema (`prefix_hash` same, `head_bytes_hash` different); moved `cache_control` (`prefix_hash` same); appended user message (both same) | as stated |
| P-14 | `tool_round_trip`: body sends a fabricated assistant tool call + result | stays false; a real emitted id → true |
| P-15 | Redacted view: a `run` scope with `vis: kernel` | not listed; `/request` returns 404 |
| P-16 | No key path: grep the kernel's config schema, CLI help, logs and traces after a full test run for `vault/keys`, `api-key`, `Bearer ` followed by a token | none found; `init-kernel keys` is an unknown subcommand. (Key-file safety tests moved to root/egress.md.) |
| P-17 (live) | DeepSeek end to end through the Root: two chat requests with tools and the same 3K-token system prefix, 15 s apart | second request `prompt_cache_hit_tokens` > 0; ledger cost equals hand computation; the Root's day spend rises by at least the kernel's cost (equal in a peak window) |
| P-18 | Routing at L0: body sends `model = deepseek-v4-pro` with default `deepseek-flash` | 400 `request.model_not_routed`; nothing sent to the egress |
| P-19 | Routing at L1: three requests in one task, two subtasks, flash and v4-pro mixed | all admitted; each record has `model`, `default_model`, `routed`, `subtask`, `level: "L1"`; `/_init/stats` `spend.by_model` and `spend.task.by_model` sum to `spend.today_musd`; reservation priced per model |
| P-20 | Routable set: model priced in kernel config but absent from `RootState.models`; model forwarded by the Root but unpriced | both 400 `request.model_not_allowed` |
| P-21 | Gate snapshot: open a gate at L1, then `set_level(L0)` mid-gate | the gate's scopes keep L1 routing; `live` refuses routed models from the next request |
| P-22 | Soft budgets at L1: `set_budgets` within bounds; `evolution_share = 0.85`; `gate_usd` above share × day; `day_usd` above the clamp bound; the same valid call at L0; then the fake Root lowers its day ceiling below the soft day | first accepted and written to `state/kernel/budget.json` with `source: "agent"`; the next three `BudgetInvalid` (`E_BUDGET`); at L0 refused; on the lowered ceiling the effective soft day becomes ceiling − `request_max_usd`, `soft_day_clamped` is alerted, and the kernel keeps running |
| P-23 | Egress down: `init-fakeroot`'s egress socket removed mid-run | 503 `egress.unavailable`; reconnect after it returns; alert `egress_down` after 30 s |
| P-24 | Fleet member lanes (L2, `FAKEROOT_MODE=level:2`): two members with $0.50 shares; one exceeds its share; a third member whose share would exceed the soft day minus the evolution share | the first gets 429 `budget.member` with `retry-after`; the other member is unaffected; the third member's `open_scope` is refused `budget.member`; records carry `scope: member:<bid>` |
| P-25 | `judge` mode (kernel/gate.md §4.1a): proxy features inside a scratch home | no file written under the home's `vault/traces/`; observations still returned in memory |
| A6 | From inside the live sandbox: connect to a `probe/<id>/run/proxy.sock`; connect to `api.deepseek.com:443`; from the kernel's own process, connect to `api.deepseek.com:443` | all refused by the OS. kernel/sandbox.md runs the first two; the agent-zone network rule is root/root.md's harness. Reading the Root's key store is root/egress.md's test |

## 9. Open questions

**Within this doc**
1. Should the proxy pre-warm a provider's cache for a gate's shared system prefix before launching parallel G4 runs (one throwaway request), to avoid paying the DeepSeek common-prefix detection misses on every arm? Both arms pay the same misses, so it is a cost question, not a fairness one. Deferred until G4 cost data exists.

2. Should a gate's routable set be narrowed to what the LKG arm actually used, so that a candidate cannot look better only by routing to a model the LKG never tried? Current answer: no. Routing is a legitimate capability from L1, both arms get the same set, and the comparison is on reference cost, so the candidate pays for the model it picks. Revisit if Phase 1 data shows routing wins that do not survive the sealed audit.
3. Should `budget.root_guard_permille` scale with in-flight reservations rather than be a fixed 2%? Fixed for now, because both reservations are already worst case and the guard reads the Root's own figure. Revisit once Root refusal counts exist.

**Cross-doc** (v0.3; all R1/R2 items stay settled in reconciliation/kernel-proxy-gate.md):
- **Q-EG2 → root/egress.md: forwarded models.** Add the set of forwarded `(provider, model)` pairs (from the Root's ceiling price table) to `GET /_root/ceilings`, so that the kernel's routable set (§3.5a) never offers a model the Root will refuse. Optionally add a per-response `x-egress-cost-musd`.
- **Q-EG3 → root/egress.md: one retry owner.** Connect-phase retries are the Root's (`x-egress-retries`); status-based retries (429 rate, 5xx, 529) stay in the kernel (§3.9). Please confirm the Root does not also retry status codes. The egress should also withhold provider 401/403 bodies; the kernel rewrites them too, as defence in depth.
- **Q-EG4 → root/egress.md and root/root.md (R7): classifying a ceiling refusal under D6(c).** This doc sends `x-egress-guard` on every request (§3.8) and proposes the rule: a fresh Root figure that still showed headroom, or no header at all, means a bypass; a stale figure (older than 2 × `proxy.root_poll_s` = 20 s) means an ordinary hit. Settled by root/egress.md §3.6: the header is `epoch=<E>` and the Root classifies by epoch.
- **Q-EG5 → root/egress.md: prices.** The Root publishes its coarse peak table read-only (with Q-EG2), so that the guard's interim estimate uses the Root's prices.
- **Q-EG6 → root/egress.md and root/root.md: naming.** egress.md still writes `ROOT_EGRESS_SOCK` and `$INIT_ROOT_HOME`; root.md writes `ROOT_HOME/run/egress.sock`, given in the boot block. This doc uses root.md's (supervise.md item 12 raises the same).
- **Q-EG7 → root/egress.md: the `member` scope** value of `x-egress-scope` (§3.4), for parallel-body counting at L2.
- **Q-PV1 → body/provider.md:** send `x-init-subtask` (optional); handle 400 `request.model_not_routed` (L0), 429 `budget.root_day` (retry after `resets_at`), 429 `budget.root_total` and `budget.root_<quota>` (not retryable), and 403 `egress.<x>` (not retryable).
- **Q-EV1 → evolution.md §3.14:** the clamp of §3.8 is `min(day_usd, ceiling − request_max_usd, soft_day_fraction × ceiling)`; please keep `soft_day_fraction` or drop it in favour of the D6 bound alone.
- **Q-IC1 → kernel/initctl.md:** add `initctl budget set <key>=<value>…` (L1+, evolution.md §4.1; calls `ProxyHandle::set_budgets`; `E_BUDGET` on a bound violation) and show `level`, `root_*` and `budgets` in `status`.
- **Q-SV1 → kernel/supervise.md:** pass the boot block's `paths.egress_sock` to `Proxy::start`; relay `ceilings.get` and the `ceilings`/`level` events (`set_ceilings`, `set_level`); open a `member` scope per fleet member with its share; add `state/kernel/budget.json` to the layout; give the proxy each body's instance id for `x-egress-body`.
- **Q-TS1 → testing.md:** `init-fakeroot`'s egress should implement `/_root/ceilings`, typed refusals, the `x-egress-*` headers and a peak-priced spend counter, so that P-08 and P-09 can run against it.
- **Q-CV1 → CONVENTIONS.md §3, §5:** the `vault/keys/` row and the "Keys" global decision (`init-kernel keys set`) are stale in v0.3. rustls, hyper-rustls and the TLS side of tokio-tungstenite are no longer kernel dependencies; they move to `init-root` if root/egress.md keeps that stack.

## 10. Spec deltas

| Spec v0.2 | This doc | Why |
|---|---|---|
| §7.2 "HTTP/2 keep-alive" | h2 when ALPN negotiates it, h1 keep-alive pool otherwise | not every provider offers h2; DeepSeek does |
| §3 "allowlisted egress via proxy" | the model proxy forwards only to model providers on allowlisted routes; it is not a general egress proxy | a key-holding general proxy widens the blast radius |
| §7.2 traces "each distinct prefix stored once by hash and deltas appended" | content-addressed packs + message-level chains (`base`, `keep`, `add`) with checkpoints | handles compaction and masking, not only pure appends |
| (new) | scopes (lanes) bound to sockets chosen by supervise; in-process `ProxyHandle` | attribution must not depend on anything the body says |
| (new) | proxy-side bounded retries before the first forwarded byte | transport mechanics, identical across body versions |
| (new) | upstream 401/403 rewritten to 401 `upstream.auth` with the body withheld | upstream error bodies can echo key fragments |
| §7.2 "Over the cap it returns 429" | 429 with a typed body and `x-init-cap`; request cap also 429 but `retryable: false` | the body must tell a kernel cap from a provider rate limit |
| (new) | reference-price cost (`cost_ref`) alongside billed cost | DeepSeek's time-of-day prices would bias gate comparisons |

**v0.3 changes** (spec v0.3 Part II, ADR-007):

| Spec / v0.2 doc | This doc (v0.3) | Why |
|---|---|---|
| §7.2 "injects keys"; v0.2 §3.3 keys in `vault/keys/`, `init-kernel keys set` | no keys in the kernel; the Root holds and injects them (root/egress.md); `vault/keys/`, `keys set` and `keys check` removed; the body's auth headers are still dropped | spec v0.3 §14 R2, §19: keys move to the Root |
| §7.2 "enforces the owner-set daily and per-gate caps"; v0.2 §3.8 caps | hard day and total ceilings are the Root's (charter `ceilings.*`); the kernel keeps **soft budgets** (day, task, gate, quick gate, probe, scope, request, evolution share) as maintainer defaults, agent-set from L1, plus a 2% guard checked against the **Root's own** spend; new 429 codes `budget.root_day`, `budget.root_total`; Root refusals mapped, never retried | spec v0.3 §14 R2; evolution.md §3.14; the Root settles at peak prices, so only its own figure can keep the kernel off its ceiling (an attempt to exceed one demotes, §15) |
| §7.2 "Reverse proxy to the configured providers … persistent upstream connections" | the only upstream is the Root egress socket over local HTTP/1.1 keep-alive; provider TLS, h2 and the persistent pool move to the Root and now survive kernel restarts too | spec v0.3 §13: the agent zone's only network path is the Root egress socket |
| v0.2 non-goal "no decision on which model to use … the body chooses" | §3.5a level policy: L0 default model only (`request.model_not_routed`); L1+ per-subtask choice within the routable set; `x-init-subtask`; `model`, `default_model`, `routed`, `subtask`, `level` in every record; per-model stats and `ScopeSummary.by_model` | spec v0.3 §15 L1 "routes models per subtask within the ceiling", attributed in traces |
| (new) | L1+ soft-budget setting by the agent (`set_budgets`, bounds from evolution.md §3.14, `state/kernel/budget.json`, a Root lineage `budget` record); soft day clamped when the Root lowers its ceiling | spec v0.3 §15 L1 "allocates its daily budget" |
| v0.2 §3.10.3 cost = spend | kernel cost is measurement; the Root's metering is authoritative for money; `root_cost_musd` cross-check and `cost_mismatch` | two meters, one authority |
| v0.2 §3.1 panic owned by the kernel | the Root owns panic (R1); `ProxyHandle::panic`/`resume` and 503 `kernel.panicked` removed | spec v0.3 §14 R1, §19; root/root.md X4 |
| v0.2 `init-kernel proxy --standalone`, `proxy.test_mode` direct upstreams | removed: kernel tests run under `init-fakeroot`; the kernel never reaches a provider directly | testing.md §3.5b; roadmap.md §3.7 item 7 |
| (new, D6) | effective soft day = min(`day_usd`, ceiling − `request_max_usd`, 0.9 × ceiling), soft total = ceiling − `request_max_usd`, clamped on every `ceilings` event, never a refusal to start; `x-egress-guard` on every request so the Root can tell a bypass (demotes) from an ordinary hit (logged) | v03-decisions D6; root/root.md X4 |
| (new, D5) | no sealed traffic through the proxy; `judge` mode keeps no traces | v03-decisions D5 |
| (new, L2) | `member` scopes per fleet body with daily shares (`budget.member`) | kernel/supervise.md §3.14 |
| v0.2 §7 "the proxy runs unsandboxed as the owner's uid; it needs the network" | runs in the agent zone with no network of its own; agent-evolvable from L3, with the contracts other docs read listed in §7 | spec v0.3 §13, §15 L3 |
| v0.2 §3.14 `vis: kernel` hides holdout traces | still hidden from the body at L0–L2; reachable by an L3 agent through the kernel; sealed traffic never enters the proxy | the kernel–body line is a stability boundary in v0.3 (§13); the sealed set is Root-only (§16) |
| (new) | explicit non-goal: no response cache | serving cached answers is the "cache answer keys" mutant class (§16) |
| v0.2 alert `key_file_insecure`; `providers.*.organization/project` | removed (Root); new alerts `root_refused`, `egress_down`, `cost_mismatch`, `budget_root_guard`, `soft_day_clamped` | keys and account headers live with the Root |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.8):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP5 D1, WP22 D8 | `x-egress-guard: epoch=<E>` replaces the `day=…;total=…;seen=…;r=…` form; the Root classifies by epoch (root/egress.md §3.6) and a malformed header demotes | §3.8, §9 Q-EG4 |
| build: WP4 D2 | `x-egress-body` is the body's Root instance id (root/egress.md §4.1), not the boot id | §3.6 |
| build: WP5 D2 | the guard's Root view is `day_spent + day_reserved` of the last figure plus root-priced estimates of requests settled since (over-counts in-flight requests); the soft total is checked against the Root's lifetime figure | §3.8 |
| build: WP5 D3 | no `models` table in `/_root/ceilings` means the Root forwards every priced model | §3.5a |
| build: WP5 D4 | `route.bad_path` is checked before the route lookup, so `..` is a 400, not a 403 | §3.5 |
| build: WP5 D5 | neutral tier values (`default`, `auto`, `standard`; `global`, `not_available` in responses) are priced at ×1, not refused | §3.6, §3.10.3 |
| build: WP5 D6 | member lanes may sit under `run/fleet/`; member scope ids are `member:<bid>`; a member's share is `OpenScope.cap_musd`, and `job_id` is optional | §3.4, §4.4 |
| build: WP5 D7 | unmetered routes are forwarded with no budget check and no trace record | §3.5 |
| build: WP5 D8 | the no-usage estimate takes `content_bytes_seen` from the response bytes forwarded | §3.10.3 |
| build: WP5 D9 | a cancelled request is settled at its full reservation and writes no `end` record | §3.9 |
| build: WP5 D10 | the `end` record has a `resp_type` field (response content type) | §3.13 |
| build: WP5 D12 | the tap and trace helpers' edge rules are recorded as doc comments in `tap.rs` and `trace.rs`; the pre-filter runs on every line until the first content token | §3.10.1, §3.13 |

## 11. References

All checked 2026-10-19 unless marked. Added in v0.3 (2026-10-20, this repo): spec v0.3 Part II §12–§19; adr/007-root-and-autonomy-ladder.md; CONVENTIONS.md v0.3 §4, §6. The hyper client-conn API in §3.7 was not re-checked on docs.rs in this revision (marked UNVERIFIED there).
- DeepSeek, Your First API Call — https://api-docs.deepseek.com/
- DeepSeek, Models & Pricing — https://api-docs.deepseek.com/quick_start/pricing/
- DeepSeek, Context Caching — https://api-docs.deepseek.com/guides/kv_cache
- DeepSeek, Create Chat Completion — https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek, Thinking Mode — https://api-docs.deepseek.com/guides/thinking_mode
- DeepSeek, Tool Calls — https://api-docs.deepseek.com/guides/tool_calls
- DeepSeek, Using the Responses API — https://api-docs.deepseek.com/guides/responses_api
- DeepSeek, Using the Anthropic API — https://api-docs.deepseek.com/guides/anthropic_api
- DeepSeek, Rate Limit & Isolation — https://api-docs.deepseek.com/quick_start/rate_limit
- DeepSeek, Error Codes — https://api-docs.deepseek.com/quick_start/error_codes
- DeepSeek, Token & Token Usage — https://api-docs.deepseek.com/quick_start/token_usage
- DeepSeek, List Models / Get User Balance — https://api-docs.deepseek.com/api/list-models , https://api-docs.deepseek.com/api/get-user-balance
- DeepSeek, Change Log and V4.1-Flash-1017 news — https://api-docs.deepseek.com/updates , https://api-docs.deepseek.com/news/news261017
- Observed: `curl -v --http2 https://api.deepseek.com/models` (ALPN h2, 401) and a placeholder-token request (401 JSON body), 2026-10-19
- Anthropic, Prompt caching — https://platform.claude.com/docs/en/build-with-claude/prompt-caching
- Anthropic, Streaming — https://platform.claude.com/docs/en/build-with-claude/streaming
- Anthropic, Errors — https://platform.claude.com/docs/en/api/errors ; Rate limits — https://platform.claude.com/docs/en/api/rate-limits
- Anthropic, Pricing — https://platform.claude.com/docs/en/about-claude/pricing
- Anthropic, Versioning — https://platform.claude.com/docs/en/api/versioning
- OpenAI, WebSocket mode — https://developers.openai.com/api/docs/guides/websocket-mode
- OpenAI, Prompt caching — https://developers.openai.com/api/docs/guides/prompt-caching
- OpenAI, Streaming events — https://developers.openai.com/api/reference/resources/responses/streaming-events
- OpenAI, Error codes — https://developers.openai.com/api/docs/guides/error-codes ; Pricing — https://developers.openai.com/api/docs/pricing
- openai-python and anthropic-sdk-python retry logic (`_base_client.py`) — https://github.com/openai/openai-python , https://github.com/anthropics/anthropic-sdk-python
- hyper 1.12.0 http1 Builder — https://docs.rs/hyper/1.12.0/hyper/server/conn/http1/struct.Builder.html ; Incoming auto traits — https://docs.rs/hyper/1.12.0/hyper/body/struct.Incoming.html
- hyper-util 0.1.21 legacy Builder / Error — https://docs.rs/hyper-util/0.1.21/hyper_util/client/legacy/struct.Builder.html , https://docs.rs/hyper-util/0.1.21/hyper_util/client/legacy/struct.Error.html
- http-body 1.1.0 — https://docs.rs/http-body/1.1.0/http_body/trait.Body.html ; http-body-util 0.1.5 — https://docs.rs/http-body-util/0.1.5/http_body_util/
- hyper-rustls 0.27.10 (v0.2 upstream client; in v0.3 input for root/egress.md) — https://docs.rs/hyper-rustls/0.27.10/hyper_rustls/struct.HttpsConnectorBuilder.html
- rustls 0.23.46 CryptoProvider (v0.2; in v0.3 input for root/egress.md) — https://docs.rs/rustls/0.23.46/rustls/crypto/struct.CryptoProvider.html ; rustls-platform-verifier 0.7.1 — https://docs.rs/rustls-platform-verifier/0.7.1/
- tokio 1.53.3 UnixListener / UnixStream::peer_cred — https://docs.rs/tokio/1.53.3/tokio/net/struct.UnixListener.html
- tokio-tungstenite 0.30.0 — https://docs.rs/tokio-tungstenite/0.30.0/tokio_tungstenite/fn.client_async_tls_with_config.html
- serde_json 1.0.154 RawValue — https://docs.rs/serde_json/1.0.154/serde_json/value/struct.RawValue.html
- sha2 0.11.1 — https://docs.rs/sha2/0.11.1/sha2/struct.Sha256.html
- security-framework 3.7.0 `generic_password` (rejected alternative) — https://docs.rs/security-framework/3.7.0/security_framework/passwords/fn.generic_password.html
- `security` CLI ACL behaviour — https://ss64.com/mac/security-password.html
- TraceLab (append and prefix medians), via research_notes/where_performance_matters.md — https://arxiv.org/html/2606.30560v2
