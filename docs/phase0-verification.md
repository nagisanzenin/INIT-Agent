# Phase 0 verification list

*Generated 2026-10-19 from every `UNVERIFIED` mark in docs/ (reconciliation ledgers excluded).*

These are claims the docs could not verify during the documentation phase.
Phase 0 must settle each one by compiling or measuring, then update the owning doc and remove its mark.
Each owner, and the exact measurement procedures, are listed in testing.md (M-1..M-5, T-* ids).

## adr/004-wasm-components-for-t1.md (1)

- [L88](adr/004-wasm-components-for-t1.md#L88): All timing and size figures below are UNVERIFIED estimates until Phase 0 measures them (ext/host.md §3.14 and §6, ext/authoring.md §3.6).

## awareness.md (1)

- [L568](awareness.md#L568): Meta-Harness (environment bootstrapping), arXiv 2603.28052: https://arxiv.org/abs/2603.28052 (finding via research_notes/harness_benchmarks.md Q4; the appendix detail is from a search excerpt there, UNVERIFIED beyond that)

## body/log-context.md (4)

- [L259](body/log-context.md#L259): New files.** A session file (named by `INIT_SESSION`, or by an id the body generated at boot or for `/new`) is created with `create_new(true)`; the `session` header is written, then synced. The directory is synced with `File::open(dir)?.sync_all()`. On macOS, 
- [L451](body/log-context.md#L451): `strip_thinking_before_epoch` | **false** (`reasoning_content` must be passed back in tool mode; whether omitting *older* ones is accepted is UNVERIFIED, T-PROV-4) | **true** (an edit before a thinking block invalidates it; stripping all prior thinking is vali
- [L452](body/log-context.md#L452): `note_placement` | `UserMessage` | `MidSystemMessage` on Fable 5.1, Mythos 5.1, Fable 5, Mythos 5, Opus 5.5, Opus 5, Opus 4.8, Sonnet 5.5, Haiku 5.5; else `TextAfterToolResults` | `DeveloperMessage` (UNVERIFIED mid-input semantics, T-PROV-6) |
- [L484](body/log-context.md#L484): `user_id` is documented as usable for "KVCache isolation". INIT sends **no** `user_id`, because its effect on hit rates is unstated (UNVERIFIED).

## body/provider.md (10)

- [L197](body/provider.md#L197): `strip_thinking_before_epoch` | false: DeepSeek returns 400 without `reasoning_content` in tool mode | true: stripping every thinking block before an epoch is valid under preserved thinking | true (conservative; UNVERIFIED whether older encrypted items stay va
- [L263](body/provider.md#L263): DeepSeek lists the context as "1M". This doc uses 1,000,000, the lower reading; whether it means 1,048,576 is UNVERIFIED and does not matter for a compaction threshold of window − 16k.
- [L588](body/provider.md#L588): The context-overflow heuristic matches case-insensitively on `context length`, `maximum context`, `too long` or `prompt is too long`. DeepSeek's exact message is UNVERIFIED (Phase 0 check V-DS-6).
- [L722](body/provider.md#L722): `Message::Note` → a `developer` message item after the group's `function_call_output` items (`note_placement = DeveloperMessage`; mid-input developer semantics UNVERIFIED, log-context T-PROV-6).
- [L761](body/provider.md#L761): 400: InvalidRequest, or ContextOverflow when `error.code == "context_length_exceeded"` (UNVERIFIED code name).
- [L849](body/provider.md#L849): **Call id sanitizing (different api only).** Replace every byte outside `[A-Za-z0-9_-]` with `_` and truncate to 64 bytes. If two ids collide after sanitizing, append `_2`, `_3` … in order. The mapping is a pure function of the message list, so it yields the
- [L850](body/provider.md#L850): **Into DeepSeek from another provider.** The foreign assistant messages carry tool calls without `reasoning_content`. DeepSeek documents that Chat Completions does not support inserting tool calls the model did not generate, and that missing `reasoning_content
- [L874](body/provider.md#L874): `trigger()`, called from the input thread, sets the flag, takes the registered stream and calls `shutdown(Shutdown::Both)` on it. A blocked `read` on the other handle then returns `Ok(0)`. This was verified on macOS 14 (Darwin 23.6, rustc 1.97.1, 2026-10-19): 
- [L958](body/provider.md#L958): Anthropic: `ceil(w·h / 750)` after scaling the long edge down to 1,568 px (UNVERIFIED against the current vision docs).
- [L1095](body/provider.md#L1095): V-DS-6: record the exact 400 message for the overflow heuristic. First send `max_tokens: 393217`, one above the documented maximum, to capture the parameter-error format at no cost. Then, only if the owner approves, send one request just over the context windo

## body/tools.md (1)

- [L346](body/tools.md#L346): `/usr/bin/kill` does not exist on macOS, which is why `/bin/kill` is tried first. On Linux, procps-ng and util-linux `kill` accept `-s SIG -- -PGID` per their man pages; this is UNVERIFIED on the target distro (Phase 0 check T-L1). Boot records which kill path

## evolution.md (2)

- [L558](evolution.md#L558): `revert`: `Reverts` is an ancestor of `parent` (`git merge-base --is-ancestor`), and the candidate's tree equals the tree git itself produces for that revert. On git 2.39 (no `merge-tree --merge-base`, verified: "unknown option") the kernel uses a throwaway, k
- [L891](evolution.md#L891): **Final pick:** the node maximizing a low percentile of Beta(1 + n_s, 1 + n_f) (HGM's "best-belief"; the percentile ε is not stated in the paper or config, UNVERIFIED).

## ext/authoring.md (12)

- [L67](ext/authoring.md#L67): du -sh "$(rustc --print sysroot)/lib/rustlib/wasm32-wasip2"   # record the disk cost (estimate: ~100–200 MB, UNVERIFIED)
- [L138](ext/authoring.md#L138): **`members = ["*"]`.** Adding an extension needs no workspace edit. How Cargo treats a non-crate subdirectory matched by the glob is UNVERIFIED, so keep `body/ext/` to crates only.
- [L157](ext/authoring.md#L157): **No `#![forbid(unsafe_code)]` requirement.** Extension code runs inside Wasm, where `unsafe` can corrupt only the guest's own linear memory. wit-bindgen's `export!` is a crate-local macro (generated by `generate!`) that emits `#[unsafe(export_name)]` function
- [L316](ext/authoring.md#L316): **Bindings live in `mod bindings`, gated on `wasm32`.** This follows the Component Model book's `mod bindings { generate!(…); export!(…) }` pattern. Native `cargo test` never compiles generated code, so it does not depend on how wit-bindgen behaves on non-
- [L346](ext/authoring.md#L346): All figures are **UNVERIFIED estimates** for the M2 (8-core, Apple Silicon) with the settings above. No published measurement of small `wasm32-wasip2` component build times was found (searched 2026-10-19).
- [L540](ext/authoring.md#L540): Builds for `wasm32-wasip2` with the existing toolchain | no: needs a C toolchain (wasi-sdk, roughly 100+ MB, UNVERIFIED) once, on the maintainer's side | **yes**, plain cargo | no: needs C++ (wasi-sdk) |
- [L542](ext/authoring.md#L542): Component size and Cranelift compile at load | ~1–2 MB, sub-second (UNVERIFIED) | larger, about 5–10 MB, 1–3 s once per process (UNVERIFIED) | ~1 MB |
- [L589](ext/authoring.md#L589): `wit-bindgen` 0.63 API differs from the template (UNVERIFIED items) | compile error in the template | fix the template once in Phase 0 (M-3 setup); the skill table covers the known ownership case |
- [L590](ext/authoring.md#L590): `body/ext/` contains a non-crate directory | workspace error (UNVERIFIED) | keep only crates there |
- [L602](ext/authoring.md#L602): **Disk:** the ext share of `work/target` is estimated at 300–700 MB (host macro crates plus wasm32 builds; UNVERIFIED). Phase 0 records it. Cleanup is by exact path only (CONVENTIONS §5).
- [L625](ext/authoring.md#L625): AU-8 | `members = ["*"]` with a stray non-crate directory in `body/ext/` | record Cargo's behavior; update §5 | UNVERIFIED item |
- [L644](ext/authoring.md#L644): compensating checks for the `forbid` exemption: every ext-workspace member has `crate-type = ["cdylib"]` only, and no body-workspace member depends on an ext crate; justify the exemption by the Wasm boundary alone, since "wit-bindgen's generated exports are no

## ext/host.md (6)

- [L177](ext/host.md#L177): `total_core_instances` | 256 | 1000 | A Rust `wasm32-wasip2` component has the main module plus wit-component shim and fixup modules: about 3–5 core instances (UNVERIFIED exact count; test H2 prints it). |
- [L260](ext/host.md#L260): The generated names `v1::Tool`, `v1::ToolPre`, `Tool::add_to_linker::<T, HasSelf<T>>`, `call_describe` and `call_call` follow the pattern of the `bindgen_examples` in the v49.0.3 docs (`HelloWorld`, `HelloWorldPre`, `add_to_linker::<_, HasSelf<_>>`, `call_gree
- [L261](ext/host.md#L261): **`#![forbid(unsafe_code)]` and `bindgen!`.** The macro's output contains `unsafe impl` blocks (for example from the `ComponentType` derives). rustc normally does not report `unsafe_code` inside expansions of macros from other crates. Forum reports confirm thi
- [L768](ext/host.md#L768): Cost of recompiling at load, for a 150-line extension.** All estimates are UNVERIFIED; Phase 0 measures them with `compile_ms`, which the host records per load.
- [L1020](ext/host.md#L1020): **Rank 3 (tool path):** per-call host overhead. All figures are UNVERIFIED estimates for the M2; test H15 measures them.
- [L1039](ext/host.md#L1039):   After the first WASI I/O call, add the lazily built tokio runtime (§3.2), roughly 1–4 MB RSS for its worker threads (UNVERIFIED). Pool slots are decommitted on deallocation (`linear_memory_keep_resident = 0`, the default), so RSS returns to baseline after

## ext/wit.md (6)

- [L227](ext/wit.md#L227): The export side takes owned values. Whether imported record and `option<string>` parameters are borrowed (`&RunRequest`, `Option<&str>`) is UNVERIFIED for 0.63.0. The compiler error tells the author at once, and the skill says so (ext/authoring.md §3.9).
- [L239](ext/wit.md#L239): Current directory | `wasi:cli/environment.initial-cwd` | `/work` | Prefer absolute paths (`/work/src/x`). Whether wasi-libc resolves relative paths against `initial-cwd` is UNVERIFIED. |
- [L320](ext/wit.md#L320): Allowed JSON Schema keywords** (draft 2020-12 subset), chosen so that the same schema is also accepted as a first-class tool by Anthropic `input_schema`, OpenAI function `parameters` and DeepSeek's OpenAI-compatible tools: `type`, `properties`, `required`, `it
- [L444](ext/wit.md#L444): W-3 | A guest that does not call `process` does not import it | import list has no `init:ext/process` (UNVERIFIED that wasm-component-ld drops unused world imports; if it does not, inventory still lists it, so this test records which) | inventory accuracy |
- [L463](ext/wit.md#L463): **Q-W3. WASI 0.3 / async.** `wasm32-wasip3` is tier 3 on stable Rust 1.99 and tier 2 from Rust 1.100 (nightly docs, checked 2026-10-19; release date about 2026-11-12, UNVERIFIED). Revisit as `init:ext@2.0.0` once wit-bindgen's async exports are stable and the 
- [L498](ext/wit.md#L498): Agent tool-schema constraints: provider docs as cited by body/tools.md (UNVERIFIED here).

## kernel/gate.md (1)

- [L717](kernel/gate.md#L717): Notes on a bare repo use the same CLI; that `git notes add` works in a bare repository is UNVERIFIED in docs, and test G-30 checks it on git 2.39.5.

## kernel/initctl.md (2)

- [L82](kernel/initctl.md#L82): **Linux:** `getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &ucred, &len)` gives `{pid, uid, gid}` as of `connect()`. The pid is reported in the kernel's own pid namespace (the kernel uses `pid_vnr` for this option; UNVERIFIED against the man page, covered by test IC-
- [L119](kernel/initctl.md#L119): Residual race.** A pid read at `connect()` could in theory be reused by another process before step 3 runs (microseconds). Removing it: `LOCAL_PEERTOKEN` (audit token) with `sandbox_check_by_audit_token` on macOS (exported from libsystem_sandbox; signature UNV

## kernel/proxy.md (10)

- [L334](kernel/proxy.md#L334): **HTTP/2 when ALPN agrees.** `api.deepseek.com` accepted `h2` in ALPN on 2026-10-19 (`curl -v --http2`, served via CloudFront). With h2 one TCP+TLS connection multiplexes all concurrent requests, so a G4 run with 4 parallel bodies shares one handshake. `http2_
- [L335](kernel/proxy.md#L335): **Keep-alive across body restarts.** A body restart (T2 handoff, crash) does not touch the proxy, so the next request reuses the warm connection: no DNS, TCP or TLS handshake. h2 PINGs every 30 s with `keep_alive_while_idle` keep idle connections alive through
- [L384](kernel/proxy.md#L384): Response header `x-should-retry: false` | never retry (the official SDKs obey it; whether servers send it is UNVERIFIED) |
- [L510](kernel/proxy.md#L510): DeepSeek's `/anthropic` endpoint ignores `cache_control` and caches automatically. Which usage fields it returns is UNVERIFIED; the parser takes whatever subset arrives and flags `usage_partial` if `input_tokens` is missing.
- [L531](kernel/proxy.md#L531): **Time-of-day windows (DeepSeek).** DeepSeek prices peak and off-peak differently (§4.8.1). Which instant DeepSeek uses to classify a request is UNVERIFIED; the proxy uses admission time. Chinese public holidays are off-peak, but the proxy has no holiday cale
- [L535](kernel/proxy.md#L535): **No usage.** On abort, timeout or a stream error without a usage object, the cost is an estimate: `input = ceil(request_bytes / 3)` at `price.input`, plus `output = ceil(content_bytes_seen / 3)` at `price.output`, capped at the reservation, with flag `usage_e
- [L719](kernel/proxy.md#L719): **URL and auth.** `wss://api.openai.com/v1/responses` with `Authorization: Bearer <key>` on the upgrade request. Both are taken from the official SDK source; the guide does not spell them out (UNVERIFIED in docs). Client: `tokio-tungstenite` 0.30.0 `client_asy
- [L946](kernel/proxy.md#L946): Context / output | 1M context; `max_tokens` 1..393,216 (384K). If unset: 8K non-thinking, 64K thinking, 128K with `reasoning_effort: "max"`. Whether "1M" is 1,000,000 or 1,048,576 is UNVERIFIED; `GET /models` returns `context_window` and `max_output_tokens`, a
- [L994](kernel/proxy.md#L994): The request field `diagnostics: {previous_message_id}` returns `cache_miss_reason` (whether a beta header is needed is UNVERIFIED).
- [L1000](kernel/proxy.md#L1000): `retry-after` is on some 429s; on 529 it is UNVERIFIED.

## kernel/sandbox.md (5)

- [L331](kernel/sandbox.md#L331): The socket files are bind-mounted individually. They are recreated only when the kernel restarts, and a kernel restart sweeps every sandbox anyway, so a stale mount can never be observed. `connect(2)` to a socket on a read-only bind mount needs write permissio
- [L401](kernel/sandbox.md#L401): **x32 ABI:** seccompiler 0.5.0 does not handle the x32 syscall bit. A kernel built with `CONFIG_X86_X32_ABI` could let x32 syscall numbers bypass a deny list. UNVERIFIED; `doctor` warns when `/proc/config.gz` or `/boot/config-*` shows `CONFIG_X86_X32_ABI=y`.
- [L435](kernel/sandbox.md#L435): macOS version | 14.x tested (2026-10-19). 15.x and 26.x UNVERIFIED: run §8 on each before use | refuse until §8 passes |
- [L438](kernel/sandbox.md#L438): Linux bwrap | `[sandbox] bwrap` exists; version ≥ 0.8.0 (UNVERIFIED that every flag used exists since then; `doctor` probes each flag with a dry run). bubblewrap ≥ 0.12.0 dropped setuid mode, so unprivileged user namespaces are mandatory | refuse to start 
- [L465](kernel/sandbox.md#L465): **No CPU or memory limits on macOS** (no cgroups). A fork bomb is limited only by `kern.maxprocperuid`, which applies to the owner's whole session. Linux: optional `[sandbox] linux_memory_max` wraps bwrap in `systemd-run --user --scope -p MemoryMax=<v>` (UNVER

## kernel/selfmap.md (5)

- [L126](kernel/selfmap.md#L126): The count is a **measure**, not the bill. Different frontier tokenizers differ on code by roughly ±15% (UNVERIFIED for DeepSeek V4.1). To relate the two, the proxy observes the real `prompt_tokens` of the head for the active model. A calibration ratio `model_
- [L184](kernel/selfmap.md#L184): The names of the synthetic root package and world that `decode` produces are UNVERIFIED. They are not stored; only exports, imports and `wit_sha256` are.
- [L199](kernel/selfmap.md#L199): // impl WasiView for Host { fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> { … } }   (exact 48.x shape UNVERIFIED)
- [L205](kernel/selfmap.md#L205):     wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;          // name per 48.x: UNVERIFIED
- [L542](kernel/selfmap.md#L542): `o200k_base` initialization: about 100–300 ms the first time per process (UNVERIFIED; measured in Phase 0);

## kernel/supervise.md (4)

- [L292](kernel/supervise.md#L292):    std 1.97.1 uses `posix_spawnp` for this shape: no `pre_exec` closure, no uid/gid, and the program is a path, so changing `PATH` through `env` does not force the fork path. `process_group(0)` maps to `POSIX_SPAWN_SETPGROUP` with pgroup 0, and the child leads
- [L399](kernel/supervise.md#L399): Hit: `cache/deps/<key>/target/` is cloned to `cache/build/slot/out/target/`. macOS: one `clonefile(2)` call (APFS clones directory trees; `libc::clonefile`, UNVERIFIED that libc 0.2.191 exports it for Apple, else declare it `extern "C"`). Linux: `cp -a --refli
- [L655](kernel/supervise.md#L655):   2. Repeat up to 50 times, 20 ms apart, until a full pass kills nothing: list all pids of the kernel's uid (`proc_listpids(PROC_UID_ONLY, uid)`, or `sysctl {CTL_KERN, KERN_PROC, KERN_PROC_UID, uid}`; UNVERIFIED that libc 0.2.191 exposes `proc_listpids`, else 
- [L1229](kernel/supervise.md#L1229): Apple: code signing and in-place overwrites, Quinn "The Eskimo!" — https://developer.apple.com/forums/thread/130313 ; "Updating Mac Software" — https://developer.apple.com/documentation/security/updating_mac_software (page body not readable without JS; UNV

## roadmap.md (4)

- [L177](roadmap.md#L177): `wasm32-wasip2` target | `~/.rustup/toolchains/…/rustlib/wasm32-wasip2` | 0.1–0.3 GB | UNVERIFIED; measured in WP0 |
- [L178](roadmap.md#L178): Vendored seed deps | `seed/vendor/` | 0.3–0.6 GB | UNVERIFIED (wasmtime 48 + cranelift + wasmtime-wasi tree) |
- [L180](roadmap.md#L180): Seed target dir (debug, deps at opt-level 2) | `seed/target/` | 2–4 GB | wasmtime + cranelift; UNVERIFIED |
- [L223](roadmap.md#L223): E1 proxy/gate (CONVENTIONS §6) + ET-E1 (evolution-testing) | Money: `gate_usd`, `day_usd`, `evolution_share`; Phase 0 test-day cap, day boundary, hard total | `"3.00"`, `"5.00"`, 0.6, G4 off-peak; Phase 0 test days `day_usd = "10.00"`, `day_starts_at_utc_offs

## testing.md (6)

- [L126](testing.md#L126): With `tools` present and thinking not disabled, every earlier `assistant` message carries `reasoning_content` | DeepSeek returns HTTP 400 otherwise (thinking-mode guide); the most likely adapter bug, caught at $0. DeepSeek's exact error body is UNVERIFIED; the
- [L136](testing.md#L136): `cache_read` = u × ⌊⌈LCP / 4⌉ / u⌋ tokens with u = `--cache-unit-tokens` (default 64), where LCP is the longest common byte prefix between R and any earlier R in the same run with the same model. DeepSeek documents that a hit needs a full match of a p
- [L310](testing.md#L310): `@n` fires on the n-th time the point is reached (default 1). `abort` calls `std::process::abort()` (closest to `kill -9` from inside). `enospc` makes the next file write at that point return `io::ErrorKind::StorageFull` (stable; verified to compile on the loc
- [L522](testing.md#L522): Included minutes: 2,000/month on GitHub Free, 3,000 on Pro. Whether macOS minutes still consume the free quota at 10× is not stated on the current pages (UNVERIFIED); the price ratio (≈ 10.3×) suggests the same effect. Cache: 10 GB per repo, entries unused
- [L536](testing.md#L536): Linux sandbox tests on hosted runners: Ubuntu 24.04 restricts unprivileged user namespaces through AppArmor, which can break `bwrap`; sandbox.md runs the selftest "with the AppArmor profile". The first CI task verifies whether a sysctl change is still needed (
- [L537](testing.md#L537): Pin every action by commit SHA. Use a Rust cache action (e.g. `Swatinem/rust-cache`, version to pin at build time, UNVERIFIED) keyed on `Cargo.lock` of each workspace. The seed's dependencies are vendored, so only target dirs are cached.

