# The extension host (`ext_host.rs`)
Status: v0.3 + build sync 2026-10-20 · Area: ext
Depends on: spec v0.2 (§2.1, §4, §5, §6.1, §7.4, §9, §10), CONVENTIONS.md, ext/wit.md, ext/authoring.md, evolution.md (§3.4), body/loop.md, body/tools.md, body/boot.md, body/log-context.md, kernel/selfmap.md, kernel/sandbox.md, kernel/supervise.md   Defines: the extension host (Wasmtime): engine `Config` and pooling limits, `StoreLimits`, the linker and per-call WASI context, the store lifecycle, the registry, the hot-swap protocol, `ext` dispatch, the epoch ticker and deadlines, trap and timeout mapping, the `init:ext/process` host, the compilation-cache decision, the `state/ext/` files (`catalog.json`, `log`), the reload report, the `init-body ext` subcommands (proposal), and the WIT-derived inventory inputs for selfmap (proposal)

---

## 1. Purpose

The extension host runs T1 extensions: WebAssembly components that the agent writes in Rust, builds for `wasm32-wasip2`, and calls through the fixed `ext` tool. It has to:
- start a call in well under a millisecond;
- stop any call that runs too long or uses too much memory;
- let the agent replace an extension between turns without restarting the body;
- do all of this inside a crate that has `#![forbid(unsafe_code)]`.

This doc settles every value and every API call needed to build it.

## 2. Responsibilities / non-goals

**Responsibilities:**
- One Wasmtime `Engine` per body process, with the exact config in §3.3.
- Loading components from disk. Validating them against `init:ext` (ext/wit.md) and building `InstancePre`s off the call path.
- Holding the registry of loaded extensions and swapping it atomically at turn boundaries.
- Running each `ext` call in a fresh `Store`. Enforcing deadlines, memory and table limits. Turning traps into tool results.
- Implementing the `init:ext/process` host interface.
- Keeping `state/ext/catalog.json` and per-extension logs. Producing the reload report.
- Exposing the same machinery as `init-body ext …` subcommands for local testing (proposal, §4.5).

**Non-goals:**
- The `ext` tool's JSON schema and result spill policy (body/tools.md).
- When reloads are requested and where events go (body/loop.md, body/log-context.md). This doc gives the API the loop calls.
- Building extensions (ext/authoring.md) and keeping them (evolution.md, kernel/gate.md).
- The `self.json` format (kernel/selfmap.md). §3.15 lists what can be read from a binary.
- Security perimeter. The OS sandbox is the perimeter (kernel/sandbox.md); Wasm gives fault isolation and resource limits.

## 3. Design

### 3.1 Overview

```
        loop thread                        ExtHost (one per body process)
  ┌──────────────────────┐   begin_reload   ┌───────────────────────────────────────────────┐
  │ turn boundary k      │ ───────────────▶ │ loader thread "ext-loader"                    │
  │                      │   commit_reload  │   scan body/ext/*, bin/ext/*, work/target/... │
  │                      │ ◀─────────────── │   Component::from_binary (Cranelift + incr.   │
  │ append `reload` event│    ReloadReport  │   cache) → linker.instantiate_pre → ToolPre   │
  │ model request k+1 …  │                  │   describe() ×2 (no caps) → validate          │
  │                      │                  │   → staged set                                │
  │ tool batch k+1:      │    snapshot()    │ current: Mutex<Arc<Registry>>  (gen n → n+1)  │
  │  ext{name,input} ────┼─────────────────▶│ call(): spawn worker thread "ext-<name>"      │
  │  (scoped thread)     │   ExtOutcome     │   Store::new → limiter → epoch deadline       │
  │                      │ ◀────────────────│   → pre.instantiate → call_call → drop Store  │
  └──────────────────────┘                  │ epoch ticker "ext-epoch" (10 ms while busy)   │
                                            │ ProcHost: sessions (pipes), run helper        │
                                            └───────────────────────────────────────────────┘
```

### 3.2 Dependencies, version and Cargo features

**Version: `wasmtime` and `wasmtime-wasi` `=48.0.6`.**
- Wasmtime makes every 12th release an LTS, supported for 24 months. 48.0.0 (2026-08-20) is the current LTS, so it gets security backports until about 2028-08.
- The latest normal release is 49.0.3 (2026-10-14), and 50.0.0 is due 2026-10-20. Normal releases get about 2 months of support.
- 48.0.6 (2026-10-14) contains the October 2026 security fixes, including GHSA-m9f7-9w4p-gv38 (a filesystem sandbox escape).
- MSRV is 1.95.0; the local toolchain is 1.97.1.
- Zed and Wassette both run on 48.
- An LTS also fits the vendoring model: every `Cargo.lock` change forces full G4 (spec §6.3), so fewer API-moving upgrades is better.
- The APIs used here were checked in the v48.0.6 source. Where only v49.0.3 was read, the reference says so.

Upgrade policy:
- Patch releases of 48.x go in through `initctl vendor` when a Wasmtime security advisory affects a feature we compile in (full G4; assumed to need no owner approval: PENDING OWNER (CONVENTIONS §6 ESC-1)).
- The next LTS (60.0.0, due about 2027-08-20) is a planned T2 change.

```toml
# body/Cargo.toml (excerpt; the body crate owns this file)
[dependencies]
wasmtime = { version = "=48.0.6", default-features = false, features = [
    "runtime", "std", "cranelift", "component-model",
    "pooling-allocator", "parallel-compilation", "incremental-cache", "demangle",
] }
wasmtime-wasi = { version = "=48.0.6", default-features = false, features = ["p2"] }
```

| Feature | On? | Why |
|---|---|---|
| `cranelift` | on | the only compiler |
| `winch` | **off** | CVE-2026-34987 (GHSA-xx5w-cvp6-jv83, critical, 2026-04-10). Winch could let guest Wasm read or write host memory outside its sandbox. There is a working proof of concept **on aarch64**, which is the M2's architecture. Leaving the feature out means `Strategy::Winch` cannot even be selected. |
| `component-model` | on | components |
| `component-model-async` | **off** | not needed (WASI p2, sync). Removes the code behind several October 2026 advisories, among them the critical GHSA-32h6-97mm-8q3c (async-lifted callback result count unvalidated). |
| `pooling-allocator` | on | spec §4 |
| `parallel-compilation` | on | rayon-parallel Cranelift; cuts reload compile time (§3.14). Brings `rayon` as a transitive dependency (a CPU thread pool used only while compiling). |
| `incremental-cache` | on | in-memory Cranelift function cache (§3.14) |
| `cache` | **off** | on-disk cache; rejected (§3.14). Also drops `zstd` and the cache worker thread. |
| `demangle` | on | readable Rust symbol names in trap backtraces shown to the agent |
| `gc`, `threads`, `stack-switching`, `wat`, `profiling`, `coredump`, `debug-builtins`, `addr2line`, `backtrace`, `anyhow`, `debug`, `compile-time-builtins`, `wit-parser` | off | not needed; less build time and less attack surface (GC had two heap-corruption advisories in October 2026) |
| `async` | on, implied | `wasmtime-wasi/p2` enables `wasmtime/async`. The body does not use async. |

**`wasmtime-wasi` brings tokio.** Its `Cargo.toml` makes `tokio` (with `rt-multi-thread`, `net`, `time`, …) an unconditional dependency. Its synchronous shims run every async-implemented WASI host call (filesystem, streams, poll) through `in_tokio`, which does `RUNTIME.block_on(…)` on a lazily built multi-thread runtime (`runtime.rs`: `RUNTIME: LazyLock<tokio::runtime::Runtime>`; verified in v48.0.6). So:
- body code stays synchronous;
- but the process gains a tokio worker pool (by default one thread per core) the first time an extension makes such a call, and every such call passes through one `block_on`.

`allow_blocking_current_thread(true)` (§3.6) removes the *extra* hop: filesystem operations then run on the calling thread inside that `block_on` instead of on a `spawn_blocking` thread. It does not avoid the runtime itself.

CONVENTIONS §5 (v0.2) accepts this explicitly: there is no async code in the body, and the tokio runtime `wasmtime-wasi` builds internally (and Wasmtime's rayon pool during compilation) are internals of allowed dependencies. Recorded as spec delta §10.3.

### 3.3 Engine `Config`

```rust
use std::num::NonZeroUsize;
use std::sync::Arc;
use wasmtime::{Config, Engine, InstanceAllocationStrategy, OptLevel, PoolingAllocationConfig, Strategy};

fn new_engine(incr: Arc<MemCacheStore>) -> wasmtime::Result<Engine> {
    let mut cfg = Config::new();
    cfg.strategy(Strategy::Cranelift)               // never Winch (feature not compiled)
        .cranelift_opt_level(OptLevel::Speed)      // default, explicit
        .wasm_component_model(true)
        .epoch_interruption(true)
        .consume_fuel(false)
        .allocation_strategy(InstanceAllocationStrategy::Pooling(pool_config()))
        .memory_reservation(4 << 30)               // 4 GiB: lets Cranelift drop bounds checks for 32-bit memories
        .memory_guard_size(32 << 20)               // 32 MiB (default)
        .memory_init_cow(true)                     // default; effective only on Linux for in-memory components
        .max_wasm_stack(512 << 10)                 // 512 KiB (default)
        .parallel_compilation(true)
        .concurrency_support(false)                // no component-model async; see note
        .wasm_backtrace_max_frames(NonZeroUsize::new(20))
        .macos_use_mach_ports(true)                // default; the body never forks without exec
        .signals_based_traps(true);                // default
    cfg.enable_incremental_compilation(incr)?;     // in-memory only (§3.14)
    Engine::new(&cfg)
}
```

| Setting | Value | Note |
|---|---|---|
| `strategy` | `Strategy::Cranelift` | §3.2 |
| `cranelift_opt_level` | `Speed` | Guests are already LLVM-optimized (`opt-level = "s"`); measure `None` in Phase 0 only if compile time matters. |
| `epoch_interruption` | `true` | about 10% guest slowdown (Wasmtime docs); deadline mechanism (§3.11) |
| `consume_fuel` | `false` | Fuel is deterministic but slower (2–3× in Wasmtime's own measurements); nothing in INIT needs determinism here. |
| `allocation_strategy` | pooling, §3.4 | |
| `memory_reservation` / `memory_guard_size` | 4 GiB / 32 MiB | Virtual address space only: 32 slots ≈ 130 GiB of virtual reservation, no RSS. See Q-H5 for `RLIMIT_AS`. |
| `memory_init_cow` | `true` | On macOS, copy-on-write only works for modules loaded with `deserialize_file`, which is `unsafe` and unused. So macOS copies data segments on each instantiation (§6). |
| `max_wasm_stack` | 512 KiB | Worker threads get 2 MiB native stacks (std default), comfortably above this. |
| `concurrency_support` | `false` | Defaults to `true`. Disabling it skips Store data structures for component-model async, which we do not compile. Accepted by `Engine::new` and `add_to_linker_sync` on 48.0.6 (test H1, WP14), so the line stays. |
| `wasm_backtrace_max_frames` | 20 | default |
| `macos_use_mach_ports` | `true` | Mach ports do not survive `fork()` without `exec`. The body only `posix_spawn`s or fork-execs children and never runs Wasm in a forked child. |
| `enable_incremental_compilation` | in-memory store, 32 MiB cap | §3.14 |
| GC, threads, exceptions, stack switching | absent (features off) | If `Engine::new` reports a proposal that is enabled by default but not compiled in, disable it explicitly (`wasm_gc(false)` etc.) and record it. |

**Never called** (all `unsafe`, so impossible under `#![forbid(unsafe_code)]` anyway): `cranelift_flag_enable`, `cranelift_flag_set`, `detect_host_feature`, `x86_float_abi_ok`, `with_host_memory` (needs an `unsafe trait MemoryCreator`).

### 3.4 Pooling allocator and `StoreLimits`

```rust
fn pool_config() -> PoolingAllocationConfig {
    let mut p = PoolingAllocationConfig::new();
    p.total_component_instances(32)
        .total_core_instances(256)
        .total_memories(32)
        .total_tables(128)
        .total_stacks(32)                      // used only by async calls; cfg(feature = "async") is on
        .max_component_instance_size(1 << 20)  // 1 MiB (default)
        .max_core_instance_size(1 << 20)       // 1 MiB (default)
        .max_core_instances_per_component(16)
        .max_memories_per_component(2)
        .max_tables_per_component(8)
        .max_memories_per_module(1)            // default
        .max_tables_per_module(2)
        .table_elements(20_000)                // default
        .max_memory_size(256 << 20);           // 256 MiB per linear memory
    p
}
```

| Pool limit | Value | Wasmtime default | Reason |
|---|---|---|---|
| `total_component_instances` | 32 | 1000 | One per in-flight call. A batch rarely exceeds 8 parallel calls, plus up to 8 stuck workers (§3.12) and up to 2 `describe` instantiations during a load. Exhaustion becomes a `busy` result (§3.10). |
| `total_core_instances` | 256 | 1000 | A Rust `wasm32-wasip2` component has the main module plus wit-component shim and fixup modules: 3 core instances, 1 memory and 2 tables (measured by test H2 on `count-lines`, WP14). |
| `total_memories` | 32 | 1000 | One linear memory per component instance |
| `total_tables` | 128 | 1000 | Main module table plus shim tables |
| `max_core_instances_per_component` | 16 | unlimited | Caps a single pathological component |
| `max_memories_per_component` | 2 | unlimited | Rust components have 1; headroom for one adapter |
| `max_tables_per_component` | 8 | unlimited | |
| `max_tables_per_module` | 2 | 1 | Headroom for toolchains that emit a second funcref table |
| `max_memory_size` | 256 MiB | 4 GiB | Hard ceiling per memory. Must be ≤ `memory_reservation`. |

Per-store limits (`StoreLimitsBuilder`, installed with `Store::limiter`):

| `StoreLimits` | Value | Wasmtime default |
|---|---|---|
| `memory_size` | 256 MiB | unlimited |
| `table_elements` | 20,000 | unlimited |
| `instances` | 16 | 10,000 |
| `tables` | 16 | 10,000 |
| `memories` | 2 | 10,000 |
| `trap_on_grow_failure` | `false` | `false` |

With `trap_on_grow_failure(false)`, a denied `memory.grow` returns −1. Rust's allocator then calls `handle_alloc_error`, which prints `memory allocation of N bytes failed` to stderr and aborts (an `unreachable` trap). A thin wrapper records the denial, so the tool result can say "out of memory" instead of "panic":

```rust
use wasmtime::{ResourceLimiter, StoreLimits, StoreLimitsBuilder};

pub(crate) struct CallLimiter { inner: StoreLimits, pub memory_denied: bool }

impl CallLimiter {
    fn new() -> Self {
        let inner = StoreLimitsBuilder::new()
            .memory_size(256 << 20).table_elements(20_000)
            .instances(16).tables(16).memories(2)
            .trap_on_grow_failure(false).build();
        CallLimiter { inner, memory_denied: false }
    }
}

impl ResourceLimiter for CallLimiter {
    fn memory_growing(&mut self, cur: usize, desired: usize, max: Option<usize>) -> wasmtime::Result<bool> {
        let ok = self.inner.memory_growing(cur, desired, max)?;
        if !ok { self.memory_denied = true; }
        Ok(ok)
    }
    fn table_growing(&mut self, cur: usize, desired: usize, max: Option<usize>) -> wasmtime::Result<bool> {
        self.inner.table_growing(cur, desired, max)
    }
    fn instances(&self) -> usize { self.inner.instances() }
    fn tables(&self) -> usize { self.inner.tables() }
    fn memories(&self) -> usize { self.inner.memories() }
}
```

Host-side memory that `StoreLimits` does not cover:
- the WASI resource table;
- wasmtime-wasi's per-read buffers (at most 64 KiB each, `MAX_READ_SIZE_ALLOC`);
- the 1 MiB log buffer;
- `process` output buffers (§3.13).

All of these are bounded by the call deadline or by explicit caps.

### 3.5 Linker and bindings

```rust
mod v1 {
    wasmtime::component::bindgen!({
        path: "wit/init-ext/v1",       // body/wit/init-ext/v1/init-ext.wit (copy of kernel/wit, G1-checked)
        world: "init:ext/tool",
    });
}

use wasmtime::component::{HasSelf, Linker};

fn new_linker_v1(engine: &Engine) -> wasmtime::Result<Linker<CallState>> {
    let mut l: Linker<CallState> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut l)?;                                // all of wasi:cli, clocks, filesystem, io, random, sockets
    v1::Tool::add_to_linker::<CallState, HasSelf<CallState>>(&mut l, |s| s)?;      // init:ext/process@1.0.0
    Ok(l)
}
```

- One `Linker` per supported major (`v1` now; `v2` added beside it when that major ships; ext/wit.md §3.6). The linkers are built once in `ExtHost::new` and shared by the loader thread. `Linker<T>` is `Send + Sync` when `T` is.
- `add_to_linker_sync` links the synchronous variants. No async host function is defined, so `InstancePre::instantiate` (the sync path) is valid. Wasmtime checks the asyncness of imports when instantiating.
- `define_unknown_imports_as_traps` is **not** used. A component that imports something we do not provide (for example `wasi:http`) must fail at reload, with the import named, not later at call time.
- The generated names `v1::Tool`, `v1::ToolPre`, `Tool::add_to_linker::<T, HasSelf<T>>`, `call_describe` and `call_call` follow the pattern of the `bindgen_examples` in the v49.0.3 docs (`HelloWorld`, `HelloWorldPre`, `add_to_linker::<_, HasSelf<_>>`, `call_greet`). WP14 compiled them for this world, and the names are as listed: `Tool`, `ToolPre`, `Tool::add_to_linker::<CallState, HasSelf<CallState>>`, `call_describe` and `call_call(&mut store, &str)`.
- **`#![forbid(unsafe_code)]` and `bindgen!`.** The macro's output contains `unsafe impl` blocks (for example from the `ComponentType` derives). rustc normally does not report `unsafe_code` inside expansions of macros from other crates. Forum reports confirm this as a known gap, and it is why `forbid` accepts the macro. Test H0 confirmed this at WP14: the body compiles with `bindgen!` under `#![forbid(unsafe_code)]`. Fallback if that ever fails: drop `bindgen!` and use the dynamic API, which is entirely safe:
  - `Linker::instance("init:ext/process@1.0.0")?.func_new(...)` for the imports;
  - `Instance::get_func` with `Func::call(&mut store, &[Val::String(input)], &mut results)` for the exports;
  - about 80 more lines.

### 3.6 Store lifecycle and where state lives

**Decision: one fresh `Store` and one instance per call.** The `Store` is dropped when the call ends, which frees the instance, its memory and its pool slot. Wasmtime never frees an instance before its `Store` is dropped, so this is also what keeps reloads leak-free.

| | Per call (chosen) | Per extension, long-lived (Zed) |
|---|---|---|
| State between calls | files in `/data`, host-owned process sessions | anything in linear memory |
| After a trap | nothing to clean up; the next call is fresh | the instance is poisoned (`CannotEnterComponent`) and must be rebuilt |
| Memory over time | flat; dropped every call | grows with the guest's heap; needs its own limits and leak handling |
| Concurrency | calls to one extension run in parallel | calls are serialized (Zed sends them through a channel) |
| Swap | trivial: new calls use the new `InstancePre` | must migrate or drop live state |
| Cost | one instantiation per call, tens of µs (§6) | none per call |

Per call wins on every axis except a cost that is negligible against model latency. It matches Spin's and Wassette's instance-per-request model, and the BEAM-style "two versions coexist, new calls go to the new one" semantics that the research note recommends.

**Where state lives:**
- Persistent extension state: `/data` → `$INIT_HOME/state/ext/<name>/data/`.
- Long-lived processes: the host's session table, keyed by extension name (§3.13).
- Nothing in Wasm memory survives a call.

```rust
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;

pub(crate) struct CallState {
    wasi: WasiCtx,
    table: ResourceTable,
    pub limits: CallLimiter,
    pub ext: Arc<str>,             // extension name
    pub deadline: Instant,         // wall-clock end of this call
    pub cancel: Arc<AtomicBool>,   // set by the dispatcher on owner abort (§3.10)
    pub caps: Caps,                // Caps::Full for `call`, Caps::None for `describe`
    pub roots: Option<GuestRoots>, // host paths of /work and /data (None for describe)
    pub procs: Arc<ProcHost>,      // §3.13
}
pub(crate) enum Caps { Full, None }
pub(crate) struct GuestRoots { pub work: PathBuf, pub data: PathBuf }

impl WasiView for CallState {
    fn ctx(&mut self) -> WasiCtxView<'_> { WasiCtxView { ctx: &mut self.wasi, table: &mut self.table } }
}

fn wasi_ctx(roots: Option<&GuestRoots>, log: &MemoryOutputPipe) -> wasmtime::Result<WasiCtx> {
    let mut b = WasiCtxBuilder::new();       // defaults: stdin closed, no env, no args, no preopens,
                                             // TCP/UDP/name lookup disabled, every socket address denied
    b.stdout(log.clone())                    // stdout and stderr share one buffer (Clone shares it)
        .stderr(log.clone())
        .max_random_size(1 << 20)
        .allow_blocking_current_thread(true); // filesystem ops on this thread, no spawn_blocking hop
    if let Some(r) = roots {
        b.preopened_dir(&r.work, "/work", FsPerms::ReadWrite)?
            .preopened_dir(&r.data, "/data", FsPerms::ReadWrite)?
            .initial_cwd("/work");
    }
    Ok(b.build())
}
```

- `MemoryOutputPipe::new(LOG_CAP)` with `LOG_CAP = 1 MiB`. When it is full, `check_write` returns `Closed` and the guest's write fails (ext/wit.md §3.4).
  - A custom truncating pipe would need tokio's `AsyncWrite` trait, which `StdoutStream::async_stream` requires.
  - Naming that trait means a direct `tokio` dependency, which the body does not have.
- `state/ext/<name>/data/` is created with `create_dir_all` before the first call.
- The task workspace path comes from the caller (`CallCtx::workspace`, body/tools.md).

### 3.7 Registry

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct Registry {
    pub generation: u64,                            // +1 on every commit
    pub exts: BTreeMap<String, Arc<ExtVersion>>,    // name → active version
    pub failed: BTreeMap<String, String>,           // name → last load error (may coexist with an active older version)
    pub not_built: Vec<String>,                     // crates in body/ext with no artifact
}

pub struct ExtVersion {
    pub name: String,
    pub info: ToolInfoV,                 // validated, canonical
    pub api: u32,                        // 1 (init:ext@1)
    pub origin: Origin,                  // Kept (bin/ext) | Trial (work/target)
    pub artifact: PathBuf,
    pub fp: Fingerprint,                 // change detection
    pub compile_ms: u32,
    pub loaded_at: SystemTime,
    pre: Pre,
}
pub enum Origin { Kept, Trial }
enum Pre { V1(v1::ToolPre<CallState>) }  // + V2(..) when init:ext@2 exists

#[derive(Clone, PartialEq, Eq)]
pub struct ToolInfoV {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value, // parsed; serialized canonically (sorted keys, compact)
    pub version: String,
    pub timeout_ms: u32,                 // after defaulting and clamping
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint { pub len: u64, pub mtime_ns: u128, pub hash: u64 }
```

- `Fingerprint.hash` is `std::hash::DefaultHasher` (SipHash, fixed keys) over the bytes. It is used only to detect changes inside one process, so a hasher that is stable within one Rust release is enough. It avoids a `sha2` dependency. The kernel computes its own SHA-256 if it needs one.
- The active registry lives in `Mutex<Arc<Registry>>`. A reader clones the `Arc` under the lock, which takes nanoseconds. A commit replaces the `Arc`. No `arc-swap` crate is needed (body dependencies are fixed; CONVENTIONS §5).
- `ExtVersion` is shared through `Arc`. A call holds its `Arc<ExtVersion>` for its whole life, so an old version is freed only when its last call finishes. That is the "drain".

### 3.8 Loading pipeline

**Artifact resolution.** Paths:
- `SRC = $INIT_HOME/body/ext`
- `KEPT = $INIT_HOME/bin/ext`: a symlink, kept by supervise, to an immutable published set `bin/ext.d/<set>/` (supervise.md §3.1, §3.6.5). Components are built by the gate from the commit and never taken from git (gate.md §3.3; reconciliation E6). In a probe root (`INIT_HOME=probe/<id>`), `bin/ext` points at the candidate's set, so the probe body loads exactly what the gate built (supervise.md §3.6.3).
- `TGT = $CARGO_TARGET_DIR`, or `$INIT_HOME/work/target` if that is unset

For a name `n`, `file(n) = n.replace('-', '_') + ".wasm"` (Cargo turns `-` into `_` in library file names).

| Reload kind | Candidate names | Artifact chosen for name `n` |
|---|---|---|
| `Boot` (process start) | every `KEPT/*.wasm` (name = file stem with `_` → `-`; names never contain `_`) | `KEPT/file(n)` |
| `Request` (`initctl reload`), `SessionStart` | every directory `SRC/<n>/` containing `Cargo.toml` | the newest of `TGT/wasm32-wasip2/debug/file(n)` and `TGT/wasm32-wasip2/release/file(n)` whose mtime > `floor(n)` (a **trial**); otherwise `KEPT/file(n)` if it exists; otherwise "not built" |
| `Kept(names)` (the kernel's `reload{kind:"kept", components}` frame after it published a kept set, supervise.md §3.6.8, §4.5) | as for `Request` | as for `Request`, after setting `kept_at[n] = now` for each published name |

where `floor(n) = max(session_start, kept_at[n])`:
- `session_start` is when the current session began. `reset_trials()` sets it, and process start sets it initially.
- `kept_at[n]` is when the kernel last published `n`.

Consequences:
- A trial is used only if it was built in this session and after the last keep of that name.
- At boot and at each new session, only kept components load (evolution.md: "On the next session boundary the body loads only kept components").
- On `Request`, a kept extension whose source directory was deleted from the live tree is unloaded, so the trial view matches the working tree.

**Per candidate:**

```
1. stat → (len, mtime). If same path as the active version and same (len, mtime): unchanged; stop.
2. read bytes (refuse > 64 MiB). hash. If (len, hash) equal to the active version's: unchanged
   (record the new path/origin, no recompile); stop.
3. t0; component = Component::from_binary(&engine, &bytes)           // Cranelift + in-memory incremental cache
   compile_ms = t0.elapsed()
4. for major in [newest .. oldest]:                                    // only v1 today
       pre = linker_vN.instantiate_pre(&component) and then vN::ToolPre::new(pre)
       first success wins; if none: error "does not match init:ext@N: <error of newest attempt>"
5. info_a = describe(pre); info_b = describe(pre)                     // §3.10 with Caps::None, deadline 1 s
   if info_a != info_b → error "describe is not deterministic"
6. validate(n, info_a) per ext/wit.md §3.7 (name == n, regexes, sizes, schema keywords); canonicalize schema
7. staged[n] = ExtVersion { .. }
errors at any step → staged_failed[n] = message (the active version, if any, stays)
```

- The loader is one long-lived thread. Cranelift parallelizes inside each compile through rayon. Candidates are processed in name order, so reports are deterministic.
- `Component::from_binary` (rather than `Component::new`) rejects the WAT text format. The `wat` feature is off anyway.

### 3.9 Hot-swap protocol

The registry changes only at turn boundaries. A turn boundary is "after a tool batch's results are appended and before the next model request" (evolution.md §2); the kernel docs and initctl.md call the same point the **step boundary**. Between two commits, every `ext` call in a batch sees the same registry generation.

Per-name state machine:

```
             load ok                    commit
 Absent ──────────────▶ Staged(v) ─────────────▶ Active(v)
 Active(v) ── new bytes, load ok ─▶ Active(v)+Staged(v') ── commit ─▶ Active(v')   (v freed when its last call ends)
 Active(v) ── new bytes, load error ─────────────── commit ─▶ Active(v) + failed[n]=err
 Active(v) ── source dir removed (Request) ──────── commit ─▶ Absent
 Absent   ── load error ──────────────────────────── commit ─▶ Absent + failed[n]=err
```

Sequence:

```
input thread / loop                    ExtHost                         loader
 │ `reload` frame arrives (input thread)│                                │
 │── begin_reload(Request) ────────────▶│── notify ─────────────────────▶│ resolve, compile, pre, describe, validate
 │ … current tool batch finishes …      │                                │
 │ turn boundary k                      │                                │
 │── commit_reload(10 s) ──────────────▶│ wait on condvar (often done)   │
 │                                      │◀──────────── staged set ───────│ (typ. 30–300 ms, §3.14)
 │                                      │ new Registry{gen+1}; swap Arc  │
 │                                      │ write state/ext/catalog.json   │
 │◀── Some(ReloadReport) ───────────────│                                │
 │ append `reload` event (tier T1, components, prefix_changed=false, text)
 │ model request k+1 (prefix unchanged)
 │ tool batch k+1: snapshot() → Arc<Registry gen+1> → ext calls use v'
```

Rules:
1. **`begin_reload(kind)`** returns at once. If a load is already running, the new request is queued and replaces any queued one, so at most one load runs and at most one waits.
2. **`commit_reload(max_wait)`** waits up to `max_wait` for the pending load, then swaps and returns the report. It returns `None` if nothing is pending, or if the load is still running when `max_wait` expires; in that case the load stays pending for the next boundary.
3. **The loop's calls** (body/loop.md §3.1, §3.4 already adopt these):
   - When the kernel's `reload` frame arrives, the **input thread** calls `begin_reload(…)` at once, so compiling overlaps the rest of the current tool batch: `Request` for `reload{kind:"request"}` (`initctl reload`), `Kept(components)` for `reload{kind:"kept", components}` (sent by the kernel after it published a kept set, supervise.md §3.6.8, §4.5).
   - At the next boundary: `commit_reload(10 s)`. Append the `reload` event before the next model request. That gives the order `bash(initctl reload)` result → `reload` event → `ext` call, which testing.md A2 expects. The agent sees load errors before its next decision.
   - At every other boundary: `commit_reload(Duration::ZERO)`. This is free and picks up loads that missed their window.
   - Before dispatching a tool batch that contains an `ext` call while `reload_pending()` is true (in practice only right after boot): `commit_reload(60 s)`. An `ext` call never sees a half-loaded registry.
4. **Boot.** `ExtHost::new` immediately calls `begin_reload(Boot)`. The compile of the kept set overlaps the boot model round trip. `ready` does not wait for it (body/boot.md).
5. **Session start.** The loop calls `reset_trials()`, which sets `session_start = now` and queues a `SessionStart` reload.
6. **Drain.** Commits happen only between batches, so the only calls still running at a commit are abandoned workers (§3.12). They hold their `Arc<ExtVersion>`. Their `Store`s and the old `Component` are freed when they finish. Process sessions belong to the name, not the version, so they survive swaps.
7. **Atomicity.** The swap is one assignment under the registry mutex. `catalog.json` is written afterwards with write-to-temp plus `rename`. A crash between the two leaves a stale catalog, which the next commit overwrites. The catalog is informational and never read back by the host.

### 3.10 Dispatch from the `ext` tool

```rust
pub struct CallCtx<'a> {
    pub call_id: &'a str,
    pub workspace: &'a Path,
    pub abort: &'a AtomicBool,      // the loop's owner-interrupt flag (loop.md Q-L3); read, never written
}

pub fn call(&self, reg: &Registry, name: &str, input: &serde_json::Value, cx: &CallCtx) -> ExtOutcome {
    self.procs.reap_idle(Instant::now());                                   // 30-min session idle timeout
    let Some(ver) = reg.exts.get(name).cloned() else {
        return ExtOutcome::unknown(name, reg);                              // lists loaded names + descriptions, failed, not_built
    };
    if self.stuck.count(name) > 0 { return ExtOutcome::busy(&ver, "a previous call is still blocked in a host call"); }
    if self.stuck.total() >= MAX_STUCK { return ExtOutcome::busy(&ver, "too many blocked calls"); }
    let Ok(input) = serde_json::to_string(input) else { return ExtOutcome::host(&ver, "input serialization") };
    if input.len() > INPUT_MAX { return ExtOutcome::invalid(&ver, "input exceeds 1 MiB"); }

    let timeout = Duration::from_millis(ver.info.timeout_ms as u64);
    let job = Job::new(self, ver.clone(), input, cx, Caps::Full, timeout); // owns Arc<ExtVersion>, Engine clone, roots
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let flag = job.flag.clone();                                            // Arc<AtomicU8>: 0 running, 1 done, 2 abandoned
    let cancel = job.cancel.clone();                                        // Arc<AtomicBool>: set on owner abort
    self.ticker.enter();                                                    // §3.11
    let spawned = std::thread::Builder::new().name(format!("ext-{name}")).spawn(move || {
        let r = job.run();                                                  // body below
        job.ticker.exit();                                                  // job holds Arc<Ticker>
        if job.flag.compare_exchange(0, 1, SeqCst, SeqCst).is_err() {
            job.stuck.release(&job.ver.name);                               // job holds Arc<StuckTable>
        }
        let _ = tx.send(r);
    });
    if spawned.is_err() { self.ticker.exit(); return ExtOutcome::host(&ver, "thread spawn failed"); }
    let hard_end = Instant::now() + timeout + WATCHDOG_GRACE;
    let mut abort_end: Option<Instant> = None;
    loop {
        match rx.recv_timeout(EPOCH_TICK) {                                 // 10 ms slices: watch the abort flag
            Ok(r) => return self.finish(&ver, cx, r),                       // format, log, truncate (§4.4)
            Err(RecvTimeoutError::Disconnected) => return ExtOutcome::host(&ver, "worker panicked"),
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if abort_end.is_none() && cx.abort.load(SeqCst) {
            cancel.store(true, SeqCst);                                     // job's Arc<AtomicBool>, seen by the epoch callback
            abort_end = Some(now + ABORT_GRACE);                            // 200 ms for the guest to trap
        }
        if abort_end.is_some_and(|t| now >= t) || now >= hard_end {
            if flag.compare_exchange(0, 2, SeqCst, SeqCst).is_ok() { self.stuck.mark(name); }
            return if abort_end.is_some() { ExtOutcome::aborted(&ver, true) }   // still blocked in a host call
                   else { ExtOutcome::timeout(&ver, true) };
        }
    }
}
```

- A worker runs on its own `std::thread` rather than on the loop's scoped tool thread. That way a call blocked in a host function (§3.12) cannot hold up `thread::scope` for the whole batch. Spawning costs about 10–20 µs.
- **Owner abort** (loop.md Q-L3, reconciliation E1). The dispatcher waits in 10 ms slices and watches `cx.abort`. On abort it sets the job's `cancel` flag, which the guest's epoch callback (§3.11) turns into a trap within one tick, and which `process.run`/`read` also watch (§3.13). If the worker has finished within `ABORT_GRACE` (200 ms) the result is `Aborted` with the log tail, even if the guest returned `Ok` while the abort was being delivered (the owner abort wins); otherwise the worker is abandoned exactly like a timeout (§3.12) and the result is `Aborted` with "still blocked in a host call". Calls not yet dispatched when the flag is set are refused by tools.md, not here.
- `INPUT_MAX = OUTPUT_MAX = 1 MiB`. Output longer than that is cut at a `char` boundary and flagged `truncated`.
- Every call appends a record to `state/ext/<name>/log` (§4.2).

`Job::run`, the worker body:

```rust
fn run(&self) -> CallResult {
    let t0 = Instant::now();
    let log = MemoryOutputPipe::new(LOG_CAP);
    let wasi = match wasi_ctx(self.roots.as_ref(), &log) { Ok(w) => w, Err(e) => return CallResult::host(e, &log, t0) };
    let state = CallState { wasi, table: ResourceTable::new(), limits: CallLimiter::new(),
                            ext: self.name.clone(), deadline: self.deadline, caps: self.caps,
                            cancel: self.cancel.clone(), roots: self.roots.clone(), procs: self.procs.clone() };
    let mut store = Store::new(&self.engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_epoch_deadline(1);                           // check every tick (§3.11)
    store.epoch_deadline_callback(|cx| {
        let s = cx.data();
        if s.cancel.load(SeqCst) || Instant::now() >= s.deadline { Ok(UpdateDeadline::Interrupt) }
        else { Ok(UpdateDeadline::Continue(1)) }
    });
    let r = match &self.ver.pre {
        Pre::V1(pre) => pre.instantiate(&mut store).and_then(|t| match self.op {
            Op::Call => t.call_call(&mut store, &self.input).map(Ret::Call),
            Op::Describe => t.call_describe(&mut store).map(Ret::Describe),
        }),
    };
    let memory_denied = store.data().limits.memory_denied;
    drop(store);                                           // frees instance, memory, pool slot
    CallResult { r, log: log.contents(), memory_denied, elapsed: t0.elapsed() }
}
```

`describe` uses the same path with `Op::Describe`, `Caps::None`, `roots: None`, a 1 s deadline, and the loader thread waiting synchronously.

### 3.11 Epoch ticker and per-call deadlines

```rust
const EPOCH_TICK: Duration = Duration::from_millis(10);

struct Ticker { active: Mutex<usize>, cv: Condvar }

fn ticker_loop(engine: wasmtime::EngineWeak, t: Arc<Ticker>) {
    loop {
        {   // park while no call is running: zero idle wake-ups
            let mut a = t.active.lock().unwrap();
            while *a == 0 {
                a = t.cv.wait_timeout(a, Duration::from_secs(5)).unwrap().0;
                if engine.upgrade().is_none() { return; }
            }
        }
        std::thread::sleep(EPOCH_TICK);
        match engine.upgrade() { Some(e) => e.increment_epoch(), None => return }
    }
}

```

**Deadline check: an epoch callback on every tick.** Each `Store` gets `set_epoch_deadline(1)` and an `epoch_deadline_callback` (§3.10, `Job::run`) that runs once per tick (every 10 ms while the guest executes) and returns `UpdateDeadline::Interrupt` (trap) if the job's `cancel` flag is set or `Instant::now() >= deadline`, else `UpdateDeadline::Continue(1)`. This replaces a fixed tick count with `epoch_deadline_trap()`. It costs one closure call per 10 ms of guest time, and gives both the wall-clock deadline and owner abort with one mechanism. `epoch_deadline_callback(impl FnMut(StoreContextMut<T>) -> Result<UpdateDeadline> + Send + Sync + 'static)` and `UpdateDeadline::{Interrupt, Continue(u64)}` are verified in the v48.0.6 source (`Yield` variants exist only for async calls and are not used).

- One thread, `ext-epoch`, started in `ExtHost::new`. It holds an `EngineWeak` (as the Wasmtime docs recommend), so it never keeps the engine alive.
- `Ticker::enter` increments `active` and notifies. `exit` decrements it. A worker exits only when it truly finishes, so an abandoned worker keeps the ticker running until it traps or returns.
- The epoch advances only while some call is active. A running call is always active itself, so its tick count tracks its own wall time. Precision is +0–10 ms, plus the time to the guest's next epoch check (function entry or loop back-edge).
- `Engine::increment_epoch` is one relaxed atomic add (signal-safe, per its docs).
- Deadlines:

| Operation | Deadline |
|---|---|
| `call` | `tool-info.timeout-ms`, default 30,000 ms, clamped to [100, 600,000] |
| `describe` | 1,000 ms |
| wall watchdog | deadline + 2,000 ms (`WATCHDOG_GRACE`) |
| `process.run` / `process.read` wait | lowered to `deadline − now` |

- **Bulk memory caveat:** `memory.copy` and similar instructions check the epoch only once (Wasmtime `Config::epoch_interruption` docs). The 256 MiB memory cap bounds one copy to well under a second.

### 3.12 Traps, timeouts and calls blocked in the host

Epochs interrupt running Wasm only. Wasmtime's docs say they "do not assist in handling WebAssembly code blocked in a call to the host". In INIT the ways to block in the host are:
- `wasi:io/poll` on a timer, which is what `std::thread::sleep` does. This is unbounded.
- Very slow filesystem calls.
- Our own `process` functions. These are bounded: they honor the deadline.

Handling:
1. The worker's epoch callback traps the guest at its first epoch check after the host call returns (the deadline has passed by then).
2. The dispatcher waits `deadline + 2 s`. If the worker has not answered, it **abandons** it:
   - it returns a `timeout` result to the model;
   - it marks the job abandoned;
   - it increments `stuck[name]`.
3. While `stuck[name] > 0`, calls to `name` return `busy` at once. Other extensions are unaffected. Once `stuck_total ≥ 8` (`MAX_STUCK`), all `ext` calls return `busy`.
4. When the worker finally finishes, it decrements the counter and drops its `Store`. (`StuckTable` is a `Mutex<BTreeMap<String, usize>>` with `mark`, `release`, `count` and `total`, shared by `Arc` between `ExtHost` and each `Job`.)
5. A new version of the same name does not clear `stuck`, because the stuck worker still holds the old instance and a pool slot. The reload report lists stuck names.

Result mapping:

```rust
fn classify(e: &wasmtime::Error, memory_denied: bool, cancelled: bool, past_deadline: bool) -> Kind {
    if cancelled { return Kind::Aborted; }                       // owner abort wins over any trap it caused
    if memory_denied { return Kind::Oom; }
    if let Some(x) = e.downcast_ref::<wasmtime_wasi::I32Exit>() { return Kind::Exit(x.0); }
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::Interrupt) if past_deadline => Kind::Timeout,
        Some(wasmtime::Trap::Interrupt) => Kind::Trap("interrupt".into()),
        Some(wasmtime::Trap::StackOverflow) => Kind::StackOverflow,
        Some(wasmtime::Trap::UnreachableCodeReached) => Kind::Panic,   // Rust panic/abort; message is in the log
        Some(t) => Kind::Trap(t.to_string()),                          // "wasm trap: out of bounds memory access", …
        None => Kind::Host(format!("{e:#}")),                          // instantiate failure, pool exhausted, …
    }
}
```

| Kind | `is_error` | Text (prefix `ext <name> <version>: `) | Log tail shown |
|---|---|---|---|
| ok | false | the returned string | no |
| invalid input (guest) | true | `invalid input: <m>` | no |
| failed (guest) | true | `failed: <m>` | no |
| timeout | true | `timed out after <ms> ms` (+ `; the call was blocked in a host call and is still running` if abandoned) | yes |
| aborted | true | `aborted by owner interrupt` (+ `; still blocked in a host call` if abandoned) | yes |
| out of memory | true | `out of memory: linear memory limit is 256 MiB` | yes |
| stack overflow | true | `trap: stack overflow (512 KiB wasm stack)` | yes |
| panic | true | `panicked (see log below)` | yes |
| other trap | true | `trap: <display>` plus up to 20 backtrace frames | yes |
| exit | true | `called exit(<code>)` | yes |
| busy | true | `busy: <reason>` | no |
| unknown name | true | `unknown extension "<n>". Loaded: <name> — <description>; … Failed: … Not built: … (input schemas: state/ext/catalog.json)` | no |
| host error | true | `host error: <message>` (for example `instance pool exhausted (32 concurrent calls): <wasmtime message>`, which keeps Wasmtime's detail after the text) | yes |

"Log tail shown" means the last 4 KiB of the captured stdout and stderr, after a line `--- log (last 4096 bytes) ---`. A trap never disables an extension. The next call gets a fresh instance.

Two results differ from what the guest's source suggests (measured at WP14):
- **Exit.** WASI p2 `exit` takes a result, not a number, so `std::process::exit(3)` arrives as `exit(1)` and the text reads `called exit(1)`. Only 0 and 1 can be told apart.
- **Stack overflow.** Recursion with locals overflows Rust's shadow stack in linear memory first, and Wasmtime reports that as `trap: wasm trap: out of bounds memory access` (with backtrace frames). Only recursion without locals reaches the `stack overflow` row.

`Engine::new` installs process-wide trap handlers (Mach ports on macOS, signals on Linux). They apply only to faults inside Wasm code. A fault in host code is still a body crash, handled by the kernel's restart and LKG rollback (spec §7.1).

### 3.13 The `init:ext/process` host

`ProcHost` is owned by `ExtHost` and shared with every `CallState` through `Arc`.

**`run`:**
1. `argv` must be non-empty.
2. `cwd` is mapped from a guest path (`/work[/rel]` or `/data[/rel]`) to a host path:
   - reject absolute `rel`, `..` components and NUL bytes;
   - `canonicalize`;
   - require the result to start with the canonical root;
   - otherwise `denied`.
3. Build `std::process::Command`:
   - stdin, stdout and stderr piped;
   - `process_group(0)` (`std::os::unix::process::CommandExt`), so the child gets its own process group;
   - environment = the body's environment plus `env`.
4. Run it with the **same helper the `bash` tool uses**, `tools::run_group` (body/tools.md §3.5.7), which:
   - writes `stdin` on a thread;
   - captures stdout and stderr **separately** (the WIT `run-output` has two fields), keeping the last 1 MiB of each; this needs a `CaptureSpec` variant for separate streams (follow-up for tools.md, reconciliation E3);
   - waits with a waiter thread blocked in `wait()` plus `recv_timeout`, up to `min(timeout-ms, deadline − now)` (tools.md's design, adopted here instead of polling `try_wait`);
   - stops early when the job's `cancel` flag is set (owner abort; tools.md must accept a plain `&AtomicBool` as the cancel signal);
   - on timeout or cancel kills the whole process group (TERM, then KILL), then reaps.
5. Map errors:
   - `NotFound` and `PermissionDenied` on spawn → `not-found`;
   - violations of the rules above → `denied`;
   - anything else → `io`.

Killing a process group without `unsafe`: `std` has no `killpg`. The shared helper runs `/bin/kill -KILL -- -<pgid>` through `std::process::Command`, then `Child::kill` as a fallback. This works on macOS and Linux. (Owner: body/tools.md. This doc only requires its behavior.)

**Sessions:**

```rust
pub(crate) struct ProcHost {
    sessions: Mutex<BTreeMap<(String, u32), Session>>,
    next_id: Mutex<BTreeMap<String, u32>>,          // per extension, starts at 1
}
struct Session {
    child: std::process::Child,
    pgid: u32,
    stdin_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>, // writer thread owns ChildStdin
    queued: Arc<AtomicUsize>,                                // bytes queued for stdin, ≤ 4 MiB
    out: Arc<(Mutex<OutBuf>, Condvar)>,
    last_used: Instant,
}
struct OutBuf { data: VecDeque<u8>, truncated: bool, eof: bool, last_byte_at: Option<Instant> }
```

- **`spawn`:**
  - checks the limits: 4 per extension, 16 in total;
  - `let (r, w) = std::io::pipe()?` (stable since Rust 1.87);
  - `cmd.stdout(w.try_clone()?).stderr(w)`, so stdout and stderr interleave in arrival order, as in a terminal;
  - `stdin(Stdio::piped())`, `process_group(0)`, `spawn()`;
  - drops `cmd` so the parent's write ends close;
  - starts a reader thread (`r` → `OutBuf`, at most 4 MiB, oldest bytes dropped, condvar notified) and a writer thread (channel → `ChildStdin`);
  - `pty: true` → `denied("pty is not supported by this body")`.
- **`write`:** at most 1 MiB per call, at most 4 MiB queued, otherwise `denied`. `try_send` never blocks.
- **`read`:** waits on the condvar until one of the conditions in ext/wit.md §3.2 holds, with `wait-ms` lowered to the time left in the call. It wakes at least every 50 ms to check the job's `cancel` flag and returns early (with what it has) when it is set. It drains at most 1 MiB. It sets `exited` when `eof` is set and `try_wait()` returns a status.
- **`close`:** kills the group, waits for the child, joins the threads, frees the id.
- **`sessions`:** the ids of this extension, ascending.
- **Idle reaping:** at every `call` and `begin_reload`, sessions with `last_used` older than 30 min are closed. No timer thread is needed.
- **Body exit:** `ExtHost::shutdown()`, called by every orderly exit path including exit 75 (handoff), kills all session groups. Sessions do not survive a T2 handoff. If the body crashes, children lose their stdin pipe (EOF), and most REPLs exit on EOF. Cleaning up the rest is the kernel's job (Q-H4).
- **Threads:** two per session, so at most 32. Each is a plain `std::thread`.
- `describe` (`Caps::None`): every `process` function returns `denied("describe has no capabilities")`.

### 3.14 Compilation caching under `#![forbid(unsafe_code)]`

**API safety, verified in the wasmtime 48.0.6 source:**

| API | Signature | Safe to call from the body? | Trust it relies on |
|---|---|---|---|
| `Component::new`, `Component::from_binary`, `Component::from_file` | `pub fn` | yes | none: full validation and compilation |
| `Component::serialize`, `Engine::precompile_component` | `pub fn` | yes, but the output is only usable through `deserialize` | none |
| `Component::deserialize`, `deserialize_raw`, `deserialize_file` | `pub unsafe fn` | **no** (forbidden) | "The structure of the binary blob read here is only lightly validated … Arbitrary input could … replace valid compiled code with any other valid compiled code" (`Module::deserialize` docs; the component versions inherit them) |
| `Module::deserialize*`, `Module::from_trusted_file` | `pub unsafe fn` | **no** | same |
| `Config::cache(Option<Cache>)` (feature `cache`) | `pub fn` | compiles, but **hides the same trust** | On a hit, the code path in `compile/runtime.rs` calls `engine.load_code_bytes(&serialized_bytes, kind)`, the same rehydration that `deserialize` uses, on a zstd file read from the cache directory. The key is a SHA-256 of the *inputs*; the content has no integrity check. Whoever can write the cache directory can put native code into the body process. |
| `Config::enable_incremental_compilation(Arc<dyn CacheStore>)` (feature `incremental-cache`) | `pub fn`; `CacheStore` is a safe trait | yes | only the store's contents, which are in our own memory and written only by Cranelift |
| `CodeBuilder::expose_unsafe_intrinsics` | `pub unsafe fn` | no (never needed) | |
| `Config::cranelift_flag_enable` / `_set`, `detect_host_feature`, `x86_float_abi_ok`, `Engine::unload_process_handlers` | `pub unsafe fn` | no (never needed) | |

**Options:**

| Option | Verdict | Why |
|---|---|---|
| A. `.cwasm` on disk + `Component::deserialize_file` | rejected | `unsafe`, cannot compile in the body. It would also be the only way to get copy-on-write memory init on macOS. Wassette does this, gated only by "the file exists". |
| B. `Config::cache` (Wasmtime's disk cache) | rejected for the seed | The signature is safe, but the soundness precondition moves to "nobody else writes the cache dir". In INIT every directory the body can write, the agent's `bash` can write too. This is exactly the hidden-`unsafe` path that `forbid(unsafe_code)` is meant to make visible. It also adds `zstd`, a worker thread and a cache directory to manage. |
| C. Recompile at load; keep `Component` in memory for the life of the process | **chosen** | Safe, no trust. Cost estimated below. |
| D. C plus Cranelift's incremental cache in memory | **chosen** (with C) | Safe. When an edited extension is recompiled, unchanged functions (std, serde_json, wit-bindgen glue: most of the binary) are reused. Zed does exactly this, with a 32 MiB in-memory store. |
| E. Kernel precompiles into a read-only dir; body deserializes | rejected | still needs `unsafe` in the body |
| F. Winch for faster compiles | never | CVE-2026-34987 |

```rust
use std::borrow::Cow;
use wasmtime::CacheStore;

#[derive(Debug, Default)]
pub(crate) struct MemCacheStore { inner: Mutex<(BTreeMap<Vec<u8>, Vec<u8>>, usize)> }
const INCR_CACHE_BYTES: usize = 32 << 20;

impl CacheStore for MemCacheStore {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let g = self.inner.lock().ok()?;
        g.0.get(key).map(|v| Cow::Owned(v.clone()))
    }
    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        let add = key.len() + value.len();
        if g.1 + add > INCR_CACHE_BYTES { g.0.clear(); g.1 = 0; }   // simplest eviction: start over
        g.1 += add;
        g.0.insert(key.to_vec(), value);
        true
    }
}
```

**Cost of recompiling at load, for a 150-line extension.** All estimates are UNVERIFIED; Phase 0 measures them with `compile_ms`, which the host records per load.
- **Size:** the template (serde_json plus wit-bindgen) built with `opt-level = "s"` and `strip = "debuginfo"` is estimated at 150–300 KB of component, mostly code. Reference points: a std hello world is 78 KB with `-Cstrip=debuginfo` (Wasmtime docs), and 2.5 MB without it, which is why stripping matters.
- **Optimized Wasmtime** (dependencies at `opt-level ≥ 2`, §3.16): estimated **30–300 ms** on the M2 with parallel compilation. With the incremental cache warm, a recompile after an edit is estimated at **10–100 ms**, because only the leaf crate's functions change.
- **Unoptimized Wasmtime** (dev profile, `opt-level = 0` dependencies): estimated **10–30× slower**, about 0.5–5 s per component. This is the reason for §3.16.
- **Boot with N kept extensions:** N × 30–300 ms on the loader thread, overlapped with the boot model round trip (§3.9 rule 4).

**Revisit** if Phase 0 shows a median `compile_ms` above 500 ms for the reference component with §3.16 applied, or a boot load for 10 extensions above 3 s. In that order, consider:
- `cranelift_opt_level(OptLevel::None)`;
- smaller guests (`opt-level = "z"`, `lto = true`);
- option B, with an explicit ADR accepting its trust precondition.

### 3.15 The WIT-derived inventory for selfmap (what can be read from a binary)

kernel/selfmap.md owns `self.json` and has adopted this approach (selfmap.md §3.6; gate.md §3.4 runs it). This section lists the facts a T1 component yields, how to get them reproducibly, and what the kernel's `describe` host must link (answer to selfmap.md Q-X1).

**Static facts** (the kernel parses the kernel-built binary; nothing is executed):

| Fact | How | Reproducible across build paths? |
|---|---|---|
| Exports: names and WIT signatures | `wit_component::decode(&bytes)` (wit-component 0.262.0) → `DecodedWasm::Component(resolve, world)`; print the world's exports with `WitPrinter` | yes |
| Imports: interface names with versions, sorted (for example `init:ext/process@1.0.0`, `wasi:filesystem/types@0.2.x`) | same; also available as `Component::component_type().imports(&engine)` (`ComponentExtern` items, wasmtime ≥ 46) | yes |
| API major | which supported world the exports and imports type-check against (ext/wit.md §3.6) | yes |
| Uses processes? | whether `init:ext/process@1.*` is imported (if the toolchain drops unused world imports; test W-3 in ext/wit.md) | yes |
| Producers (rustc, wit-component, wit-bindgen versions) | the `producers` custom section (wasmparser 0.262.0) | yes for one toolchain, but informational only |
| SHA-256, byte size | hashing | **no**: rustc embeds absolute source paths in panic locations, so the kernel's worktree build and the agent's live-tree build differ. Keep them out of the drift-compared part of `self.json`. |

**Dynamic fact: `describe()` output.** It is the source of the tool's name, description and schema for the inventory. To keep it binary-derived without running agent code inside the kernel process:
- the generator runs as a **child process** (`init-kernel selfmap --root … --binary … --components cache/gate/<job>/ext …`, which calls `describe` through `init-kernel selfmap --describe`) under the gate's `build` sandbox profile: no network, no sockets, no `vault/` (gate.md §3.4, selfmap.md §3.6);
- it calls `describe` twice, requires equal results, validates per ext/wit.md §3.7, and emits canonical JSON.

A Wasm sandbox escape in that child cannot reach the kernel's memory.

**What the kernel's `describe` host must link (selfmap.md Q-X1, confirmed):** exactly
1. `wasmtime_wasi::p2::add_to_linker_sync` (all of WASI p2: cli including `terminal-*`, clocks, filesystem, io, random, sockets) with an **empty** `WasiCtx` (no preopens, env, args or stdio; network uses denied by default), and
2. `init:ext/process@1.0.0`, every function returning `Err(ProcessError::Denied("describe has no capabilities"))` (`sessions` returns `[]`).

Nothing else. This is the same import set the body host gives `describe` (§3.6, `Caps::None`), so a component loads in both or in neither. A component importing anything outside these (for example `wasi:http`) fails both, with the same reason. The kernel's own engine choices (fuel instead of epochs, `OptLevel::None`, a 64 MiB memory cap) are fine for a one-shot `describe` and need not match §3.3. Recommended: the kernel pins the same `wasmtime =48.0.6` (LTS) so a component never compiles in one and fails in the other; `bindgen!` path `kernel/wit/init-ext/v1`.

Proposed `self.json` fragment (sorted keys; final shape is selfmap's):

```json
{"ext": {"semver-sort": {
  "api": "init:ext@1",
  "describe": {"description": "Sort version strings by SemVer 2.0.0 precedence.",
               "input_schema": {"additionalProperties": false, "properties": {"versions": {"items": {"type": "string"}, "type": "array"}},
                                "required": ["versions"], "type": "object"},
               "name": "semver-sort", "timeout_ms": 30000, "version": "0.1.0"},
  "exports": ["call: func(input: string) -> result<string, tool-error>", "describe: func() -> tool-info"],
  "imports": ["wasi:cli/environment@0.2.x", "wasi:cli/exit@0.2.x", "wasi:cli/stderr@0.2.x", "wasi:io/streams@0.2.x"],
  "source": "ext/semver-sort"}}}
```

(The `0.2.x` patch numbers are whatever the toolchain's std imports; the list shown is illustrative.)

### 3.16 Requirements on the body crate's build

- **Optimize dependencies in the dev profile.** Cranelift compiled at `opt-level = 0` compiles Wasm many times slower, and Wasmtime's runtime paths are slower too. Spec §9 keeps the body itself in the dev profile, which is fine; only dependencies need this:
  ```toml
  [profile.dev.package."*"]
  opt-level = 2
  debug = false
  ```
  It costs a slower *cold* dependency build once (Phase 0 measurement 1) and nothing on incremental builds of the body.
- **Exclude the ext workspace from the body's workspace**, if the body's `Cargo.toml` has a `[workspace]` table: `exclude = ["ext"]` (ext/authoring.md §3.3).
- **Vendor for both workspaces:** `cargo vendor --locked --sync ext/Cargo.toml vendor/`, run in `body/` (kernel/initctl.md, `initctl vendor`).
- Adopted in R1 by body/boot.md §3.16 (seed `Cargo.toml` and `.cargo/config.toml`), kernel/gate.md §3.3 (two workspaces in G0) and kernel/initctl.md §4.1 (`--sync`). A proposal to merge the two workspaces was dropped (reconciliation E8).

### 3.17 Prior art: Zed and Wassette

| | Zed (`main`, checked 2026-10-19) | Wassette 0.8.1 (2026-10-19) | INIT (this doc) |
|---|---|---|---|
| Wasmtime | 48, `default-features = false` | 48.0.6 | `=48.0.6`, `default-features = false` |
| Compiler and cache | Cranelift + in-memory incremental cache (moka, 32 MiB) | Cranelift; `.cwasm` loaded with `unsafe { Component::deserialize_file }` | Cranelift + in-memory incremental cache; no disk cache; no `unsafe` |
| Allocator | on-demand | on-demand | pooling (§3.4) |
| Interruption | epochs every 100 ms, `epoch_deadline_async_yield_and_update(1)`: yields, never traps | none | epochs every 10 ms while busy; hard trap at deadline; wall watchdog |
| Store | one long-lived per extension; calls serialized through a channel | fresh per call | fresh per call |
| `InstancePre` | no | yes, per component at load | yes, per version at load |
| Memory limits | none | `StoreLimits.memory_size` if the policy sets one | `StoreLimits` + pool caps (256 MiB) |
| WASI | p2 async; preopen of its own work dir; inherited stdio; no network; process exec behind a manifest capability | p2 async + `wasi:http` with host allowlist + `wasi:config`; preopens from policy | p2 sync; `/work` + `/data`; captured stdio; no network; `init:ext/process` |
| API versioning | a `since_v*` bindgen module for every version ever (0.0.1–0.7.0 on Stable); version in a `zed:api-version` custom section | n/a | N and N−1; trial type-check |
| Build | plain `cargo build --target wasm32-wasip2` (no cargo-component) | n/a | same as Zed |
| Tool schema | n/a | JSON Schema derived from WIT types (component2json) | declared by the guest in `describe` (ext/wit.md D9) |

## 4. Interfaces

### 4.1 Rust API used by the loop, the tools and boot

```rust
pub struct ExtPaths {
    pub home: PathBuf,        // $INIT_HOME
    pub src: PathBuf,         // $INIT_HOME/body/ext
    pub kept: PathBuf,        // $INIT_HOME/bin/ext            (kernel-published; §3.8)
    pub target: PathBuf,      // $CARGO_TARGET_DIR or $INIT_HOME/work/target
    pub state: PathBuf,       // $INIT_HOME/state/ext
}

pub enum ReloadKind { Boot, Request, SessionStart, Kept(Vec<String>) }

pub struct CallCtx<'a> { pub call_id: &'a str, pub workspace: &'a Path, pub abort: &'a AtomicBool }

pub const SUPPORTED_MAJORS: &[u32] = &[1];   // init:ext majors, newest first; boot.md §4.3 prints it as `ext_api_majors`

pub struct ExtOutcome {
    pub text: String,          // what goes into the tool_result
    pub is_error: bool,
    pub kind: OutcomeKind,     // Ok | InvalidInput | Failed | Timeout | Aborted | Oom | StackOverflow | Panic | Trap | Exit | Busy | Unknown | Host
    pub version: Option<String>,
    pub elapsed_ms: u64,
    pub truncated: bool,
}

impl ExtHost {
    pub fn new(paths: ExtPaths) -> Result<Arc<ExtHost>, String>;          // engine, linkers, ticker, loader; queues Boot
    pub fn begin_reload(&self, kind: ReloadKind);
    pub fn commit_reload(&self, max_wait: Duration) -> Option<ReloadReport>;
    pub fn reload_pending(&self) -> bool;
    pub fn reset_trials(&self);                                            // session start
    pub fn snapshot(&self) -> Arc<Registry>;                               // once per tool batch
    pub fn call(&self, reg: &Registry, name: &str, input: &serde_json::Value, cx: &CallCtx) -> ExtOutcome;
    pub fn shutdown(&self);                                                // kill sessions; called on every orderly exit
}
```

`CallCtx`, `ReloadKind`, `ExtOutcome`, `OutcomeKind` and `ReloadReport` are defined once, in `tools/ext.rs`, as body/tools.md's `ExtHostApi` seam. `ext_host` re-exports them instead of defining a second copy. `ExtHost` implements `ExtHostApi`; across that trait `snapshot()` returns a type-erased `Snapshot` (`Arc<dyn Any + Send + Sync>`), which `api.rs` downcasts to `Arc<Registry>`. A snapshot from a foreign host falls back to the current registry.

Constants (one table; all live in `ext_host.rs`):

| Name | Value |
|---|---|
| `EPOCH_TICK` | 10 ms |
| `CALL_TIMEOUT_DEFAULT` / min / max | 30,000 / 100 / 600,000 ms |
| `DESCRIBE_TIMEOUT` | 1,000 ms |
| `WATCHDOG_GRACE` | 2,000 ms |
| `ABORT_GRACE` | 200 ms |
| `MAX_STUCK` | 8 |
| `INPUT_MAX` / `OUTPUT_MAX` | 1 MiB / 1 MiB |
| `LOG_CAP` per call / log tail in errors / log file rotation | 1 MiB / 4 KiB / 1 MiB |
| `WASM_MAX_BYTES` | 64 MiB |
| `INCR_CACHE_BYTES` | 32 MiB |
| linear memory / table elements | 256 MiB / 20,000 |
| reload wait at a requested boundary / before an ext batch | 10 s / 60 s |
| process limits | ext/wit.md §4.3 |

### 4.2 Files under `state/ext/`

| Path | Writer | Format |
|---|---|---|
| `state/ext/catalog.json` | host, at every commit (temp + `rename`) | ICJ v1 rules 2–5 (sorted keys, integers only; body/log-context.md §3.8.1), pretty-printed with 2-space indentation (`serde_json::to_vec_pretty` on a `Value`), below |
| `state/ext/<name>/data/` | the extension (as `/data`) | anything |
| `state/ext/<name>/log`, `log.1` | host, append per call; rotated at 1 MiB (`log` → `log.1`, replacing it) | text records, below |

`catalog.json`:

```json
{
  "extensions": {
    "semver-sort": {
      "api": "init:ext@1",
      "artifact": "work/target/wasm32-wasip2/debug/semver_sort.wasm",
      "compile_ms": 84,
      "description": "Sort version strings by SemVer 2.0.0 precedence.",
      "input_schema": {"additionalProperties": false, "properties": {"versions": {"items": {"type": "string"}, "type": "array"}}, "required": ["versions"], "type": "object"},
      "origin": "trial",
      "timeout_ms": 30000,
      "version": "0.1.0"
    }
  },
  "failed": {"broken": "describe trapped: wasm trap: wasm `unreachable` instruction executed"},
  "generation": 7,
  "not_built": ["draft"],
  "stuck": {}
}
```

Paths inside it are relative to `INIT_HOME`. Why pretty-printed rather than strict ICJ (rule 1 forbids insignificant whitespace): the catalog is a file for the agent to read, never hashed and never embedded in a prompt by the harness, and the `read` tool cuts lines over 2,000 bytes (body/tools.md §3.2), which a one-line catalog with several schemas would exceed. Key order and number form are still canonical, so the bytes are deterministic. Every value is an integer or a string; schemas are already integer-only (ext/wit.md §3.7).

Log record (one per call):

```
--- 2026-10-19T12:34:56.789Z call=toolu_01ab semver-sort 0.1.0 trial result=ok 12ms log=0B
--- 2026-10-19T12:35:10.002Z call=toolu_01cd semver-sort 0.1.0 trial result=panic 3ms log=212B
thread '<unnamed>' panicked at src/lib.rs:41:9:
index out of bounds: the len is 3 but the index is 7
```

### 4.3 Reload report

```rust
pub struct ReloadReport {
    pub ok: bool,                         // no load failures in this reload
    pub generation: u64,
    pub components: Vec<String>,          // names added, changed or removed, sorted
    pub text: String,                     // rendered for the model, below
}
```

The loop turns it into the `reload` event: `tier: "T1"`, `ok`, `components`, `prefix_changed: false` (T1 never changes the head), head hashes unchanged, `error` set to the first failure, and `text` (body/log-context.md schema). The text starts with the harness tag `[init:reload]` (log-context.md X9, §4.3; every harness text carries an `[init:` tag), so the body uses it verbatim. Rendered text, deterministic, in name order:

```
[init:reload] ext generation 7
+ semver-sort 0.1.0 (trial, compiled in 84 ms): Sort version strings by SemVer 2.0.0 precedence.
  input_schema: {"additionalProperties":false,"properties":{"versions":{"items":{"type":"string"},"type":"array"}},"required":["versions"],"type":"object"}
~ count-lines 0.1.0 → 0.2.0 (trial)
- old-tool (removed)
! broken: describe trapped: wasm trap: wasm `unreachable` instruction executed (no previous version)
· not built: draft (looked for work/target/wasm32-wasip2/{debug,release}/draft.wasm)
```

Lines the host prints beyond this example:
- A `~` line is followed by `  description:` and `  input_schema:` lines when those changed.
- A switch of origin with identical bytes (trial to kept) prints `~ n 0.1.0 (kept)`.
- A `!` line ends with `(no previous version)` or `(version X still serving)`.
- A name with blocked calls prints `· stuck: <name> (<n> call(s) still blocked in a host call; calls to it return busy)`.
- `  note: …` lines follow a name that loaded with a remark (for example a clamped `timeout-ms`).
- A reload that changed nothing prints `(no changes)` after the `[init:reload]` line.
- A describe load error ends with `; log: <last 2 log lines, joined with " | ", cut at 400 bytes>` when `describe` wrote to the log, so the cause is visible. For example, a `describe` that calls `process` ends with `describe has no capabilities`.

### 4.4 Outcome text

§3.12 gives the table. Success text is the guest's string. Errors start with `ext <name> <version>: `. Body/tools.md applies its spill and truncation policy to `text`.

### 4.5 `init-body ext` subcommands (proposal for body/boot.md)

They exist so the agent and tests can run a component **with the exact host configuration**, without touching the live registry. No `wasmtime` or `wasm-tools` CLI is needed.

| Command | Does | Output (stdout, JSON, sorted keys) |
|---|---|---|
| `init-body ext describe <wasm>` | load (§3.8 steps 3–6, name check skipped) | `{"api":"init:ext@1","compile_ms":…,"info":{…}}` or `{"error":"…"}` |
| `init-body ext call <wasm> --input '<json>' [--workspace <dir>] [--data <dir>]` | load + one `call` (defaults: workspace = cwd, data = `state/ext/<name>/data`) | `{"is_error":…,"kind":"…","text":"…","elapsed_ms":…,"log":"…"}` |
| `init-body ext inspect <wasm>` | `Component::component_type()` → imports and exports | `{"exports":[…],"imports":[…]}` |

Exit code: 0 if the operation ran (even if the tool returned an error), 2 for usage errors, 1 for load errors.

### 4.6 Environment variables read

- `INIT_HOME`, to build `ExtPaths`.
- `CARGO_TARGET_DIR`; if it is unset, `$INIT_HOME/work/target`.

## 5. Failure modes and recovery

| Failure | Detection | Effect | Recovery |
|---|---|---|---|
| Guest infinite loop | epoch deadline | `timeout` result; `Store` dropped | fix the guest; nothing to clean up |
| Guest sleeps or blocks in a host call past its deadline | wall watchdog | `timeout` (abandoned); name `busy` until the worker ends | wait or fix; reload report lists stuck names; at most 8 stuck in total |
| Guest exceeds 256 MiB | limiter flag + trap | `oom` result | use less memory or stream through `/data` |
| Stack overflow / panic / other trap | `Trap` downcast | error result with log tail | fix the guest |
| Pool exhausted (> 32 concurrent instances) | instantiate error | `host error: instance pool exhausted (32 concurrent calls): <wasmtime message>` | retry later; indicates stuck workers or a runaway batch |
| Component fails to compile, link, type-check or describe | load pipeline | `failed[name]`; the previous version keeps serving | fix and rebuild; reload |
| Artifact missing (`not built`) | resolution | listed in report and catalog | build it |
| Artifact half-written when reloading (cargo still linking) | compile error, or fingerprint changes between stat and read | load error | reload again after the build finishes (the authoring flow builds first, then reloads) |
| Trial and kept builds of the same name confused | `floor(n)` rule (§3.8) | the newer authoritative one wins | the kernel's `reload{kind:"kept"}` after a keep is automatic (`Kept`) |
| Owner interrupt during a call | `cx.abort` (§3.10) | `aborted` result within ≈ 10–200 ms; abandoned like a timeout if blocked in a host call | none |
| Loader thread panics | `JoinHandle`/channel disconnect | `commit_reload` returns an error report; registry unchanged | the next `begin_reload` restarts the thread |
| Host-side fault in Wasmtime (bug) | process crash | body exits non-zero | kernel restart, crash backoff, LKG rollback (spec §7.1) |
| `catalog.json` write fails (disk full) | `io::Error` | warning in the report; registry still swapped | none needed; the next commit rewrites it |
| Session child ignores stdin EOF after a body crash | n/a in body | orphaned process | kernel kills the body's descendants (Q-H4) |
| A Wasmtime advisory affecting compiled-in features | maintainer watch | n/a | `initctl vendor wasmtime@48.0.x` (forces G4; approval rule PENDING OWNER (CONVENTIONS §6 ESC-1)) |

## 6. Performance notes

Mapped to spec §9 ranks:

- **Rank 1 (cache):** nothing in the host touches the prefix. The reload report and catalog reach the model only through the tail (`reload` event) or on demand.
- **Rank 3 (tool path):** per-call host overhead. All figures are UNVERIFIED estimates for the M2; test H15 measures them.

  | Step | Estimate |
  |---|---|
  | thread spawn + channel | 10–20 µs |
  | `WasiCtx` build incl. 2 preopen `open`s | 10–30 µs |
  | `Store::new` + limiter | < 5 µs |
  | `instantiate` from pool | 5 µs with CoW (Linux); 20–100 µs on macOS, where data segments are copied (150–300 KB component, tens of KB of data) |
  | `call` lift/lower of ≤ 1 MiB strings | µs to low ms |
  | **total** | about **0.05–0.3 ms**, against a 0.5–10 s model step |

  `process.run` costs a `posix_spawn` plus the child's runtime, the same as `bash`.
- **Rank 4 (self-edit latency):** the reload compile (30–300 ms, estimate, §3.14) is paid once, at the requested turn boundary, before the next model request, and bounded by the 10 s wait. The guest build dominates (ext/authoring.md §3.6).
- **Rank 5 (footprint × N):** idle cost:
  - two parked threads (`ext-epoch`, `ext-loader`);
  - the compiled code of loaded extensions (≈ 1–3× the Wasm size each);
  - the incremental cache (≤ 32 MiB);
  - the pool's virtual reservation, which uses no RSS.

  After the first WASI I/O call, add the lazily built tokio runtime (§3.2), roughly 1–4 MB RSS for its worker threads (UNVERIFIED). Pool slots are decommitted on deallocation (`linear_memory_keep_resident = 0`, the default), so RSS returns to baseline after calls. Test H16 checks this over 10,000 calls (spec Phase 0, measurement 5).
- **Not done on purpose:** fuel metering, a disk cache, a long-lived per-extension store, async host functions. Each would cost body size or safety for no measured gain.

## 7. Invariants and security

Tied to spec §2.1:

- **Invariant 3 (blast radius).**
  - The OS sandbox is the perimeter. Wasm preopens and the `process` rules are fault isolation inside it.
  - WASI preopens have had escapes (GHSA-vqjp-4c8c-hfgg, GHSA-m9f7-9w4p-gv38, both fixed in 48.0.6), which is why they are not treated as the boundary.
  - Network: every socket use is denied in WASI, and the OS sandbox blocks everything but the two kernel sockets.
- **"The agent cannot corrupt its own memory" (spec §4).**
  - No `unsafe` API is called (§3.14 table).
  - The disk cache, which would hide `unsafe` behind a safe signature, is not compiled in.
  - Winch is not compiled in.
  - Caveat: `forbid(unsafe_code)` does not see `unsafe` produced by other crates' macros (§3.5). The body's only such macro is `wasmtime::component::bindgen!`, a vendored dependency the agent cannot change without `initctl vendor` (full G4).
- **Invariant 2 (honest measuring stick).**
  - The kernel never loads agent-built `.wasm` in its gate. It rebuilds from source (evolution.md §3.4) and derives the inventory itself (§3.15), running `describe` in a sandboxed child, never in the kernel process.
  - The `describe` the body runs gets no capabilities either, so it cannot behave differently in the two places by probing files.
- **Invariant 1 (recoverability).** A host-side crash is an ordinary body crash. Extensions cannot stop the body: every call has a deadline, and blocked calls are abandoned and counted.
- **Determinism (CONVENTIONS §5).** Every map is a `BTreeMap`. `input` reaches the guest with sorted keys. Catalog, report and outcome texts are generated in name order.

## 8. Test plan

Seed tests (`seed/src/ext_host/tests.rs`) are testing.md level **L1** unless marked. The fixture guests are `.rs` files in `seed/src/ext_host/fixtures/`, not crates: the tests write them into a throwaway ext workspace under `<target>/test-homes/` (the seed's target dir) and build them with the real toolchain, because a `Cargo.toml` in the seed would be an unlisted crate under gate.md §3.3 step 3. The "Maps to" column names testing.md scenario and measurement ids (roadmap.md Q2):

| Id | Case | Expected | Maps to |
|---|---|---|---|
| H0 | The body compiles with `#![forbid(unsafe_code)]` and `bindgen!` | builds; if not, switch to the dynamic API (§3.5) and record it | G0, A6 |
| H1 | `new_engine()` on macOS arm64 and Linux x86_64 | `Engine::new` ok; `concurrency_support(false)` accepted (else drop it and record) | — |
| H2 | Load the `echo` fixture | report `+ echo`; print core-instance, memory and table counts; `compile_ms` recorded | M-2 |
| H3 | `echo` call round trip | output equals input; p50 host overhead < 1 ms | rank 3 |
| H4 | Guest `loop {}` with `timeout-ms: 200` | `timeout` within 200–260 ms; the next call succeeds | — |
| H5 | Guest `std::thread::sleep(60 s)`, `timeout-ms: 500` | `timeout` (abandoned) within ≤ 2.6 s; `busy` for that name; other names fine; after 60 s `stuck` is 0 | §3.12 |
| H6 | Guest allocates 512 MiB | `oom` text | `StoreLimits` |
| H7 | Infinite recursion | `trap: stack overflow` (recursion without locals); with locals, `trap: wasm trap: out of bounds memory access` (§3.12) | — |
| H8 | `panic!("boom")` | `panicked`; log tail contains `boom` | — |
| H9 | Hot swap: v1 returns "1"; rebuild to return "2"; `begin_reload` + `commit_reload` | the next batch gets "2"; a v1 call made long-running before the commit (via `process.run sleep`) still returns "1" | A2, drain |
| H10 | Load errors: wrong `call` signature, imports `wasi:http`, `$ref` in schema, name ≠ dir, nondeterministic `describe` | each listed under `!`; previous version (if any) still serves | ext/wit.md W-4 |
| H11 | `/etc/passwd`, `/work/../x`, symlink out of `/work` | guest-side errors | A6 |
| H12 | `TcpStream::connect` | guest-side error | A6 |
| H13 | `process.run`: echo, timeout kill of `sleep 10` at 100 ms, `cwd` escape | ok / `timed-out` within 200 ms and no surviving pgid / `denied` | ext/wit.md W-10, W-11 |
| H14 | Sessions with `cat`: spawn, write, read until; 5th session `denied`; idle reap with a 1 s test override; `shutdown` kills all | as specified | A2-repl |
| H15 | 1,000 sequential `echo` calls | host overhead p50/p99 recorded | rank 3 |
| H16 | 10,000 `echo` calls | RSS growth < 5 MiB after a 100-call warm-up. `#[ignore]`: run it alone (`cargo test --offline h16 -- --ignored`), because RSS is per process and the parallel suite moves it by tens of MiB | M-5 (testing.md) |
| H17 | 8 parallel calls to one extension, then 40 parallel blocked calls | all 8 ok; the 40 produce `busy`/pool-exhausted results, no panic | — |
| H18 | Trial/kept resolution: a trial built before `reset_trials` is ignored; `Kept([n])` makes the kept artifact win over an older trial | as specified | A2b (testing.md), evolution.md §3.4 |
| H19 | `catalog.json` | valid, sorted, written atomically (`rename`) | — |
| H20 | `init-body ext describe|call|inspect` on `echo` | JSON as §4.5 | authoring |
| H21 | Recompile of an edited 150-line extension with the incremental cache warm vs cold | warm ≤ cold; both recorded | §3.14 |
| H22 | `describe` tries `process.run` | `denied` inside `describe`, so a describe that depends on it fails to load | D10 |
| H23 | Owner abort: set `abort` during (a) a guest `loop {}`, (b) a `process.run(["sleep","10"])`, (c) a `process.read(wait 10 s)` | (a) `aborted` within ≤ 30 ms; (b) `aborted`, process group gone within 200 ms; (c) `aborted` within ≤ 60 ms; the next call works | loop.md Q-L3 |
| H24 | Probe root: `INIT_HOME=probe/<id>` with `bin/ext` → the candidate's set | the probe body's Boot load lists exactly those components | G2 (gate.md), supervise.md §3.6.3 |

## 9. Open questions

### 9.1 Local

- **Q-H1.** Do `concurrency_support(false)` and the trimmed feature set build and run with `wasmtime-wasi` p2 sync on 48.0.6? Answered by H0/H1: yes (WP14). The fallback to default values is not needed.
- **Q-H2.** Real component structure for Rust `wasm32-wasip2` (core instances, tables) to tighten the pool numbers. Answered by H2 (WP14): 3 core instances, 1 memory and 2 tables, so the pool totals have plenty of headroom.
- **Q-H3.** Real compile and instantiate times on the M2 (§3.14, §6). Phase 0.

### 9.2 Cross-doc

Settled in R1 and R2 (ledger docs/reconciliation/ext.md); no longer open: supervise.md (`bin/ext` symlink to immutable sets, probe roots, `reload{kind:"kept", components}`, `state/ext/`, no `RLIMIT_AS`, descendant sweep), gate.md and selfmap.md (§3.15 inventory, sandboxed `describe`, no hash or size compared, two workspaces), loop.md (reload call sites, `snapshot`, `reset_trials`, `shutdown`, `Abort::flag()`), tools.md (`CallCtx`, shared helpers, catalog pointer), boot.md (`ExtHost::new`, `init-body ext`, build settings, `ext_api_majors`), the 48.0.6 pin everywhere (CONVENTIONS §5, testing.md, roadmap.md, adr/005), H0/H1/H23 in L1, A2-repl, and every CONVENTIONS request. Remaining follow-ups (R1 items not yet applied by their owners):

- **→ body/loop.md (E1, E7):** pass `abort: abort.flag()` in `CallCtx`; the input thread calls `begin_reload(Kept(components))` on `reload{kind:"kept", components}` and `Request` on `kind:"request"`; update §3.5's abort table ("a running `ext` call stops within one 10 ms epoch tick"); close its Q-L3.
- **→ body/tools.md (E1, E3):** `CallCtx` gains `abort`; map `OutcomeKind::Aborted` → `aborted`; `run_group` must accept a plain `&AtomicBool` cancel signal (the host's per-call flag) and a `CaptureSpec` that keeps stdout and stderr separately (the WIT `run-output` has two fields).
- **→ kernel/selfmap.md (E12):** `bindgen!` path `kernel/wit/init-ext/v1`, not `wit/init-ext`.
- **→ kernel/gate.md (E9, E10):** compensating checks for the `forbid` exemption; source-less lockfile entries are not a `dep` change.
- **→ kernel/supervise.md (E24):** §3.6.8's "the live body already runs the new data" holds for T0 only; for T1 the live body runs the trial until `reload{kind:"kept"}`.
- **→ adr/003 (E23):** `allow_blocking_current_thread(true)` removes the extra `spawn_blocking` hop, not the runtime; mention Wasmtime's compile-time rayon pool (CONVENTIONS §5 already words it so).

## 10. Spec deltas

| # | Spec v0.2 | This doc | Why |
|---|---|---|---|
| 10.1 | §4 "Instantiation in ≈5 µs" | ≈5 µs on Linux; about 20–100 µs on macOS (estimate) | Copy-on-write memory init on non-Linux Unix needs `deserialize_file`, which is `unsafe`. Still negligible next to model latency. |
| 10.2 | Wasmtime version unspecified (notes cite v49) | `=48.0.6`, the current LTS | 24-month security support, fewer vendored upgrades (each forces G4); same choice as Zed and Wassette |
| 10.3 | CONVENTIONS §5 "no async runtime: std threads only" | Body code is sync, but `wasmtime-wasi` lazily starts a tokio multi-thread runtime for WASI I/O (and `parallel-compilation` brings rayon) | unconditional dependency of wasmtime-wasi; accepted by CONVENTIONS §5 (v0.2) and adr/003 |
| 10.4 | §4 "in-flight calls drain on the old version" | Kept, but commits happen only between tool batches, so in practice only abandoned workers drain. Calls blocked in host calls cannot be interrupted by epochs and are abandoned with a watchdog. | Wasmtime epoch semantics |
| 10.5 | §4 pooling allocator (implying fast CoW instantiation) | Pooling kept, with small totals (32 instances). On macOS its benefit is slot reuse, not CoW. | as 10.1 |
| 10.6 | Spec is silent on where kept T1 binaries live | `bin/ext` (symlink to an immutable kernel-published set), built by the gate and never committed; trials from `work/target/` | evolution.md §3.4, gate.md §3.3, supervise.md §3.6.5; the kernel never trusts agent-built artifacts |
| 10.9 | Spec is silent on interrupting a running extension | owner abort stops a guest within one 10 ms tick and a blocked host call within 200 ms (abandoned otherwise) | loop.md Q-L3 |
| 10.7 | §9 "release builds of the body" kept out | Body stays in the dev profile, but its dependencies are compiled at `opt-level = 2` | Cranelift at `opt-level = 0` makes reload compiles seconds long |
| 10.8 | T1 limited to tools in a sandbox (implicit) | `init:ext/process` (run and pipe sessions) implemented in the host | ext/wit.md D3 |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.18):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP14 F (Q-H1) | `concurrency_support(false)` is accepted on 48.0.6 with `wasmtime-wasi` p2 sync, so the line stays and the fallback is not needed | §3.3, §9.1 |
| build: WP14 F (Q-H2) | a Rust `wasm32-wasip2` component uses 3 core instances, 1 memory and 2 tables (test H2), replacing "about 3–5 (UNVERIFIED)" | §3.4, §9.1 |
| build: WP14 Done (Bindings), Tests H0 | the generated names are confirmed (`Tool`, `ToolPre`, `call_describe`, `call_call(&mut store, &str)`), and `bindgen!` compiles under `#![forbid(unsafe_code)]`; the two UNVERIFIED marks are gone (beyond the F list; checked in `bindings.rs`) | §3.5 |
| build: WP14 F (exit, stack overflow) | WASI p2 `exit` carries a result, so `exit(n)` arrives as `exit(1)`; recursion with locals surfaces as `out of bounds memory access`, and only recursion without locals gives `stack overflow` | §3.12, §8 (H7) |
| build: WP14 D1 | the shared types are defined in `tools/ext.rs` and re-exported by `ext_host`; `snapshot()` crosses the `ExtHostApi` trait type-erased | §4.1 |
| build: WP14 D2 | the pool-exhaustion text is `host error: instance pool exhausted (32 concurrent calls): <wasmtime message>` | §3.12, §5 |
| build: WP14 D3 | extra report lines: `~` with `description:` and `input_schema:`, `(kept)` origin switch, `(version X still serving)`, `· stuck:`, `note:`, `(no changes)` | §4.3 |
| build: WP14 D4 | a describe load error ends with `; log: <last 2 log lines>` | §4.3 |
| build: WP14 D5 | an `Ok` returned while an owner abort is being delivered is reported as `aborted` | §3.10 |
| build: WP14 D7 | fixtures are `.rs` files that the tests write into a throwaway workspace, because a `Cargo.toml` in the seed would be an unlisted crate; tests live in `seed/src/ext_host/tests.rs` | §8 |
| build: WP14 D9 | H16 is `#[ignore]` and runs alone, because RSS is per process | §8 |

## 11. References

All checked 2026-10-19.

- Wasmtime release and LTS policy: https://docs.wasmtime.dev/stability-release.html
- crates.io versions and dates (wasmtime 48.0.6 and 49.0.3 on 2026-10-14; 50.0.0-rc.1): https://crates.io/api/v1/crates/wasmtime/versions
- CVE-2026-34987 / GHSA-xx5w-cvp6-jv83 (Winch, aarch64 PoC): https://github.com/advisories/GHSA-xx5w-cvp6-jv83 ; https://api.github.com/advisories/GHSA-xx5w-cvp6-jv83
- Wasmtime advisories list (Aug–Oct 2026, incl. GHSA-m9f7-9w4p-gv38, GHSA-vqjp-4c8c-hfgg, GHSA-32h6-97mm-8q3c, GHSA-q554-3cjc-rx7p): https://api.github.com/repos/bytecodealliance/wasmtime/security-advisories
- `Config` (all methods, defaults, `unsafe` ones, epoch semantics, mach ports, CoW platforms): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/config.rs (also v49.0.3)
- `PoolingAllocationConfig` defaults (`InstanceLimits::default`): same file
- Cargo features: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/Cargo.toml
- `Component::{new, from_binary, deserialize*, serialize, component_type}`: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/runtime/component/component.rs
- `Module::deserialize` unsafety text: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasmtime/src/runtime/module.rs
- Disk-cache hit path (`load_code_bytes`): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/compile/runtime.rs ; cache store: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/cache/src/lib.rs
- `CacheStore` trait: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/environ/src/compile/mod.rs
- `StoreLimits`, `ResourceLimiter`: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasmtime/src/runtime/limits.rs
- `Store` (limiter, epoch deadline) and `Engine` (`increment_epoch`, `weak`, `upgrade`): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasmtime/src/runtime/store.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/engine.rs
- `Trap` variants: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/environ/src/trap_encoding.rs ; `wasmtime::Error::downcast_ref`: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/core/src/error/error.rs
- Component `Linker` (`instantiate_pre`, `define_unknown_imports_as_traps`, semver): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/runtime/component/linker.rs
- `InstancePre`: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasmtime/src/runtime/component/instance.rs
- `bindgen!` examples and options: https://docs.rs/wasmtime/49.0.3/wasmtime/component/bindgen_examples/_0_hello_world/index.html ; https://docs.rs/wasmtime/49.0.3/wasmtime/component/macro.bindgen.html
- wasmtime-wasi (tokio dependency, `in_tokio` runtime, `WasiCtxBuilder`, `add_to_linker_sync`, pipes): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasi/Cargo.toml ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/runtime.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasi/src/ctx.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/p2/mod.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/p2/pipe.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/cli/mem.rs
- Wasmtime guides: https://docs.wasmtime.dev/examples-fast-instantiation.html ; https://docs.wasmtime.dev/examples-interrupting-wasm.html ; https://docs.wasmtime.dev/examples-fast-compilation.html ; https://docs.wasmtime.dev/examples-minimal-cwasm.html
- 5 µs instantiation figure: https://bytecodealliance.org/articles/wasmtime-10-performance
- `forbid(unsafe_code)` and macro-generated `unsafe`: https://internals.rust-lang.org/t/hidden-unsafe-due-to-unintentionally-abusable-macros-and-include/14107 ; https://internals.rust-lang.org/t/pre-rfc-unsafe-attributes/17462?page=2
- `std::io::pipe` (Rust 1.87) and `PipeWriter::try_clone`: https://doc.rust-lang.org/std/io/fn.pipe.html ; https://doc.rust-lang.org/std/io/struct.PipeWriter.html
- Zed host: https://raw.githubusercontent.com/zed-industries/zed/main/crates/extension_host/src/wasm_host.rs ; https://raw.githubusercontent.com/zed-industries/zed/main/Cargo.toml ; https://raw.githubusercontent.com/zed-industries/zed/main/crates/extension/src/extension_builder.rs
- Wassette: https://raw.githubusercontent.com/microsoft/wassette/main/Cargo.toml ; https://raw.githubusercontent.com/microsoft/wassette/main/crates/component2json/src/lib.rs ; https://api.github.com/repos/microsoft/wassette/releases
- wit-component 0.262.0 `decode`: https://docs.rs/wit-component/0.262.0/wit_component/fn.decode.html
