# The T1 contract: WIT package `init:ext@1.0.0`
Status: v0.3 + build sync 2026-10-20 · Area: ext
Depends on: spec v0.2 (§2.1, §4, §5, §6.1, §6.7, §9), CONVENTIONS.md, ext/host.md, ext/authoring.md, body/tools.md (`ext` tool schema), kernel/selfmap.md   Defines: WIT package `init:ext@1.0.0` (world `tool`, interface `process`); the guest-visible environment of an extension; the `tool-info` validation rules; the mapping from `tool-info` to the `ext` tool; the WIT versioning and deprecation policy

---

## 1. Purpose

This doc fixes the one contract that every T1 extension and every version of the body must agree on. The contract is the WIT package `init:ext`. Spec §8 names it, with the kernel, as the part of INIT that truly locks in, so it gets the most care.

The doc gives:
- the exact WIT text;
- what an extension can reach on the host, and why;
- how an extension's self-description becomes something the model can call;
- the rules for changing the contract later without breaking extensions the agent has already built.

## 2. Responsibilities / non-goals

**This doc owns:**
- The text of `init:ext@1.0.0`: world `tool` and interface `process`.
- The guest-visible environment: preopens, environment variables, stdio, clocks, random, sockets.
- The validation rules for `tool-info`: name, description, schema, version, timeout.
- How `tool-info` maps to the `ext` tool call and to a promoted first-class tool.
- The versioning and deprecation policy, including support for majors N and N−1.
- The decision on hooks: deferred, not in 1.0.

**Non-goals:**
- How the host implements the contract (Wasmtime config, limits, registry, swap). That is ext/host.md.
- The build toolchain, template and skill. That is ext/authoring.md.
- The `ext` tool's own JSON schema and result formatting. body/tools.md owns those; this doc states only what it needs from them (§9.2).
- How `self.json` stores the T1 inventory. kernel/selfmap.md owns that; ext/host.md §3.15 says what can be read from a binary.
- WASI 0.3 and async. They are out of scope for 1.x; see §9.1, Q-W3.

## 3. Design

### 3.1 Decisions at a glance

| # | Question | Decision | Why (short) |
|---|---|---|---|
| D1 | Where do `describe` and `call` live? | World-level exports of world `tool`, with world-level types `tool-info` and `tool-error`. | Simplest guest code (`impl Guest for Component`), so the model is more likely to get it right. Export names carry no version, so a minor bump of the package never changes what the host looks up (§3.6). |
| D2 | Filesystem | WASI preopens of exactly two directories: `/work` (the task workspace) and `/data` (this extension's own data directory). Both read-write. | Least authority that still lets a tool do useful work. `/data` gives state that persists across calls without long-lived instances (ext/host.md §3.6). |
| D3 | Process execution | **Yes**, host-mediated, through `init:ext/process`: a one-shot `run` and pipe-based sessions (`spawn`, `write`, `read`, `close`, `sessions`). | Most valuable agent tools wrap CLIs, and the spec's own T1 example (A2, §6.4) is an interactive REPL, which needs a process that outlives one call. This gives no new authority: the child runs inside the body's OS sandbox, exactly like `bash`. |
| D4 | PTY | Not in the seed host. The field `spawn-request.pty` exists from 1.0. A seed host answers `denied` when it is true. | Rust `std` has no PTY API, and the body may not use `unsafe` or add crates freely. The field is reserved now because WIT records cannot grow compatibly (§3.6), so the agent can implement PTY later as a T2 change without a contract break. |
| D5 | Network / HTTP | **None** in 1.x. `wasi:sockets` is linked so that std-built components load, but every use is denied. No `wasi:http`. | The body's sandbox reaches only the two kernel sockets (spec §7.4), and the proxy routes only to model providers. An `http` interface can be added in a minor version if the proxy ever offers general allowlisted egress (Q-W2). |
| D6 | Logging | No custom import. The guest writes to stdout or stderr (`println!`, `eprintln!`). The host captures both into one buffer of at most 1 MiB per call. | Zero extra WIT surface. Works for every guest language and for Rust panic messages. Levels add nothing the model needs. |
| D7 | Hooks (context or tool-call interception) | **Deferred.** Not in 1.0. If ever added, they come as a separate world (for example `hook`) in a minor version. | §3.5 |
| D8 | Version identification | By trial type-check, newest supported major first (ext/host.md §3.8), plus the versions of any `init:ext/*` imports. | Needs no custom section and no `unsafe` link-section tricks in guest code (Zed's approach). Unambiguous, because each major changes at least one export signature or one import. |
| D9 | Input encoding | `call(input: string)` takes the `input` object of the `ext` call as compact JSON with keys sorted. | One parser per tool (the guest's own serde types). Deterministic bytes. No JSON Schema validator needed in the body (§3.7). |
| D10 | `describe` | Must be pure: no host calls, no file access, same bytes every time. The host calls it at load with **no** capabilities. The kernel calls it the same way. | Lets the kernel derive the inventory from the binary itself instead of trusting the agent (spec §5, "verify, don't trust"). |

### 3.2 The package text

The authoritative file is `kernel/wit/init-ext/v1/init-ext.wit` (§4.1). The body carries a byte-identical copy. The full text:

```wit
/// INIT extension contract, major version 1.
///
/// Owner: the kernel maintainer. The agent cannot change this file.
/// Authoritative copy: $INIT_HOME/kernel/wit/init-ext/v1/init-ext.wit
/// Body copy (byte-identical, checked at G1): $INIT_HOME/body/wit/init-ext/v1/init-ext.wit
/// Policy: docs/ext/wit.md section 3.6. Within 1.x, nothing below is ever changed or
/// removed; minor versions only add new interfaces or new worlds.
package init:ext@1.0.0;

/// Host-mediated subprocesses.
///
/// A child runs on the host, inside the body's OS sandbox, with the same authority as
/// the `bash` tool and the body's environment. Paths in requests are guest paths:
/// "/work" (the task workspace), "/data" (this extension's data directory), or a
/// directory below one of them. Nothing else is accepted.
@since(version = 1.0.0)
interface process {
    /// A command to run to completion.
    record run-request {
        /// Program and arguments. argv[0] is looked up on the body's PATH. Must not be empty.
        argv: list<string>,
        /// Guest path of the working directory, for example "/work" or "/work/src".
        cwd: string,
        /// Variables set on top of the body's environment, in order (a later entry wins).
        env: list<tuple<string, string>>,
        /// Bytes written to the child's stdin, which is then closed. At most 1 MiB.
        stdin: list<u8>,
        /// Kill the child's process group after this many milliseconds. The host lowers
        /// it to the time left in the current call.
        timeout-ms: u32,
    }

    /// The result of `run`.
    record run-output {
        /// Exit status. None if the child was killed by a signal or by the timeout.
        exit-code: option<s32>,
        /// The last 1 MiB of standard output.
        stdout: list<u8>,
        /// The last 1 MiB of standard error.
        stderr: list<u8>,
        /// True if the host killed the child because the timeout elapsed.
        timed-out: bool,
        /// True if stdout or stderr was longer than 1 MiB, so its beginning was dropped.
        truncated: bool,
    }

    /// A long-lived child, for REPL-like tools. A session belongs to the extension
    /// name (not to one call or one version), survives later calls and version swaps,
    /// and ends when it is closed, when it is idle for 30 minutes, or when the body exits.
    record spawn-request {
        /// Program and arguments, as in `run-request`.
        argv: list<string>,
        /// Guest path of the working directory, as in `run-request`.
        cwd: string,
        /// Variables set on top of the body's environment.
        env: list<tuple<string, string>>,
        /// Give the child a pseudo-terminal instead of pipes. A host without PTY support
        /// returns `denied`.
        pty: bool,
    }

    /// Output taken from a session by `read`.
    record read-result {
        /// Bytes the child wrote to stdout and stderr, interleaved in arrival order.
        /// At most 1 MiB.
        data: list<u8>,
        /// True if bytes were dropped because more than 4 MiB was buffered since the
        /// previous read.
        truncated: bool,
        /// True once the child has exited and all of its output has been returned.
        exited: bool,
        /// Exit status once `exited` is true. None while running or if killed by a signal.
        exit-code: option<s32>,
    }

    /// Why a process request failed.
    variant process-error {
        /// argv[0] was not found or is not executable.
        not-found(string),
        /// The request broke a rule: empty argv, a cwd outside /work and /data, an
        /// unknown session id, a limit reached, or an unsupported option.
        denied(string),
        /// Any other operating-system error.
        io(string),
    }

    /// Run a command to completion and return its output.
    run: func(req: run-request) -> result<run-output, process-error>;

    /// Start a session. Returns an id that is unique within this extension.
    spawn: func(req: spawn-request) -> result<u32, process-error>;

    /// Queue bytes for the session's stdin. Never blocks. At most 1 MiB per call and
    /// 4 MiB queued.
    write: func(session: u32, data: list<u8>) -> result<_, process-error>;

    /// Wait for output, then return everything unread (up to 1 MiB). Returns as soon as
    /// one of these holds:
    /// - `until` is given and appears in the unread output;
    /// - `until` is none, at least one byte has arrived, and no new byte for 50 ms;
    /// - `wait-ms` has elapsed (also lowered to the time left in the current call);
    /// - the child has exited.
    read: func(session: u32, wait-ms: u32, until: option<string>) -> result<read-result, process-error>;

    /// Kill the session's process group, reap it, and free its id.
    close: func(session: u32) -> result<_, process-error>;

    /// Ids of this extension's open sessions, ascending.
    sessions: func() -> list<u32>;
}

/// The world every tool extension targets.
@since(version = 1.0.0)
world tool {
    import process;

    /// What the tool is. Returned by `describe`.
    record tool-info {
        /// The extension's name. Must equal its crate directory name under body/ext/.
        /// Pattern: ^[a-z][a-z0-9-]{0,31}$ with no "--" and no trailing "-".
        name: string,
        /// For the model: what the tool does and when to use it. 1 to 1024 bytes of UTF-8.
        description: string,
        /// JSON Schema for the `input` object, as JSON text. The root must be an object
        /// schema ({"type":"object", ...}). At most 16384 bytes. See docs/ext/wit.md 3.7.
        input-schema: string,
        /// The extension's own version: MAJOR.MINOR.PATCH, for example "0.1.0".
        version: string,
        /// Requested per-call deadline in milliseconds. None means 30000.
        /// The host clamps it to the range [100, 600000].
        timeout-ms: option<u32>,
    }

    /// A failure the tool reports itself (a trap is reported by the host instead).
    variant tool-error {
        /// The input did not parse or did not validate. The model should fix the input.
        invalid-input(string),
        /// The tool ran and could not finish. The same input will probably fail again.
        failed(string),
    }

    /// Describe the tool. Must be pure and deterministic: no host calls, no file access,
    /// identical bytes on every call. The host calls it at load time and the kernel at
    /// self-map time, both with no capabilities.
    export describe: func() -> tool-info;

    /// Run the tool. `input` is the `input` object of the `ext` tool call, serialized
    /// as compact JSON with object keys sorted. The ok string goes back to the model
    /// verbatim (plain text or JSON). At most 1 MiB.
    export call: func(input: string) -> result<string, tool-error>;
}
```

Syntax notes (checked against WIT.md on 2026-10-19):
- `@since(version = 1.0.0)` uses the gate grammar `'@since' '(' 'version' '=' <valid semver> ')'`. A package with a gate must declare its version, which it does.
- `list` is a WIT keyword, so the function that lists sessions is named `sessions`.
- `///` comments are documentation for the following item (Component Model book).
- Worlds may contain type definitions (`typedef-item`). Types defined in the world and used by exports are exported at the component's root, with no version in their names.

### 3.3 What each side sees

**Guest (Rust, wit-bindgen 0.63.0):** `wit_bindgen::generate!({ path: "../../wit/init-ext/v1", world: "init:ext/tool" })` generates the items below. The template puts it inside `#[cfg(target_arch = "wasm32")] mod bindings`, so the items live under `bindings::` (ext/authoring.md §3.4).

| WIT | Rust item |
|---|---|
| world exports `describe`, `call` | `trait Guest { fn describe() -> ToolInfo; fn call(input: String) -> Result<String, ToolError>; }` |
| `record tool-info` | `struct ToolInfo { name: String, description: String, input_schema: String, version: String, timeout_ms: Option<u32> }` |
| `variant tool-error` | `enum ToolError { InvalidInput(String), Failed(String) }` |
| `interface process` | module `init::ext::process` with `fn run(req: &RunRequest) -> Result<RunOutput, ProcessError>`, `fn spawn(req: &SpawnRequest) -> Result<u32, ProcessError>`, `fn write(session: u32, data: &[u8]) -> Result<(), ProcessError>`, `fn read(session: u32, wait_ms: u32, until: Option<&str>) -> Result<ReadResult, ProcessError>`, `fn close(session: u32) -> Result<(), ProcessError>`, `fn sessions() -> Vec<u32>` |
| registration | `export!(Component);` |

The export side takes owned values. Imported record and `option<string>` parameters are borrowed in 0.63.0 (`&RunRequest`, `&SpawnRequest`, `&[u8]`, `Option<&str>`, as the table shows; checked at WP14, and the skill's snippet compiles verbatim). An author who passes them by value gets a compiler error at once, and the skill says so (ext/authoring.md §3.9).

**Host (Rust, `wasmtime` 48.0.6 `bindgen!`):** `bindgen!({ path: "wit/init-ext/v1", world: "init:ext/tool" })` inside `mod v1` generates `v1::Tool`, `v1::ToolPre<T>`, `v1::ToolInfo`, `v1::ToolError`, the trait `v1::init::ext::process::Host` and its record types. The calls are `tool.call_describe(&mut store)` and `tool.call_call(&mut store, &input)` (ext/host.md §3.5).

### 3.4 Host imports: the guest-visible environment

An extension sees exactly the following. Anything else is absent, and a component that imports it fails to load with a message naming the import.

| Capability | Interface(s) | Granted | Detail |
|---|---|---|---|
| Task workspace | `wasi:filesystem/preopens`, `wasi:filesystem/types` | preopen `/work`, read-write | Host path: the body's working directory (`ToolCx.paths.cwd`, body/tools.md §3.6), the directory every `bash` call starts in; a G4 run gets its task directory there. |
| Own data | same | preopen `/data`, read-write | Host path: `$INIT_HOME/state/ext/<name>/data/`. Created on first load. Persists across calls, versions, body restarts and promotions. Shared by concurrent calls of the same extension (the guest must tolerate that). |
| Current directory | `wasi:cli/environment.initial-cwd` | `/work` | Use absolute paths (`/work/src/x`). wasi-libc does **not** resolve relative paths against `initial-cwd`: a guest's `read("ok.txt")` fails (measured at WP14). |
| Arguments, environment | `wasi:cli/environment` | none, none | Deterministic. Nothing secret can leak in. |
| Stdin | `wasi:cli/stdin` | closed (EOF) | |
| Stdout, stderr | `wasi:cli/stdout`, `wasi:cli/stderr`, `wasi:cli/terminal-*` | captured into one shared 1 MiB buffer per call; not a terminal | Shown to the model only when the call traps, times out, exits or hits a host error (last 4 KiB), not with a guest-returned `tool-error`. Always appended to `state/ext/<name>/log`. Guest stdout is line-buffered: a `print!` without a newline is lost when the call ends. When the buffer is full, further writes fail, and Rust's `println!`/`eprintln!` then panic. Keep logs short. |
| Exit | `wasi:cli/exit` | yes | `std::process::exit(n)` ends the call with the error `called exit(1)`. WASI p2 `exit` takes a result, not a number, so every non-zero code arrives as 1 (measured at WP14). |
| Clocks | `wasi:clocks/wall-clock`, `wasi:clocks/monotonic-clock` | host clocks | Sleeping (`std::thread::sleep`) works but cannot be interrupted by the deadline; see ext/host.md §3.12. Do not sleep. |
| Randomness | `wasi:random/*` | host CSPRNG | At most 1 MiB per request. |
| Network | `wasi:sockets/*` | linked, **every use denied** | TCP and UDP disabled, name lookup disabled, every address refused. A std `TcpStream::connect` returns an error, not a trap. |
| HTTP | `wasi:http/*` | **not provided** | A component importing it fails to load. |
| Processes | `init:ext/process` | yes (§3.2) | Same authority as `bash`. Limits: ext/host.md §3.13. |

What `describe` sees: the same table, but with **no** preopens, and `init:ext/process` returns `denied("describe has no capabilities")` on every function.

WASI versions: Rust's `wasm32-wasip2` std imports `wasi:*@0.2.x` interfaces at whatever patch level the toolchain uses. Wasmtime's component linker matches semver-compatible names in both directions: it resolves `0.2.x` against `0.2.y` and `1.a.b` against `1.c.d` (verified in `crates/wasmtime/src/runtime/component/linker.rs` and `crates/environ/src/component/names.rs` at both v48.0.6 and v49.0.3). So the WASI patch level of the toolchain and the host do not need to match.

A recent std change makes even a minimal component import `wasi:cli/terminal-*` (rust-lang/rust#150120). That is harmless here, because the host links all of `wasi:cli`.

### 3.5 Hooks are deferred

**Decision:** 1.0 has no hook surface: no interception of the context, of tool calls, or of model requests.

Reasons:
1. **No observed failure asks for it.** Spec §6.4 requires a `Repro:` for every new rule or guardrail, and hooks are where guardrails would live. Building the slot first invites phantom guardrails (15 of 60 runs in the evidence).
2. **Hooks sit on the hot path of every step.** A slow or trapping hook costs every turn, and its failure semantics (fail open or fail closed) have to be designed against real cases first.
3. **Context hooks threaten cache rank 1.** Anything that rewrites the request must touch only the tail, never the head, or the cache breaks. That rule needs an enforcement design, which is not cheap.
4. **The core can already do it.** Behavior that wraps every step belongs to the loop, and the loop is agent-owned and changes through T2 with the full gate. That is the right price for something on every step.
5. **Deferral is free.** A future `world hook` is a different world in the same package, added in a minor version (§3.6). `tool` components are unaffected.

What a later hook world would need (recorded here so the next design starts from it, **not** a contract):
- a separate world, so a component is either a tool or a hook;
- tail-only edits, with the body enforcing that the head hash is unchanged;
- a per-hook deadline far below the tool deadline (for example 50 ms);
- fail-open by default, with each failure logged;
- a `Repro:` trailer on the commit that introduces the first hook.

Consequence for other docs: evolution.md's component-id kind `hook/<ext>/<hook>` and the "hooks" field of `self.json` stay empty until a hook world ships (§9.2).

### 3.6 Versioning and deprecation policy

The package version is `MAJOR.MINOR.PATCH`. Only the kernel maintainer changes it (spec §6.7). Every change, including a minor one, is a kernel change.

| Change | Allowed in | Examples |
|---|---|---|
| Comments only | PATCH | Fix a doc comment. |
| Add a new interface, and import it in an existing world | MINOR | `interface fs-watch` with `@since(version = 1.1.0)` and `@since(version = 1.1.0) import fs-watch;` in `world tool` |
| Add a new world | MINOR | `world hook` (§3.5) |
| Mark an item for removal in the next major | MINOR | `@deprecated(version = 1.3.0)` on an item, paired with its `@since` |
| Anything else: change, rename or remove an item; add a field to a record or a case to a variant; add a function to an existing interface; change `describe`/`call` | MAJOR | |

Why records and interfaces are frozen within a major: Component Model types are structural. Adding a record field or a variant case produces a different type, and a component built against the old type no longer type-checks. Wasmtime's linker treats `1.a.b` and `1.c.d` as the same interface and does not check that they really are the same (linker.rs: "relies on hosts being well-behaved ... interfaces once defined are never changed"). A minor version that changed an existing interface would break silently. Hence the table.

**Kernel and body support N and N−1** (spec §6.7):
- WIT files are kept per major: `wit/init-ext/v1/`, `wit/init-ext/v2/`. Minor versions update the file of their major in place, because they are additive.
- The body's ext host carries one `bindgen!` module and one `Linker` per supported major (ext/host.md §3.5). It tries the newest major first.
- When the maintainer ships major N+1:
  1. the kernel installs `kernel/wit/init-ext/v{N+1}/` next to `v{N}/`;
  2. the next seed release (merged through the upstream flow, evolution.md) adds `v{N+1}` bindings to the host and keeps `v{N}`;
  3. reload reports and the `boot` event list the extensions still built against N, so the agent can migrate them at its own pace.
- When major N+2 ships, support for N is removed from the next seed's host. Extensions still built against N then fail to load with: `built for init:ext@N; this body supports @N+1 and @N+2; rebuild against wit/init-ext/v{N+2}`.
- Before a major removes something, the last minor of the previous major marks it `@deprecated(version = …)`. This is at least one seed release ahead.
- The window is therefore "one major", not a fixed time. Zed keeps every API version ever released (0.0.1 to 0.7.0 on Stable); INIT keeps two, because the whole body must fit in one context (spec §4, budget).

**How the host knows which major a component targets (D8).** Export names are unversioned (`describe`, `call`), so the version cannot be read from export names. The host tries `instantiate_pre` plus the typed `ToolPre::new` for each supported major, newest first. The first one whose imports resolve and whose export types check wins. Each major must therefore differ from the previous one in at least one export signature or one imported interface version. That rule is part of this policy.

**WASI changes are separate.** Which WASI the host links (`0.2.x` today) is a host property, not part of `init:ext`. Moving to WASI 0.3 changes the async model of guests, so it will be done as `init:ext@2.0.0` (Q-W3).

### 3.7 From `tool-info` to the `ext` tool

**Calls go through one fixed tool.** The model never sees an extension's schema in the tool list. It calls the fixed `ext` tool (body/tools.md) with `{"name": ..., "input": {...}}`. Adding, changing or removing an extension therefore never changes the prefix (spec §4, rank 1).

**Validation of `tool-info`** happens at load, in the host (ext/host.md §3.8), and again in the kernel's self-map:

| Field | Rule | On violation |
|---|---|---|
| `name` | `^[a-z][a-z0-9-]{0,31}$`, no `--`, no trailing `-`, equal to the crate directory name, not one of `read`, `write`, `edit`, `bash`, `ext`. No name is reserved by the body (body/tools.md §3.6 dropped `_list`/`_describe` in R1); a leading `_` cannot match the pattern anyway | load error |
| `description` | 1–1024 bytes, valid UTF-8 (it is a WIT string, so always UTF-8) | load error |
| `input-schema` | ≤ 16,384 bytes; parses as JSON; root is an object with `"type": "object"`; uses only the keywords listed below; every number is an integer (ICJ v1 rule 4, body/log-context.md §3.8.1: a promoted schema enters the head, where floats are forbidden) | load error |
| `version` | `^[0-9]+\.[0-9]+\.[0-9]+$` | load error |
| `timeout-ms` | none → 30,000; otherwise clamped to [100, 600,000] | clamped, noted in the reload report |
| determinism | two `describe` calls return identical values | load error |

**Allowed JSON Schema keywords** (draft 2020-12 subset), chosen so that the same schema is also accepted as a first-class tool by Anthropic `input_schema`, OpenAI function `parameters` and DeepSeek's OpenAI-compatible tools: `type`, `properties`, `required`, `items`, `enum`, `const`, `description`, `additionalProperties`, `default`, `minimum`, `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`. Not allowed: `$ref`, `$defs`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`. Recommended: `"additionalProperties": false` and a `description` on every property. Whether all three providers accept every listed keyword in their strict modes is UNVERIFIED; body/tools.md owns the final provider-specific rules.

**Canonical form.** The host parses `input-schema` and re-serializes it as **ICJ v1** (body/log-context.md §3.8.1: sorted keys, no whitespace, integers only; serde_json without `preserve_order`, so maps are `BTreeMap`). The canonical form is what goes into `state/ext/catalog.json`, the reload report, and `self.json`. A cosmetic edit to the schema text therefore causes no diff.

**The input path, end to end:**

1. The model calls:
   ```json
   {"name": "count-lines", "input": {"path": "src/main.rs", "needle": "fn "}}
   ```
2. body/tools.md checks that `input` is a JSON object of at most 1 MiB once serialized.
3. The host serializes `input` compactly with sorted keys and passes it to `call`:
   ```json
   {"needle":"fn ","path":"src/main.rs"}
   ```
4. The guest parses it into its own type (serde, with `deny_unknown_fields`). On failure it returns `tool-error::invalid-input("...")`.
5. On `ok(s)`, `s` is the tool result text. On `err`, the host formats an error result (ext/host.md §4.4).

**The host does not validate `input` against `input-schema` in 1.x.** The guest's parser is the single source of truth, and the schema is documentation for the model. The cost is that a schema and its parser can drift apart. The guard is the template's unit test (ext/authoring.md §3.4), which checks that the schema is an object schema and that an input carrying exactly the schema's `required` properties deserializes into the guest's input type. A host-side validator is a known-good growth option for the agent (Q-W4).

**Discovery.** Since schemas are not in the tool list, the model learns about extensions from:
- `$INIT_HOME/state/ext/catalog.json`, which the body rewrites at every reload commit and which the static `ext` description points at (body/tools.md §3.6, §4.1; ext/host.md §4.2);
- the reload report it receives after `initctl reload` (name, description, canonical schema of each added or changed extension; ext/host.md §4.3);
- `self.json` for kept extensions (kernel-verified);
- an `ext` call with an unknown name, whose error result lists every loaded extension with its description (ext/host.md §3.10).

**Promotion to a first-class tool.** Spec §4 lets the agent turn a heavily used extension into a tool of its own. The mapping is direct:

```json
{"name": "count-lines",
 "description": "<tool-info.description>",
 "input_schema": <canonical tool-info.input-schema>}
```

Dispatch still goes through the ext host. This is a T2 change, because the tool list changes. It needs `Prefix-change: yes` and takes effect at the handoff, as one cache miss (body/log-context.md §3.8.2: the new code needs its own tool list, so it is not deferred). To combine misses, promote right after a compaction.

## 4. Interfaces

### 4.1 Files

| Path | Owner | Agent access | Content |
|---|---|---|---|
| repo `wit/init-ext/v1/init-ext.wit` | maintainer | n/a | the text in §3.2 |
| `$INIT_HOME/kernel/wit/init-ext/v1/init-ext.wit` | kernel (installed by `init-kernel install`) | read-only | authoritative copy |
| `$INIT_HOME/body/wit/init-ext/v1/init-ext.wit` | body (seeded from `seed/wit/`) | read-write, but any diff from the kernel copy fails G1 | used by the host's `bindgen!` and by every extension's `generate!` |
| `$INIT_HOME/body/ext/<name>/` | agent | read-write | one extension crate per directory (ext/authoring.md §3.3) |

Spec §3 shows `kernel/wit/init-ext@1.wit`. The directory per major replaces it (§10).

### 4.2 `call` wire format

- `input`: UTF-8 JSON text, compact, keys sorted byte-wise at every depth, at most 1 MiB. Produced by `serde_json::to_string` on a `serde_json::Value` (whose map is a `BTreeMap`, since `preserve_order` is never enabled; CONVENTIONS §5). This is ICJ v1 rules 1–3 and 5; numbers are passed through exactly as the model sent them (a non-integer is the model's own data, not harness output), and the guest's parser decides whether it accepts them.
- `ok`: any UTF-8 string up to 1 MiB. Longer strings are truncated by the host at a character boundary and flagged (ext/host.md §3.10). The `ext` tool result applies body/tools.md's spill policy on top.
- `err(invalid-input(m))` and `err(failed(m))`: `m` is shown to the model. Keep it under 2 KiB and actionable.

Examples of `call` results and what the model sees (exact formatting is ext/host.md §4.4):

| Guest returns | Tool result `is_error` | Text shown |
|---|---|---|
| `Ok("{\"count\":42}")` | false | `{"count":42}` |
| `Err(InvalidInput("missing field path"))` | true | `ext count-lines 0.1.0: invalid input: missing field path` |
| `Err(Failed("/work/src/main.rs: not found"))` | true | `ext count-lines 0.1.0: failed: /work/src/main.rs: not found` |

### 4.3 `process` limits (normative; the host enforces them)

| Limit | Value |
|---|---|
| `run` stdout and stderr kept | last 1 MiB each |
| `run` stdin | ≤ 1 MiB |
| `run` timeout | `min(timeout-ms, time left in the call)`; the whole process group is killed |
| sessions per extension | 4 |
| sessions per body | 16 |
| session idle timeout (no `write`/`read`) | 30 min, then killed |
| unread session output buffered | 4 MiB (oldest bytes dropped, `truncated` set) |
| `read` returns | ≤ 1 MiB |
| `read` quiet period when `until` is none | 50 ms |
| `write` per call / queued | 1 MiB / 4 MiB |
| `cwd` | `/work`, `/data`, or a subdirectory of either; no `..`; must exist; resolved with symlinks and still inside its root |
| owner interrupt | `run` kills the process group and `read` returns early; the call then ends as `aborted` (ext/host.md §3.10) |
| environment | the body's environment plus `env` |
| `pty: true` | `denied("pty is not supported by this body")` in the seed |

## 5. Failure modes and recovery

| Failure | Detection | Result | Recovery |
|---|---|---|---|
| Component built against a WIT the host does not support (old major, or a newer minor's new interface) | `instantiate_pre` fails for every supported major | load error naming the missing import or mismatched export; the previous version keeps serving | rebuild against `body/wit/init-ext/v<current>` |
| Component imports `wasi:http` or another absent interface | `instantiate_pre` fails | load error naming the import | remove the dependency (for example a crate that pulls `wasi:http`) |
| `describe` traps, touches files or the process interface, or is nondeterministic | load-time `describe` with no capabilities, called twice | load error | make `describe` return a constant |
| `name` differs from the directory name | validation | load error | fix either one |
| Body copy of the WIT drifts from the kernel copy | G1 compares bytes (kernel/gate.md) | gate fails | `cp kernel/wit/... body/wit/...` (the agent can read `kernel/`) |
| A minor version changes an existing interface by mistake | review of the WIT diff (maintainer) and a host test that builds a 1.0 component against the newer host | silent type confusion would be possible, since the linker assumes semver honesty | policy §3.6; test W-6 |
| Log output over 1 MiB | buffer full | guest write fails; in Rust, `println!` panics, so the call traps | log less; write large data to `/data` |

## 6. Performance notes

- **Rank 1 (cache):** the `ext` tool schema is constant, so T1 changes never touch the prefix. Schemas reach the model only in the tail (reload report) or on demand (catalog, `self.json`).
- **Rank 3 (tool path):** an extension call costs host-side work of tens to a few hundred microseconds (ext/host.md §6). `process.run` adds a `posix_spawn` and the child's own runtime, the same as `bash`.
- **Rank 4 (self-edit latency):** D1 (world-level exports, one template) and D6 (no logging import) keep the guest code short, which helps both build time and the model's first-try success (spec Phase 0, measurement 3).
- Passing `input` as JSON text costs one serialize and one parse, at most 1 MiB each, so microseconds to milliseconds against seconds of model time (rank 7: "keep it incremental, then stop").

## 7. Invariants and security

Tied to spec §2.1:

- **Invariant 3 (blast radius).** T1 adds no authority beyond what `bash` already has inside the body's sandbox:
  - files: `/work` and `/data`, both inside the sandbox's write roots;
  - processes: run inside the same OS sandbox;
  - network: none.

  The Wasm boundary gives fault isolation and resource limits. **The OS sandbox is the security perimeter.** This matters because WASI preopens are not a perfect wall. Wasmtime published two high-severity filesystem-sandbox escapes in 2026 (GHSA-vqjp-4c8c-hfgg on 2026-08-20, GHSA-m9f7-9w4p-gv38 on 2026-10-14, both about trailing slashes and symlinks). Both are fixed in 48.0.6, which ext/host.md pins.
- **Invariant 1 (recoverability).** The authoritative WIT is in `kernel/`, which the agent cannot write. The body's copy is checked at G1.
- **Invariant 2 (honest measuring stick).** The T1 part of `self.json` comes from the kernel's own build of the committed source plus a capability-free `describe` call (D10, ext/host.md §3.15), not from the agent's claims.
- **Invariant 4 (lineage).** An extension is source under `body/ext/`, committed like any other change. A trial lives only in the live tree and dies at the session boundary (evolution.md §3.4).
- **Secrets.** Extensions get no environment variables, and `describe` gets nothing at all. Children started through `process` inherit the body's environment, which by design holds no keys (spec §3: keys live only in the proxy).

## 8. Test plan

"Maps to" names testing.md scenario ids where one applies (roadmap.md Q2).

| Id | Case | Expected | Maps to |
|---|---|---|---|
| W-1 | The §3.2 file parses: `wit-parser` (kernel test) and both `bindgen!` and `generate!` compile it | ok | G0 |
| W-2 | A guest built from the template (ext/authoring.md) exports exactly `describe` and `call`; `wasm-tools component wit` (or `init-body ext inspect`) shows world-level exports and, if used, `import init:ext/process@1.0.0` | ok | A2 |
| W-3 | A guest that does not call `process` does not import it | import list has no `init:ext/process` (measured at WP14: unused world imports are not in the component; `count-lines` imports only `wasi:cli`, `wasi:clocks`, `wasi:filesystem` and `wasi:io` at 0.2.9, including the `terminal-*` interfaces) | inventory accuracy |
| W-4 | `tool-info` validation: each row of §3.7 violated once | load error with the field name | |
| W-5 | A `describe` that unwraps `std::fs::read_to_string("/work/x")` or `process::run(…)` | load error: describe has no preopens and `process` is denied, so the unwrap traps | D10 |
| W-6 | Forward compatibility: a component built against 1.0.0 loads in a host built from a synthetic 1.1.0 that adds `interface extra` | loads and runs | §3.6 |
| W-7 | Synthetic major 2 (change `call` to return a record): a v1 component loads with v1 bindings, a v2 component with v2 bindings, a v0 (bogus) component fails with the policy message | ok | §3.6 |
| W-8 | Network: guest calls `std::net::TcpStream::connect("1.1.1.1:80")` | `Err` returned to the guest; the call itself succeeds and reports the error | A6 |
| W-9 | Filesystem: guest opens `/etc/passwd`, `/work/../x`, and `/work/link-to-outside` (symlink) | all fail inside the guest | A6 |
| W-10 | `process.run(["sh","-c","echo hi"])` in `/work` | `exit-code: some(0)`, stdout `hi\n` | A2-repl |
| W-11 | `process.run` with `cwd: "/etc"` and with `cwd: "/work/../.."` | `denied` | A6 |
| W-12 | Session: `spawn(["cat"])`, `write("x\n")`, `read(1000, some("x"))` | `data` = `x\n` | A2-repl |
| W-13 | `spawn` with `pty: true` | `denied` | D4 |
| W-14 | Same input serialized from `{"b":1,"a":2}` and `{"a":2,"b":1}` | guest receives identical bytes `{"a":2,"b":1}` | D9 |
| W-15 | Schema with `$ref` | load error | §3.7 |

## 9. Open questions

### 9.1 Local

- **Q-W1. PTY.** Should a later seed implement `pty: true`? It needs `openpty`, which means `rustix` with its `pty` feature (already in the dependency tree through wasmtime-wasi, but a new direct dependency) or `unsafe`. Proposal: leave it to the agent as a T2 growth step (spec §4 lists persistent PTY as the first growth step). The contract slot already exists.
- **Q-W3. WASI 0.3 / async.** `wasm32-wasip3` is tier 3 on stable Rust 1.99 and tier 2 from Rust 1.100 (nightly docs, checked 2026-10-19; release date about 2026-11-12, UNVERIFIED). Revisit as `init:ext@2.0.0` once wit-bindgen's async exports are stable and the body has a reason to be async.
- **Q-W4. Host-side input validation.** A ~120-line validator for the §3.7 keyword subset would catch schema/parser drift. It is deferred, because the guest already validates. A candidate for known-good growth.

### 9.2 Cross-doc

Settled in R1 and R2 (ledger docs/reconciliation/ext.md); no longer open: the `ext` tool contract and `/work` (body/tools.md), the WIT copy check (kernel/gate.md G1), empty `hooks` (kernel/selfmap.md, evolution.md), A2-repl (testing.md), HTTP for extensions (none in 1.x, D5; ADR-002 and kernel-proxy-gate WI-3). Remaining:

- **→ kernel/selfmap.md:** `bindgen!` path `kernel/wit/init-ext/v1` (one directory per major, §4.1, CONVENTIONS §2), not `wit/init-ext`. (R1 follow-up E12; not yet applied there.)

## 10. Spec deltas

| Spec v0.2 | This doc | Why |
|---|---|---|
| §3 and §6.7: `kernel/wit/init-ext@1.wit` | `kernel/wit/init-ext/v1/init-ext.wit`, plus a byte-identical `body/wit/` copy | one directory per major makes N/N−1 side by side natural, and `bindgen!` and `generate!` need a path inside the crate's tree |
| Language report: "one versioned WIT world for tools and hooks"; spec §5 lists "hooks" in `self.json` | hooks deferred; `tool` world only | §3.5 |
| Spec is silent on what T1 can reach | `/work`, `/data`, host-mediated processes (run and sessions), no network | §3.1 D2–D5; A2's REPL example needs processes |
| §6.7 "kernel supports N and N−1" | made precise: minors are additive only; records and interfaces are frozen within a major; support for N ends when N+2 ships | §3.6 |
| §4 "this tool's schema never changes" | kept, and made concrete: the model discovers schemas through reload reports, the catalog, `self.json`, and unknown-name errors | §3.7 |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.18):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP14 F (Q-A2) | imported record and `option<string>` parameters are borrowed in wit-bindgen 0.63.0; the UNVERIFIED mark is gone | §3.3 |
| build: WP14 F (wasi-libc) | wasi-libc does not resolve relative paths against `initial-cwd`, so guests must use `/work/…` | §3.4 |
| build: WP14 F (exit) | WASI p2 `exit` takes a result, so `exit(n)` arrives as `exit(1)` and the text reads `called exit(1)` | §3.4 |
| build: WP14 F (stdout) | guest stdout is line-buffered; a `print!` without a newline is lost when the call ends | §3.4 |
| build: WP14 F (W-3) | a guest that never calls `process` does not import it (measured on `count-lines`); the UNVERIFIED mark is gone | §8 (W-3) |
| build-deviations §5 L1 (WP11 D5/D6) | the task-workspace preopen's host path is `ToolCx.paths.cwd`, not `ToolCx.cwd` | §3.4 |

## 11. References

All checked 2026-10-19.

- WIT format and gates: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
- Component Model book, WIT and Rust pages: https://component-model.bytecodealliance.org/design/wit.html ; https://component-model.bytecodealliance.org/language-support/building-a-simple-component/rust.html
- wit-bindgen README and `generate!` options: https://github.com/bytecodealliance/wit-bindgen ; https://docs.rs/wit-bindgen/latest/wit_bindgen/macro.generate.html
- wit-bindgen 0.63.0 metadata: https://crates.io/api/v1/crates/wit-bindgen/0.63.0
- Wasmtime 48.0.6 component linker (semver matching): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasmtime/src/runtime/component/linker.rs ; `alternate_lookup_key`: https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/environ/src/component/names.rs
- wasmtime-wasi `WasiCtxBuilder` defaults (no preopens, stdin closed, network uses disabled, all addresses denied): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v48.0.6/crates/wasi/src/ctx.rs ; https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/sockets/mod.rs
- wasmtime-wasi `MemoryOutputPipe` (fails writes past capacity): https://raw.githubusercontent.com/bytecodealliance/wasmtime/v49.0.3/crates/wasi/src/p2/pipe.rs
- Wasmtime security advisories (GHSA-vqjp-4c8c-hfgg, GHSA-m9f7-9w4p-gv38 and others): https://github.com/bytecodealliance/wasmtime/security/advisories
- std `wasm32-wasip2` imports terminal interfaces: https://github.com/rust-lang/rust/issues/150120
- `wasm32-wasip3` tier status: https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-wasip3.html
- Zed API versioning (`since_v*` modules, `zed:api-version` custom section): https://raw.githubusercontent.com/zed-industries/zed/main/crates/extension_host/src/wasm_host/wit.rs
- Wassette WIT-to-JSON-Schema mapping (component2json), as a contrast to D9: https://raw.githubusercontent.com/microsoft/wassette/main/crates/component2json/src/lib.rs
- Agent tool-schema constraints: provider docs as cited by body/tools.md (UNVERIFIED here).
