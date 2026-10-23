# Rust and WebAssembly should carry INIT Agent

Write INIT Agent in Rust: the kernel and the agent-owned core body. Extend it at tier T1 with WebAssembly components hosted in Wasmtime. Keep prompts, skills and state as plain Markdown, JSON and JSONL. Apply core (T2) changes only through a gated process restart, never by reloading native code in-process. Keep C out of any code the agent edits. Rust is the only systems language where the 2026 evidence agrees on every axis that matters for a harness that edits itself:

- **LLM editing.** Rust is the top language for 7 of 11 frontier models on an independent repo-level benchmark (GPT-5.5 93.0%, Opus 4.7 88.4%).
- **Error catching.** Its compiler turns into build errors the bug classes that LLM-written C++ carries silently (9.3% of compiled AI C++ programs violate a sanitizer).
- **Build speed.** An incremental check of a 9k-line project takes 0.25 s on the owner's M2.
- **Footprint.** A Rust binary starts in 2.0 ms at 1.2 MB, only 0.3 ms and 0.2 MB behind C.

"Extreme performance" should not go into raw language speed. The harness's own CPU work is at most about 9% of end-to-end agent time, so removing it entirely gives at most a 1.10× speedup. The effort that pays goes, in order, to:

1. Prompt-cache stability. One cache miss costs about 17× the input price of a cached step on Opus 5.5.
2. Fewer model round-trips.
3. The tool-execution path.
4. Footprint when many instances run at once. This is the one place where Rust's native efficiency is decisive.

This reverses the spec's §8 choice of TypeScript on Bun and keeps its kernel/body split and T0–T1–T2 tiers. Bun's real advantage, a 35 ms loop with no build step, disappears when every kept T2 change waits minutes for the gate. The main open risks have not been measured yet: how long a Rust-to-Wasm extension takes to build on the M2, and how reliably frontier models write Component Model code, whose tooling has changed quickly.

## Twelve candidates narrow to Rust once the evidence is dated

INIT asks more of its language than an ordinary harness does:

- **Reliable model edits.** The model edits the source thousands of times with no human reviewer. The language must be one that frontier models edit reliably, with a compiler that rejects bad edits before they run.
- **Fast builds.** The self-edit loop runs many times a day.
- **Low startup and memory cost.** Evolution happens by restart, and later phases run many instances in parallel.
- **Safe live change.** T1 needs code that loads into a running agent and unloads cleanly. T2 needs a restart that cannot brick the agent.
- **Ecosystem.** The harness needs HTTPS with server-sent events, PTYs, OS sandboxing and git.

The table below condenses the evidence. Build and startup figures marked M2 are local measurements taken on the owner's Apple M2 on 2026-10-19. Other agents were loading the machine at the time, so the ratios are more reliable than the absolute values. Benchmark percentages are Claw-SWE-Bench resolve rates for GPT-5.5 and Opus 4.7 unless noted.

| Language | LLM edit reliability (2026) | What the compiler catches | Edit → rebuild (M2) | Start / RSS floor (M2) | Live-change path | Harness ecosystem | Verdict |
|---|---|---|---|---|---|---|---|
| **Rust** | Top for 7 of 11 models; 88.4–93.0% | Types, ownership, exhaustive matching; no UB in safe code; overflow panics in debug | `check` 0.25 s, `build` 0.39 s | 2.0 ms / 1.2 MB | Exec restart; Wasmtime host; dylib reload is dev-only | Codex's sandbox stack, rustls; git via CLI | **Kernel and core body** |
| C | Mid-pack (C/C++ 71.4–81.0%) | Syntax and types; UB compiles silently | 0.19 s | 1.7 ms / 1.0 MB | `dlopen` + cr.h; unloading is hazardous | Every C library; no TLS or JSON in std | Keep out of agent-edited code |
| C++ | Mid-pack (as C) | As C | not measured | ≈ C | Live++ (commercial developer tool) | Rich; complex builds | Exclude |
| Zig | No benchmark; stale syntax after each release | Good, but 0.17 changed `@bitCast` with no compile error | Fast backend is x86_64-only; aarch64 not usable | native, not measured | Hot-swap proofs of concept only | std TLS is young | Exclude |
| Go | Weakest language for all 11 models (61.9%) | Types; nil and data races are not caught | 0.53–0.85 s; 4.8–5.7 s with a cold cache | 2.9 ms / 3.7 MB | Exec restart (tableflip) | Strong; Landlock is process-wide | Runner-up |
| Odin | Anecdotes only | Types | not measured | native | DIY `dlopen` | No TLS in core | Exclude |
| OCaml | Correct, but 1.28–1.69× the tokens | Strong types | not measured | not measured | Limited | Thin | Exclude |
| Swift | No repo-level data | Strong; Swift 6 concurrency churn | 440 ms for hello world | 2.3 ms / 1.3 MB | None practical | macOS-centric | Exclude |
| Erlang/Elixir | Function-level only, and contested | Dynamic typing: little | not measured | BEAM VM; 2.6 KB per process | Best semantics: two versions, `code_change`, supervisors | OTP; sandbox by shelling out | Borrow its semantics |
| Lisp (CL, Clojure) | No 2026 frontier data | Dynamic typing: little | n/a (image-based) | not measured | Richest live redefinition, but the image becomes the source of truth | Thin | Exclude |
| TypeScript/Bun | 79.1–81.4% (JS/TS); Claude Code and Pi exist | Type errors (94% of LLM TS compile errors); `any` holes | 35 ms, with no type check | 15–18 ms / 19–21 MB; real harnesses boot at 84–286 MB | ESM cannot unload; restart | `fetch`, PTY; sandbox by shelling out | Strongest alternative |
| Python | 78.0–82.0%; cheapest in tokens | Nothing by default | none | 35 ms / 12 MB | `importlib.reload` is unreliable | Richest | Scripts and evals only |

Six candidates drop out for reasons unrelated to taste:

- **Zig** has the best build story on paper. Its new ELF linker rebuilds a small app in about 30 ms and the Zig compiler itself in 228–288 ms ([Zig devlog 2026](https://ziglang.org/devlog/2026/)). That path is x86_64-Linux only. The aarch64 backend passed 1,656 of 1,972 behavior tests (84%), and the release notes say it is not yet usable for real work ([Zig 0.15.1 notes](https://ziglang.org/download/0.15.1/release-notes.html)), so the owner's M2 still compiles through LLVM. LLM proficiency is shaky:
  - After each breaking release, models emit syntax for older Zig. Only the frontier closed models finished a Zig 0.15 migration task, and every open model failed ([akitaonrails](https://akitaonrails.com/en/2026/01/11/ai-agents-comparing-top-llms-on-the-zig-challenge/)).
  - Zig 0.17 changed `@bitCast` semantics in a way the compiler does not flag ([daily.dev summary of 0.17 notes](https://daily.dev/posts/ffqew3gdc)).
  - The Zig Software Foundation bans LLM-generated contributions outright ([OpenSourceForU](https://www.opensourceforu.com/2026/05/zig-draws-hard-line-on-ai-bun-chooses-fork-over-upstreaming/)).
  - Bun, the most prominent AI-maintained Zig codebase, left for Rust.
- **Odin** has no TLS in its core library. Its maintainer estimates native TLS at about a man-year of work ([Odin forum](https://forum.odin-lang.org/t/will-odin-have-its-own-tls-library-like-golang/288)). LLM proficiency evidence is only anecdotal.
- **OCaml** reaches Python-level correctness in agent runs, but it was the most token-expensive of the four languages tested, and agents fought its syntax ([Tokenmaxxing study](https://arxiv.org/html/2607.22807v1)). Jane Street trained its own model because off-the-shelf models handled OCaml poorly ([Gulati on Jane Street](https://shekhargulati.com/2025/03/29/talk-ai-engineering-at-jane-street)).
- **Swift** has no repo-level LLM data, and scores drop on problems that need Swift-specific features ([SwiftEval](https://arxiv.org/pdf/2505.24324)). It took 440 ms to compile hello world on the M2, three to five times as long as C or Rust.
- **Common Lisp and Clojure** offer the richest live redefinition of any runtime. CLOS migrates each object to a redefined class lazily, through a user-overridable hook ([CLHS](https://www.lispworks.com/documentation/HyperSpec/Body/f_upda_1.htm)). But image-based editing makes the mutated image, not the git repository, the source of truth. That breaks the lineage invariant (spec §2.1 #4) that INIT's gate and rollback depend on. There is also no 2026 frontier evidence for either language.
- **C++** has the same undefined-behavior problem as C, plus heavier build pipelines. The Multi-SWE-bench authors name both as sources of its variance ([Multi-SWE-bench](https://arxiv.org/html/2504.02605v1)).

Three more languages earn partial roles rather than the core:

- **Go** builds fast and has a strong ecosystem. But on the one independent 2026 repo-level benchmark with per-language results, its authors found Go the weakest language for every one of the 11 models, 10–22 points below each model's total ([Claw-SWE-Bench](https://arxiv.org/html/2606.12344v2)). Its type system leaves nil dereferences and data races to run time. Its Landlock binding must restrict the whole process, because goroutines are not pinned to OS threads ([go-landlock](https://github.com/landlock-lsm/go-landlock/blob/main/README.md)). It would be the runner-up if Rust were unavailable.
- **Erlang/Elixir** has the best semantics for live code change ever shipped. OpenAI's Symphony orchestrator uses BEAM for its supervision trees ([InfoQ](https://infoq.com/news/2026/05/openai-symphony-agents)). But:
  - Dynamic typing leaves the gate little to check.
  - BEAM is a second runtime to install.
  - Sandboxing means shelling out to external binaries.
  - The LLM evidence is function-level and contested ([AutoCodeBench](https://arxiv.org/pdf/2508.09101); [Elixir Forum critique](https://elixirforum.com/t/paper-finds-llms-perform-twice-as-well-with-elixir-than-with-python-or-javascript/73284)).

  INIT should copy BEAM's semantics into its host rather than adopt the VM.
- **Python** is the cheapest language for agents in tokens across all five models tested ([Tokenmaxxing study](https://arxiv.org/html/2607.22807v1)). It is also where evaluation tooling lives: Harbor, a Python library, ran about 0.3M trajectories for Harbor-Index ([Harbor-Index](https://arxiv.org/abs/2609.04298)). But it has no compile-time gate, and `importlib.reload` leaves existing instances on the old class definitions ([Python docs](https://docs.python.org/3/library/importlib.html)). It belongs in scripts the agent runs through `bash` and in Phase 2 evaluation, not in the body.

That leaves the owner's two preferences, C and Rust, and the spec's incumbent, TypeScript on Bun.

## The compiler is the only reviewer that reads every self-edit

LLM proficiency per language changed sharply between 2025 and 2026:

- **2025.** On Multi-SWE-bench, Claude 3.7 Sonnet resolved 52.2% of Python issues but only 15.9% in Rust, 8.6% in C and 2.2% in TypeScript ([Multi-SWE-bench](https://arxiv.org/html/2504.02605v1)).
- **2026.** On Claw-SWE-Bench (350 instances in 8 languages, v2 September 2026), the picture is reversed ([Claw-SWE-Bench](https://arxiv.org/html/2606.12344v2)):

  | Model | Rust | C/C++ | JS/TS | Python | Go |
  |---|---|---|---|---|---|
  | GPT-5.5 | **93.0%** | 81.0% | 79.1% | 78.0% | 61.9% |
  | Claude Opus 4.7 | 88.4% | 71.4% | 81.4% | 82.0% | 61.9% |

  Rust is the top language for 7 of the 11 models.
- **SWE-bench Multilingual** already ranked Rust highest (58.1%) and C/C++ lowest (28.6%) with Claude 3.7 Sonnet ([swebench.com](https://www.swebench.com/multilingual.html)).

Three caveats limit these numbers:

- Each language has only about 42–50 instances and no significance test, so a few points is one or two tasks.
- With the model held fixed, the harness changes resolve rates about as much as the language does.
- No vendor publishes per-language results for the newest models (Opus 5.5, GPT-5.6, Gemini 3.5). Any statement about INIT's actual models is an extrapolation from GPT-5.5 and Opus 4.7.

SWE-Bench ProMax, a refactoring benchmark, found that no single model leads in every language ([ProMax](https://arxiv.org/html/2608.09802)). The defensible reading is that frontier models now edit existing Rust codebases at least as reliably as code in any other language, not that Rust is uniquely easy.

### C fails silently; Rust fails at compile time

For INIT, what happens when an edit is wrong matters more than how often the first attempt is right. INIT's body is a long-lived codebase that a model edits thousands of times without human review, and its first gate stage (G0) can check only what the toolchain can prove. LLM-written C and C++ compile easily and fail silently:

- In a 2026 study of 8,918 C++ programs from three open models, **9.3% of compiled AI programs triggered ASan/UBSan violations, against 2.6% of human programs**. Signed overflow and heap and stack buffer overflows dominated ([Illusion of Safety](https://arxiv.org/html/2607.00107v1)).
- **1.8% of the AI programs that passed every test still violated a sanitizer** ([same study](https://arxiv.org/html/2607.00107v1)).
- The sanitizers and the ESBMC model checker barely overlapped (Jaccard ≈ 0.05). A C gate would need sanitizers, fuzzing and a formal tool together to catch most of these bugs.
- Formal verification of LLM-written C found 51–62% of programs vulnerable ([FormAI](https://arxiv.org/abs/2307.02192v3)).

Rust moves the same bug classes to compile time or to a loud run-time panic. A 2026 taxonomy of 86,726 LLM errors found that Rust code produced the most compile errors (23,416, against 12,304 for C). It also produced the most overflow panics, because debug builds trap the overflows that pass silently in C, C++ and Java ([ISSRE 2026](https://arxiv.org/html/2608.00661)). These figures come from small and mid-sized models. No study yet holds the task and budget fixed and compares the final correctness of Rust and C for frontier agents.

The production evidence points the same way. Bun's team kept hitting use-after-free, double-free and missed frees in Zig, even after patching the Zig compiler. Then 64 Claude agents ported 535,496 lines of Zig to Rust in 11 days, for about $165k at API prices. Jarred Sumner's summary: "Compiler errors are a better feedback loop than a style guide" ([Pragmatic Engineer](https://blog.pragmaticengineer.com/the-pulse-what-can-we-learn-from-buns-rapid-rust-rewrite-with-ai/)). The port passed 99.8% of the test suite five days before it merged, and Claude now maintains it ([DevClass](https://www.devclass.com/ai-ml/2026/05/15/anthropics-bun-rust-rewrite-merged-at-speed-of-ai/5240541)).

Rust gives INIT something the spec values highly: an invariant enforced by a mechanism rather than by a prompt. Put `#![forbid(unsafe_code)]` at the root of the body crate and have the gate reject any change to that line. "The agent-owned body cannot corrupt memory" then becomes a compiler check, the same way the spec makes `kernel/` read-only at the OS level instead of asking the model to leave it alone. C cannot offer that at any gate cost.

C's real advantages are small at INIT's scale:

- **Rebuild.** On the M2, C rebuilt a one-file edit in 0.19 s, against 0.39 s for Rust.
- **Startup.** C started in 1.7 ms at 1.0 MB, against 2.0 ms at 1.2 MB for Rust. **That gap of 0.3 ms and 0.2 MB** is below anything an agent loop can notice.
- **Cost to write.** C is not cheaper for the agent. In 600 Claude Code runs that implemented the same program in 13 languages, C took 155.8 s and $0.74 per run, against 113.7 s and $0.54 for Rust ([mame/ai-coding-lang-bench](https://github.com/mame/ai-coding-lang-bench)).
- **Libraries.** C's standard library has no TLS, HTTP or JSON, so a C harness would pull in OpenSSL, libcurl and a JSON parser anyway.

### What Rust costs, and why it is small for INIT

Rust's costs are real and should be budgeted, not dismissed:

- **Tokens.** Agents spend 1.16× (Claude Sonnet 4.6) to 1.36× (GPT-5.5) the tokens of Python to reach a passing solution ([Tokenmaxxing study](https://arxiv.org/html/2607.22807v1)).
- **Variance and failures.** Rust runs had higher variance (113.7 s ± 54.8 s) and accounted for 2 of the 3 failures in the 600-run study ([mame](https://github.com/mame/ai-coding-lang-bench)).
- **API drift.** Models do worse on crate APIs that changed after their training cutoff. In RustEvo², success fell from about 56% to 32.5%, and supplying the docs recovered about 13.5 points ([RustEvo²](https://arxiv.org/abs/2503.16922v1)).

Two facts shrink these costs for INIT. First, the token premium applies only to self-edits. The user's tasks are in whatever languages the user's repositories use, so the body's language does not tax the main workload. Second, API drift comes through dependencies, and INIT controls those. A body with two pinned, vendored dependencies and no async runtime leaves little room for drift. Bun's port shipped with no async Rust at all ([DevClass](https://www.devclass.com/ai-ml/2026/05/15/anthropics-bun-rust-rewrite-merged-at-speed-of-ai/5240541)). Leaving out `tokio` also removes the largest clean-build cost measured on the M2: 8.1 s for a release binary whose only dependency was tokio.

### Build speed does not separate the compiled candidates

Build speed turns out not to discriminate. On a synthetic 9k-line project on the M2, a one-file edit cost:

| Toolchain | One-file edit |
|---|---|
| Rust `cargo check` | 0.25 s |
| Rust debug `cargo build` | 0.39 s |
| C | 0.19 s |
| Go | 0.53–0.85 s (Go recompiles the whole package) |
| Bun running the TypeScript version | 35 ms, but no type checking |

`tsc --noEmit`, the spec's G0 check, was not measured. The spec budgets 1–3 minutes for a T2 change, because the gate must boot the candidate, complete a real model round trip, and push a scripted self-edit through G0–G2. A sub-second compile is noise inside that.

Rust's actual hazards in a self-edit loop are elsewhere:

- **Release builds.** Release builds skip incremental compilation by default, so the M2 rebuilt the whole project (1.21 s) after a one-line change. The body should run in a development profile. Amdahl's law (next sections) says that costs nothing measurable.
- **Clean builds with dependencies.** These are slow. The gate's "fresh sandbox" should therefore mean a fresh filesystem view with a warm, shared, read-only build cache, not a cold cargo cache.
- **Disk.** Each worktree with its own `target/` multiplies disk use. During this research the owner's Mac had **9.1 GiB free, below the 20 GB floor**. Worktrees should share compiled artifacts through one cache and be cleaned by exact path.

Rust's 2026 "fast builds" goals (the TPDE backend, relink-don't-rebuild, a parallel front end) are plans, not results ([Rust 2026 roadmap](https://goals.rust-lang.org/2026/roadmap-fast-builds.html)).

### The case for TypeScript on Bun

TypeScript on Bun is the strongest alternative, and the spec's reasons for it are sound as far as they go:

- Claude Code is TypeScript and is reportedly 80–90% written by Claude ([Pragmatic Engineer interview](https://newsletter.pragmaticengineer.com/p/building-claude-code-with-boris-cherny)).
- Pi, the minimal core INIT copies, is TypeScript.
- 94% of compile errors in LLM-written TypeScript are type errors that `tsc` reports ([Mündler et al., PLDI 2025](https://arxiv.org/abs/2504.09246)).
- Frontier models resolve 79–81% of JS/TS issues on Claw-SWE-Bench.

Its distinguishing advantage, running edited code in 35 ms with no build, is worth little when every kept change waits minutes for a gate. Meanwhile its costs land exactly where INIT is exposed:

- **Type escape hatches.** `any` and unchecked casts are available to a self-editing model under pressure.
- **Memory growth.** Claude Code's issue tracker holds reports of processes growing to 13 GB, 120 GB and 304 GB ([#24840](https://github.com/anthropics/claude-code/issues/24840); [#4953](https://claudeissues.com/issue/4953-claude-code-memory-leak-process-grows-to-120-gb-ram-and-gets-oom-killed); [#17615](https://claudeissues.com/issue/17615-memory-leak-claude-process-using-304gb-memory)).
- **No unloading.** ESM modules cannot be unloaded, and the cache-busting workaround leaks memory ([Balkan](https://ar.al/2021/02/22/cache-busting-in-node.js-dynamic-esm-imports/)). That is why the spec must restart the body after 50 extension reloads.
- **Binary size.** A compiled Bun executable is at least 60 MB on the M2, and Claude Code's is 199 MB. Keeping N rollback versions costs N × 60–200 MB, against a few MB each for Rust.
- **Sandboxing.** It requires shelling out to `sandbox-exec` or bubblewrap ([Claude Code sandboxing](https://docs.claude.com/en/docs/claude-code/sandboxing)).
- **Agent time.** In the 13-language study, TypeScript also took longer than Rust: 133.0 s against 113.7 s ([mame](https://github.com/mame/ai-coding-lang-bench)).

Given the owner's stated preference for Rust, nothing in the evidence argues for keeping TypeScript in the body.

## Restart the core, sandbox the leaves, never unload native code

Hot-reloading native code inside a running process is a developer convenience, not a mechanism a self-editing agent can rely on. Rust has no stable ABI, and every approach hits a wall:

- **Classic dylib reloading.** The standard reloader crashes after a signature change, and a layout mismatch between host and plugin is undefined behavior ([hot-lib-reloader](https://github.com/rksm/hot-lib-reloader-rs)).
- **`abi_stable`.** It type-checks libraries at load time but cannot unload them ([abi_stable](https://docs.rs/abi_stable)).
- **Thread-locals.** A `thread_local!` in a library that has been `dlclose`d still runs its destructor after the code is gone ([rust#88737](https://github.com/rust-lang/rust/issues/88737)).
- **Platforms.** musl does not implement `dlclose`, and one production Rust project found unloading broken on macOS ([Continuwuity](https://forgejo.ellis.link/mmaudet/continuwuity/src/branch/main/docs/development/hot_reload.mdx)).
- **Subsecond.** Dioxus's Subsecond, the most sophisticated Rust hot-patcher, routes calls through a jump table and rewinds stale stack frames. But it compiles only into debug builds, patches only the tip crate, and cannot change structs ([subsecond docs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html)).

Other languages are no better:

- **C.** `cr.h` adds automatic state transfer and rollback on SIGSEGV ([cr](https://github.com/fungos/cr)). A signal handler is not a safety boundary, though: by the time it fires, memory may already be corrupt.
- **Zig.** In-process hot swapping exists only as 2022–2023 proofs of concept, and the 2026 devlog announces nothing newer ([Zig devlog](https://ziglang.org/devlog/2026/)).

The managed runtimes tell the same story from the other side. BEAM is the gold standard:

- two versions of each module coexist;
- running code switches to the new version at its next fully qualified call;
- `code_change` migrates process state;
- supervisors shut down when restarts exceed a limit, by default 1 per 5 s ([Erlang code loading](https://www.erlang.org/doc/system/code_loading.html); [supervisor](https://www.erlang.org/doc/apps/stdlib/supervisor.html)).

Even so, Erlang practitioners treat in-place release upgrades as a last resort and prefer rolling restarts ([LYSE](https://learnyousomeerlang.com/relups)). The conclusion the spec reached for TypeScript holds in every language: the core changes only by process restart.

### T2: a fork-and-exec handoff with a readiness check

For T2, the proven Rust pattern is a fork-and-exec handoff. Cloudflare's `ecdysis` has run in production since 2021 across more than 330 data centers ([Cloudflare](https://blog.cloudflare.com/ecdysis-rust-graceful-restarts)):

1. The old process forks a child.
2. The child `execve`s the new binary, which gives it a clean address space.
3. The parent keeps serving until the child signals ready.
4. If the new binary fails during initialization, it exits and the parent simply carries on.

The `bye` crate packages the same contract ([bye](https://docs.rs/crate/bye/latest)). `rename(2)` makes the swap of the binary path atomic ([man7 rename](https://man7.org/linux/man-pages/man2/rename.2.html)).

INIT can adopt this with one simplification and one improvement:

- **Simplification.** INIT's state already lives in `events.jsonl`, so it needs no memfd or systemd fd store. That matters because neither exists on macOS.
- **Improvement.** The spec's swap (§7.1) stops the old body before starting the new one. Instead, the kernel can start the candidate while the current body pauses at a turn boundary, and hand over the log only when the candidate writes `ready`. If the candidate fails, the old body resumes at once. A failed swap stops being a 90-second probe followed by a rollback boot and becomes a non-event.

Crash rollback to the last-known-good version (LKG) stays as the spec designed it. The kernel's proxy, not the body, holds the connection to the model provider, so a body restart also costs no TLS handshake and no reconnect.

### T1: WebAssembly components on Wasmtime

For T1, WebAssembly components on Wasmtime fit INIT's leaf tier better than anything else the research found:

- **Fast instantiation.** Instantiating a pre-compiled module fell from about 2 ms to **5 µs** with copy-on-write heaps, the pooling allocator and `InstancePre` ([Bytecode Alliance](https://bytecodealliance.org/articles/wasmtime-10-performance); [Wasmtime tuning docs](https://docs.wasmtime.dev/examples-fast-instantiation.html)).
- **Runaway control.** Epoch interruption stops runaway code at about a 10% slowdown. Fuel does it deterministically, at a higher cost ([Wasmtime docs](https://docs.wasmtime.dev/examples-interrupting-wasm.html)).
- **Deny-by-default capabilities.** A component sees only the files, sockets and host functions it is granted.
- **Hot swap with no special runtime feature.** Compile the new component off the hot path, build an `InstancePre`, swap a pointer at a turn boundary, and let in-flight calls finish on the old version. That reproduces BEAM's two-version semantics in a Rust host.
- **No leak on reload.** Dropping a `Store` frees its instances ([Store docs](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html)), so the spec's rule to restart after 50 reloads (exit code 76) is no longer needed.
- **Acceptable compute cost.** The penalty is 1.46–2.41× native on a crypto-heavy suite in June 2026 ([00f.net](https://00f.net/2026/06/23/webassembly-runtimes-2026/)). That is irrelevant for tools that mostly wait on files, processes and networks.

There is precedent. Zed moved its extensions to Wasm after native tree-sitter scanners crashed the editor, so that no extension could take it down ([Zed](https://zed.dev/blog/language-extensions-part-1)). Microsoft's Wassette already runs Wasm components as agent tools and converts their WIT interfaces to JSON Schema ([Wassette](https://opensource.microsoft.com/blog/2025/08/06/introducing-wassette-webassembly-based-tools-for-ai-agents/)). That last property matters for awareness. A component carries a typed description of its exports, so the kernel can build the T1 part of `self.json` from the binaries themselves instead of trusting what the agent says it built.

In INIT, the Wasm boundary provides fault isolation and resource limits. The security perimeter is still the OS sandbox around the whole body.

Three cautions apply:

- **WASI version.** WASI 0.3, with native async, was ratified on 2026-06-11 and is on by default in Wasmtime 46 and later. Guest-language async bindings are still landing ([WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3)), so INIT's tools should target the stable `wasm32-wasip2` for now and move later.
- **Compiler backend.** The non-default Winch compiler had a sandbox-escape CVE in versions up to early 2026, so INIT should use Cranelift ([CVE-2026-34987](https://releasealert.dev/cve/CVE-2026-34987)).
- **Model proficiency.** This is the biggest unknown. Component Model tooling has moved quickly, and the RustEvo² results suggest models will lag behind it. No study measures how well frontier models write components.

The alternatives each fail on one axis INIT needs:

- **Embedded Luau or QuickJS** reload source in microseconds; a complete QuickJS runtime life cycle takes under 300 µs ([QuickJS](https://bellard.org/quickjs/)). But they add a second language with no compile-time gate and a weaker sandbox. Luau, for example, can abort under a memory limit with untrusted input ([mlua#488](https://github.com/mlua-rs/mlua/issues/488)).
- **Out-of-process MCP servers** isolate well, but they take 200 ms to 5 s to start. That was slow enough that Codex and Claude Code both made MCP startup non-blocking in 2026 ([Codex MCP cold start](https://codex.danielvaughan.com/2026/07/23/codex-cli-mcp-cold-start-non-blocking-startup-oauth-timeout-tool-catalog-reliability/); [Claude Code #76239](https://claudeissues.com/issue/76239-sdk-headless-mcp-tools-silently-missing-on-first-turn-when-stdio-server-startup)).
- **Pi's in-process extensions** iterate fastest, but they run with the user's full system permissions ([Pi extension docs](https://app.unpkg.com/@earendil-works/pi-coding-agent@0.83.0/files/docs/extensions.md)).

| Mechanism | Load latency | Isolation | INIT tier |
|---|---|---|---|
| Atomic file `rename`, reread at the turn boundary | Instant | n/a | T0 |
| Wasm component on Wasmtime (pooling, `InstancePre`) | Guest build not measured; instantiation ≈ 5 µs | Linear memory, capability grants, epochs or fuel | **T1** |
| Embedded Luau or QuickJS | µs–ms, no build | VM sandbox, weaker | Fallback lane for quick T1 glue code |
| MCP stdio subprocess | 200 ms – 5 s cold | OS process | Third-party tools only |
| Native `dylib` or Subsecond | ms | None; unloading is unsafe | Never |
| Fork + exec handoff with readiness check | New process start plus log rehydration | Full process | **T2** |

One interaction ties this tier design to cost. Model providers cache the prompt prefix in the order tools → system → messages, and changing any tool definition invalidates all three caches ([Anthropic caching docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)). A T1 reload that adds or changes a tool schema therefore makes the next step a full cache miss. A reload that changes only hook code or tool internals costs nothing. INIT should tell the agent which kind of reload it is doing and batch schema changes. The next section puts a price on that.

## Harness CPU is under 9% of agent time, so aim the performance effort elsewhere

Agent time goes almost entirely to the model and the tools:

- **Real sessions.** Across 4,265 Claude Code and Codex sessions, 92.3% of wall-clock time was the human thinking, 3.3% model generation and 4.8% tool execution. Within a single request, tools took 59.8% and generation 41.0% ([TraceLab](https://arxiv.org/html/2606.30560v2)).
- **Controlled harness experiments.** Host-side orchestration took at most 9% of end-to-end time ([Aries](https://arxiv.org/abs/2607.29069)).
- **Self-hosted models.** Inference took 71–98% of runtime ([Yuan et al.](https://arxiv.org/html/2605.26297v1)).

By Amdahl's law, eliminating that 9% entirely would give at most **1.10×**, and a language change recovers only part of it. Two rewrites test this directly:

- Bun's Rust port cut Claude Code's Linux p50 startup from 517 ms to 464 ms. The Bun team reports that users noticed little else ([Bun blog](https://bun.com/blog/bun-in-rust)).
- Codex, already in Rust, still spent 18.6% of its timed tool time in harness-side overhead around the tool, with a p99 of 10 s ([TraceLab](https://arxiv.org/html/2606.30560v2)). It also burned about 30% CPU while idle on a status animation that redrew every 32 ms ([aident.ai](https://aident.ai/blog/fix-codex-cli-high-cpu-background-terminal)).

Language moved the margins. Design moved the outcomes.

Faster inference changes the ranking but not the conclusion. GPT-5.3-Codex-Spark serves more than 1,000 tokens per second. When OpenAI got there, the first new bottleneck was transport, not harness CPU. A persistent connection with incremental input cut per-round-trip overhead by 80% (vendor-reported) and made rollouts with 20+ tool calls up to about 40% faster ([OpenAI Spark](https://openai.com/index/introducing-gpt-5-3-codex-spark/); [WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)). At 1,000 tokens per second with a 0.3–0.5 s time to first token, a step's model time falls to about 0.5–0.7 s. A typical tool call outside the slow tail (about 2.6 s, derived from TraceLab) then dominates the step:

- a fixed harness cost of 300–500 ms per action becomes 10–15% of the step;
- a 50 ms cost becomes about 1.5%;
- microseconds of language speed never register.

### A prompt-cache miss costs about 17× the input of a cached step

Prompt caching is the largest cost lever the harness controls. Prices for Opus 5.5 ([Anthropic caching docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)):

| Rate | Price per MTok |
|---|---|
| Base input | $4.00 |
| Cache read | $0.20 |
| Five-minute cache write | $5.00 |

TraceLab's median step has a 119K-token prefix and an 875-token append. At these prices, a cached step costs about $0.028 of input and a full miss about $0.48, **roughly 17× more**. Anthropic measured time to first token falling from 11.5 s to 2.4 s for a 100K-token cached prompt ([Anthropic](https://claude.com/blog/prompt-caching)). Real sessions already hit the cache for 95.7% of tokens, yet the prefix is still 59.5% of cost ([TraceLab](https://arxiv.org/html/2606.30560v2)).

Manus ranks KV-cache hit rate as its most important production metric, and it warns that some languages serialize keys in unstable order ([Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)). That warning applies directly to Rust. The default `HashMap` is randomly seeded per process, so any map serialized into the prompt in `HashMap` order produces different bytes after every restart, which is exactly when INIT most needs the cache to survive. INIT should use ordered maps or sorted keys. G2 should compare a hash of the serialized prefix before and after the candidate boots.

### Footprint is the one place where Rust's efficiency pays

Language efficiency clearly pays when many instances run at once. Production harnesses now run inside RL rollouts by the thousand. LEGO-RL's trainer spent 66.1% of training time waiting for rollouts when Claude Code was the harness ([LEGO-RL](https://arxiv.org/abs/2608.17393)). A native Rust ACP adapter idles at about 3 MiB with a 4 ms cold start, against about 76 MiB and 100 ms for the Node version ([pi-acpinator](https://libraries.io/cargo/pi-acpinator)). At 1,000 instances that is roughly 3 GiB against 74 GiB.

Per-task overhead inside a runtime is noise by comparison. On the M2, an idle async task costs about 0.3 KB in tokio and 2.8 KB as a goroutine. What multiplies is RSS per process: the TypeScript harnesses measured on the M2 boot at 84–286 MB. Even so, sandbox working sets (up to 28 GB per session) and GPU KV cache (up to 11 GB per Claude Code session) dwarf any harness process ([AgentSysBench](https://arxiv.org/abs/2608.15127)). Footprint is a secondary cost, but a real one. It is also exactly INIT's Phase 3 scenario of parallel explorers, and it applies to any future use of INIT as an evaluation or RL harness.

Rendering and per-turn CPU work come last. A terminal UI that re-renders the whole message on every token becomes a real CPU sink at high token rates; the fixes are differential rendering and O(delta) streaming ([Claude Code fullscreen](https://code.claude.com/docs/en/fullscreen); [Helix Zed fork](https://blog.helix.ml/p/how-we-forked-zed-to-run-a-fleet)). Context assembly and JSON take milliseconds against seconds of model and tool time. They only need to avoid copying the full history on every step, the bug that once made vLLM's detokenizer O(n) per step ([vLLM PR #24174](https://github.com/vllm-project/vllm/pull/24174)).

### Ranked: where to spend INIT's performance effort

| Rank | Where to spend effort | Size of the lever (evidence) | What INIT should do | Does the language matter? |
|---|---|---|---|---|
| 1 | Prompt-cache stability across turns, reloads and restarts | A miss costs ≈ 17× a cached step's input on Opus 5.5; time to first token 11.5 s → 2.4 s at 100K cached | Byte-stable prefix; sorted serialization; prefix-hash check in G2; batch tool-schema changes; volatile facts at the tail | No (only determinism) |
| 2 | Fewer and overlapped model round-trips | Up to ≈ 40% faster on 20+ tool-call chains ([OpenAI](https://developers.openai.com/api/docs/guides/websocket-mode)); programmatic tool calling cut tokens 37% ([Anthropic](https://www.anthropic.com/engineering/advanced-tool-use)); speculative tool runs cut task time 43–48% ([PASTE](https://arxiv.org/html/2603.18897)) | Kernel proxy keeps the upstream connection open across body restarts; parallel tool calls; allow programmatic tool calling | No |
| 3 | Tool-execution path | PTY instead of tmux: 0.78 → 0.42 s per action ([ProRL](https://arxiv.org/html/2603.18815v1)); Codex harness-side tool overhead 18.6% with p99 10 s; `rg` > 15 s in big monorepos ([Cursor](https://cursor.com/blog/fast-regex-search)) | Sandbox the body once, not per call; persistent PTY as the first growth step; spill output to files; shell out to `rg`; run long tools in the background | Partly (process and PTY handling) |
| 4 | Self-edit loop latency (T1/T2) | Compile 0.25–0.39 s against a gate of minutes | Warm shared build cache; vendored dependencies; staged gate with early exit; G4 asynchronous | Little |
| 5 | Footprint and stability × N | 3 MiB vs 76 MiB per idle process → 3 GiB vs 74 GiB at 1,000; GC-runtime leak reports up to 304 GB | Rust body; bounded in-memory view of the log; deduplicated traces; RSS-over-time test | **Yes** |
| 6 | Streaming render (only with a TUI) | O(N²) → O(delta) fixes; 16 ms frame batching | Differential rendering | Somewhat |
| 7 | Per-turn CPU: JSON, token counts, context assembly | Milliseconds against seconds; ≤ 9% for all host-side work | Keep it incremental, then stop | No |

Ranks 1, 2 and 4 are architecture, and they work the same in any language. Rust's speed shows up only at ranks 3, 5 and 6. Optimizing below that line is where "extreme performance" effort gets wasted: SIMD JSON parsers, custom allocators, release-mode builds of the body, an async runtime at seed.

## One stack per layer, mapped onto the spec

| Layer (spec section) | Language and runtime | How it changes | Guard that keeps quality |
|---|---|---|---|
| **Kernel / supervisor** (§3, §7): supervise, proxy, gate, initctl, selfmap, sandbox | One Rust release binary with subcommands. tokio, hyper and rustls only in the proxy. Seatbelt on macOS and Landlock/seccomp/bubblewrap on Linux, as Codex does. The `git` CLI. | Maintainer only; rebuilt rarely | Restart-intensity limit escalating to safe mode; the kernel never links agent code |
| **Core body, T2** (§4, §6.1): main, loop, provider, context, log, read/write/edit/bash, extension host | Rust 2024 edition with `#![forbid(unsafe_code)]`. Std threads, no async runtime at seed. Plain HTTP and SSE over a Unix socket to the kernel proxy. Only vendored `serde_json` and `wasmtime`. Development build profile. | Edit in a worktree → G0 (`cargo check`, `clippy -D warnings`, `cargo test`) → kernel gate → fork+exec swap with readiness | Compiler as G0; prefix-hash check in G2; shared warm build cache |
| **Leaf extensions, T1** (§4 extension loader, §6.1) | Wasm components on `wasm32-wasip2`, written in Rust by default. One versioned WIT world for tools and hooks. Wasmtime with Cranelift, pooling, an `InstancePre` swapped atomically, epoch deadlines, `StoreLimits`, deny-by-default WASI. | Build the component → `initctl reload` → swap at the turn boundary; old Stores drain | Trap budget per extension; WIT-derived inventory in `self.json`; tool-schema changes batched for the cache |
| **Ad-hoc tools** | Any language through `bash` (Python, shell) | None; the next call runs the new file | The OS sandbox |
| **Data, T0** (§5, App. B) | Markdown (SELF.md, skills), JSON with sorted keys (state), JSONL (events) | Atomic `rename`, reread at the turn boundary | Byte-stable prefix; itemized deltas, never wholesale rewrites |

### Kernel

The kernel becomes one Rust binary with subcommands for supervise, proxy, gate, selfmap and the initctl server. The `initctl` the agent runs inside the sandbox is a thin client that talks to that server over a Unix socket, so the agent never runs kernel logic with kernel privileges. The kernel holds the provider connections, keys and spend caps, which means TLS exists only in the kernel. Its sandbox setup can follow Codex's published design rather than invent one ([Codex sandbox internals](https://codex.danielvaughan.com/2026/05/03/codex-cli-sandbox-internals-seatbelt-bubblewrap-landlock-windows-dacl/)):

- Seatbelt through `sandbox-exec` on macOS;
- Landlock, a seccomp network filter and bubblewrap on Linux;
- `.git` re-protected read-only.

Git work goes through the CLI, because gitoxide's push support was still incomplete in April 2026 ([gitoxide](https://awesome.ecosyste.ms/projects/github.com%2FGitoxideLabs%2Fgitoxide)). The supervisor's restart limit should follow OTP's escalation model and end in the spec's safe mode.

The proxy's raw-trace log (spec §7.2) needs one change for the owner's disk budget. At a median prefix of 119K tokens, logging every request in full stores about 0.5 MB per step, or roughly 40 MB per 84-step session. Storing each distinct prefix once by hash and appending only deltas keeps `vault/` small.

### Body

In the body:

- **Model calls.** The provider speaks plain HTTP/1.1 with SSE to the kernel proxy over a Unix socket, which needs only `std`.
- **Concurrency.** Parallel tool execution uses scoped std threads. Real sessions average about 1.2 tool calls per step ([TraceLab](https://arxiv.org/html/2606.30560v2)), and an OS thread costs about 14 KB resident, so an async runtime adds build time and LLM error surface for no measurable gain.
- **Dependencies.** The only seed dependencies are `serde_json` and `wasmtime`, both vendored. The sandbox's network reaches only the kernel proxy, so `cargo` cannot fetch crates. Adding a dependency should therefore be a kernel-mediated fetch (for example an `initctl vendor` command) that triggers the full gate.
- **Size budget.** Express the body budget in tokens rather than lines. Line counts do not carry over between languages, and the spec's real goal is that the whole body fits in context at about 20k tokens. The current 1,440-line estimate was made for TypeScript and must be redone after a Phase 0 port.

### Changes to the spec

The changes are local:

- §0 and §8 name Rust and Wasm instead of TypeScript and Bun.
- §3 and §4 rename files from `.ts` to `.rs`, and add `ext/*.wasm` and a `wit/` directory.
- In §6.3, G0 becomes `cargo check`, `clippy` and `cargo test`, plus two flags: the `forbid(unsafe_code)` line must be untouched, and any `Cargo.lock` change forces full G4.
- G2 gains the prefix-hash comparison.
- §4's 50-reload rule and §7.1's exit code 76 can go.
- §7.1 gains the overlapped swap.
- §7.4 notes that dependencies are vendored.

### Phase 0 measurements before locking the stack

Phase 0 should measure five things before the stack is locked:

1. **Body build time.** Clean and incremental builds of a Rust body skeleton that includes `wasmtime`, with a warm shared cache and with a cold one. This is the gate's worst case.
2. **T1 latency.** Warm and cold build time of a roughly 150-line `wasm32-wasip2` tool. The spec targets about 1 s for T1. I propose that if a warm rebuild exceeds 5 s, INIT adds the Luau lane for quick glue code.
3. **Agent success writing components.** About ten trials of the agent writing a component from the WIT template. This costs model tokens and needs the owner's authorization and a dollar cap.
4. **Cache survival across restarts.** The prefix hash must be identical across a T2 restart.
5. **Memory stability.** RSS must stay flat across a synthetic 1,000-step session.

Throughout, Phase 0 must keep at least 20 GB of disk free.

## Conclusion

In a self-editing system, the language matters less for speed than for what the gate can prove. The compiler is the only reviewer that reads every edit. Choosing Rust, and forbidding `unsafe` in the body, turns "the agent cannot corrupt its own memory" from a hope into a mechanical check, the same move the spec already makes by putting invariants in OS boundaries rather than prompts. Self-modification also couples performance to evolution policy more tightly than in an ordinary harness. Every tool-schema change the agent makes is a full prompt-cache miss, so the gate should report what a change costs in cache as well as what it does to scores, and the agent should learn to batch such changes.

The decision is also less permanent than it looks. The kernel–body contract is files, exit codes and an HTTP proxy, so the body's language is itself a T2 choice. A future agent could port its own body through the gate, as Bun's agents ported half a million lines in 11 days, if models or languages shift. The parts that truly lock in are the kernel and the T1 WIT interface. They deserve the most design care, and versioning the WIT interface from day one, as Zed does with its extension API ([Zed](https://zed.dev/blog/zed-decoded-extensions)), is the cheapest insurance INIT can buy.
