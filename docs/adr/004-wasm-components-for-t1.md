# ADR-004: WebAssembly components on Wasmtime for T1 extensions
Status: draft v0.1 · 2026-10-19 · Area: ext
Depends on: spec v0.2 (§2.1, §4, §6.1, §8, §9, §11), ext/wit.md, ext/host.md, ext/authoring.md   Defines: the decision record for the T1 mechanism (no contract of its own; the contracts are in the three ext docs)

Decision status: **proposed**, for owner review with spec v0.2. It becomes "accepted" when Phase 0 confirms H0 (`forbid(unsafe_code)` compiles with `bindgen!`) and C1 (warm build ≤ 5 s); see Consequences.

---

## Context

INIT has three tiers of self-change (spec §6.1). T1 is for **leaf capabilities**: tools the agent writes during a task, loads without restarting, and uses in the same conversation. T1 has to:
1. **Load and swap in-process at a turn boundary,** without a restart and without leaking memory over many reloads.
2. **Keep the prompt cache intact** (spec §9 rank 1). Adding a capability must not change the tool list.
3. **Never take the body down.** A bad extension must not crash the process or corrupt its memory. The body is `#![forbid(unsafe_code)]`, and that guarantee must not be bypassed by the extension mechanism.
4. **Stop runaway code.** Wall-clock deadlines, memory caps.
5. **Be verifiable by the kernel.** The T1 part of `self.json` must come from the binaries, not from the agent's claims (spec §5).
6. **Be writable by a frontier model** in minutes, with a fast edit-build loop (spec §11 calls this the biggest unknown).
7. **Add no authority** beyond what the body's OS sandbox already grants (spec §2.1, invariant 3).

Facts that constrain the choice (sources in References):
- **Native reload is unsafe in Rust.** Rust has no stable ABI. `dlclose` with thread-local destructors is a documented memory-safety bug (rust-lang/rust#88737). `abi_stable` cannot unload. musl has no `dlclose`. Dioxus Subsecond works only in debug builds and only on the tip crate.
- **Wasmtime instantiates fast.** A pre-compiled module instantiates in about 5 µs with pooling and `InstancePre` (Bytecode Alliance). Epoch interruption costs about 10%. `StoreLimits` caps memory. Dropping a `Store` frees its instances.
- **Plain cargo builds components.** `wasm32-wasip2` has been Tier 2 since Rust 1.82. `cargo build --target wasm32-wasip2` emits a component through the bundled `wasm-component-ld`. The Component Model book now calls `cargo-component` deprecated.
- **Winch is out.** Wasmtime's Winch backend had a critical sandbox escape, CVE-2026-34987 (GHSA-xx5w-cvp6-jv83, 2026-04-10), with a working proof of concept on aarch64, the M2's architecture. Cranelift is the default and was not affected.
- **WASI preopens have escaped too.** Wasmtime published two high-severity WASI filesystem escapes in 2026 (2026-08-20 and 2026-10-14), both fixed in 48.0.6 and 49.0.3. The Wasm boundary is strong fault isolation, but not the security perimeter.
- **Wasmtime 48 is the current LTS.** It is supported until about 2028-08, while normal releases get about 2 months.
- **Deserializing is unsafe.** `Component::deserialize*` is `unsafe`. Wasmtime's on-disk compilation cache has a safe signature, but on a hit it runs the same unchecked rehydration on bytes read from disk.
- **Two production hosts exist:**
  - Zed hosts Rust components on Wasmtime 48 with epochs and an in-memory incremental compilation cache, and versions its API with one bindgen module per version.
  - Microsoft Wassette hosts components as MCP tools on Wasmtime 48.0.6, with a fresh store per call and `InstancePre`.

## Decision

1. **T1 extensions are WebAssembly components targeting WIT world `init:ext/tool@1.0.0`, written in Rust and built with plain cargo for `wasm32-wasip2`,** in a separate extension workspace `body/ext/` (cdylib-only; exempt from the body's `forbid(unsafe_code)` because they run in Wasm; nothing in the body workspace may depend on them).
   - The world exports `describe() -> tool-info` and `call(input: string) -> result<string, tool-error>`.
   - It imports one host interface, `init:ext/process` (one-shot `run` plus pipe sessions).
   - Through WASI p2 it gets two preopens (`/work`, `/data`), captured stdio, clocks and random. Network is denied, and there are no hooks in 1.x (ext/wit.md).
2. **The host is Wasmtime `=48.0.6`, Cranelift only** (the `winch` feature is not compiled), with:
   - epoch interruption on 10 ms ticks while calls run;
   - the pooling allocator (32 instances, 256 MiB per memory);
   - `StoreLimits`;
   - synchronous WASI p2 through `wasmtime-wasi`;
   - one `InstancePre` per loaded version;
   - a fresh `Store` per call (ext/host.md).
3. **Swap protocol:**
   - compile and type-check on a loader thread;
   - swap an `Arc<Registry>` at a turn boundary;
   - calls in flight keep their `Arc<ExtVersion>` until they finish;
   - a call blocked in a host call past its deadline is abandoned by a watchdog and counted;
   - an owner interrupt stops a running call within one 10 ms epoch tick (an epoch-deadline callback checks the abort flag and the wall-clock deadline).
4. **Compilation caching is in memory only:**
   - compiled `Component`s live for the life of the process;
   - Cranelift's incremental cache is kept in a 32 MiB in-memory `CacheStore`;
   - no disk cache, no `deserialize`;
   - the body never calls an `unsafe` Wasmtime API.
5. **The model calls every extension through the fixed `ext` tool** `{name, input}`. Each extension's JSON Schema comes from `describe` and reaches the model through reload reports, `state/ext/catalog.json` and `self.json`, never through the tool list.
6. **The kernel derives the T1 inventory itself:**
   - it rebuilds kept extensions from source;
   - it decodes their WIT;
   - it runs `describe` in a sandboxed child process with no capabilities.
7. **The contract is versioned by major.**
   - Minors are additive: new interfaces or worlds only.
   - Records and existing interfaces are frozen within a major.
   - The body supports N and N−1.

## Alternatives considered

| Alternative | Why not |
|---|---|
| **Native dylib plugins** (`libloading`, `abi_stable`, hot-lib-reloader) | Undefined behavior on layout drift. Unloading is unsafe (TLS destructors, rust#88737), impossible on musl, unreliable on macOS. Needs `unsafe` in the body. Gives no isolation: a bad plugin crashes or corrupts the body. |
| **Dioxus Subsecond hot-patching** | Debug builds only, tip crate only, no struct changes. A developer tool, not a self-modification mechanism. |
| **Everything as T2 (rebuild and hand off the core)** | Correct but slow (minutes, dominated by the gate). The agent could not grow a capability and use it in the same few turns, which is what Live-SWE-agent's online tool creation shows pays (+3.2 points). |
| **Out-of-process MCP servers** | Good isolation, but 200 ms–5 s cold starts (Codex and Claude Code both had to make MCP startup non-blocking). One process per tool hurts footprint (rank 5). Kept for third-party tools only, started lazily (spec §4). |
| **In-process Luau (mlua) or QuickJS (rquickjs)** | No build step (µs reloads), but a second language with no compile-time gate, a weaker sandbox (mlua#488: abort under a memory limit), and new C/C++ dependencies in a body whose dependencies are fixed. Kept as a **fallback lane, run inside Wasm**, if Phase 0's warm build exceeds 5 s (ext/authoring.md §3.10). |
| **Wasmer, WAMR, WasmEdge** | Faster on some compute benchmarks (WAMR AOT 1.57× native against Wasmtime 2.41× on libsodium, June 2026). But Component Model and WASI p2 support was not verified for them, they have no `InstancePre`/pooling story we could check, and they have smaller security track records. T1 tools are I/O-bound, so compute speed is not the lever. |
| **Winch for faster compiles** | CVE-2026-34987, with an aarch64 proof of concept. Not compiled in. |
| **Wasmtime 49.x/50.x (latest) instead of 48 LTS** | Two-month support windows mean frequent vendored upgrades, and each forces full G4. 48 has every API we need and 24-month security support. |
| **`cargo-component`** | Being deprecated per the Component Model book. Plain cargo already emits components. |
| **WASI 0.3 / async components** | Rust's `wasm32-wasip3` is Tier 3 on stable today (Tier 2 from Rust 1.100). Guest async bindings are still settling. The body has no async runtime. Revisit as `init:ext@2`. |
| **Long-lived store per extension (Zed)** | In-memory state between calls, but poisoned instances after a trap, calls serialized, memory that grows over time, and state to migrate on swap. A fresh store per call costs tens of µs and removes all of that. |
| **One typed WIT signature per tool, JSON Schema derived from WIT (Wassette's component2json)** | Each tool would need its own world, so changing a tool's input would change the contract the kernel owns. A fixed `call(string)` keeps the contract frozen while tools evolve freely. The schema is declared by the tool and checked for drift by its own unit test. |
| **On-disk compiled cache (`Config::cache` or `.cwasm` + `deserialize`)** | `deserialize` is `unsafe`. The disk cache hides the same trust behind a safe signature: whoever writes the cache directory (here, the agent's `bash`) can inject native code into the body. Recompiling in memory costs an estimated 30–300 ms per component. |
| **Hooks in 1.0** | No observed failure requires them. They sit on every step's hot path and threaten the byte-stable prefix. The loop (T2) already covers the need. Adding a `hook` world later is a free minor version. |
| **Network for extensions (`wasi:http`)** | The body's sandbox reaches only the kernel sockets, and the proxy routes only to model providers. Can be added in a minor version if the proxy gains general allowlisted egress. |

## Consequences

All timing and size figures below are UNVERIFIED estimates until Phase 0 measures them (ext/host.md §3.14 and §6, ext/authoring.md §3.6).

**Positive:**
- An extension can be written, built (estimated 1–2.5 s warm), loaded (estimated 30–300 ms) and used in the same task, with no restart and no prefix change. The A2 acceptance test exercises exactly this.
- A broken extension cannot crash the body or corrupt its memory:
  - traps, timeouts and out-of-memory become tool results;
  - the next call gets a fresh instance;
  - no `unsafe` Wasmtime API is reachable from the body.
- Memory stays flat across thousands of calls and reloads, because every store is dropped (test H16).
- The kernel's inventory of T1 is derived from binaries it builds itself, which keeps the self-model honest (spec §5).
- T1 adds no authority. Processes and files are what `bash` already has, and there is no network.
- The contract can grow (new interfaces, a hook world) without breaking built extensions. Majors are supported N and N−1.

**Negative / costs:**
- **Build and size.** `wasmtime` + Cranelift + `wasmtime-wasi` make the body's cold build heavier (Phase 0 measurement 1). `wasmtime-wasi` also brings tokio, whose runtime starts lazily inside the body; CONVENTIONS §5 (v0.2) accepts it as an internal of an allowed dependency, since the body itself has no async code (ext/host.md §10.3).
- **Body size.** The ext host is a sizable part of the body's token budget. Estimated 450–600 lines including the process host; to be measured after the Phase 0 port.
- **Dependencies must be optimized.** The body's dependencies must be compiled at `opt-level = 2` even in the dev profile, or Cranelift compiles take seconds.
- **Instantiation on macOS.** Without `deserialize_file`, copy-on-write memory init is unavailable on macOS. Instantiation is about 20–100 µs there instead of about 5 µs (estimate). Negligible against model latency.
- **Host-blocked calls.** A guest that sleeps in a host call cannot be interrupted. Such calls are abandoned and counted, and that extension is `busy` until they end.
- **Model proficiency** with components is still unmeasured. The mitigations are the template, the skill and the fallback lane, gated on Phase 0 measurements M-2/M-3.
- **Cross-doc work.** The runtime layout gains `bin/ext/`, `state/ext/` and `body/wit/`. The loop gains its reload call sites and passes its abort flag. The kernel gains a WIT decoder and a sandboxed `describe` runner, publishes gate-built components (never committed ones), and should classify member-only `ext/Cargo.lock` changes as T1, not as dependency changes. Ledger: docs/reconciliation/ext.md.

**Follow-ups (each owned elsewhere):**
1. Phase 0:
   - H0 (`forbid(unsafe_code)` + `bindgen!`; fallback is the dynamic `Val` API);
   - H1 (engine config);
   - M-2 (build times, criterion C1);
   - M-3 (authoring success);
   - H16 (RSS).
2. Owner installs the `wasm32-wasip2` target before Phase 0 (not installed as of 2026-10-19).
3. Revisit: Wasmtime 60 LTS (about 2027-08); WASI 0.3 (`init:ext@2`); the PTY implementation (agent-driven, T2).

## References

All checked 2026-10-19.

- CVE-2026-34987 / GHSA-xx5w-cvp6-jv83: https://github.com/advisories/GHSA-xx5w-cvp6-jv83
- Wasmtime advisories (2026, incl. GHSA-vqjp-4c8c-hfgg, GHSA-m9f7-9w4p-gv38): https://github.com/bytecodealliance/wasmtime/security/advisories
- Wasmtime LTS policy: https://docs.wasmtime.dev/stability-release.html
- Wasmtime source v48.0.6 (`Config`, `Component::deserialize*`, cache hit path, `CacheStore`): https://github.com/bytecodealliance/wasmtime/tree/v48.0.6
- Instantiation speed and pooling: https://bytecodealliance.org/articles/wasmtime-10-performance ; https://docs.wasmtime.dev/examples-fast-instantiation.html
- Epoch vs fuel: https://docs.wasmtime.dev/examples-interrupting-wasm.html
- Native unload hazards: https://github.com/rust-lang/rust/issues/88737 ; https://docs.rs/abi_stable ; Subsecond: https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html (via research_notes/…/live_modification_isolation.md)
- `wasm32-wasip2` Tier 2: https://blog.rust-lang.org/2024/11/26/wasip2-tier-2/ ; cargo-component deprecation: https://component-model.bytecodealliance.org/language-support/building-a-simple-component/rust.html
- Zed extension host: https://raw.githubusercontent.com/zed-industries/zed/main/crates/extension_host/src/wasm_host.rs ; https://zed.dev/blog/zed-decoded-extensions
- Wassette: https://github.com/microsoft/wassette ; https://opensource.microsoft.com/blog/2025/08/06/introducing-wassette-webassembly-based-tools-for-ai-agents/
- Runtime benchmarks (June 2026): https://00f.net/2026/06/23/webassembly-runtimes-2026/
- MCP cold starts and mlua#488: reports/Ngôn ngữ cho INIT Agent.md (T1 section) and its research notes
- `wasm32-wasip3` status: https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip3.html
