# Live code modification, hot reloading and isolation per language/runtime (as of 2026-10-19), for INIT Agent's T0/T1/T2 design

Scope note: INIT Agent tiers are T0 = data reload, T1 = leaf plugins/tools hot-loaded in-process, T2 = core changes via gated process restart. Native (Rust/C) and WebAssembly get the most depth. Research date: 2026-10-19. Every finding carries the date of its source where known. Items marked "(secondary)" come from aggregators or third-party write-ups, not primary docs.

## 1. Native Rust: dylib hot reload, Subsecond hot-patching, ABI instability, abi_stable, unloading safety, exec-based self-replacement

### Takeaway
Rust has no stable ABI. In-process native hot reload therefore works only for development (Subsecond, hot-lib-reloader) or under strict "load-only, never unload, C-ABI or abi_stable types" rules. `abi_stable` explicitly does not support unloading, and dlclose plus thread-locals is a documented crash source. For production self-modification of a Rust core, the robust primitive is process replacement: atomic `rename(2)` of the new binary, then fork+`execve` (or plain `execve`) with inherited listening fds and serialized state in a memfd or systemd fdstore. Cloudflare's ecdysis has run this pattern in production since 2021.

### Cited Findings

**Dioxus Subsecond (runtime hot-patching)**
- Dioxus 0.7, which shipped Subsecond hot-patching, was published 2025-09-08. Subsecond ships inside the `dx` CLI, and `dx serve` patches every `subsecond::call` site. The team chose explicit integration points ("synchronization points") where frameworks can close connections, re-create state or drop listeners. — [Dioxus 0.7 release blog](https://dioxuslabs.com/blog/release-070)
- An earlier Subsecond prototype modified process memory externally, which could leave a program stuck waiting on I/O so edits never took effect. The final design is "an incremental linking or binary patching method" that the authors relate to an idea Andrew Kelley proposed for Zig. — [Dioxus 0.7 release blog](https://dioxuslabs.com/blog/release-070)
- Mechanism: "Subsecond works by detouring function calls through a jump table." Subsecond does not modify process memory. An external tool compiles the changed code, links it against the running program's addresses, and sends a new jump table, which the app applies before it "continues running". Because of ASLR, "running apps need to communicate the address of `main` to the patcher", and `main` is the anchor for offsets. Patches arrive over the Dioxus devtools websocket or through `apply_patch(JumpTable)`. — [subsecond 0.7.10 lib.rs docs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html)
- Stale-frame handling: calls to stale instances raise a panic that the next `subsecond::call` up the stack catches and retries. The README says "If a hot function is actively being called, then subsecond will rewind the stack to the 'cleanest' entrypoint." — [subsecond lib.rs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html); [subsecond README](https://docs.rs/crate/subsecond/0.7.10/source/README.md)
- ThinLink is "simply a wrapper around your existing linker", with automatic dynamic linking to dependencies, jump-table generation and object-file diffing. It also speeds up dev-mode linking in general. — [subsecond lib.rs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html)
- Limitations, quoted from the docs:
  - "Subsecond currently does not support hot-reloading of structs."
  - New globals can be added, but "their destructors will never be called". Renamed globals count as new ones, and changes to static initializers are not observed.
  - Thread-locals in the tip crate "will seemingly reset to their initial value on new patches", which can crash.
  - "Subsecond currently only patches the 'tip' crate."
  - "Patching pointers is wildly unsafe and can lead to crashes and undefined behavior."
  - `HotFn::ptr_address` changes on every patch even when a function did not change.
  — [subsecond lib.rs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html); [Dioxus hot-reload reference 0.7](https://dioxuslabs.com/learn/0.7/essentials/ui/hotreload)
- Production readiness: Subsecond "is only enabled when debug_assertions are enabled so you can safely ship your application", so release builds do not include it. Dioxus 0.7 requires `--hotpatch` "while Subsecond is still experimental." — [subsecond lib.rs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html); [Dioxus hot-reload reference](https://dioxuslabs.com/learn/0.7/essentials/ui/hotreload)
- Platforms: Android (arm64-v8a, armeabi-v7a), iOS (arm64), Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64, arm64) and wasm32. "iOS device is currently not supported due to code-signing requirements." — [subsecond lib.rs](https://docs.rs/subsecond/0.7.10/src/subsecond/lib.rs.html)
- Bevy and Iced have adopted Subsecond, and starter integrations exist for Axum, Bevy and Ratatui. The CLI works on non-Dioxus Rust projects. — [Dioxus 0.7 release blog](https://dioxuslabs.com/blog/release-070)
- A 0.8.0-alpha.1 of the subsecond crate is published on docs.rs, so the API is still pre-stable. — [subsecond 0.8.0-alpha.1 README](https://docs.rs/crate/subsecond/0.8.0-alpha.1/source/README.md)
- Neither the Dioxus docs nor the blog gives a millisecond patch-latency figure. The name "Subsecond" is the only latency claim. — [Dioxus hot-reload reference](https://dioxuslabs.com/learn/0.7/essentials/ui/hotreload)

**libloading and hot-lib-reloader (classic dylib reload)**
- hot-lib-reloader "is build around the libloading crate and will require you to put code you want to hot-reload inside a Rust library (dylib)". Reloadable functions need `#[unsafe(no_mangle)]`. The file-watch debounce defaults to 500 ms. Loaded copies use a shadow name template, `{lib_name}-hot-{load_counter}`. — [hot-lib-reloader-rs README](https://github.com/rksm/hot-lib-reloader-rs)
- hot-lib-reloader caveats:
  - After a signature change "you'll likely see a crash".
  - "Types of structs and enums that are used in both the executable and library cannot be freely changed." Differing layouts are UB.
  - Generics cannot be exported because `no_mangle` does not support them.
  - Global state must be re-initialized after a reload, and TypeId-based systems such as ECS frameworks may break.
  - On macOS the library must be codesigned.
  - Known crash with `tracing`: "Attempted to register a DefaultCallsite that already exists!"
  — [hot-lib-reloader-rs README](https://github.com/rksm/hot-lib-reloader-rs)
- libloading `Library::new` safety: "When a library is loaded, initialisation routines contained within it are executed … conceptually the same [as] calling an unknown foreign function". Termination routines "may be executed when the library is unloaded". `close` "might be a no-op, depending on the flags", and `Drop` closes the library and ignores errors, so "underlying data structures may still get leaked". — [libloading Library docs](https://docs.rs/libloading/latest/libloading/struct.Library.html)

**Unloading hazards (dlclose and thread-locals)**
- Rust issue #88737: a `thread_local!` in a dlopen'd crate registers its destructor on a thread. If the crate is dlclose'd before that thread exits, the destructor still runs afterwards, which is a memory-safety problem. — [rust-lang/rust#88737](https://github.com/rust-lang/rust/issues/88737)
- Rust issue #91979: a cdylib loaded with dlopen and unloaded with dlclose segfaults at thread exit on glibc 2.17. — [rust-lang/rust#91979](https://github.com/rust-lang/rust/issues/91979)
- The C hot-reload library `cr` hit the same class of bug with Rust plugins. Its workarounds were defining `__cxa_thread_atexit` in the host, which disables TLS destructors program-wide, or giving each plugin version a unique file name. — [fungos/cr#35](https://github.com/fungos/cr/issues/35)
- glibc's `__cxa_thread_atexit_impl` takes a reference on the owning DSO, so dlclose leaves the library mapped until TLS destructors run. The libc++abi fallback ignores the DSO handle, so the library can be unmapped too early. — [LLVM review D21803](https://reviews.llvm.org/D21803?id=71508); [libcxxabi cxa_thread_atexit.cpp](https://llvm.googlesource.com/libcxxabi/+/refs/heads/release_90/src/cxa_thread_atexit.cpp)
- glibc bug BZ 33598 (dlclose called from a thread_local or static destructor during exit unmaps the caller) still had fix patches under review in May 2026. — [glibc patchwork v4, 2026-05-06](https://patchwork.sourceware.org/project/glibc/patch/20260506202903.691892-1-adhemerval.zanella@linaro.org/)
- The Matrix server Continuwuity says its Rust hot-reload setup works only on x86_64/aarch64 Linux glibc. It says musl "does not implement dlclose", and on macOS loading works but unloading does not in its setup (RTLD_GLOBAL and thread-local issues). — [Continuwuity hot_reload.mdx](https://forgejo.ellis.link/mmaudet/continuwuity/src/branch/main/docs/development/hot_reload.mdx)

**ABI-stability crates and proposals**
- `abi_stable` supports libraries "loaded at program startup, and with load-time type-checking", and it can load libraries "built with a different Rust version", but it has no support for unloading. — [abi_stable docs](https://docs.rs/abi_stable)
- `stabby` gives Rust types a stable layout, including sum types with niche optimization. The motivation is that Rust's only stable ABI, the C ABI, "has no concept of sum-types". Its docs warn that "due to a breaking change in Rust 1.78, stabby's implementation of trait objects may raise performance issues". Version metadata is inconsistent: docs.rs "latest" shows 72.1.16 with a failed build, and DepScope shows an update on 2026-01-18. — [stabby docs.rs](https://docs.rs/crate/stabby/latest); [stabby-abi](https://docs.rs/stabby-abi); [DepScope (secondary)](https://depscope.dev/pkg/cargo/stabby)
- crABI (`extern "crabi"`, `repr(crabi)`) is an experimental feature gate. A full RFC with an ABI spec is required before stabilization. It was approved by T-lang as an experiment but not by T-compiler, and a November 2025 LWN commenter noted the thread had been quiet for over a year. — [rust-lang/rust#111423](https://github.com/rust-lang/rust/issues/111423); [Rust internals "A new beginning"](https://internals.rust-lang.org/t/a-new-beginning-for-rust/23871); [LWN comment (secondary)](https://lwn.net/Articles/1047761/)
- Envoy's native "dynamic modules" (Rust SDK) aim for "minimal overhead" relative to built-in C++ and zero-copy header/body access. A module must be ABI-compatible with the exact Envoy binary that loads it, and the feature is marked "work in progress" (Envoy v1.39 docs). — [Envoy dynamic modules v1.39](https://www.envoyproxy.io/docs/envoy/v1.39.0/_sources/intro/arch_overview/advanced/dynamic_modules.rst.txt)

**Exec-based self-replacement and state handoff**
- `rename(2)`: if newpath exists "it will be atomically replaced, so that there is no point at which another process" finds it missing. There may still be "a window in which both oldpath and newpath refer to the file". — [man7 rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html)
- `execve(2)` keeps the PID. "By default, file descriptors remain open across an execve()", except close-on-exec fds. "All threads other than the calling thread are destroyed", "Memory mappings are not preserved", caught signal handlers reset to default, and pthread mutexes and condvars, POSIX timers and atexit handlers are lost. — [man7 execve(2)](https://man7.org/linux/man-pages/man2/execve.2.html)
- Cloudflare ecdysis (blog 2026-02-13, in production since 2021, in 330+ data centers):
  - The parent `fork()`s, and the child `execve()`s the new binary for "a clean slate: new address space, fresh code, and no inherited memory".
  - The listening socket passes through a named pipe, and "Only listening sockets and communication pipes are inherited."
  - The parent closes its listener only after the child calls `ready()`. If the new binary fails during init it exits, and "Since the parent never stopped listening, no connections are dropped."
  - ecdysis does not hand off application state. Cloudflare's sibling library shellflip "focuses on transferring arbitrary application state between parent and child."
  — [Cloudflare blog: ecdysis](https://blog.cloudflare.com/ecdysis-rust-graceful-restarts)
- The `bye` crate (tokio): the parent fork+execs itself and passes a pipe through an env var. The child calls `bye::ready()` once listeners, tasks and caches are initialized. If the child exits before signalling ready, the parent keeps running. — [bye docs.rs](https://docs.rs/crate/bye/latest)
- Go's tableflip spawns argv[0], passes the listening fds, and the old process exits when the new one signals readiness ("similar to the approach used by nginx, but as a library"). — [tableflip (pkg.go.dev)](https://beta.pkg.go.dev/github.com/flyaways/tableflip)
- systemd fdstore:
  - With `FileDescriptorStoreMax=` > 0, a service can send fds with `FDSTORE=1` (plus `FDNAME=`), and "Whenever the service is restarted the fds in its fdstore will be passed to the new instance".
  - memfd "may conveniently be used to serialize service state into and then place in the fdstore". After abnormal termination the state is lost unless it was saved incrementally.
  - `FileDescriptorStorePreserve=yes|on-success` controls how long the store lives.
  — [systemd.io FILE_DESCRIPTOR_STORE](https://systemd.io/FILE_DESCRIPTOR_STORE/)
- Cloudflare's example of why handoff matters: for a service handling thousands of requests per second, "even a gap of 100ms means hundreds of dropped connections". — [Cloudflare ecdysis blog](https://blog.cloudflare.com/ecdysis-rust-graceful-restarts)

### Inferences
- For INIT T1, native Rust dylibs are only defensible as append-only loads: load each version under a unique filename, never dlclose, accept the leak, and keep the boundary to `extern "C"` or `abi_stable`/`stabby` types. Every unload path runs into the TLS-destructor and dlclose problems documented above, and musl cannot unload at all.
- Subsecond is a developer-loop accelerator (debug_assertions only, tip crate only, no struct changes). It is not a production self-modification mechanism. Its jump-table design is still a useful pattern to borrow: route every hot entry point through one indirection table and swap the table atomically at a known safe point.
- For T2, the recommended Rust pattern is a fork+exec handoff in the style of ecdysis or bye, not a bare execve. The old process stays alive until the new one passes its self-check and signals `ready()`, which gives automatic rollback for free. The steps are: build to a temp path, verify (tests plus a canary run), `rename()` over the live path, then fork+exec with listener fds and a memfd state snapshot. A bare execve loses that rollback, because the old image is gone once exec succeeds.
- The cost of a T2 restart is dominated by the new binary's startup and state reload, plus the build time, which is outside this note's scope. The handoff mechanics themselves (fork, exec, fd passing) are typically sub-millisecond to low-millisecond kernel operations. That is general systems knowledge, not measured in these sources.

### Gaps
- No primary-source millisecond latency for a Subsecond patch cycle was found. Neither Dioxus docs nor the blog give one.
- Nothing found on Subsecond in long-running production servers. The docs only say it is compiled out of release builds.
- No current (Oct 2026) status update on crABI. The latest signal found is from Nov 2025.
- shellflip (Cloudflare's state-transfer library) was mentioned but not researched in depth.

## 2. C/C++: dlopen/dlclose live-reload (Handmade Hero style), cr.h, Live++, risks

### Takeaway
The proven C pattern (Handmade Hero day 021, cr.h) works like this: the host owns all memory and passes it to a code-only DLL each frame or tick, the DLL is reloaded from a uniquely named copy, and function pointers are re-resolved after every reload. cr.h adds automatic transfer of the static `.state`/`.bss` sections and rollback on SIGSEGV. Live++ (the engine behind Unreal's Live Coding) does function-level binary patching. Changes to data layout or class members remain unsupported in practice across all of these.

### Cited Findings
- Handmade Hero day 021, "Loading Game Code Dynamically", splits platform and game code. It covers loading and unloading the game DLL on the fly, moving persistent locals into game state, and avoiding locking the DLL so the build can overwrite it. — [Handmade Hero guide day 021](https://guide.handmadehero.org/code/day021)
- The design rationale, from Casey's forum explanation as summarized: the DLL holds only code, and the platform layer owns the memory block and hands it to the game each frame, so code can be swapped between frames. Community notes add that function pointers and pointers to DLL statics must be reloaded after each reload, and that changing data layout or ABI breaks safe reload. — [Handmade Hero forum thread](https://hero.handmade.network/forums/code-discussion/t/253/p/1507) (secondary summary)
- On Windows, a DLL in use cannot be replaced, so loaders use copied or randomized filenames. — [nullprogram: interactive C programming (2014)](https://nullprogram.com/blog/2014/12/23/)
- cr.h is a single-header hot-reload library, "works and tested on Linux, MacOSX and Windows":
  - On reload it copies the library to a versioned path, then loads the copy with dlopen/dlclose or LoadLibrary/FreeLibrary. `CR_UNLOAD` and `CR_LOAD` callbacks let the guest save and restore.
  - Variables tagged `CR_STATE` go into a `.state` section, and cr.h copies that section plus `.bss` across reloads ("automatic static state transfer"). Safety modes are `CR_SAFEST` (address and size check), `CR_SAFE` (size), `CR_UNSAFE` (default) and `CR_DISABLE`.
  - Crash protection uses SIGILL/SIGBUS/SIGSEGV/SIGABRT handlers, or SEH on Windows. A crash rolls back to the last working version and its backed-up state. A crash in version 1 reports `CR_INITIAL_FAILURE`.
  - The FAQ says that for C, "simple things will mostly work". Code "bordering C++" is riskier because the linker may rearrange things. Heap memory must be freed by the same runtime (/MD), and multi-plugin crash handling may have issues on Linux/macOS.
  — [fungos/cr README](https://github.com/fungos/cr)
- Unreal Engine's Live Coding "uses an integration of Live++". It works in the editor, during Play-In-Editor sessions, and on packaged desktop builds attached to the editor. — [Epic docs: Using Live Coding](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-live-coding-to-recompile-unreal-engine-applications-at-runtime)
- Live++ is a binary patching tool that reloads changes while the app runs, "even when paused at a breakpoint". Its three main features are Hot-Reload, Hot-Restart and Hot-Deoptimize. — [JetBrains blog 2024-09-24](https://blog.jetbrains.com/dotnet/2024/09/24/hot-reloading-cplusplus-with-liveplusplus-and-jetbrains-rider/) (403 on fetch; quoted via search snippet)
- Live Coding "cannot add or remove UPROPERTYs". "Changes to the memory layout of your class (such as adding a property) can not be reloaded since instances of that class may be in memory." Function-body edits in .cpp files are the reliable case. — [Unreal community wiki: Hot Reload and Live Coding](https://unrealcommunity.wiki/6100e80a9c9d1a89e0c2eaa1) (community, secondary)
- Third-party UE plugins wrap Live++ 2.x, and one requires Live++ ≥ 2.7.0 because of breaking API changes. — [brickadia/LivePP2](https://github.com/brickadia/LivePP2)

### Inferences
- C and Rust share the same hazards in-process: dangling function pointers into an unloaded image, static and TLS state lost or duplicated, layout drift between host and plugin, and dlclose races. C just has fewer hidden runtime registrations, such as Rust's std TLS destructors and `tracing` callsite registries.
- For an agent-written T1 tool, cr.h-style rollback on SIGSEGV is not a safety boundary. Memory corruption can damage host state before the signal fires. That is why Zed and Envoy moved untrusted extensions to Wasm (Q4).
- Live++ is a commercial developer tool aimed at MSVC/Clang game workflows. It does not fit autonomous self-modification on macOS/Linux servers. Its exact platform matrix is unverified (see Gaps).

### Gaps
- The Live++ official site (liveplusplus.tools) failed DNS resolution from the research environment, and the JetBrains post returned 403. Live++'s internal patching mechanism, platform list (Linux/macOS?), latest version and date are therefore unverified from primary sources. The MSVC/Clang/LLD compiler support claim comes only from a weak directory listing.
- No benchmark numbers (reload latency in ms) were found for cr.h or Handmade-style reloads.

## 3. Zig: incremental compilation and in-place binary patching / hot code swapping (2026 status)

### Takeaway
As of mid-2026, Zig's incremental compilation is real and fast. Zig 0.16.0 brought `-fincremental --watch` with LLVM, and the new ELF linker gives rebuilds of about 30 ms for small apps and 228–288 ms for the Zig compiler itself. That speed patches the output binary on disk. Hot code swapping into a running process still exists only as 2022–2023 proofs of concept (Linux `process_vm_writev`, macOS ptrace). No 2026 devlog announces it as a shipped feature.

### Cited Findings
- 2026-04-08 (Matthew Lugg): incremental compilation now works with the LLVM backend on master, slated for 0.16.0, enabled with `-fincremental --watch`. It does not speed up LLVM object emission, but compile errors appear much faster. The core team has used incremental compilation in its own workflows for about a year. — [Zig devlog 2026](https://ziglang.org/devlog/2026/)
- 2026-03-10 (Lugg): a 30,000-line type-resolution redesign fixed many incremental bugs and largely removed "over-analysis". — [Zig devlog 2026](https://ziglang.org/devlog/2026/)
- 2026-05-30 (Lugg): the new ELF linker (`-fnew-linker`, debuted in 0.16.0) supports fast incremental rebuilds on x86_64 Linux, including with external libraries and C sources. DWARF for Zig code is not yet supported. Edits to a Tetris clone rebuilt in about 30 ms each. Incremental rebuilds of the Zig compiler took 244/228/288/283 ms after a 36 s initial build. — [Zig devlog 2026](https://ziglang.org/devlog/2026/)
- 2026-05-26 (Kelley): splitting the build system brought `zig build -h` mean wall time from 150 ms to 14.3 ms (−90.4%). The June 30 entry put 0.17.0 blockers to wrap "by early August". — [Zig devlog 2026](https://ziglang.org/devlog/2026/)
- The 2026 devlog contains no entry on hot code swapping or in-place patching of a running process. — [Zig devlog 2026](https://ziglang.org/devlog/2026/)
- Historical PoC: Andrew Kelley's commit "hot code swapping PoC working" added a CLI mode that takes commands over a socket and writes updates into the running process with `process_vm_writev`, using ELF segments marked writable and an append-only linker. A macOS PoC by Jakub Konka (ptrace-based) was merged 2023-03-19. — [Zig commit 4f4ddf5 (mirror)](https://git.jakstys.lt/motiejus/zig/src/commit/4f4ddf5ef2f2e2ee2774e0a5311b099ea19d9ce2); [Zig hcs branch mirror](https://git.adamlamers.com/PublicArchive/zig/commits/branch/hcs/lib/std)
- Zig's 2020 self-hosted backend announcement said the incremental design "gets us 90% of the way to hot code swapping". — referenced in [kristoff.it: Zig's new relationship with LLVM](https://kristoff.it/blog/zig-new-relationship-llvm/)
- Unverified secondary claims: Kelley described sub-50 ms rebuilds of million-line codebases with the native x86 backend (May 2026 interview, relayed). A Fizzy pixel editor demo did about 5 s initial and 50–70 ms incremental rebuilds. — [Eiffel/Zig paper (secondary)](https://www.eiffel.org/blog/Finnian%20Reilly/2026/05/zig-intermediate-language-eiffelstudio); [zeli.app (secondary)](https://zeli.app/en/story/49085666)
- The Subsecond authors cite Andrew Kelley's Zig idea as the inspiration for their incremental-linking hot-patch approach. — [Dioxus 0.7 blog](https://dioxuslabs.com/blog/release-070)
- DIY Zig hot reload (game code in a DLL) hits Windows file locking. The workaround is to copy the DLL and load the copy. — [Ziggit: Hotreloading in zig](https://ziggit.dev/t/hotreloading-in-zig/1737)

### Inferences
- For INIT, Zig in October 2026 gives the fastest native edit-compile loop (tens of ms on x86_64 Linux with the self-hosted backend and new linker). Live in-process swapping would still be DIY: a dlopen pattern like C, or a debugger-style `process_vm_writev` PoC. Zig is pre-1.0 with a 0.17 release cycle, which adds churn risk.
- Zig's fast incremental builds make the T2 path cheaper: rebuild in under 0.3 s, then exec-handoff. They do not change the T1 safety analysis.

### Gaps
- No official 2026 statement on whether hot code swapping is on the near-term roadmap. aarch64/macOS incremental status in 2026 was not covered by the devlog entries found.
- Whether Zig 0.17.0 shipped by October 2026 was not verified.

## 4. WebAssembly as the plugin/body layer (Wasmtime/WAMR/Wasmer; Component Model; WASI 0.2/0.3; hot swap; sandboxing; fuel/epochs; real projects)

### Takeaway
Wasm is the strongest fit for INIT's T1. Wasmtime instantiates pre-compiled modules in microseconds (SpiderMonkey.wasm: 2 ms → 5 µs), and the pooling allocator, copy-on-write heaps and `InstancePre` keep it there. Isolation is capability-based (WASI, deny by default). Runaway code is stopped by epochs (about 10% slowdown) or deterministic fuel (slower). Hot swap is a host-side pointer swap: compile or deserialize a new Component, build an `InstancePre`, and route new calls to it while old Stores drain. The cost is CPU throughput. On a crypto-heavy libsodium suite (June 2026), Wasmtime 46 ran 2.41x slower than native, or 1.46x with `wide_arithmetic`. WAMR AOT ran 1.57x and Wasmer 7.1 ran 2.08x (1.33x with `wide_arithmetic`). WASI 0.3 (native async) was ratified 2026-06-11 and is on by default in Wasmtime ≥ 46. Guest-language async toolchains are still landing.

### Cited Findings

**Performance vs native**
- Frank Denis's libsodium benchmark, 2026-06-23, geometric-mean slowdown vs native on an AMD Ryzen AI 9 HX 470 locked at 2 GHz (ITERATIONS=3, so small tests are noisy):

  | Runtime | 2024 | 2025 | 2026 |
  |---|---|---|---|
  | Wasmtime | 2.67x | 2.54x | 2.41x |
  | WAMR AOT | n/a | 1.59x | 1.57x |
  | WasmEdge AOT | – | – | 1.74x |
  | wazero | 4.84x | 4.70x | 4.72x |
  | Node/V8 | 8.60x | – | 7.95x |
  | Bun | – | – | 8.77x |

  With `wide_arithmetic`, Wasmtime 46.0.0 drops to 1.46x and Wasmer 7.1.0 to 1.33x (2.08x without it). WAVM nightly had the fastest 2026 baseline. The author's conclusion is that runtime, release, enabled features and run-mode flags all matter, so benchmark your own workload. — [00f.net: Performance of WebAssembly runtimes in 2026](https://00f.net/2026/06/23/webassembly-runtimes-2026/)
- Older baselines: in browsers, Wasm was 45% (Firefox) to 55% (Chrome) slower on average, with peaks of 2.08x and 2.5x (USENIX ATC 2019). PolyBench/C on x86 and Arm showed about 1.3x slowdown (2022). With modern bounds-checking schemes, performance-oriented runtimes run within 20% of native on x86-64, 35% on Armv8 and 17% on RISC-V, while naive bounds checks can cost up to 650% (IISWC 2022). — [Jangda et al., arXiv 1901.09056](https://arxiv.org/abs/1901.09056v3); [arXiv 2206.12888](https://arxiv.org/pdf/2206.12888); [St Andrews "Leaps and bounds"](https://research-repository.st-andrews.ac.uk/handle/10023/27004)
- Wasure (ICPE 2026, Carissimi and Titzer, 2026-02-05) found median execution times "differing by up to two orders of magnitude between engines" (Wasmtime 33, Wasmer 4.3, WasmEdge 0.14.1, WAMR 2.3.1, wazero 1.9, V8 13.9). Max RSS was "within a relatively narrow band". — [arXiv 2602.05488](https://arxiv.org/html/2602.05488v1)
- Cranelift vs Winch: Winch is a single-pass baseline compiler with no IR that compiles faster but emits slower code. Cranelift is the default. The TPDE paper (2025) measured a TPDE backend compiling 4.27x faster than Cranelift but 1.74x slower than Winch, and found 37% of Cranelift's time goes to Wasm→CLIF translation. — [Wasmtime: fast compilation](https://docs.wasmtime.dev/examples-fast-compilation.html); [TPDE arXiv 2505.22610](https://arxiv.org/pdf/2505.22610)
- Security caveat: from Wasmtime 25.0.0 up to (but not including) 36.0.7, 42.0.2 and 43.0.1, the non-default Winch backend could let crafted guest Wasm access host memory outside the sandbox. — [CVE-2026-34987 via releasealert](https://releasealert.dev/cve/CVE-2026-34987)
- Wasmtime 10-era performance work: Cranelift codegen improvements of 22% on SpiderMonkey.wasm and 18% on bz2, and regalloc2 parallel compile 20–42% faster. Fast stack walking replaced libunwind, which could take "up to half a second" for a trap backtrace with thousands of modules loaded. — [Bytecode Alliance: Wasmtime 10 performance](https://bytecodealliance.org/articles/wasmtime-10-performance)

**Instantiation time and pooling**
- SpiderMonkey.wasm instantiation went "from about 2 milliseconds" to 5 µs, "400 times faster". Copy-on-write and lazy init cut per-instantiation heap writes from tens of MB to "a few kilobytes". — [Bytecode Alliance: Wasmtime 10 performance](https://bytecodealliance.org/articles/wasmtime-10-performance); [Wasmtime 1.0 article](https://bytecodealliance.org/articles/wasmtime-1-0-fast-safe-and-production-ready)
- Fast-instantiation recipe:
  - The pooling allocator pre-allocates memories, tables and instances for a configured maximum (the example uses 100 memories of up to 2 GiB each and 100 core instances), so instantiation takes slots from the pool.
  - `memory_init_cow(true)` defers copying until first write.
  - `InstancePre` does import resolution and type-checking ahead of time.
  - Past the pool limit, instantiation errors, so add back-pressure such as a semaphore.
  — [Wasmtime docs: Tuning for fast instantiation](https://docs.wasmtime.dev/examples-fast-instantiation.html)
- The pooling allocator's main cost is "a very large reservation of virtual memory". It is "not as fast in a single-threaded scenario" but avoids VM locks and IPIs under parallelism. A past CVE (CVE-2022-39393) leaked data in the pooling allocator combined with CoW heaps (fixed in 2.0.2 and 1.0.2). — [Wasmtime InstanceAllocationStrategy docs](https://docs.wasmtime.dev/api/wasmtime/enum.InstanceAllocationStrategy.html); [GitLab advisory CVE-2022-39393](https://advisories.gitlab.com/cargo/wasmtime/CVE-2022-39393/)
- Fastly (Wasmtime-based Compute) claims instantiation "typically within a few microseconds" and a sandbox per request created and destroyed "in microseconds". A third-party figure puts cold start at 35 µs. — [Fastly Compute datasheet](https://www.fastly.com/resources/datasheets/fastly-compute); [jacar.es (secondary)](https://jacar.es/en/fastly-compute-high-performance-edge-with-webassembly/)
- Spin "creates a new instance for every request", so modules are stateless. Fermyon's CTO put startup at under 1 ms to a few ms (2023). Sub-0.5 ms and "microsecond" claims are marketing. — [DevClass, 2023-03-29](https://devclass.com/2023/03/29/wasm-based-spin-1-0-released-to-address-limitations-of-serverless/); [Fermyon Cloud](https://fermyon.com/cloud)
- Hyperlight Wasm (Microsoft micro-VM plus embedded Wasmtime): 1–2 ms to start a VM and run a Wasmtime app, with a target under 1 ms. The 900 µs/request figure is warm-pool dispatch. Hyperlight is pre-1.0. — [Microsoft OSS blog 2025-02-11](https://opensource.microsoft.com/blog/2025/02/11/hyperlight-creating-a-0-0009-second-micro-vm-execution-time/); [The New Stack](https://thenewstack.io/microsofts-hyperlight-webassembly-for-vms-is-open-source/); [DevClass 2025-03-27](https://www.devclass.com/development/2025/03/27/microsoft-releases-experimental-hyperlight-wasm-micro-vms-that-run-wasm-apps/1623450)

**Store lifetime, limits and runaway code**
- A `Store` is "intended to be a short-lived object". "Once an instance is created within a Store it will not be deallocated until the Store itself is dropped", because there is no GC for instances. The default cap is 10,000 instances, memories and tables. `ResourceLimiter`/`StoreLimits` cap growth. — [Wasmtime Store docs](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html)
- Epoch interruption "imposes relatively low overhead … measured at around a 10% slowdown". It is non-deterministic because it runs on wall time. Fuel is deterministic, since the same program with the same fuel is always interrupted at the same point, but "imposes more overhead". Either mechanism can trap (not resumable) or async-yield to the host. — [Wasmtime docs: Interrupting Wasm](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
- SpiderMonkey.wasm runs "roughly twice as fast" with epoch interruption as with fuel. — [Bytecode Alliance: Wasmtime 10 performance](https://bytecodealliance.org/articles/wasmtime-10-performance)
- A Store starts with 0 fuel and a 0 epoch deadline, so it traps immediately unless these are set. `epoch_deadline_async_yield_and_update` supports cooperative time-slicing. — [Wasmtime Store docs](https://docs.wasmtime.dev/api/wasmtime/struct.Store.html)
- Shopify Functions documents deterministic instruction counts and recommends staying well below the cap ("every instruction adds checkout latency"). In a case study, p75 dropped from 610,000 to 430,000 instructions. An 11M-instruction / 5 ms limit is quoted only by third parties. — [Shopify: optimize instruction counts](https://shopify.dev/docs/apps/build/functions/optimize-instruction-counts); [Shopify Partners blog](https://www.shopify.com/partners/blog/discount-kit-shopify-functions-webassembly-api); [third-party skill page (unverified)](https://www.skills.sh/orcaqubits/agentic-commerce-claude-plugins/shopify-functions)

**Component Model and WASI status**
- WASI 0.3.0 was approved on 2026-06-11. Async is native: `stream<T>` and `future<T>` are canonical-ABI types that replace the `wasi:io` pollables and streams of 0.2. The model is completion-based, like io_uring and IOCP. Components can import and export `async func`. "The host is now the one in charge of managing the one event loop." — [Bytecode Alliance: WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3)
- In WASI 0.3, wasi:http's 0.2 `proxy` world is replaced by `service` and `middleware` worlds, which enable chaining. Composing components in one process can cut inter-service calls "from milliseconds to nanoseconds". — [Bytecode Alliance: WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3)
- Runtime support: "Wasmtime 46 will ship WASI 0.3.0 with Component Model Async enabled by default." Wasmtime 46.0.0 was reported released 2026-06-22. jco supports all of WASI 0.3. Guest async bindings are "in-progress for many languages" (Python, JS, C#, C), and Go uses stackful coroutines through componentize-go. 0.3.x patch releases follow about every two months. — [BA WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3); [youngju.dev (secondary)](https://www.youngju.dev/transcribe/2026-07-16-wasi-0-3-native-async.en); [wasi.dev roadmap](https://wasi.dev/roadmap)
- No firm date for Component Model 1.0 or WASI 1.0 was found. — [wasi.dev roadmap](https://wasi.dev/roadmap)
- Wasmtime release cadence: v47.0.0 on 2026-07-20 (GC proposal on by default), v49.0.0 on 2026-09-21 (`Component::same`, experimental exception handling in Winch). — [newreleases v49](https://newreleases.io/project/github/bytecodealliance/wasmtime/release/v49.0.0); [newreleases v47](https://newreleases.io/project/github/bytecodealliance/wasmtime/release/v47.0.0)

**Real projects using Wasm plugins**
- Zed (2024-04-18) chose Wasm to "make it impossible for an extension to crash Zed", because native shared-library Tree-sitter scanners had crashed it, and because Wasm is "cross-platform" and "designed for running untrusted code safely". — [Zed: Extensible Language Support Part 1](https://zed.dev/blog/language-extensions-part-1)
- Zed's extension mechanics:
  - Extensions are Rust compiled to Wasm components. The host reads `extension.wasm`, creates a Wasmtime `Component` and a `Store`, and instantiates it.
  - Bindings come from shared WIT through `wit_bindgen` (guest) and `wasmtime::component::bindgen!` (host), with `async: true`.
  - `extension.toml` records the API version, and the host picks matching bindings (for example `since_v0_2_0`). Dev and Nightly builds allow the latest API, Stable and Preview do not.
  - `extensions_updated` "reloads installed ones".
  - Language servers are returned as a `Command` that Zed runs as a subprocess.
  — [Zed: Life of a Zed Extension](https://zed.dev/blog/zed-decoded-extensions)
- Extism manifests sandbox plugins: `memory.max_pages` (64 KiB pages), `allowed_hosts` (empty = none, `null` = all) and `allowed_paths` (empty or null = no file access). A third-party guide says plugin creation (parse plus JIT) takes "tens to hundreds of milliseconds", so plugins should be reused. — [Extism manifest docs](https://extism.org/docs/concepts/manifest); [Extism Elixir skill (unverified)](https://skillsmp.com/creators/badbeta/elixir-phase-skills/extism-elixir)
- Envoy: Proxy-Wasm filters can be loaded and reloaded at runtime, and a crash in a Wasm filter does not take down Envoy. Commentators describe them as slower than C++ or dynamic modules, with memory scaling by plugins × VMs × worker threads. Dynamic modules run in-process, with no isolation and exact-ABI coupling. — [Jimmy Song (secondary)](https://jimmysong.io/blog/envoy-extension-vs-integration/); [Istio Wasm guide (secondary)](https://oneuptime.com/blog/post/2026-02-24-istio-webassembly-plugin-system/markdown); [Envoy dynamic modules](https://www.envoyproxy.io/docs/envoy/v1.39.0/_sources/intro/arch_overview/advanced/dynamic_modules.rst.txt)
- Microsoft Wassette (announced 2025-08-06, v0.3.4 by May 2026) is an MCP server that runs Wasm components as agent tools on Wasmtime. Permissions are deny-by-default and capability-based. WIT is converted to JSON Schema (component2json), and components can be fetched from OCI registries. — [Microsoft OSS blog: Introducing Wassette](https://opensource.microsoft.com/blog/2025/08/06/introducing-wassette-webassembly-based-tools-for-ai-agents/); [Wassette docs](https://microsoft.github.io/wassette/latest/); [Codex+Wassette guide (secondary)](https://codex.danielvaughan.com/2026/05/13/codex-cli-webassembly-development-rust-wasm-wassette-mcp-component-model/)

### Inferences
- Hot swap on Wasmtime needs no special runtime feature. Keep `Arc<InstancePre<T>>` (or a pre-compiled Component) behind an `ArcSwap`. Compiling the new component with Cranelift happens off the hot path and takes milliseconds to seconds. Then swap the pointer, so each new request instantiates in µs from the pool. In-flight Stores finish on the old code and are dropped, which frees them, since Stores never GC instances. This matches Spin's instance-per-request and Fastly's sandbox-per-request models. State must live in the host, or be passed or serialized explicitly. Wasm instances are naturally "shared-nothing", which is what makes the swap safe.
- The CPU cost of Wasm T1 matters only for compute-heavy tools. On crypto-like code it is 1.3–2.5x; on I/O- or LLM-bound agent tools it is negligible next to network and model latency. Add epochs at about 10% on top. Quality guard: benchmark the agent's actual tool mix with and without epochs before fixing the policy.
- Picking a runtime for INIT:
  - Wasmtime: best Component Model, WASI 0.3 and async support, a security track record, and a monthly cadence. Cranelift by default; avoid Winch for untrusted code given CVE-2026-34987.
  - WAMR AOT: about 1.6x native and a tiny footprint, but its Component Model and WASI p2/p3 support was not verified here.
  - Wasmer 7: fastest with `wide_arithmetic`.
- WASI 0.3 async aligns with an async Rust host (tokio). Until Rust guest bindings for p3 are confirmed stable, target WASI 0.2 (wasm32-wasip2) components for agent-written tools and adopt 0.3 when guest toolchains land.

### Gaps
- No published nanosecond figure for Wasmtime host↔guest call overhead or Component Model lifting/lowering cost. It needs a microbenchmark.
- No primary measurement of Wasmtime component compile time for typical tool sizes (Cranelift vs Winch, in ms).
- WAMR and Wasmer support for Component Model and WASI 0.3 as of October 2026 was not verified.
- The Rust guest toolchain status for WASI 0.3 (wasm32-wasip3 target, wit-bindgen async) was not confirmed by a primary source.
- Shopify's exact instruction limit is unverified (third party only).
- Zed's use of epochs or fuel, and whether its extension updates apply without restarting the editor, are not stated in the sources.

## 5. Embedded scripting engines for agent-written extensions in a native host (Lua/LuaJIT/Luau via mlua, Rhai, QuickJS/rquickjs, Starlark, Deno/V8)

### Takeaway
Scripting engines avoid a compile step (reload means re-evaluating source) but trade raw speed and sandbox strength.
- **Luau (via mlua)**: the most production-hardened sandbox (read-only globals, memory limit, interrupt callback, optional JIT).
- **QuickJS (rquickjs)**: a full JS runtime lifecycle in under 300 µs at about 367 KiB, with memory limits and interrupt handlers.
- **Rhai**: pure Rust and safe, but slow, roughly 2–3x slower than CPython.
- **Starlark**: deterministic and hermetic, but needs host-imposed resource limits.
- **V8 isolates (deno_core)**: fastest JIT, but heavyweight. Context creation is under 2 ms with a snapshot (about 40 ms without), and a deno_core runtime cold start is about 22 ms.

### Cited Findings
- mlua `Lua` API:
  - `sandbox()` (Luau only) "Enables (or disables) sandbox mode", which makes globals read-only and restricts `collectgarbage` to `count`.
  - `set_memory_limit()` makes allocations past the limit error. It "Does not work in module mode".
  - `set_interrupt()` (Luau only) is "periodically … called by Luau VM" and can return `VmState::Yield`.
  - `set_hook()` (non-Luau) can enforce execution limits.
  - Safe mode "will not allow to load unsafe standard libraries or C modules".
  - `enable_jit()` (feature `luau-jit`) "Toggles JIT compilation mode for new chunks of code."
  — [mlua Lua docs](https://docs.rs/mlua/latest/mlua/struct.Lua.html)
- mlua issue #488: memory limit plus untrusted input "can lead to abort on Luau". The reproducer is unbounded recursion with `sandbox(true)` and `set_memory_limit`. — [mlua-rs/mlua#488](https://github.com/mlua-rs/mlua/issues/488)
- Luau performance:
  - The interpreter can "match the performance of LuaJIT interpreter" on some workloads and is "noticeably faster than Lua 5.x".
  - The optional JIT targets x64 and arm64 but only compiles functions or modules the user marks, specialized by type annotations.
  - The compiler handles about 950K lines/s on one Ryzen 5900X core, and the interpreter core is about 16 KB of x64 code.
  — [Luau performance page](https://luau.org/performance)
- Rhai's own benchmarks (2.6 GHz Linux VM):

  | Benchmark | Rhai | Rhai "Full" | Rhai Grain (experimental) | Python 3 | V8 |
  |---|---|---|---|---|---|
  | Fibonacci | 2.25 s | 3.2 s | 1.2 s | 0.6 s | 0.07 s |
  | 1M-iteration loop | 0.13 s | – | – | 0.08 s | 0.05 s |
  | Primes | 0.85 s | – | – | 0.4 s | 0.09 s |

  Rhai describes itself as "roughly 2-3x slower than [Python 3]" on typical workloads, and the page recommends Lua or JS for full application logic. — [Rhai book: benchmarks](https://rhai.rs/book/about/benchmarks.html)
- QuickJS release 2026-06-04: "The complete life cycle of a runtime instance completes in less than 300 microseconds". Hello world is 367 KiB of x86 code. It supports ES2025 and is "42% faster than the previous release" on bench-v8. — [bellard.org/quickjs](https://bellard.org/quickjs/)
- rquickjs `Runtime`:
  - `set_memory_limit` is "a Noop when a custom allocator is being used" (features `rust-alloc` or `allocator`).
  - `set_max_stack_size` defaults to 256×1024 bytes.
  - `set_interrupt_handler` returning `true` raises "an uncatchable exception".
  - `Send`/`Sync` are only available with the `parallel` feature.
  — [rquickjs Runtime docs](https://docs.rs/rquickjs/latest/rquickjs/struct.Runtime.html)
- script-bench-rs (khvzak, the mlua author) benchmarks embedded engines for Rust, covering eval and Rust interop with a "Sort Rust objects" workload. The current README lists boa 0.22.0, koto 0.16.1, mlua 0.12.2 (Lua 5.5 and Luau), rhai 1.26.1, roto 0.12.0, rquickjs 0.14.0, wasmi 2.0.0 and wasmtime 49.0.2, on rustc 1.99.0 and an Apple M5 Max. Results are published only as an image. — [khvzak/script-bench-rs](https://github.com/khvzak/script-bench-rs)
- Starlark is deterministic and hermetic by design: execution "cannot access the file system, network, system clock" unless the host exposes them. starlark-rust powers Buck2, and ownership moved from Google to Meta at 0.4.0. One practical write-up warns "hermetic ≠ safe for untrusted code" without added wall-time and output limits. — [Wikipedia: Starlark](https://en.wikipedia.org/wiki/Starlark); [starlark-pyo3 (PyPI)](https://pypi.org/project/starlark-pyo3/2023.1/); [goagent issue #62 (secondary)](https://github.com/hyperifyio/goagent/issues/62)
- V8 snapshots cut context creation from about 40 ms to under 2 ms on desktop, and snapshot size sets a floor on per-isolate heap. — [v8.dev: lazy deserialization](https://v8.dev/blog/lazy-deserialization)
- deno_core: the openworkers runtime reports about 22 ms cold start, attributed to extension initialization. A Deno change took `deno run empty.js` from about 23.1 ms to 21.4 ms (end-to-end process time). — [openworkers-runtime-deno](https://github.com/openworkers/openworkers-runtime-deno); [Deno commit history mirror](https://code.fosterhangdaan.com/foster/denoland-deno/commits/commit/159f444d168ea187c58e087b8cb763e96989112e/core/runtime.rs)
- Cloudflare: an isolate starts "around a hundred times faster than a Node process" and uses "an order of magnitude less memory" at startup. One runtime process hosts many isolates. Secondary sources cite about 5 ms and about 2 MB per isolate. — [Cloudflare Workers: How it works](https://developers.cloudflare.com/workers/about/how-it-works/); [secondary](https://www.kunalganglani.com/blog/cloudflare-workers-v8-isolates-ai-agents.md)

### Inferences
- Sandbox strength, ordered by inference:
  1. Wasm (memory-safe linear memory plus capability imports, with fuel or epochs)
  2. V8 isolates (browser-grade, but a huge C++ attack surface and dependency)
  3. Luau sandbox (designed for untrusted Roblox code, but see the mlua #488 abort)
  4. QuickJS (small C codebase, memory-unsafe implementation language)
  5. Rhai and Starlark (memory-safe Rust interpreters, but resource limits are the host's job)
- For agent-written extensions, the main practical advantage of scripts over Wasm is iteration latency, since no `cargo build --target wasm32-wasip2` is needed. Luau or QuickJS evaluation is µs–ms, against seconds for a Rust→Wasm compile. A hybrid fits INIT T1 well: quick agent "glue" in Luau or JS, and performance-sensitive or long-lived tools as Wasm components. Javy and ComponentizeJS can also run JS inside Wasm when the stronger boundary is needed.
- Per-engine notes:
  - Rhai's 30x gap to V8 on fib rules it out for hot paths. Its value is pure-Rust safety and easy embedding.
  - LuaJIT is not separately benchmarked in the sources. Luau's claims are relative to the LuaJIT interpreter, not its JIT.

### Gaps
- Numeric results from script-bench-rs (only an image) could not be extracted, so no mlua vs rquickjs vs Rhai vs Wasmtime interop numbers.
- No measured per-call FFI overhead (ns) for mlua, rquickjs or Rhai was found.
- LuaJIT's 2026 maintenance status and its sandboxing story were not researched.
- No current deno_core isolate-only creation benchmark (with snapshot) for a Rust embedder was found.

## 6. Managed runtimes with first-class live modification (Erlang/BEAM + OTP, Common Lisp/SBCL, Smalltalk/Pharo, Julia/Revise.jl, Clojure)

### Takeaway
These runtimes make code replacement a language-level operation, with defined semantics for what happens to running code and live data:
- **BEAM**: two code versions per module, fully-qualified calls switch to the new version, `code_change` migrates process state, and supervisors restart failures.
- **Common Lisp**: `update-instance-for-redefined-class` migrates objects lazily.
- **Smalltalk**: whole-heap instance migration through `become:`.
- **Julia 1.12**: struct redefinition through world-age and binding partitions.
- **Clojure**: var indirection, which direct linking defeats.

Native languages lack this, since they have no runtime type metadata to drive migration, no safe point at which to switch, and no stable call indirection. The price is throughput. BeamAsm brought Erlang 30–130% gains but it remains well behind native for compute. Julia and SBCL compile natively, and Julia is fast, but world-age invalidation costs recompile latency.

### Cited Findings
- Erlang code loading:
  - A module has "current" and "old" versions. "Fully qualified function calls always refer to current code", and old code keeps running for processes still in it.
  - A third load purges the old version, and "any processes lingering in it are terminated".
  - The `-on_load` function must return `ok`, otherwise "the current code (if any) will remain current".
  — [Erlang/OTP: Compilation and Code Loading](https://www.erlang.org/doc/system/code_loading.html)
- OTP release handling:
  - `release_handler` calls `sys:suspend`, then `sys:change_code`, then `sys:resume`.
  - The appup option `{advanced, Extra}` makes gen_server and gen_statem run `code_change` to convert state.
  - `PrePurge` defaults to `brutal_purge`, which kills processes still on old code, while `soft_purge` fails the install instead.
  - Every release except the first needs a relup, and failed-upgrade reboot needs heartbeat.
  — [Erlang/OTP: Release Handling](https://www.erlang.org/doc/system/release_handling.html); [appup manual](https://www.erlang.org/docs/20/man/appup.html)
- BeamAsm JIT (OTP 24, default on most x86-64): 30–130% more iterations/s on JSON benchmarks (average about 70%), 30–50% more RabbitMQ messages/s, estone +50% (pattern matching +170%), at about 10% more memory. Communication-heavy workloads stay about the same. — [Erlang Solutions: Performance testing the JIT](https://erlang-solutions.com/blog/performance-testing-the-jit-compiler-for-the-beam-vm); [Phoronix](https://www.phoronix.com/news/Erlang-JIT-BeamAsm); [OTP 24.0 RC1 news](https://Erlang.Org/news/144)
- Common Lisp: when a class is redefined, each instance "is being updated" (per instance, on access) when it is touched. Its structure "is transformed so that it conforms to the current class definition". Added slots get initforms, and discarded slot values are passed in a plist so user methods can migrate data (the spec example converts x/y to rho/theta). — [CLHS: update-instance-for-redefined-class](https://www.lispworks.com/documentation/HyperSpec/Body/f_upda_1.htm)
- SBCL: before 1.3.12, calling a DEFUN concurrently with redefining it "could lead to execution of random bytes". This was fixed in sbcl-1.3.12. — [SBCL 1.3.12 tag/release notes](https://salsa.debian.org/ralt/sbcl/-/tags/sbcl-1.3.12)
- Smalltalk/Pharo: on class reshape, Smalltalks "create a new class and scan object memory for instances of the old class", copy shared instance variables, then `becomeForward:`. A 2017 vm-dev post says the full migration runs in a high-priority uninterruptible process. The IDE offers no hook for salvaging dropped instance-variable data. — [pharo-dev (GemStone dev quoted)](https://lists.pharo.org/hyperkitty/list/pharo-dev@lists.pharo.org/message/MA5MSYDQ7SP2I54AJ4AW4SSUU7X6UE6Y/); [vm-dev 2017](https://lists.squeakfoundation.org/archives/list/vm-dev@lists.squeakfoundation.org/message/CCUFDF3MARHZ5RYAAWCG5QJXESYBFWCN/); [pharo-dev 2012 (Shearar)](https://lists.pharo.org/hyperkitty/list/pharo-dev@lists.pharo.org/message/4T4BHHB5SGMF3NMEQD43ZKK6CZ4J6LD7/)
- Julia and Revise:
  - Julia 1.12 finished the "binding partitions" work, allowing safe replacement of const bindings and struct redefinition. Old and new struct definitions are separate types under world-age rules, and the old one is reachable only through `@world` (introspection).
  - Revise 3.13 added automatic struct and const redefinition. Revise 3.16 made it faster and runs internals in a "frozen world", but struct revision is still off by default (`revise_structs = true` in LocalPreferences.toml). Default-on is planned for 3.17.
  — [Julia manual: World Age](https://docs.julialang.org/en/v1/manual/worldage/); [JuliaCon 2025 talk](https://pretalx.com/juliacon-2025/talk/TJXV9L/); [Discourse: Revise 3.16](https://discourse.julialang.org/t/revise-3-16-fast-type-revision-frozen-worlds-and-complete-language-support/138211); [This month in Julia world 2026-01](https://julialang.org/blog/2026/01/this-month-in-julia-world/)
- Clojure: direct linking replaces var indirection with static invocation. "Var redefinitions will not be seen by code that has been compiled with direct linking." Clojure core has been compiled this way since 1.8. `^:dynamic` vars are never direct-linked, and `^:redef` opts out. — [Clojure reference: Compilation (direct linking)](https://clojure.org/reference/compilation)

### Inferences
- BEAM gives three things native code lacks:
  - A defined switch point: the next fully-qualified call.
  - Two-version coexistence, with purge semantics.
  - A state-migration callback (`code_change`).
  Wrapped in supervisors, these make "let it crash and restart" cheap. INIT can copy these semantics in a Rust host. Give each T1 Wasm tool a version with old and new coexisting, switch at call boundaries (each new call goes to current), add an explicit `migrate(old_state) -> new_state` export, and add a supervisor tree with restart intensity limits. The OTP restart-intensity parameters were not verified in this note.
- Clojure's direct linking shows the general tradeoff: every indirection that enables hot swap costs some performance, and production builds often remove it. Rust has the same trade, where a jump table (Subsecond), `dyn` dispatch, or `ArcSwap<Arc<dyn Tool>>` is the indirection price.
- Image-based systems (SBCL, Pharo) offer the richest live editing, including live object migration. They make reproducibility and auditing of a self-modifying agent harder, because the source of truth is the mutated image, not the repository. For INIT that argues for keeping the repository as the source of truth and treating live patches as derived.

### Gaps
- No direct throughput comparison of BEAM, SBCL or Julia against Rust for agent-harness-style workloads (JSON, I/O, orchestration) was gathered.
- OTP supervisor restart-intensity semantics were not fetched (known from general knowledge only, not cited).
- Pharo 12/13 has no official documentation on its class-migration hook. The sources are mailing-list posts from 2011–2017.
- Revise 3.17 release status in October 2026 was not verified.

## 7. How existing agent systems load plugins/extensions, and their latency (Pi/jiti, Zed/Wasm, Codex and Claude Code MCP subprocesses, Goose)

### Takeaway
The existing agent harnesses split into two camps:
- **In-process, fully privileged JS/TS** (Pi via jiti, reloaded with `/reload`). Fastest to iterate, no isolation.
- **Out-of-process MCP servers over stdio or HTTP** (Codex, Claude Code, Goose). Process isolation, but measured cold starts of 200 ms to 5 s per server, which forced Codex (v0.145, July 2026) and Claude Code (2.1.144) to make MCP startup non-blocking.

Zed and Wassette show the third option: Wasm components with capability sandboxing and µs instantiation. That option matches INIT T1 best.

### Cited Findings
- Pi extensions (docs for `@earendil-works/pi-coding-agent` 0.83.0):
  - Loading: "Extensions are loaded via jiti, so TypeScript works without compilation." Auto-discovery runs from `~/.pi/agent/extensions/` and `.pi/extensions/`, and project-local extensions load only after project trust.
  - Capabilities: register tools and commands, subscribe to lifecycle events, block or mutate tool calls and provider payloads, and override built-in tools.
  - Reload: `/reload` (or `ctx.reload()`) emits `session_shutdown`, reloads, then emits `session_start` with reason `"reload"`. "Code after `await ctx.reload()` must not assume old in-memory extension state is still valid." The docs advise rebuilding state from the session log and tool-result `details`.
  - Security: extensions "run with your full system permissions" and "can execute arbitrary code".
  — [Pi extensions.md v0.83.0](https://app.unpkg.com/@earendil-works/pi-coding-agent@0.83.0/files/docs/extensions.md)
- Pi was created by Mario Zechner ("badlogic"). It is an MIT monorepo, and the package moved from `@mariozechner/` to `@earendil-works/`. — [Armin Ronacher on Pi, 2026-01-31](https://lucumr.pocoo.org/2026/1/31/pi.md); [Pi extensions.md (old namespace)](https://app.unpkg.com/@mariozechner/pi-coding-agent@0.73.1/files/docs/extensions.md)
- Zed: Wasm components on Wasmtime, versioned WIT APIs, language servers spawned as subprocess `Command`s, and the goal "impossible for an extension to crash Zed" (details in Q4). — [Zed: Life of a Zed Extension](https://zed.dev/blog/zed-decoded-extensions); [Zed: Extensible Language Support Part 1](https://zed.dev/blog/language-extensions-part-1)
- Codex CLI MCP:
  - A per-server `startup_timeout_ms` defaults to 10,000 ms; it was hard-coded before. — [core-agent-ide mirror commit "per-server startup timeout"](https://forge.lthn.ai/core/core-agent-ide/commit/6efb52e545aad2b15c102d51ce1fd174b345021e)
  - A third-party write-up (2026-07-23) reports six project-scoped servers exposing 156 tools, each taking 1.78–5.03 s to initialize and blocking the first prompt. Codex v0.145 made startup non-blocking. v0.151 (2026-08-29) added MCP tool-result middleware and an optional-server grace period. — [Codex MCP cold-start post (secondary)](https://codex.danielvaughan.com/2026/07/23/codex-cli-mcp-cold-start-non-blocking-startup-oauth-timeout-tool-catalog-reliability/); [Codex v0.151 post (secondary)](https://codex.danielvaughan.com/2026/08/29/codex-cli-v0151-mcp-tool-result-middleware-optional-server-grace-period/)
  - A local stdio server booting in about 200 ms is described as "harmless". — [same secondary post](https://codex.danielvaughan.com/2026/07/23/codex-cli-mcp-cold-start-non-blocking-startup-oauth-timeout-tool-catalog-reliability/)
- Claude Code MCP: an issue report says a Python stdio server under macOS launchd background QoS "consistently needs 2.4–3.1s to start". Since CLI 2.1.144 the first request no longer waits for stdio servers beyond a window of about 2 s; older versions blocked for up to 30 s. — [Claude Code issue #76239 mirror](https://claudeissues.com/issue/76239-sdk-headless-mcp-tools-silently-missing-on-first-turn-when-stdio-server-startup)
- Goose: "goose utilizes MCP to connect to MCP systems/servers … referred to as extensions". There are built-in extensions, and external or custom ones run as MCP servers. When Goose delegates to an ACP agent, it passes extensions through as MCP servers. — [Goose architecture docs](https://goose-docs.ai/docs/goose-architecture/)
- Goose's built-ins (developer, computercontroller, memory, autovisualiser, tutorial) live in the `goose-mcp` crate as Rust `rmcp::ServerHandler`s and run in-process. Stdio and remote (Streamable HTTP) extensions are separate processes or endpoints with the Goose process's permissions. A docs mirror contradicts itself on whether every extension is a separate process. — [Goose docs mirror: extensions concepts (secondary)](https://mintlify.com/block/goose/concepts/extensions); [Goose MCP-native architecture post (secondary)](https://codex.danielvaughan.com/2026/04/07/goose-mcp-native-architecture/)
- Wassette exposes Wasm components as MCP tools with deny-by-default capabilities (details in Q4). It shows Wasm and MCP can be combined: one MCP process hosting many sandboxed components. — [Microsoft OSS blog](https://opensource.microsoft.com/blog/2025/08/06/introducing-wassette-webassembly-based-tools-for-ai-agents/)

### Inferences

Latency and isolation by mechanism. Figures are from the sources above; the "in-process call" column is general knowledge, not measured here.

| Mechanism | Load / first-use latency | Per-call cost | Isolation | Hot-reload semantics |
|---|---|---|---|---|
| Pi (jiti TS, in-process Node) | Transpile + import; no measured figure found | In-process JS call | None (full user permissions) | `/reload` restarts the extension session; old closures stay live |
| MCP stdio subprocess (Codex, Claude Code, Goose) | ~200 ms best case, 1.8–5 s typical with npx/Python/OAuth | JSON-RPC over a pipe | OS process, same user permissions | Restart the server process |
| Wasm component (Zed, Wassette, Spin) | Compile ms–s (cacheable); instantiate ~5 µs with pooling (SpiderMonkey example) | Wasm call + canonical-ABI lift/lower (ns figure not found) | Linear-memory sandbox + capability imports, fuel or epochs | Swap `InstancePre`; per-request instances |
| Native dylib (abi_stable, hot-lib-reloader) | dlopen in ms | Direct call | None; UB on layout drift | Unload unsafe (TLS/dlclose) |
| Subsecond | Patch per edit; "subsecond" claim only | Jump-table indirection | None | Dev-only; tip crate; no struct changes |
| Exec handoff (ecdysis) | New process startup + state reload | n/a | Full process restart | Parent stays alive until child is `ready()`, giving rollback |

Mapping to INIT's tiers:
- **T0 (data)**: atomic `rename()` of a config or prompt file plus a file watch, then swap an `ArcSwap` snapshot. This is trivial in any language.
- **T1 (leaf tools)**: Wasm components in Wasmtime, using pooling, `InstancePre`, epoch deadlines, StoreLimits and WASI capability grants. Optionally put a Luau or QuickJS lane in front for fast agent-authored glue. Keep MCP stdio for third-party tools only, started lazily and non-blocking (the lesson from Codex and Claude Code).
- **T2 (core)**: a Rust binary. Self-replace with fork+exec in the ecdysis style, with a readiness pipe and parent fallback, so a failed new core never takes the agent down. Carry state in memfd or the systemd fdstore. Keep a small, rarely-changed supervisor that holds the last-known-good binary path, which is the BEAM supervision lesson.

Quality guards:
- Per-tool fuel or epoch budgets.
- Deny-by-default capability manifests, as in Extism and Wassette.
- A WIT API version field, as in Zed's `since_vX` dispatch, so old tools keep working as the host evolves.

### Gaps
- No measured latency for jiti's TypeScript transpile and load per extension in Pi.
- No official Codex or Claude Code documentation numbers for MCP startup. The figures above are from third-party posts and issue reports.
- Goose's official docs page did not settle whether built-in extensions run in-process (the mirror contradicts itself).
- Claude Code's plugin and hook loading internals (beyond MCP) were not researched.
