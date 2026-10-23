# ADR-005: Rust for the kernel and body, WebAssembly components for T1

```
Status: draft v0.3 (reconciled R2) · 2026-10-19 · Area: overview · Decision: proposed (records spec v0.2 §8; final after Phase 0 M-1..M-3)
Depends on: spec v0.2 §4, §6.1, §8, §9, §11; reports/Ngôn ngữ cho INIT Agent.md; testing.md §3.11;
            adr/003-std-threads-no-async.md; adr/004-wasm-components-for-t1.md (host details); ext/host.md §3.2
Defines:    the language and runtime decision for every layer; the conditions that would reopen it
```

## Context

INIT's body is source code that a model edits thousands of times with no human reviewer, and the kernel must keep that body recoverable. The language therefore has to satisfy five requirements at once:

1. **Reliable model edits** on an existing codebase, with a compiler that rejects bad edits before they run (the gate's G0 can check only what the toolchain proves).
2. **Fast enough builds** for a self-edit loop that runs many times a day.
3. **Low startup and memory cost**, because evolution happens by restart and Phase 3 runs many instances.
4. **Safe live change:** leaves that load and unload in a running agent, and a core that can be replaced without bricking it.
5. **Ecosystem:** HTTPS with SSE, PTYs, OS sandboxing (Seatbelt, Landlock, seccomp, bubblewrap), git.

Spec v0.1 chose TypeScript on Bun. The owner prefers Rust or C. The language report compared twelve candidates with dated 2026 evidence and local M2 measurements; this ADR records its conclusion.

**Evidence that decided it** (from the report; author-reported unless marked):

| Question | Finding |
|---|---|
| Can frontier models edit it? | Claw-SWE-Bench (independent, v2 Sept 2026, 350 instances in 8 languages): Rust is the top language for 7 of 11 models (GPT-5.5 93.0%, Opus 4.7 88.4%); C/C++ 71.4–81.0%; JS/TS 79.1–81.4%; Go weakest for all 11 (61.9%). About 42–50 instances per language, no significance test. |
| What does the compiler catch? | 9.3% of compiled AI-written C++ programs violated ASan/UBSan against 2.6% of human ones; 1.8% still did after passing every test. Rust turns these classes into compile errors or loud debug-mode panics. `#![forbid(unsafe_code)]` makes "the body cannot corrupt its own memory" a compiler check. |
| Build speed (M2, 9k-line synthetic project) | `cargo check` 0.25 s, debug build 0.39 s one-file edit; C 0.19 s; Go 0.53–0.85 s; Bun 35 ms without type checking. Release builds rebuild everything (1.21 s), so the body uses a development profile. |
| Startup and memory (M2) | Rust 2.0 ms / 1.2 MB; C 1.7 ms / 1.0 MB; Bun 15–18 ms / ~20 MB; installed TS harnesses 84–286 MB. |
| Cost for the agent to write it | 600 Claude Code runs, same program in 13 languages: Rust 113.7 s / $0.54 per run; C 155.8 s / $0.74; TypeScript 133.0 s. Rust spends 1.16–1.36× Python's tokens with no accuracy loss. |
| Live change | Rust has no stable ABI; `dlclose` with thread-locals is unsound (rust-lang #88737); Subsecond works only in debug builds and cannot change structs. Fork+exec handoff is proven (Cloudflare ecdysis since 2021). Wasmtime instantiates a pre-compiled component in ≈ 5 µs with pooling and `InstancePre`, and dropping a `Store` frees its memory. |
| Where performance actually comes from | Harness CPU is ≤ 9% of agent time (max 1.10× from an infinitely fast harness). Ranks 1, 2 and 4 of spec §9 (cache stability, round trips, self-edit latency) are language-independent; the language matters for footprint × N (≈ 3 MiB vs ≈ 76 MiB per idle process). |

## Decision

| Layer | Language and runtime | Rules |
|---|---|---|
| **Kernel** (`init-kernel`, `initctl`, `init-proto`) | Rust 2024, one release binary with subcommands; tokio, hyper and rustls only inside the proxy | never links agent code; git through the CLI; sandbox design follows Codex's published Seatbelt/Landlock/seccomp/bwrap stack |
| **Body, T2** (`init-body`) | Rust 2024, development profile (dependencies compiled at `opt-level = 2`, ext/host.md 10.7) | `#![forbid(unsafe_code)]` on every target root (G0 checks it); std threads only in body code, no async runtime at seed (ADR-003; `wasmtime-wasi` starts its own internal runtime for WASI I/O, ext/host.md 10.3); dependencies `serde`, `serde_json` (never `preserve_order`), `wasmtime`, `wasmtime-wasi`, all vendored; plain HTTP/1.1 + SSE to the proxy over a Unix socket; changes take effect only through a new process (overlapped fork+exec handoff) |
| **Leaves, T1** | WebAssembly components, target `wasm32-wasip2`, written in Rust by default, hosted by Wasmtime with **Cranelift** (not Winch: CVE-2026-34987), pooling allocator, `InstancePre`, epoch deadlines, `StoreLimits`, deny-by-default WASI (host details: ADR-004, ext/host.md) | one versioned WIT world `init:ext@1.0.0`; swap at the step boundary; kept components are built by the gate, never taken from the agent; the component's exports feed `self.json` |
| **Data, T0** | Markdown (`GENESIS.md`, `SELF.md`, skills), JSON with sorted keys (state), JSONL (event log) | atomic `rename`; reread at the step boundary |
| **Ad-hoc tools** | any language through `bash` (Python, shell) | no gate, no reload; the OS sandbox is the boundary |
| **Never** | C or C++ in agent-edited code; native `dylib` reload; Subsecond-style hot patching | |

Versions (crates.io, 2026-10-19): the seed pins `wasmtime` and `wasmtime-wasi` **=48.0.6**, the current LTS line with 24 months of security support (ext/host.md §3.2, CONVENTIONS §5; the newest release is 49.0.3, 2026-10-14), `wit-bindgen` =0.63.0 (used only by the separate extension workspace `seed/ext/`, never linked into `init-body`; extension crates are `cdylib`-only and exempt from the body's `forbid(unsafe_code)` rule, CONVENTIONS §5), `serde` 1.0.229, `serde_json` 1.0.154, `tokio` 1.53.3, `hyper` 1.12.0, `rustls` 0.23.46. Local toolchain rustc 1.97.1. WASI 0.3 (native async) was ratified on 2026-06-11 and is on by default in Wasmtime 46+, but guest async bindings are still landing, so T1 targets the stable `wasm32-wasip2` now and migrates later through a WIT version bump.

## Alternatives considered

| Option | Why not |
|---|---|
| **C** | Saves 0.3 ms startup and 0.2 MB; in exchange LLM-written C fails silently (UB compiles), which breaks Article 0. Not cheaper for the agent to write ($0.74 vs $0.54 per run). No TLS/HTTP/JSON in its standard library. |
| **C++** | C's UB problem plus heavier builds; Multi-SWE-bench names both as variance sources. |
| **Zig** | Fast self-hosted backend is x86_64-Linux only; aarch64 backend not usable for real work on the M2. Models emit stale syntax after breaking releases; 0.17 changed `@bitCast` semantics without a compile error. The Zig Software Foundation bans LLM-generated contributions. |
| **Go** | Weakest language for all 11 models on Claw-SWE-Bench; nil and data races reach run time; Landlock must restrict the whole process. Runner-up if Rust were unavailable. |
| **TypeScript on Bun** | Strongest alternative (Claude Code and Pi are TS; 94% of LLM TS compile errors are type errors). Its real advantage, a 35 ms loop with no build, is irrelevant when every kept T2 change waits minutes for the gate. Costs: `any` escape hatches, reported memory growth to 13–304 GB in a production TS harness, ESM cannot unload, ≥ 60 MB binaries, sandboxing only by shelling out. |
| **Python** | Cheapest in tokens and home of Harbor, but no compile-time gate and `importlib.reload` is unreliable. Kept for scripts and Phase 2 evaluation. |
| **Erlang/Elixir** | Best live-upgrade semantics ever shipped, but dynamic typing leaves G0 little to check, a second VM to install, sandboxing by shelling out, contested LLM evidence. INIT copies BEAM's semantics (two versions, drain, supervisor intensity → safe mode) instead. |
| **Common Lisp / Clojure** | Richest live redefinition, but the image becomes the source of truth, breaking the git lineage invariant; no 2026 frontier evidence. |
| **OCaml, Swift, Odin** | OCaml most token-expensive and models fought its syntax; Swift has no repo-level LLM data and 440 ms hello-world builds; Odin has no TLS and only anecdotal LLM evidence. |
| **T1 alternatives** | Embedded Luau/QuickJS in the body process: µs reloads but a second language with no compile gate and a weaker sandbox. The **fallback lane** if M-2/M-3 fail is instead a JavaScript engine (QuickJS, else boa) running *inside* a Wasm component, so it keeps the same sandbox and limits and adds no body dependency (ext/authoring.md §3.10). MCP stdio servers: 0.2–5 s cold start; third-party tools only. Native dylib or Subsecond: unloading is unsafe. |

## Consequences

**Positive**
- G0 is a real reviewer: types, ownership, exhaustive matching, and `forbid(unsafe_code)` on every self-edit, at no gate cost beyond the build.
- Footprint lets Phase 3 run several explorers on the M2 (≈ 3 MiB idle each).
- Codex's Rust sandbox stack and Wasmtime are native, so the kernel's riskiest parts follow published designs.
- T1 components carry typed exports, so the kernel derives the T1 part of `self.json` from binaries instead of trusting the agent.
- The body's language is itself a T2 choice: the kernel–body contract is files, exit codes and two sockets, so a future agent could port its body through the gate. What locks in is the kernel and the WIT contract.

**Negative, with mitigations**
- **Token premium** of 1.16–1.36× on self-edits only; user tasks are in whatever language the user's repos use.
- **API drift** on post-cutoff crate APIs (RustEvo²: 56% → 32.5%): two pinned, vendored dependencies; `initctl vendor` routes any new dependency through the kernel and full G4.
- **Heavy clean builds** with wasmtime: shared warm build caches, APFS-cloned for the gate; measured as M-1 at M0 (roadmap R2).
- **Component authoring by models is unmeasured** and its tooling moves fast: template plus skill; M-2 (warm build ≤ 5 s) and M-3 (≥ 6/10) at M0/M4; the JavaScript-in-Wasm lane if criteria C1–C3 trigger (ext/authoring.md §3.10, roadmap R1).
- **Disk:** Rust target dirs are large; one shared `CARGO_TARGET_DIR` per workspace and cleanup by exact path (roadmap §3.8).
- **Variance:** Rust runs had higher variance in the 600-run study (113.7 s ± 54.8 s); the gate's budgets and attempt caps absorb it.

**What would reopen this decision**
- M-1: cold body build > 10 min or target dir > 4 GB on the M2 → revisit wasmtime features first, the language last.
- M-2: warm component build > 5 s, or M-3 failing criteria C2/C3 after template fixes → add the JavaScript-in-Wasm lane for T1 glue (ext/authoring.md §3.10); the core stays Rust.
- A future independent benchmark showing frontier models editing another compiled language markedly better than Rust on repo-level tasks.

## References

All checked 2026-10-19 unless noted; full citations are in `reports/Ngôn ngữ cho INIT Agent.md`.

- Claw-SWE-Bench v2: https://arxiv.org/html/2606.12344v2
- Illusion of Safety (sanitizer violations in AI C++): https://arxiv.org/html/2607.00107v1
- mame/ai-coding-lang-bench (600 runs, 13 languages): https://github.com/mame/ai-coding-lang-bench
- Tokenmaxxing study (token premium): https://arxiv.org/html/2607.22807v1
- RustEvo² (API drift): https://arxiv.org/abs/2503.16922v1
- rust-lang #88737 (thread-local destructors after `dlclose`): https://github.com/rust-lang/rust/issues/88737
- Cloudflare ecdysis: https://blog.cloudflare.com/ecdysis-rust-graceful-restarts
- Wasmtime fast instantiation: https://docs.wasmtime.dev/examples-fast-instantiation.html
- WASI 0.3 announcement: https://bytecodealliance.org/articles/WASI-0.3
- CVE-2026-34987 (Winch): https://releasealert.dev/cve/CVE-2026-34987
- Aries (harness CPU ≤ 9%): https://arxiv.org/abs/2607.29069 ; TraceLab: https://arxiv.org/html/2606.30560v2
- crates.io versions: https://crates.io/api/v1/crates/wasmtime (and `wasmtime-wasi`, `wit-bindgen`, `serde`, `serde_json`, `tokio`, `hyper`, `rustls`); LTS choice and its rationale: ext/host.md §3.2, spec delta 10.2
- Local M2 measurements (build, startup) from the language report, taken under load on 2026-10-19.
