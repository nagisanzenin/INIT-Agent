# Runtime and toolchain performance of candidate languages for a self-rebuilding agent harness (as of 2026-10-19)

Scope: C, C++, Rust, Zig, Go, Odin, OCaml, Swift, Erlang/Elixir (BEAM), TypeScript on Bun/Node, Python.

How to read this file:
- **[LOCAL]** marks numbers I measured in this session (2026-10-19) on the owner's machine: Apple M2, 16 GB RAM, macOS (Darwin 23.6.0). Toolchains: rustc/cargo 1.97.1, go 1.27.2, Apple clang 16.0.0, Swift 6.0.3, Bun 1.3.9, Node v25.5.0, Python 3.14.4. Method: Python `time.perf_counter()` around `subprocess.run` (adds about 1 ms of spawn overhead), median of 20 runs after 3 warm-ups for startup, median of 3–5 runs for builds, and peak RSS from `/usr/bin/time -l` (median of 5). **Caveat:** the load average was about 4.8–5.1 during the runs because other agents were running, so absolute numbers are noisy; the ratios are more reliable. Zig, Odin, OCaml, Erlang/Elixir, tcc, mold and hyperfine are **not installed** on this machine, so none of them was measured locally. All local scratch files were deleted afterwards.
- "Claim" means a vendor or author statement that has not been independently verified. "Measured" means a benchmark with a stated method.
- **Disk warning for the owner:** `/` had **9.1 GiB free** at the time of the measurements, which is below the 20 GB floor in the owner's CLAUDE.md. This affects any plan that builds Rust or Zig locally, since target directories and caches grow quickly.

## Q1. Build speed for the edit → build → test → swap loop (small 3k–10k LOC project)

### Takeaway
On a ~9k LOC project, every compiled candidate measured here rebuilds an edit in under a second: Rust `cargo check` 0.25 s and `cargo build` 0.39 s, C 0.19 s, Go 0.5–0.85 s. Bun runs TypeScript with no build step in about 35 ms, which is 10–20x faster than any compile. Rust's real costs are the first clean build with dependencies (8.1 s for a tokio-only binary) and release builds, where incremental compilation is not used (1.2 s even when only one file changed). Zig's fast self-hosted backend and incremental compilation are x86_64-Linux features: on Apple Silicon, Zig still uses LLVM in Debug.

### Cited Findings

**[LOCAL] Synthetic project builds (2026-10-19, M2).** I generated equivalent code in each language: 50 modules × 20 functions with structs and loops, no external dependencies. That came to 9,204 LOC in Rust, 12,561 in Go, 9,305 in C and 9,152 in TS. "1-file edit" means a one-line change to one module.

| Toolchain | Clean build | 1-file edit rebuild | Notes |
|---|---|---|---|
| Rust `cargo build` (debug) | 1.31 s | **0.39 s** | incremental on |
| Rust `cargo check` | 1.04 s | **0.25 s** | no codegen |
| Rust `cargo build --release` | 1.24 s | 1.21 s | release has no incremental compilation, so an edit costs about a full rebuild |
| Rust + tokio 1.53 (rt-multi-thread, macros), release, deps from clean | **8.1 s** wall (13.1 s user) | n/a | small `main.rs`; shows that dependencies dominate clean builds |
| Go 1.27 `go build` (stdlib already cached) | 0.55 s | 0.53–0.85 s | one package, so an edit recompiles the whole package; a no-op build takes 0.09 s |
| Go 1.27 `go build` (empty GOCACHE, includes stdlib) | 4.8–5.7 s | n/a | first build on a fresh cache or CI runner |
| Go `go vet` after an edit | n/a | 0.23 s | type check plus vet |
| C clang -O0, 51 files, `-j8` | 1.13 s | **0.19 s** (1 file + relink) | |
| C clang -O2, `-j8` | 1.68 s | n/a | |
| Bun 1.3.9 `bun main.ts` (51 modules, no build) | **35 ms** to run (131 ms first run) | 35 ms | transpiles at load; this does no type checking |
| Node 25.5 `node main.ts` (type stripping) | 442 ms to run | 442 ms | |
| `bun build --compile main.ts` | 0.25 s | n/a | produces a standalone executable |

**[LOCAL] Hello-world compile times (median of 5).**

| Command | Time |
|---|---|
| clang -O0/-O2 | 76–78 ms |
| rustc debug | 133 ms |
| rustc -O | 170 ms |
| go build (warm cache) | 97 ms |
| go build (cold cache) | 5.3 s |
| swiftc -Onone | 440 ms |
| swiftc -O | 385 ms |
| bun build --compile | 180 ms |

Swift is 3–5x slower than C or Rust even on a trivial file.

**Rust toolchain (official sources).**
- Rust 1.90 (Sept 2025) made LLD the default linker on `x86_64-unknown-linux-gnu`. On ripgrep, an incremental rebuild linked 7x faster, which cut end-to-end time by 40%. A from-scratch debug build was 20% faster. Opt-out: `-C linker-features=-lld`. — [Rust blog, 2025-09-01](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable); [Rust 1.90.0 release](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/)
- **Cranelift backend (cg_clif), measured February 2026.** On Zed incremental builds (16-core machine), Cranelift was 5% faster than LLVM with `-Cdebug-info=none` and 12% faster with `line-tables-only`. On the rustc benchmark suite (single crate, `line-tables-only`) the gain ranged from negligible to about 30%, with an average of 10–15%. The goal page says Cranelift "has not yet (fully) delivered on its promise". The 2026 goal targets a 2x backend speedup, with $65,000 of funding from OpenAI and AWS. — [Rust Project Goal: improve cg_clif performance](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)
- **2026 "Fast Builds" roadmap (plans, not results).**
  - TPDE backend: claimed 10–20x faster than LLVM -O0 with similar code quality; experimental option targeted for 2026.
  - Relink-Don't-Rebuild: targets 5–10x faster rebuilds for common changes.
  - Crate slicing: experiments suggest 1.4x–30x.
  - Parallel front end: stabilization continuing in 2026.
  - Incremental linking with Wild: "2026 and beyond".
  - [Rust 2026 roadmap: Fast Builds](https://goals.rust-lang.org/2026/roadmap-fast-builds.html)
- The 2025 compiler performance survey reported that linking is a common complaint because it always runs from scratch. — [Rust compiler performance survey 2025 results](https://blog.rust-lang.org/2025/09/10/rust-compiler-performance-survey-2025-results)

**Linkers, measured April 2026.** Linking clang-23 with `--threads=8`, hyperfine, 10 runs; mold and wild run with `--no-fork`.

| Workload | lld (Feb 2026 main) | lld (parallel input loading) | mold | wild |
|---|---|---|---|---|
| Release+Asserts, `--gc-sections` | 1.255 s | 917.8 ms | 552.6 ms | **367.2 ms** |
| Debug | 4.582 s | 4.306 s | 2.464 s | **1.565 s** |

— [MaskRay, "Recent lld/ELF performance improvements", 2026-04-12](https://maskray.me/blog/2026-04-12-recent-lld-elf-performance-improvements)

- Wild's README (older numbers) for an ~80 MB static binary: lld 905 ms, mold 457 ms, wild 363 ms. The author warns that many features are unsupported. Older numbers with debug info had wild slower than mold (clang with debuginfo: wild 31.5 s, mold 16.9 s, lld 30.5 s). — [wild README](https://github.com/davidlattimore/wild)
- Some 2026 roundup blogs claim mold beats wild by 1.9x at the median. Their dates are inconsistent and the numbers come from the vendor's suite, so I treat them as unreliable. — [pistack.xyz roundup](https://www.pistack.xyz/posts/2026-10-04-mold-vs-lld-vs-gold-linker-benchmarks-2026/)

**Zig.**
- Zig 0.15.1 (Aug 2025) made the self-hosted x86_64 backend the Debug default, except on NetBSD, OpenBSD and Windows.
  - The notes claim "Debug compilation is 5 times faster", described as "around a 5x decrease compared to LLVM in most cases".
  - Building the compiler itself takes "just a few seconds rather than 1-2 minutes".
  - Threaded codegen gives anywhere from insignificant gains to over 50%; the compiler build went from 13.8 s to 10.0 s.
  - Incremental compilation (`-fincremental --watch`) is still experimental. The notes say it "can lead to miscompilations" and is reliable only with `-fno-emit-bin`, which reports errors without producing a binary.
  - **The aarch64 backend passes 1656/1972 (84%) of behavior tests and is "not ... currently usable in any real use case".**
  - [Zig 0.15.1 release notes](https://ziglang.org/download/0.15.1/release-notes.html)
- Ghostty, measured with Zig 0.15 (mostly still on LLVM): the build script went from 7.17 s to 1.70 s, and a full uncached build from 41 s to 32 s. The project expects about 25 s once the self-hosted backend works for it. — [Mitchell Hashimoto, "Zig builds are getting faster"](https://mitchellh.com/writing/zig-builds-getting-faster) (via search summary; the direct fetch failed)
- A Ziggit user's Debug unit tests: 19.8 s on 0.14.0, 11.8 s on 0.15.1 with forced LLVM, and 6.7 s on 0.15.1 with the x86 backend. — [Ziggit: x86 backend default in Debug](https://ziggit.dev/t/self-hosted-x86-backend-is-now-default-in-debug-mode/10447) (via search summary)
- Zig 0.16.0 shipped in mid-April 2026. Its headline is the I/O-as-an-interface change; I found no primary statement on backend or incremental changes. — [Zig 0.16.0 release notes](https://ziglang.org/download/0.16.0/release-notes.html); [LWN via linux.org](https://www.linux.org/threads/lwn-net-zig-0-16-0-released.65185/)
- Zig 0.17.0 reportedly brings incremental compilation "for everyone on x86_64-linux" through a new ELF linker (not yet the default), and the cache format is about 25% smaller with 5–10% faster cache hits. This comes from **secondary coverage only**; I did not read the primary notes, and the linuxiac page was geo-blocked. — [zeli.app](https://zeli.app/story/49938521); [besthub.dev](https://www.besthub.dev/articles/zig-0-17-0-released-sub-second-builds-new-build-system-architecture-747c321ce337)
- Bun maintains a Zig fork with about 4x faster compilation, which upstream declined in favor of self-hosted backends and incremental compilation. — [Ziggit thread](https://ziggit.dev/t/bun-s-zig-fork-got-4x-faster-compilation-times/15183/19)

**Go.**
- Go 1.27 (August 2026) lists **no build-speed changes**. It makes encoding/json v2 the default and adds size-specialized malloc (up to 30% cheaper allocations under 80 bytes, about 1% overall, about +60 KB binary). — [Go 1.27 release notes](https://go.dev/doc/go1.27)
- Go 1.26 turned on the Green Tea GC by default: an estimated 10–40% reduction in GC overhead, plus about 10% more on Ice Lake / Zen 4 and newer CPUs. — [Go 1.26 release notes](https://go.dev/doc/go1.26)

**C with tcc.** TinyCC 0.9.22 compiled the Links browser (1.95M lines after headers) in 2.27 s, or 859k lines/s. GCC 3.2 -O0 took 20.0 s (98k lines/s), so tcc was about 9x faster. These are very old measurements on a 2.4 GHz Pentium 4, and Bellard no longer works on TCC. — [TinyCC homepage](https://bellard.org/tcc/)

**OCaml.** I found only anecdotal dune timings, mostly for js_of_ocaml, such as about 3 s incremental for a WebGL example and about 600 ms for a tiny project. These do not represent native `ocamlopt` builds. — [discuss.ocaml.org: jsoo seconds to compile](https://discuss.ocaml.org/t/jsoo-webgl-example-seconds-to-compile/11367); [discuss.ocaml.org: dune vs make](https://discuss.ocaml.org/t/dune-build-vs-makefile/11394)

### Inferences
- For a 3k–10k LOC harness, every compiled language except Swift (and release-mode Rust) stays under 1 s per edit. The loop is then dominated by **tests, dependency churn and linking**, not by the compiler front end. A Rust harness should iterate in `cargo check` plus debug builds and keep release builds off the self-edit hot path, because release builds have no incremental compilation.
- Rust's real hazard for "rebuild many times a day" is the clean build with a dependency tree such as tokio, reqwest and rustls. Tokio alone took 8.1 s locally, so a typical async HTTP and TLS stack could plausibly take 30–90 s clean; this is an extrapolation, not a measurement. Mitigation: persistent `target/`, sccache, and never wiping it during the swap step. The quality guard is that the cached build must match a clean build bit for bit, checked periodically.
- Go's clean build is 0.55 s once the stdlib cache is warm, but **a fresh cache costs about 5 s**. Go also recompiles a whole package on any edit, so splitting the harness into packages keeps rebuilds sub-second.
- Zig's "5x faster Debug" and incremental compilation **do not apply on the owner's Apple Silicon Mac** as of 0.15/0.16, because aarch64 still goes through LLVM, and 0.17's incremental support is reportedly x86_64-Linux only. The fast self-rebuild story is only available on x86_64-Linux machines such as Modal or CI.
- Bun + TS has the shortest raw loop: about 35 ms from edit to running code. But it skips type checking, and a `tsc --noEmit` pass (not measured here) brings back seconds of cost if used as a gate.

### Gaps
- No local or published numbers for Odin, OCaml (native), Elixir (`mix compile`), C++ or Zig on a matched 3k–10k LOC project. Zig, Odin, OCaml, Elixir and tcc are not installed locally.
- No measured Cranelift or Wild numbers on macOS aarch64. Cranelift's macOS/aarch64 support status was not confirmed by the goal page. mold does not target Mach-O; Apple's ld is the default on macOS, and I found no 2026 benchmark for it.
- No `tsc --noEmit` timing; `tsc` is not installed globally.
- The Zig 0.17 details come from secondary sources only.

## Q2. Startup time, memory footprint, binary size; measured comparison of real agent harnesses

### Takeaway
Hello-world native binaries start in about 2 ms with about 1 MB RSS: C, Rust and Swift. Go takes about 3 ms and 3.7 MB, Bun about 15–18 ms and about 20 MB, Node about 100 ms and 45 MB, Python about 35 ms and 12 MB. Among real harnesses, Codex (Rust) and the Bun-compiled harnesses (Claude Code, OpenCode) start in tens of ms. The Node-based Pi and Gemini CLI take 0.5 s and 2.2 s with 130–285 MB RSS. OpenAI's stated reasons for moving Codex to Rust were a zero-dependency install (no Node 22+), native sandbox bindings, no GC (lower memory) and a wire protocol; it published no numbers. Claude Code has many issue reports of multi-GB to 100+ GB memory growth.

### Cited Findings

**[LOCAL] Hello-world startup and memory (2026-10-19, M2, median of 20).**

| Program | Startup | Peak RSS | Binary size |
|---|---|---|---|
| C (clang -O2) | 1.7 ms | 1.0 MB | 33 KB |
| Rust (rustc -O) | 2.0 ms | 1.2 MB | 466 KB (342 KB stripped) |
| Swift (-O) | 2.3 ms | 1.3 MB | 50 KB (links the OS `libswiftCore.dylib`) |
| Go | 2.9 ms | 3.7 MB | 2.4 MB (static Go runtime) |
| Bun `--compile` executable | 17.9 ms | 18.8 MB | **60.3 MB** |
| `bun hello.ts` | 15.3 ms | 21.0 MB | n/a |
| `node hello.js` | 97.9 ms | 44.8 MB | n/a |
| `node hello.ts` (type stripping) | 167 ms | 77.3 MB | n/a |
| `python3 hello.py` | 34.6 ms (22.1 ms with `-S`) | 12.0 MB | n/a |
| Rust + tokio multi-thread runtime | n/a | 1.6 MB idle (1 task) | 786 KB |

**[LOCAL] Installed agent CLIs (2026-10-19, M2, median of 20).** `--version` and `--help` measure process boot only, not a live TUI session or a model call.

| Harness (version) | Runtime | On-disk size | `--version` | `--help` |
|---|---|---|---|---|
| Claude Code 2.1.261 | Bun-compiled binary; embeds "Bun v1.4.1", and 554 `.rs` source paths are present, consistent with the Rust port of Bun | 199 MB single binary | 15 ms / 26.5 MB | 145 ms / 125 MB |
| OpenCode 2.0.15 | Bun-compiled Mach-O; embeds "Bun v1.4.2" | 177 MB | 72 ms / 83 MB | 91 ms / 84 MB |
| Pi (pi-coding-agent) 0.85.0 | Node bundle (`#!/usr/bin/env node`) | 436 MB package dir | 513 ms / 128 MB | 664 ms / 135 MB |
| Gemini CLI 0.27.0 (Homebrew) | Node 25 | 424 MB Cellar dir | **2.20 s / 284 MB** | 2.29 s / 286 MB |
| Codex, Goose | not installed, so not measured | | | |

Claude Code's 15 ms `--version` is likely a fast path that skips loading the app; `--help` is the fairer boot number.

**Published hyperfine comparison** (OpenDev README; macOS ARM64; `hyperfine --shell=none --warmup 10 --runs 100`; peak memory from `/usr/bin/time -l`, median of 20; date not given, © 2026):

| Agent (version) | Startup | Peak memory | Install size |
|---|---|---|---|
| OpenDev 0.1.4 (Rust) | 4.3 ms | 9.4 MB | 18 MB |
| Codex 0.116.0 | 37.8 ms | 43.7 MB | 116 MB |
| Claude Code 2.1.87 | 87.3 ms | 214.6 MB | 188 MB |
| OpenCode 1.2.27 | 557.4 ms | 285.9 MB | 90 MB |

OpenDev is a competing product, so this is a vendor-run comparison against now-old versions. — [opendev-to/opendev README](https://github.com/opendev-to/opendev)

- An npm-installed `codex` is a Node script that spawns the native binary, so its startup includes Node's. Claude Code headless mode waits up to 30 s by default for a slow MCP server, so MCP config can dominate startup. — [DEV: Time 4 coding agent CLIs with hyperfine](https://dev.to/gabrielanhaia/how-long-before-your-agent-says-anything-time-4-coding-agent-clis-with-hyperfine-4o2e)
- Anecdotal Gemini CLI startup: about 16 s, or 8 s on a fresh install, per one HN commenter; DataCamp says 5–10 s. — [HN comment](https://news.ycombinator.com/item?id=46902510); [DataCamp](https://www.datacamp.com/blog/gemini-cli-vs-claude-code)

**OpenAI's stated reasons for rewriting Codex CLI from TypeScript to Rust** (fouad-openai, "Codex CLI is Going Native", 2025-05-30). The post gives no numbers.
- **Zero-dependency install:** "currently Node v22+ is required, which is frustrating or a blocker for some users".
- **Native security bindings:** "we already ship a Rust for linux sandboxing since the bindings were available".
- **Performance:** "no runtime garbage collection, resulting in lower memory consumption".
- **Extensibility:** a wire protocol for extensions in other languages, plus MCP.
- The TypeScript/Ink version had been "easy to whip up and iterate".
- Sources: [openai/codex Discussion #1174](https://github.com/openai/codex/discussions/1174); [InfoQ](https://www.infoq.com/news/2025/06/codex-cli-rust-native-rewrite); [DevClass: "Node.js frustrating and inefficient"](https://devclass.com/2025/06/02/node-js-frustrating-and-inefficient-openai-rewrites-ai-coding-tool-in-rust/)

**Claude Code and Bun.**
- Anthropic acquired Bun on 2025-12-03 and says "Claude Code ships to millions of users as a Bun executable". — [Anthropic announcement](https://anthropic.com/news/anthropic-acquires-bun-as-claude-code-reaches-usd1b-milestone)
- Jarred Sumner's claim: Claude Code v2.1.181 (released 2026-06-17) and later use **Bun's Rust port** (Bun was originally written in Zig), giving 10% faster startup on Linux. Simon Willison confirmed it by binary inspection: the binary embeds "Bun v1.4.0" and lists 563 `.rs` source paths. The port was largely automated with an agent harness, ending in a PR of more than a million lines checked against a language-independent test suite with about a million assertions. — [Simon Willison, 2026-07-19](https://simonwillison.net/2026/jul/19/claude-code-in-bun-in-rust)
- A third-party analysis says roughly 95 MB of a roughly 100 MB Claude Code binary is the Bun runtime with JavaScriptCore. — [pbxscience](https://pbxscience.com/claude-code-quietly-ships-a-rust-rewritten-bun-runtime-delivering-a-10-faster-startup-on-linux/). The local binary is now 199 MB.

**Reported memory and CPU problems in TS harnesses (user reports, not controlled measurements).**
- Claude Code #24840 (Feb 2026, Windows, Bun canary 1.3.9-canary.51): 13.21 GB RSS and 47.17 GB commit on a 42.56 GB machine. — [GitHub #24840](https://github.com/anthropics/claude-code/issues/24840)
- Claude Code issue #4953: grows to 120+ GB and is OOM-killed. — [claudeissues mirror #4953](https://claudeissues.com/issue/4953-claude-code-memory-leak-process-grows-to-120-gb-ram-and-gets-oom-killed)
- Claude Code issue #17615: 304 GB process on macOS. — [mirror #17615](https://claudeissues.com/issue/17615-memory-leak-claude-process-using-304gb-memory)
- Claude Code issue #33356: 50.4 GB. — [mirror #33356](https://claudeissues.com/issue/33356-bug-high-memory-usage-50-4gb)
- Claude Code issue #22183: 13 GB peak on an 8 GB Mac in v2.1.27; one reporter said 2.1.74 fixed it. — [mirror #22183](https://claudeissues.com/issue/22183-bug-extreme-memory-usage-peak-13gb-in-v2-1-27-with-plan-mode-and-screenshots)
- Claude Code issue #20777 (Linux): 20 GB+ growth in simple conversations; the workaround is capping the heap via `NODE_OPTIONS`. — [GitHub #20777](https://github.com/anthropics/claude-code/issues/20777)
- A guide notes that process RAM and context-window size are independent: a process can leak 20 GB with an almost empty context. — [startdebugging.net, 2026-07](https://startdebugging.net/2026/07/fix-claude-code-high-memory-usage-and-context-overflow/)
- OpenCode: one user reported about 700 MB at idle and over 1 GB for basic tasks. This is a secondary Chinese-language article, July 2026. — [80aj.com](https://www.80aj.com/2026/07/26/opencode-memory-usage-comparison/)
- Gemini CLI: one write-up of a V8 "Zone Allocation failed - process out of memory" crash on Node 24. — [claudeissues mirror #45442](https://claudeissues.com/issue/45442-feedback-claude-code-diagnosed-and-fixed-what-gemini-cli-couldnt-in-a-full-day)
- Codex (Rust core) problems are mostly reported in the desktop app, not the Rust CLI. Examples:
  - The Electron renderer slows down at about 4 GB.
  - Excessive git subprocesses consume CPU, for example issue #22085 on Windows.
  - CPU sits at 90–100% during tool calls on Windows.
  - A user notes the Codex TUI "does not have this issue".
  - Sources: [OpenAI community thread](https://community.openai.com/t/codex-becomes-extremely-slow-and-causes-severe-ui-lag-when-working-with-many-files/1376629); [openai/codex discussion #36406](https://github.com/openai/codex/discussions/36406)

### Inferences
- **The startup tiers are clear and stable across both data sets.**
  - Native (C, Rust, Swift, Zig, Odin): about 2 ms and about 1 MB.
  - Go: about 3 ms and 4 MB.
  - Bun: 15–20 ms and about 20 MB at the floor.
  - Node: about 100 ms and 45 MB at the floor.
  - Python: 20–35 ms and 10–12 MB.
  - Real harnesses add their own init cost on top: 0.1–2 s and 80–290 MB for the TS ones.
- For a harness that hot-swaps itself many times a day, startup matters at swap time and for every sub-agent process spawn. At 1,000 sub-agent spawns a day, 2 ms versus 150 ms is 2 s versus 150 s per day, so it is negligible either way. The RSS floor matters more if each sub-agent is a separate process: 1,000 processes × 125 MB is 125 GB, versus 1,000 × 5–10 MB at 5–10 GB.
- The multi-GB growth reports for Claude Code look like retained JS heap or native buffers in a long-lived GC'd process. A non-GC native harness does not make leaks impossible, but it removes the GC-heap headroom behavior. OpenAI cited exactly this, though without numbers.
- Bun-compiled harnesses pay a 60–200 MB binary size per version. A harness that keeps N old versions for rollback pays N × about 100–200 MB on disk. A Rust or Go binary would be about 1–20 MB. With 9 GB free on the owner's Mac, this matters.

### Gaps
- No independent, version-matched benchmark of Codex, Claude Code, Gemini CLI, OpenCode, Goose and Pi under a live session with identical tasks. All published data is vendor-run or anecdotal.
- No RSS-over-time data (an hour-long session) for any harness.
- Goose and Codex were not installed, so they were not measured locally.
- No primary Anthropic statement on why Claude Code is written in TypeScript. I could not verify the commonly repeated "on distribution" rationale from Anthropic sources.

## Q3. Concurrency for many parallel tool calls and sub-agents

### Takeaway
Runtime overhead per idle concurrent task ranges from about 0.3 KB (Rust tokio) to about 0.6–0.9 KB (Bun/Node promise plus timer), about 1.4 KB (Python asyncio), about 2.1 KB (Swift Task), about 2.6–2.8 KB (Go goroutine, Erlang process) and about 14 KB (OS thread). At agent-harness scale, with hundreds to thousands of agents, the largest of these is only a few MB, which is noise next to per-agent conversation state and the subprocesses each tool call spawns. Production orchestrators of tens to hundreds of agents use Elixir/BEAM (OpenAI Symphony) and Go (Gas Town). Cursor's hundreds-of-agents harness does not name its language.

### Cited Findings

**[LOCAL] Peak RSS for N concurrent idle tasks that each sleep 2 s (2026-10-19, M2).** Per-task cost is (RSS(N) − RSS(1)) / N.

| Runtime | N=1 | N=10k | N=100k | Per task |
|---|---|---|---|---|
| Rust tokio 1.53 (multi-thread) | 1.6 MB | 5.7 MB | 32.8 MB | **~312 B** |
| Bun 1.3.9 Promise + setTimeout | 21.6 MB | 30.9 MB | 79.5 MB | ~579 B |
| Node 25.5 Promise + setTimeout | 45.0 MB | 61.2 MB | 130.9 MB | ~859 B |
| Python 3.14 asyncio | 21.8 MB | 35.1 MB | 161.4 MB | ~1,396 B |
| Swift 6.0 TaskGroup | 3.1 MB | 24.9 MB | 215.7 MB | ~2,126 B |
| Go 1.27 goroutine | 3.6 MB | 32.7 MB | 281.5 MB | ~2,779 B |
| Rust OS threads (std::thread) | 1.7 MB | 19.2 MB (N=1k) | 72.1 MB (N=5k) | ~14,080 B resident |

**Other figures.**
- **Erlang/BEAM:** a newly spawned process is 327 words, or 2,616 bytes on 64-bit (OTP 27), of which 233 words are heap plus stack. The default sizes are designed for "hundreds of thousands or even millions of processes". — [Erlang Efficiency Guide: Processes](https://www.erlang.org/doc/system/eff_guide_processes.html)
- **1M-task benchmark (end of 2024, i7-13700K; Rust 1.82, Go 1.23.3, .NET 9, Node 23.2, Python 3.13):** at 1M tasks, C# NativeAOT was lowest and Rust close, while Go used more than 13x the winner and more than 2x Java. In "Take 2", Java and Python could not finish 1M within 10 s. The text gives no MB figures; the numbers are only in the charts. — [hez2010 async runtimes benchmark 2024](https://hez2010.github.io/async-runtimes-benchmarks-2024/); [Take 2](https://hez2010.github.io/async-runtimes-benchmarks-2024/take2.html)
- **Swift:** 1M tasks used 1,448 MB (about 1.4 KB per task). — [Swift Forums](https://forums.swift.org/t/taskgroup-and-tasks-memory-consumption/84772)

**Harnesses running many agents.**
- **Cursor, "Scaling long-running autonomous coding" (Jan 2026):**
  - "hundreds of concurrent agents" on one project.
  - The browser (FastRender) reached over 1M LOC across 1,000 files in about a week, using "trillions of tokens".
  - With lock-based coordination, "Twenty agents would slow down to the effective throughput of two or three". Cursor moved to planner/worker/judge roles with optimistic concurrency.
  - The post does not name the harness language or runtime.
  - Sources: [Cursor blog](https://cursor.com/blog/scaling-agents); [Simon Willison](https://simonwillison.net/2026/Jan/19/scaling-long-running-autonomous-coding/)
- **OpenAI Symphony:** an open-source orchestrator spec whose **reference implementation is in Elixir/BEAM**, chosen for supervisor trees, lightweight processes and crash recovery.
  - It spawns an isolated workspace per Linear ticket, restarts crashed runs, and drives the Codex App Server, a headless JSON-RPC mode.
  - Apache 2.0; reportedly open-sourced around 2026-03-05, with over 15k stars by late April 2026.
  - It was re-implemented by Codex in other languages, though sources differ on which: TS/Python/Rust in one, or TS/Go/Rust/Java/Python in another.
  - Sources: [InfoQ, 2026-05](https://infoq.com/news/2026/05/openai-symphony-agents); [IT Brief](https://itbrief.co.uk/story/openai-launches-symphony-for-coding-agent-workflows); [Daniel Vaughan](https://codex.danielvaughan.com/2026/04/28/openai-symphony-codex-orchestration-linear-autonomous-agent-workflows/)
- **Gas Town (Steve Yegge, Jan 2026):** a Go orchestrator that runs **20–30 Claude Code agents** in tmux. The README notes that 4–10 agents get chaotic without coordination. Status: alpha. — [pkg.go.dev gastown](https://pkg.go.dev/github.com/steveyegge/gastown); [heise](https://heise.de/-11178824)
- **Jido (Elixir agent framework):** claims "thousands of agents per node", one OTP process per agent. This is a vendor claim with no per-agent memory figure. — [jido.run](https://jido.run/)

### Inferences
- For 1,000 concurrent agents, runtime task overhead is about 0.3 MB with tokio, about 2.8 MB with Go and about 2.6 MB with BEAM. That is irrelevant next to per-agent state: transcripts of 100 KB–10 MB of JSON each, TLS connections, and above all **child processes**. Every shell tool call forks a process of several MB, and the measured TS harness boots cost 80–290 MB each if sub-agents are separate harness processes.
- The language choice for concurrency therefore matters less for the "memory per task" figure. It matters more for:
  - **Supervision and crash isolation:** BEAM has this natively; Rust and Go need hand-built supervisors.
  - **Cheap in-process sub-agents** versus one OS process per sub-agent.
  - Robust **process-group kill and sandbox setup per tool call**.
- Cursor's 20-to-2–3 collapse shows that coordination design, not runtime, is the throughput bottleneck at hundreds of agents.
- A single Node or Bun event loop handles thousands of concurrent awaits cheaply, at under 1 KB each. Its weakness is CPU-bound work on the main thread (JSON parse, diffing, TUI render) blocking all agents. Native multi-threaded runtimes such as tokio, Go and BEAM schedulers do not have this problem.

### Gaps
- No measured memory or throughput for a *real* agent session under each runtime (HTTP/2 + TLS + SSE + transcript), only idle-task microbenchmarks.
- Not measured locally: BEAM, OCaml 5 (effects/eio), Zig (`std.Io` async in 0.16) and Odin, because the toolchains are not installed.
- The language of Cursor's harness is unknown. I did not research Anthropic's own parallel-agent experiments.

## Q4. Ecosystem maturity: HTTPS/TLS, SSE, JSON, process groups, PTY, git, OS sandboxing

### Takeaway
Rust has production-grade, harness-proven libraries for every item: Codex itself uses landlock, seccomp, bubblewrap and Seatbelt. Go is close behind, with one important caveat: Landlock in Go must restrict the whole process because goroutines float between threads. TS on Bun/Node gets sandboxing only by shelling out to bubblewrap or sandbox-exec, which is what Claude Code does. Bun has had a native PTY since v1.3.5. Zig's std TLS/HTTP is young and recently had interop bugs. Odin has no core TLS; its maintainer estimates it at "a man year's worth of work". For Rust, gitoxide is still incomplete: push was unchecked as of April 2026, so libgit2 or shelling out to `git` remains the safe choice.

### Cited Findings
- **Codex (Rust) sandbox stack.**
  - macOS: Seatbelt via `sandbox-exec`.
  - Linux, legacy path: Landlock plus mount namespaces.
  - Linux, newer path: vendored bubblewrap behind `features.use_linux_sandbox_bwrap`. It adds PR_SET_NO_NEW_PRIVS and a seccomp filter blocking connect/accept/bind/listen/sendto/sendmsg, with an AF_UNIX exemption; the filesystem is `--ro-bind / /` with writable roots, and `.git` and `.codex` are re-protected read-only.
  - Windows: DACL-based.
  - Sources: [Daniel Vaughan: Codex sandbox internals, 2026-05-03](https://codex.danielvaughan.com/2026/05/03/codex-cli-sandbox-internals-seatbelt-bubblewrap-landlock-windows-dacl/); [OpenAI Codex sandboxing docs](https://developers.openai.com/codex/concepts/sandboxing); [Mintlify mirror of codex architecture](https://www.mintlify.com/openai/codex/architecture/sandboxing)
- **Claude Code (TS/Bun) sandbox.** The `@anthropic-ai/sandbox-runtime` package uses **bubblewrap + socat** on Linux and **Seatbelt** on macOS. A seccomp filter is optional and blocks Unix sockets. It is deny-by-default for writes and network, and WSL1 is unsupported. — [Claude Code sandboxing docs](https://docs.claude.com/en/docs/claude-code/sandboxing); [Anthropic engineering post](https://anthropic.com/engineering/claude-code-sandboxing)
- **Landlock and seccomp libraries.**
  - **Rust `landlock`** is the official crate from landlock-lsm, maintained by Landlock's author. It is at 0.4.x and covers Landlock features up to Linux 5.19.
  - **Rust `seccompiler`** comes from rust-vmm (extracted from Firecracker) and supports x86_64, aarch64 and riscv64. The **`libseccomp`** crate binds the C library.
  - **Go: `landlock-lsm/go-landlock`** is official. It "always enables the given Landlock restriction for the entire process" because goroutines are not pinned to OS threads.
  - Sources: [docs.rs landlock](https://docs.rs/landlock); [docs.rs seccompiler](https://docs.rs/seccompiler/); [docs.rs libseccomp](https://docs.rs/libseccomp); [go-landlock README](https://github.com/landlock-lsm/go-landlock/blob/main/README.md)
- **PTY in Bun.** `Bun.spawn({terminal})` and `Bun.Terminal` arrived in Bun v1.3.5 (2025-12-17). They use openpty on Linux and macOS, and ConPTY on Windows according to the child-process docs; the API reference says POSIX only, so the sources conflict. — [Bun v1.3.5 release notes](https://bun.com/blog/release-notes/bun-v1.3.5); [Bun child-process docs](https://bun.com/docs/runtime/child-process)
- **Git in Rust: gitoxide (`gix`).** The README checklist (April 2026 snapshot) marks clone and fetch done but **push unchecked**. Most crates are not stabilized: only gix-lock is "Tier 1, production grade". Cargo's gitoxide path is "only for testing and developing". — [ecosyste.ms snapshot of gitoxide README](https://awesome.ecosyste.ms/projects/github.com%2FGitoxideLabs%2Fgitoxide); [cargo docs: cargo_use_gitoxide_instead_of_git2](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/features/fn.cargo_use_gitoxide_instead_of_git2.html)
- **Zig std TLS/HTTP.**
  - `std.crypto.tls` gained TLSv1.2 in November 2024.
  - An August 2025 fix addressed a client that always sent the secp256r1 key, which broke servers that prefer X25519.
  - 0.15.1 decoupled the TLS client from `std.net` and `std.fs`, and the HTTP server dropped its header-count limit.
  - The third-party `tls.zig` (TLS 1.2/1.3) connects to about 6k tested domains but needs a patched std copy to work with `std.http`.
  - Sources: [Zig mirror commit log for tls.zig](https://git.jakstys.lt/motiejus/zig/commits/branch/master/lib/std/crypto/tls.zig); [ianic/tls.zig](https://github.com/ianic/tls.zig); [Ziggit: Is std.http insecure?](https://ziggit.dev/t/is-std-http-insecure/11314); [Zig 0.15.1 release notes](https://ziglang.org/download/0.15.1/release-notes.html)
- **Odin.**
  - There is no core TLS. A maintainer said native TLS "is a man year's worth of work to do well"; the plan is a pluggable TLS interface.
  - The third-party `odin-http` is "not intended for production use" and depends on OpenSSL for TLS.
  - Sources: [Odin forum](https://forum.odin-lang.org/t/will-odin-have-its-own-tls-library-like-golang/288); [laytan/odin-http](https://github.com/laytan/odin-http)
- **Erlang/Elixir (BEAM).** OpenAI's Symphony reference implementation shows BEAM is viable for orchestrating Codex processes: it drives the Codex App Server via JSON-RPC rather than reimplementing the agent loop. — [InfoQ](https://infoq.com/news/2026/05/openai-symphony-agents)

**Ecosystem matrix.** "Verified" means checked in a source above this session. Other cells are canonical, well-known libraries that I list from background knowledge and **did not re-verify this session**; the URLs are their canonical homes.

| Need | Rust | Go | TS (Bun/Node) | Zig | Odin | Swift | OCaml | BEAM |
|---|---|---|---|---|---|---|---|---|
| HTTPS/TLS | rustls / reqwest (unverified) | crypto/tls in std (unverified) | built-in fetch (unverified) | std, young (**verified**: recent fixes) | none in core (**verified**) | URLSession on macOS (unverified) | ocaml-tls (unverified) | `:ssl` in OTP (unverified) |
| Linux sandbox | landlock, seccompiler, bwrap (**verified**: used by Codex) | go-landlock, process-wide (**verified**) | shell out to bwrap/sandbox-exec (**verified**: Claude Code) | raw syscalls only (no source) | raw syscalls (no source) | n/a | n/a | shell out |
| PTY | portable-pty (unverified) | creack/pty (unverified) | Bun.Terminal (**verified**), node-pty (unverified) | (no source) | (no source) | (no source) | (no source) | (no source) |
| git | git2 (libgit2) / gix: no push (**verified**) | go-git (unverified) | shell out (unverified) | shell out | shell out | shell out | shell out | shell out |

Canonical URLs for the unverified entries: [rustls](https://github.com/rustls/rustls), [reqwest](https://github.com/seanmonstar/reqwest), [portable-pty](https://docs.rs/portable-pty), [git2-rs](https://github.com/rust-lang/git2-rs), [creack/pty](https://github.com/creack/pty), [go-git](https://github.com/go-git/go-git), [node-pty](https://github.com/microsoft/node-pty).

### Inferences
- Only Rust has a **reference production harness** (Codex) that already solved sandboxing natively on macOS, Linux and Windows, with a public codebase to copy patterns from. Go can do the same, but its Landlock is process-wide, so a Go harness should apply Landlock in a forked helper (as Codex does with its `codex-linux-sandbox` helper) rather than in the main process.
- TS, Python and BEAM harnesses depend on external binaries (bwrap, socat, sandbox-exec, git), which works but adds install dependencies. That is exactly the cost OpenAI cited for leaving Node.
- Zig and Odin would require writing or vendoring TLS, PTY, git and sandbox glue: weeks of work and security risk for an "extreme performance" gain that matters little in an I/O-bound harness. C and C++ have every C library (libcurl, OpenSSL, libgit2, libseccomp) but no memory safety for code that parses untrusted model output.

### Gaps
- Not verified this session: Swift on Linux maturity (Foundation, swift-subprocess), OCaml (eio, ocaml-tls, cohttp), Elixir PTY and process-group kill (erlexec, MuonTrap), and SSE client libraries per language.
- No benchmark comparing gix with libgit2.

## Q5. Raw performance where relevant: JSON, regex/grep, file IO (Rust, C, C++, Zig, Go)

### Takeaway
The best SIMD JSON parsers (C++ simdjson, C yyjson, Rust sonic-rs) reach about 1–3.5 GB/s, versus about 0.15–0.5 GB/s for scalar parsers such as serde_json and RapidJSON. Go 1.27 made json v2 the default, which is "significantly faster" for unmarshal, but map-based decoding is slower. ripgrep (Rust) leads code search by 2–36x over git grep, ugrep, ag and ack on the Linux kernel. None of these limits an agent loop that waits seconds per model turn; they matter only for repo-wide search and indexing tools.

### Cited Findings
- **simdjson (C++)**, from the project's own numbers: twitter.json parses at 2.2 GB/s on Skylake with GCC 10, versus 0.51 GB/s for RapidJSON. It minifies at 6 GB/s, validates UTF-8 at 13 GB/s and handles NDJSON at 3.5 GB/s. Number-heavy files are slower. These are 2019–2020-era numbers. — [simdjson README (Debian copy)](https://sources.debian.org/src/simdjson/4.3.1-4/README.md)
- **yyjson (C)**, run by its author, twitter.json, DOM API, parse / stringify GB/s:

  | Library | AWS EPYC 7R32, gcc 9.3 | Apple A14, clang 12 |
  |---|---|---|
  | yyjson | 1.72 / 1.42 | 2.39 / 2.01 |
  | yyjson insitu | 1.80 / 1.51 | 3.51 / 2.41 |
  | simdjson | 1.52 / 0.61 | 2.19 / 0.80 |
  | rapidjson utf8 | 0.26 / 0.39 | n/a |
  | cJSON | 0.32 / 0.17 | n/a |

  These are 2020-era numbers, unchanged in the README as of 0.11.1. — [yyjson README (mirror)](https://github.com/gerhobbelt/yyjson)
- **sonic-rs vs serde_json (Rust), twitter.json:** sonic_rs DOM `from_slice` takes about 556 µs, versus serde_json `from_slice` about 3.8 ms and `from_str` about 2.9 ms, a 5–7x gap. A third-party guide reports about 2.9x, so the ratio depends on the harness and API. serde_json is a scalar parser. — [docs.rs sonic-rs](https://docs.rs/crate/sonic-rs/latest)
- **Go encoding/json v2** is the default in Go 1.27 (Aug 2026): "Marshal performance is broadly at parity ... unmarshal performance is significantly faster". It also rejects invalid UTF-8 and duplicate keys. An independent re-run found struct unmarshal −38.9% (faster) but `map[string]any` +48% (slower). — [Go 1.27 release notes](https://go.dev/doc/go1.27); [Daniel Lemire's blog](https://lemire.me/blog); [besthub.dev summary](https://www.besthub.dev/articles/go-1-27-makes-encoding-json-v2-default-after-5-years-timeline-and-benchmarks-3ea99ddddcad)
- **ripgrep (Rust)**, Linux kernel search for `rg -n -w '[A-Z]+_SUSPEND'`, from the author's README:

  | Tool | i7-6900K | i9-12900K |
  |---|---|---|
  | ripgrep | **0.136 s** | **0.082 s** |
  | hypergrep | n/a | 0.167 s |
  | git grep -P | 0.348 s | 0.273 s |
  | ag | 0.654 s | 0.443 s |
  | ugrep | 0.506 s | 0.639 s |
  | git grep (LC_ALL=C) | 1.150 s | n/a |
  | ack | 4.054 s | 2.935 s |
  | git grep (UTF-8 locale) | 4.205 s | n/a |

  On the i9 run, ugrep is about 7.8x slower than ripgrep and ack about 36x. — [ripgrep README (mirror)](https://github.com/sunfishcode/ripgrep); [aicodereview.cc summary](https://aicodereview.cc/blog/ripgrep-vs-grep-performance/)

### Inferences
- twitter.json is about 631 KB; this size is my background knowledge, not verified this session. On that basis, serde_json at about 3 ms works out to about 0.2 GB/s and sonic-rs at about 0.56 ms to about 1.1 GB/s. A 1 MB agent transcript then parses in roughly 1–5 ms in any compiled language, which is negligible next to a 1–30 s model turn. JSON only matters if the harness re-parses the whole transcript on every SSE delta, which is a design mistake in any language.
- The agent harness's raw-speed hot paths are search (ripgrep is already available as a binary from any language), diffing and patching large files, and TUI rendering of long streams. All of these can be delegated to native binaries or libraries, so the harness language's own raw speed is a second-order factor. Startup, RSS, build loop and sandbox ecosystem (Q1–Q4) dominate.

### Gaps
- No same-machine, same-API JSON benchmark across Rust, C, C++, Zig and Go dated 2026.
- No Zig `std.json` or Odin JSON throughput numbers.
- No file-IO benchmark across languages found. File IO is OS-bound and largely language-independent for sequential reads, but I have no measurement.
- I did not use the Benchmarks Game or TechEmpower data; they are CPU and web-server benchmarks of limited relevance to an I/O-bound harness.
