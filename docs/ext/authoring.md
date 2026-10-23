# Writing a T1 extension: toolchain, template and workflow
Status: v0.3 + build sync 2026-10-20 · Area: ext
Depends on: spec v0.2 (§4, §6.1, §6.2, §10 Phase 0, §11), CONVENTIONS.md, ext/wit.md, ext/host.md, evolution.md (§3.4), testing.md (A2, M-2, M-3), kernel/initctl.md (`vendor`), kernel/gate.md (§3.3), body/tools.md, body/boot.md   Defines: the extension workspace layout (`body/ext/`), the build command and artifact paths, toolchain and crate versions, the template crate (`skills/ext-authoring/assets/template/`), the local test procedure, build-time estimates and the Phase 0 measurement definition, the agent-facing skill `skills/ext-authoring/SKILL.md` (draft), and the decision criteria and outline of the script fallback lane

---

## 1. Purpose

A T1 extension is only useful if the agent can write one, from nothing, in a few minutes, and get it right on the first or second build. Spec §11 names this as the biggest unknown ("how well frontier models write Wasm components is unmeasured") and asks for three mitigations: a WIT template, a skill, and a fallback lane. This doc provides all three. It also fixes the toolchain and the build loop, whose speed decides whether the fallback lane is needed.

## 2. Responsibilities / non-goals

**This doc owns:**
- the toolchain;
- the `body/ext/` workspace files;
- the template crate;
- the build command and where its output goes;
- the three levels of local testing;
- the skill text;
- the criteria for adding a script lane, and its outline.

**Non-goals:**
- The contract itself (ext/wit.md).
- How the host loads, swaps and runs components (ext/host.md).
- Keeping an extension: commits, gate, publication (evolution.md §3.4, kernel/gate.md).
- The Phase 0 measurement harness (testing.md M-2, M-3). This doc only defines what is measured and the thresholds.
- Installing anything on the owner's machine now. **The `wasm32-wasip2` target is not installed there (2026-10-19), and this doc does not install it.** §3.2 gives the command for Phase 0.

## 3. Design

### 3.1 The workflow at a glance

```
1. decide      bash one-off?  → just use bash (no gate, no reload)          [skill §When]
               new structured tool or long-lived helper?  → T1 (this doc)
               behavior on every step (loop, context)?  → T2 (evolution.md)
2. scaffold    cp -R skills/ext-authoring/assets/template ext/<name>; mv Cargo.toml.template Cargo.toml;
               set the name in 2 places
3. write       src/lib.rs: logic in plain Rust; describe() constant; call() parses JSON input
4. unit test   cd ext/<name> && cargo test                       (native, logic only)
5. build       cd ext/<name> && cargo build --target wasm32-wasip2
6. try it      init-body ext call <wasm> --input '{…}'           (exact host config, no reload)
7. reload      initctl reload    → `reload` event lists +<name> (or ! <error>)
8. use         ext {"name": "<name>", "input": {…}}
9. keep        commit on evo/<id> with trailers; initctl promote  (evolution.md §3.4)
```

Steps 4 and 6 are optional, but cheap. A trial (steps 2–8) lives only for the current session (ext/host.md §3.8).

### 3.2 Toolchain

| Component | Version / value | Status on the owner's machine (2026-10-19) | Source |
|---|---|---|---|
| rustc / cargo | 1.97.1 (stable) | installed | CONVENTIONS §5 |
| target `wasm32-wasip2` | Tier 2 without host tools, with std, since Rust 1.82 (2024-10-17) | **not installed** | rustc platform-support; Rust blog |
| `wasm-component-ld` (the target's linker) | ships inside the host toolchain whenever LLD ships (all tier-1 hosts, including `aarch64-apple-darwin`) | comes with rustc | rust-lang/rust#126967 |
| `wit-bindgen` | `=0.63.0` (2026-10-14), features `macros`, `realloc`, `std` | to vendor | crates.io |
| `serde` / `serde_json` | `1` (the lockfile pins the body's versions: 1.0.229 / 1.0.154) | to vendor | testing.md §2 |
| `cargo-component` | **not used** | — | The Component Model book: "the project is in the process of being deprecated as native tooling can be used directly". Last release 0.21.1, 2025-03-18. |
| `wasm-tools`, `wasmtime` CLIs | **not needed** | — | `init-body ext inspect|describe|call` (ext/host.md §4.5) does the same with the exact host config |

Phase 0 setup, run once by the owner (documented here, **not executed**):

```sh
rustup target add wasm32-wasip2
rustup target list --installed | grep -x wasm32-wasip2
du -sh "$(rustc --print sysroot)/lib/rustlib/wasm32-wasip2"   # record the disk cost (estimate: ~100–200 MB, UNVERIFIED)
```

Plain `cargo build --target wasm32-wasip2` on a `crate-type = ["cdylib"]` crate produces a **component** directly:
- `wasm-component-ld` runs `wasm-ld`, then wit-component's `ComponentEncoder`;
- with no `_start` export it builds a reactor;
- Zed's extension builder uses the same flow.

`wit-bindgen` without default features drops the `async`, `bitflags` and `macro-string` features (none is used). It does not need `wit-bindgen-rt`. Its macro chain, compiled for the host, is:
- `wit-bindgen-rust-macro` → `wit-bindgen-rust` → `wit-component` 0.262 and `wasm-metadata` 0.262;
- `wit-bindgen-core` → `wit-parser` 0.262;
- `syn` 3, `prettyplease` 0.3, `heck`, `indexmap`, `anyhow`.

That chain is the bulk of a cold build (§3.6).

### 3.3 Workspace layout

```
$INIT_HOME/body/
  .cargo/config.toml          vendored sources + offline (requirement on the body; §9.2)
  vendor/                     body AND ext dependencies (cargo vendor --sync ext/Cargo.toml)
  wit/init-ext/v1/init-ext.wit  byte-identical copy of kernel/wit (ext/wit.md §4.1)
  ext/
    Cargo.toml                the ext workspace (below)
    Cargo.lock                one lockfile for all extensions
    count-lines/              one crate per extension; the directory name IS the extension name
      Cargo.toml
      src/lib.rs
    semver-sort/
      …
  skills/ext-authoring/
    SKILL.md                  §3.9
    assets/template/          §3.4 (Cargo.toml.template, src/lib.rs); copied, never built in place
```

`body/ext/Cargo.toml`, complete:

```toml
# INIT T1 extensions (init:ext@1). One crate per subdirectory; every subdirectory
# must be an extension crate whose directory name is the extension name.
[workspace]
resolver = "3"
members = ["*"]

[workspace.package]
edition = "2024"

[workspace.dependencies]
wit-bindgen = { version = "=0.63.0", default-features = false, features = ["macros", "realloc", "std"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# `cargo build` (the dev profile) is the only profile used for extensions.
# Small output keeps Cranelift compile and instantiation fast in the host.
[profile.dev]
opt-level = "s"
debug = false
strip = "debuginfo"
incremental = true

# Proc macros (wit-bindgen, serde_derive) run on every build of a crate: make them fast.
[profile.dev.build-override]
opt-level = 2
debug = false
```

Why these settings:
- **One workspace, one lockfile, one target directory** (`$CARGO_TARGET_DIR = $INIT_HOME/work/target`, CONVENTIONS §3). Every extension shares the compiled `wit-bindgen`, `serde` and `serde_json`. Adding the second extension costs only its own crate.
- **Dev profile only.** One profile means one artifact path (`wasm32-wasip2/debug/`) and nothing to remember (no `--release`). The host also accepts a `release/` artifact, taking the newest (ext/host.md §3.8).
- **`opt-level = "s"` + `strip = "debuginfo"`.** A std hello world is 2.5 MB unstripped and 78 KB with `-Cstrip=debuginfo` (Wasmtime docs). Host compile time scales with code size (ext/host.md §3.14). Optimizing a 150-line leaf crate costs a fraction of a second.
- **`build-override` at `opt-level = 2`.** Warm builds are dominated by the leaf crate, and `generate!` parses the WIT and generates code on every build. An optimized macro is faster. The cost is a slower cold build, paid once.
- **`members = ["*"]`.** Adding an extension needs no workspace edit. A non-crate subdirectory matched by the glob is a hard error: every cargo command in the workspace fails (measured at WP0). So keep `body/ext/` to crates only, and point `CARGO_TARGET_DIR` outside `body/ext/`, because a `target/` inside it would be matched too. The installed body uses `work/target`; the seed's `ci/local.sh` uses `seed/target`.

Requirements on the body (owners in §9.2):
- `body/.cargo/config.toml` contains:
  ```toml
  [source.crates-io]
  replace-with = "vendored-sources"
  [source.vendored-sources]
  directory = "vendor"
  [net]
  offline = true
  ```
  Cargo finds config files by walking up from the **current directory**, so builds must run with the cwd inside `body/` (the command in §3.5 does).
- If `body/Cargo.toml` has a `[workspace]` table, it has `exclude = ["ext"]`.
- `vendor/` is produced with `cargo vendor --locked --sync ext/Cargo.toml vendor/` in `body/`, so it holds both workspaces' dependencies. Only the kernel runs this, through `initctl vendor` (the sandbox cannot reach crates.io; initctl.md §4.1 adopted `--sync` in R1).

Status after reconciliation round 1: body/boot.md §3.16 ships these files verbatim, kernel/gate.md §3.3 builds and tests the ext workspace in G0 (into supervise's build-slot target dir, `cache/build/slot/out/target`, not `cache/target`), and kernel/selfmap.md §3.6 reads its members. A first R1 proposal to merge extensions into the body workspace was dropped for that reason (docs/reconciliation/ext.md E8).

Rules for extension crates (kernel/gate.md §3.3 step 3, R1):
- **No `#![forbid(unsafe_code)]` requirement.** Extension code runs inside Wasm, where `unsafe` can corrupt only the guest's own linear memory. wit-bindgen's `export!` is a crate-local macro (generated by `generate!`) that emits `#[unsafe(export_name)]` functions, which the `unsafe_code` lint is likely to report, so `forbid` would probably not even compile (UNVERIFIED; test AU-9).
- **`crate-type = ["cdylib"]` only, and nothing in the body workspace may depend on an extension crate.** These two checks keep extension code (and its `unsafe`) out of the `init-body` binary, which is what the exemption must never allow (follow-up for gate.md, reconciliation E9).
- **Lockfile entries.** Adding an extension adds a source-less `[[package]]` entry to `ext/Cargo.lock`. That is not a dependency change; gate.md's scope rule should classify it as `t1`, not `dep` (follow-up, reconciliation E10).

### 3.4 The template crate

`skills/ext-authoring/assets/template/Cargo.toml.template` (renamed to `Cargo.toml` when copied; under its template name it is not a crate, so gate.md §3.3 step 3, "every `Cargo.toml` outside `vendor/` is a member of one of the two workspaces", never sees it):

```toml
[package]
name = "count-lines"            # CHANGE 1 of 2: must equal the directory name (kebab-case)
version = "0.1.0"
edition.workspace = true
publish = false

[lib]
crate-type = ["cdylib"]         # cdylib only: nothing may depend on an extension

[dependencies]
wit-bindgen.workspace = true
serde.workspace = true
serde_json.workspace = true
```

`skills/ext-authoring/assets/template/src/lib.rs`:

```rust
//! count-lines: count the lines of a workspace file that contain a substring.
//!
//! Template for INIT T1 extensions (WIT init:ext@1, world `tool`).
//! Copy this directory to body/ext/<name>/, rename Cargo.toml.template to Cargo.toml,
//! and change the two lines marked CHANGE.
//! Logic lives in plain functions (testable with `cargo test` on the host);
//! the bindings exist only when building for wasm32.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use serde::Deserialize;
use serde_json::{Value, json};

const NAME: &str = "count-lines"; // CHANGE 2 of 2: must equal the directory name
const VERSION: &str = env!("CARGO_PKG_VERSION");

const DESCRIPTION: &str = "Count the lines of a workspace file that contain a substring. \
Returns {\"count\": n, \"lines\": [line numbers of the first 20 matches]}.";

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path":   {"type": "string", "description": "File path relative to the workspace, e.g. \"src/main.rs\""},
    "needle": {"type": "string", "description": "Substring to look for, e.g. \"fn \""}
  },
  "required": ["path", "needle"],
  "additionalProperties": false
}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    needle: String,
}

/// Pure logic: no I/O, so it is unit-testable natively.
fn count(text: &str, needle: &str) -> Value {
    let mut lines = Vec::new();
    let mut count: u64 = 0;
    for (i, line) in text.lines().enumerate() {
        if line.contains(needle) {
            count += 1;
            if lines.len() < 20 {
                lines.push(i + 1);
            }
        }
    }
    json!({"count": count, "lines": lines})
}

/// The tool: parse input, read the file under /work, run the logic.
fn run(input: &str) -> Result<String, Fail> {
    let input: Input = serde_json::from_str(input).map_err(|e| Fail::Input(e.to_string()))?;
    if input.path.starts_with('/') || input.path.split('/').any(|c| c == "..") {
        return Err(Fail::Input(format!("path must be relative to the workspace: {}", input.path)));
    }
    let full = format!("/work/{}", input.path);
    let text = std::fs::read_to_string(&full).map_err(|e| Fail::Run(format!("{full}: {e}")))?;
    Ok(count(&text, &input.needle).to_string())
}

enum Fail {
    Input(String),
    Run(String),
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: "../../wit/init-ext/v1",
        world: "init:ext/tool",
    });

    use super::{DESCRIPTION, Fail, NAME, SCHEMA, VERSION};

    struct Component;

    impl Guest for Component {
        fn describe() -> ToolInfo {
            // Must be constant: no I/O, same value every time.
            ToolInfo {
                name: NAME.to_string(),
                description: DESCRIPTION.to_string(),
                input_schema: SCHEMA.to_string(),
                version: VERSION.to_string(),
                timeout_ms: None, // 30 s default; Some(ms) up to 600_000
            }
        }

        fn call(input: String) -> Result<String, ToolError> {
            super::run(&input).map_err(|f| match f {
                Fail::Input(m) => ToolError::InvalidInput(m),
                Fail::Run(m) => ToolError::Failed(m),
            })
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_matching_lines() {
        let v = count("fn a\nlet x\nfn b\n", "fn ");
        assert_eq!(v, json!({"count": 2, "lines": [1, 3]}));
    }

    #[test]
    fn schema_is_an_object_schema_and_matches_input() {
        let s: Value = serde_json::from_str(SCHEMA).expect("SCHEMA is JSON");
        assert_eq!(s["type"], "object");
        // An input with exactly the schema's required properties must deserialize.
        // Adapt: if a required property is not a string, give it a valid example value.
        let mut example = serde_json::Map::new();
        for k in s["required"].as_array().unwrap() {
            example.insert(k.as_str().unwrap().to_string(), Value::String("x".into()));
        }
        let _: Input = serde_json::from_value(Value::Object(example)).expect("schema/Input drift");
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(matches!(run(r#"{"path":"a","needle":"b","extra":1}"#), Err(Fail::Input(_))));
    }
}
```

Notes on the template:
- **It compiles and works as is.** The agent's first build is of known-good code. It then changes the logic, never the scaffolding.
- **Bindings live in `mod bindings`, gated on `wasm32`.** This follows the Component Model book's `mod bindings { generate!(…); export!(…) }` pattern. Native `cargo test` never compiles generated code, so it does not depend on how wit-bindgen behaves on non-wasm targets (UNVERIFIED for 0.63.0).
- **`describe` returns constants.** `version` comes from `Cargo.toml`.
- **The `schema_is_an_object_schema_and_matches_input` test** is the guard for schema/parser drift that ext/wit.md §3.7 relies on.
- **Output is JSON built with `json!`.** Keys are sorted, because serde_json is used without `preserve_order`. Plain text is equally valid.
- **About 120 lines including tests.** It is close to the "150-line tool" that Phase 0 measures.
- **Two homes in the seed.** The template is the two files under `skills/ext-authoring/assets/template/`. The same two files are also the seed's reference extension `seed/ext/count-lines/` (`Cargo.toml`, `src/lib.rs`), a real crate and a member of the ext workspace. Its `src/lib.rs` and `Cargo.toml` are byte-identical to the template files. `ci/local.sh` and the ext host tests (AU-1, AU-2) build it. The template copy is still never built in place.

### 3.5 Build

```sh
cd "$INIT_HOME/body/ext/<name>" && cargo build --target wasm32-wasip2
```

- Output: `$CARGO_TARGET_DIR/wasm32-wasip2/debug/<name with '-' → '_'>.wasm`, for example `work/target/wasm32-wasip2/debug/count_lines.wasm`.
- `--offline` is implied by `body/.cargo/config.toml`, and harmless to add (testing.md's A2 script adds it).
- Success ends with ``Finished `dev` profile [optimized] target(s)``.
- `cargo build` in `body/ext/` (no `-p`) builds every extension. The gate runs the equivalent with `--manifest-path ext/Cargo.toml` (gate.md §3.3).

Common build errors and fixes (also in the skill):

| Error | Cause | Fix |
|---|---|---|
| ``can't find crate for `core` `` / `the wasm32-wasip2 target may not be installed` | target missing | owner runs `rustup target add wasm32-wasip2` (§3.2) |
| ``failed to resolve directory specified by `path` `` in `generate!` | wrong relative path | `path: "../../wit/init-ext/v1"` from `body/ext/<name>/` |
| ``cannot find trait `Guest` `` | `impl` outside `mod bindings` or wrong world | keep `impl Guest` inside `mod bindings`; `world: "init:ext/tool"` |
| `no matching package named …` / `failed to get … as a dependency` | crate not vendored | ask for `initctl vendor <crate>@<ver>` (forces full G4; owner approval assumed not needed, PENDING OWNER (CONVENTIONS §6 ESC-1)), or do without |
| ``expected `&RunRequest`, found `RunRequest` `` (or the reverse) | import parameter ownership (ext/wit.md §3.3) | add or remove the `&` as the compiler says |
| `current package believes it's in a workspace when it's not` | crate outside `body/ext/` | move it under `body/ext/` |

### 3.6 Expected build times (estimates; Phase 0 measures them)

All figures are **UNVERIFIED estimates** for the M2 (8-core, Apple Silicon) with the settings above. No published measurement of small `wasm32-wasip2` component build times was found (searched 2026-10-19).

| Case | Estimate | Dominated by |
|---|---|---|
| First build of the ext workspace (cold `work/target` for wasm32 + host macros) | 25–60 s | host builds of `syn` 3, `wit-parser`, `wit-component`, `wasm-metadata`, `serde_derive` (opt-level 2), then `serde` + `serde_json` for wasm32 |
| First build of a new extension when the workspace is warm | 2–5 s | the new crate (`opt-level = "s"`) and linking |
| **Warm rebuild after editing `lib.rs` of a 150-line extension** | **1–2.5 s** | rustc on the leaf crate (`generate!` expansion, LLVM at `s`), then `wasm-component-ld` (`wasm-ld` + component encoding, about 0.1–0.4 s) |
| No-op `cargo build` | 0.1–0.3 s | fingerprint checks |
| Host load at `initctl reload` (Cranelift) | 30–300 ms | ext/host.md §3.14 |
| Edit → usable | warm rebuild + reload + the next model turn | the model turn |

**Phase 0 measurement definition (for testing.md M-2):**
- **Subject:** the 150-line reference component `fixtures/ext/semver-sort/` (testing.md), placed at `body/ext/semver-sort/`, with this doc's workspace settings and vendored dependencies.
- **Warm:** after one successful build, `touch src/lib.rs`, then `cargo build --target wasm32-wasip2`. 5 runs; report the median and max of wall time.
- **Cold:** the same command with an empty `CARGO_TARGET_DIR` (an exact path, deleted afterwards). 3 runs; median.
- **Also record:** the `.wasm` size, the host `compile_ms` from the reload report, and the disk used by the target directory.
- **Threshold:** a warm median above **5.0 s** triggers the fallback lane (§3.10).

### 3.7 Testing an extension locally

Three levels, cheapest first. All run inside the sandbox with no network and no model spend.

| Level | Command | What it proves | Cost |
|---|---|---|---|
| L1 unit | `cd body/ext/<name> && cargo test` (G0 runs the ext workspace's tests the same way, gate.md §3.3) | the logic and the schema/input agreement, natively | about 1–3 s warm |
| L2 component | `"$INIT_HOME/bin/init-body" ext describe "$CARGO_TARGET_DIR/wasm32-wasip2/debug/<file>.wasm"` and `… ext call <wasm> --input '{"path":"README.md","needle":"INIT"}' --workspace "$PWD"` | the real component under the exact host config: type check, `describe` validity, WASI behavior, traps, timeouts | about 0.1–0.5 s |
| L3 live | `initctl reload`, then an `ext` tool call | the agent's actual tool path | one turn |

- `init-body ext inspect <wasm>` lists imports and exports. Use it to confirm the component imports nothing unexpected, for example `wasi:http`, which would fail to load.
- L2 uses the real data directory `state/ext/<name>/data/` unless `--data <dir>` is given. Pass `--data` with a throwaway directory to test from a clean state.
- L1 needs the body's host target and the vendored crates only.

### 3.8 Reload, use, keep

- **Reload:** `initctl reload` in a `bash` call. At the next turn boundary the host:
  - loads every `body/ext/<name>/` whose artifact was built in this session;
  - swaps the registry;
  - reports with a `reload` event, before the next model request (ext/host.md §3.9).

  The report gives the canonical schema of each added or changed extension. `$INIT_HOME/state/ext/catalog.json` (named in the `ext` tool's description, body/tools.md §3.6) lists every loaded extension with its schema; an `ext` call with an unknown name returns the list too.
- **Use:** `ext {"name": "<name>", "input": {…}}`. Errors start with `ext <name> <version>:` and, for traps and timeouts, include the last 4 KiB of the extension's stdout and stderr. The full per-call log is `state/ext/<name>/log`.
- **Keep:**
  1. Commit the crate (and `ext/Cargo.lock`, which gained the crate's entry) on a branch, with trailers: `Why`, `Predicts`, `Compensates-for`, `Prefix-change: no`, and `Repro` when relevant. Never commit the `.wasm`: components are built by the gate, not taken from git.
  2. `initctl promote <branch>`.
  3. The kernel builds the component itself from the commit (gate.md §3.3 step 6), runs G0–G3, publishes the kernel-built set to `bin/ext` and sends the body `reload{kind:"kept", components}` (supervise.md §3.6.8), on which the host switches from the trial to the kernel-built artifact (ext/host.md §3.8–§3.9). G4 decides LKG (evolution.md §3.4).
  4. Until kept, the trial dies at the next session boundary.

### 3.9 The skill (draft text of `skills/ext-authoring/SKILL.md`)

Agent Skills format (agentskills.io): YAML frontmatter with `name` (must equal the directory name, lowercase and hyphens) and `description` (≤ 1024 characters, says what and when). The body should stay under 500 lines; this draft is about 130.

````markdown
---
name: ext-authoring
description: Build, test, reload and keep a T1 extension, a WebAssembly component written in Rust for wasm32-wasip2 against WIT init:ext@1, that you call through the ext tool. Use when you need a new structured tool, a tool that wraps a CLI, or a long-lived helper process such as a REPL, and a bash one-off is not enough; also when an ext call fails to load, traps, times out or returns busy.
---

# Extensions (T1)

An extension is a Rust crate in `body/ext/<name>/` compiled to a WebAssembly
component. You call it with the `ext` tool: `{"name": "<name>", "input": {…}}`.
Adding or changing one never changes your prompt prefix, so your cache survives.

## When to use what
- One-off command or script → just use `bash`. No build, no reload.
- A tool you will call repeatedly, with structured JSON input/output, or a helper
  process that must stay alive across calls (REPL, server) → an extension.
- Behavior on every step (loop, context, prompt) → a T2 change (git worktree +
  `initctl promote`), not an extension.

## Recipe
1. Pick a name: kebab-case, `^[a-z][a-z0-9-]{0,31}$`, not read/write/edit/bash/ext.
2. Scaffold:
   `cp -R "$INIT_HOME/body/skills/ext-authoring/assets/template" "$INIT_HOME/body/ext/<name>"`
   `mv "$INIT_HOME/body/ext/<name>/Cargo.toml.template" "$INIT_HOME/body/ext/<name>/Cargo.toml"`
   Set the name in `Cargo.toml` (`name = "<name>"`) and in `src/lib.rs` (`const NAME`).
   Keep `crate-type = ["cdylib"]`.
3. Write the logic in plain functions (`count`/`run` in the template). Keep
   `mod bindings` as it is, except `describe` text and the `call` mapping.
   Update `DESCRIPTION` and `SCHEMA` to match your `Input` struct.
4. Test the logic: `cd "$INIT_HOME/body/ext/<name>" && cargo test`
5. Build: `cd "$INIT_HOME/body/ext/<name>" && cargo build --target wasm32-wasip2`
   Output: `$CARGO_TARGET_DIR/wasm32-wasip2/debug/<name with - as _>.wasm`
6. Try it with the real host config, no reload needed:
   `"$INIT_HOME/bin/init-body" ext call <that .wasm> --input '<json>' --workspace "$PWD"`
7. `initctl reload`. The next `reload` event lists `+ <name>` or `! <name>: <error>`.
8. Call it: `ext {"name": "<name>", "input": {…}}`. What is loaded, with input
   schemas: `jq . "$INIT_HOME/state/ext/catalog.json"`.

## The contract (init:ext@1, world `tool`)
- `describe()` returns `ToolInfo { name, description, input_schema, version, timeout_ms }`.
  It must be constant: no file access, no process calls, same value every time.
  `name` must equal the directory name. `input_schema` is JSON Schema text with
  `"type":"object"` at the root; allowed keywords: type, properties, required,
  items, enum, const, description, additionalProperties, default, minimum, maximum,
  minLength, maxLength, minItems, maxItems. No $ref/oneOf/anyOf/allOf.
- `call(input: String) -> Result<String, ToolError>`. `input` is your `input`
  object as JSON with sorted keys. Parse it with serde (`deny_unknown_fields`).
  Return `ToolError::InvalidInput(msg)` for bad input (the caller should fix it),
  `ToolError::Failed(msg)` when the work fails. The Ok string is shown verbatim.
- Files: `/work` is the task workspace, `/data` is your own persistent directory.
  Nothing else exists. Use absolute paths (`/work/src/x.rs`).
- No network. `wasi:http` is not available; a crate that needs it will not load.
- `println!`/`eprintln!` go to a 1 MiB per-call log: shown to you (last 4 KiB) only
  when a call traps, times out or exits, always appended to `state/ext/<name>/log`.
  Past 1 MiB, printing panics. Stdout is line-buffered: a `print!` without a
  trailing newline is lost when the call ends.
- Limits: 256 MiB memory, 30 s per call by default (`timeout_ms: Some(ms)`, up to
  600000), 1 MiB input and output. Never sleep: a sleeping call cannot be
  interrupted and blocks that extension until it wakes.
- Each call is a fresh instance: keep state in `/data` files, not in statics.
- Concurrent calls of the same extension can run at once and share `/data`.

## Running processes (init:ext/process)
Same authority as `bash`; paths are guest paths (`/work`, `/data`, below them).
Process functions exist only on wasm32, so call them from inside `mod bindings`:

```rust
// inside `mod bindings { … }`
use init::ext::process::{self, RunRequest};

fn git_status() -> Result<String, ToolError> {
    let out = process::run(&RunRequest {
        argv: vec!["git".into(), "status".into(), "--porcelain".into()],
        cwd: "/work".into(), env: vec![], stdin: vec![], timeout_ms: 10_000,
    }).map_err(|e| ToolError::Failed(format!("{e:?}")))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
```
Pass the raw output to a plain function outside `mod bindings` for parsing, so
`cargo test` can still test the parsing.

Long-lived helper (REPL): `process::spawn(&SpawnRequest { argv, cwd, env, pty: false })`
returns a session id (u32) that survives later calls. Keep the id in `/data`.
`process::write(id, b"1+1\n")`, then `process::read(id, 2000, Some(">>> "))`
waits up to 2 s for the prompt. `process::close(id)` ends it. At most 4
sessions per extension; idle sessions die after 30 min; all die when the body
restarts. `pty: true` is denied (no pseudo-terminal): run REPLs in unbuffered
or interactive mode (`python3 -u -i`, `node -i`).
<!-- Homebrew tools such as node are visible only if /opt/homebrew is readable in the sandbox: PENDING OWNER (CONVENTIONS §6 ESC-2), default yes. -->
If the compiler says `expected RunRequest, found &RunRequest` (or the reverse),
add or remove the `&`.

## When something fails
| Symptom | Meaning | Fix |
|---|---|---|
| `! <name>: does not match init:ext@1: …` | exports or imports wrong | keep the template's `mod bindings`; check `world: "init:ext/tool"` |
| `! <name>: … import wasi:http …` | a dependency wants network | drop that dependency |
| `! <name>: describe …` | describe trapped, did I/O, or varies | make describe return constants |
| `! <name>: name … does not match directory` | names differ | fix `NAME` or the directory |
| `· not built: <name>` | no artifact from this session | build (step 5), then reload |
| `timed out after N ms` | infinite loop, or too slow | fix, or raise `timeout_ms` |
| `aborted by owner interrupt` | the owner stopped the turn | nothing to fix; wait for the owner |
| `… blocked in a host call` / `busy` | the call slept or blocked | remove sleeps; wait for it to end |
| `out of memory` | over 256 MiB | stream through `/data` |
| `trap: stack overflow` or `out of bounds memory access` in a recursive function | runaway recursion (Rust's stack lives partly in linear memory) | bound the recursion or use a loop |
| `panicked (see log below)` | Rust panic; message is in the log tail | fix the panic |
| `unknown extension` | not loaded | read the list in the error (or `state/ext/catalog.json`); reload |

## Keeping it
A trial lasts until the next session. To keep it, commit `body/ext/<name>/`
and `body/ext/Cargo.lock` on a branch with trailers (Why, Predicts,
Compensates-for, Prefix-change: no, Repro if it fixes an observed failure),
then `initctl promote <branch>`. Never commit the .wasm. The kernel rebuilds it
from source, gates it and publishes it. Unused extensions are listed for
retirement later.

## Don't
- Don't put state in statics or expect it to survive a call.
- Don't sleep or poll in a loop waiting for time to pass.
- Don't read anything outside `/work` and `/data`; it does not exist for you.
- Don't edit `body/wit/`: it must equal `kernel/wit/` or the gate fails.
- Don't add crates casually: `initctl vendor` forces the full gate.
  <!-- PENDING OWNER (CONVENTIONS §6 ESC-1): whether vendoring also needs owner approval; default no. -->
- Don't make the body depend on an extension crate, and keep its crate type
  cdylib: extension code must never end up inside the body binary.
````

### 3.10 The script fallback lane (Luau / QuickJS)

Spec §4 and §11 keep a script lane in reserve "in case Wasm build latency turns out too high". This section fixes when to add it and what it would be. It is **not** in the seed.

**Decision criteria.** These are measured in Phase 0 on the M2 with warm caches and §3.3's settings.

| # | Criterion | Threshold | Action |
|---|---|---|---|
| C1 (primary) | Median warm rebuild of the 150-line reference component (§3.6 definition) | **> 5.0 s** | add the lane |
| C2 | Median "edit → usable" over the M-3 authoring trials (from writing `lib.rs` to the first successful `ext` call), excluding model think time | > 10 s | add the lane |
| C3 | testing.md M-3 success rate, after the template/skill revision M-3 prescribes when it is below 6/10 | < 6/10 on the rerun **and** ≥ 3 failures classed as toolchain (M-3's classes "WIT mismatch", "`wit-bindgen` API misuse", "Cargo/target config"), not "logic" | add the lane (testing.md's < 3/10 case also lands here, as roadmap risk R1) |
| — | **Not** triggers | | cold builds (paid once); host `compile_ms` (fix with ext/host.md §3.14's revisit options); the model's own logic errors |

**Phase 0 result for C1 (WP14, M2, 2026-10-20).** The M-2 warm rebuild of `count-lines` (`touch src/lib.rs`, then `cargo build --offline --target wasm32-wasip2`, 5 runs) had a **median of 380 ms and a maximum of 385 ms**, against the §3.6 estimate of 1–2.5 s and the 5.0 s threshold. Each run was a real recompile, with `CARGO_INCREMENTAL=0` and no lock waits. **C1 is not triggered, so the script lane is not needed on this evidence.** The subject was `count-lines` (about 120 lines), not the 150-line `semver-sort` fixture, so that run is still to do.

**Engine choice, if triggered:**

| Criterion | JavaScript (QuickJS via rquickjs) | JavaScript (boa, pure Rust) | Luau (mlua) |
|---|---|---|---|
| Frontier-model fluency | high | high | lower: the Roblox dialect is less common in training data |
| Builds for `wasm32-wasip2` with the existing toolchain | no: needs a C toolchain (wasi-sdk, roughly 100+ MB, UNVERIFIED) once, on the maintainer's side | **yes**, plain cargo | no: needs C++ (wasi-sdk) |
| Speed for glue code | fast (QuickJS release 2026-06: full runtime life cycle < 300 µs natively) | slower, but irrelevant for glue | fast |
| Component size and Cranelift compile at load | ~1–2 MB, sub-second (UNVERIFIED) | larger, about 5–10 MB, 1–3 s once per process (UNVERIFIED) | ~1 MB |
| Sandbox | inside Wasm: the same as any extension | same | same |

**Recommendation:** JavaScript, with the engine running **inside a Wasm component**, not in the body process.
- Engine: QuickJS (rquickjs) if the maintainer can build the engine component once with wasi-sdk; otherwise boa.
- Why inside Wasm:
  - it adds no body dependency (CONVENTIONS §5 fixes the body's dependencies);
  - it keeps the same epochs, limits, preopens and `process` interface;
  - it avoids in-process engines' weaker sandboxes, for example the mlua#488 abort under a memory limit.

**Outline, if triggered:**
1. **Engine component `js`.** An ordinary `init:ext@1` `tool` component, built once by the maintainer, shipped with the seed, and published by the kernel in the kept set (`bin/ext/js.wasm` through the `bin/ext` symlink, CONVENTIONS §3).
   - It reads a script from a read-only preopen `/script`, evaluates it, and calls the script's `describe()` and `call(input)` functions.
   - It exposes `process.run`/`spawn`/`write`/`read`/`close` and file access under `/work` and `/data` to JavaScript.
2. **Scripts as extensions.** A file `body/ext-js/<name>.js` becomes registry entry `<name>`, of kind `Script`, sharing the `js` component's `InstancePre`.
   - Per call, the host adds the preopen `/script` → `body/ext-js/` (read-only) and the env var `INIT_SCRIPT=<name>.js`.
   - About 40 lines in `ext_host.rs`: one more registry kind, one more preopen.
   - No WIT change.
3. **Reload = re-read.** `initctl reload` re-reads changed `.js` files and calls their `describe` (milliseconds). No build step exists.
4. **Gate.** Scripts are code, so keeping one takes G0–G3 like a component. G0 for scripts is a parse check through `describe`.
5. **Smallest first step** (zero host change): ship only the `js` component, with input `{"script": "/work/x.js", "input": {…}}`. This is usable at once but invisible to the inventory. Promote to step 2 when the lane is actually used.
6. **Exit.** Remove the lane (retirement, evolution.md) if a later toolchain brings warm builds under 2 s and the lane goes unused for N tasks.

## 4. Interfaces

| Item | Value |
|---|---|
| Extension source | `$INIT_HOME/body/ext/<name>/{Cargo.toml, src/lib.rs, …}`, a member of the ext workspace `body/ext/Cargo.toml` |
| Name rule | `^[a-z][a-z0-9-]{0,31}$`, no `--`, no trailing `-`, not `read`/`write`/`edit`/`bash`/`ext`; directory name = package name = `tool-info.name` |
| Build command | `cd "$INIT_HOME/body/ext/<name>" && cargo build --target wasm32-wasip2` |
| Artifact | `$CARGO_TARGET_DIR/wasm32-wasip2/debug/<name with '-' → '_'>.wasm` (`CARGO_TARGET_DIR` defaults to `$INIT_HOME/work/target`, CONVENTIONS §3) |
| Template | `$INIT_HOME/body/skills/ext-authoring/assets/template/{Cargo.toml.template, src/lib.rs}` |
| Discovery | `$INIT_HOME/state/ext/catalog.json` (named in the `ext` description); unknown-name errors list loaded extensions |
| WIT path from a crate | `../../wit/init-ext/v1` |
| Local test CLI | `init-body ext describe|call|inspect` (ext/host.md §4.5) |
| Reload / use / keep | `initctl reload` / `ext` tool / commit + `initctl promote` |

## 5. Failure modes and recovery

| Failure | Effect | Recovery |
|---|---|---|
| `wasm32-wasip2` not installed | build error | owner installs it once (§3.2); testing.md lists it as an A2 precondition |
| Crate not vendored | offline build error | `initctl vendor` (forces G4; approval rule PENDING OWNER (CONVENTIONS §6 ESC-1)) or avoid the crate |
| Agent edits the template instead of copying it | only the template changes, nothing is built | the skill says copy; the template is never in the build |
| Agent builds in a worktree, not the live tree | artifact lands in the shared target dir, but the crate is not in `body/ext/`, so reload reports nothing | build trials in the live tree (skill recipe); worktrees are for keeping |
| Two crates with the same name (live tree and worktree) | shared artifact path; last build wins | rebuild in the live tree before `initctl reload` |
| Toolchain upgrade changes WASI patch versions imported by std | none: the linker matches 0.2.x semver-compatibly (ext/wit.md §3.4) | — |
| `wit-bindgen` 0.63 API differs from the template (UNVERIFIED items) | compile error in the template | fix the template once in Phase 0 (M-3 setup); the skill table covers the known ownership case |
| `body/ext/` contains a non-crate directory, for example a `target/` | hard workspace error: every cargo command fails (measured at WP0) | keep only crates there; set `CARGO_TARGET_DIR` outside `body/ext/` |
| Template copied without renaming `Cargo.toml.template` | the directory is not a crate; build says "package ID specification … did not match" | `mv Cargo.toml.template Cargo.toml` (skill step 2) |
| Extension crate with a crate type other than `cdylib`, or a body crate depending on one | G0 fails (gate.md follow-up E9) | restore the template's `[lib]` lines; remove the dependency |

## 6. Performance notes

- **Rank 4 (self-edit latency):** the warm rebuild is the critical number (target ≤ 5 s, estimate 1–2.5 s). It drives:
  - one shared workspace and lockfile;
  - `opt-level = "s"` on a tiny leaf crate;
  - optimized proc macros;
  - stripped debug info, which also cuts host compile time.
- **Rank 1 (cache):** nothing in this workflow changes the prefix. Extension schemas reach the model through reload reports in the tail.
- **Disk:** the ext share of `work/target` is estimated at 300–700 MB (host macro crates plus wasm32 builds; UNVERIFIED). Phase 0 records it. Cleanup is by exact path only (CONVENTIONS §5).

## 7. Invariants and security

- **Invariant 3 (blast radius).**
  - Building runs inside the body's sandbox with no network; dependencies come only from `vendor/`.
  - Extensions get only `/work`, `/data` and `bash`-equivalent processes (ext/wit.md §7).
- **Invariant 2 (honest measuring stick).** The kernel never uses an agent-built `.wasm`. On keep it rebuilds from the committed source in its own target directory (evolution.md §3.4).
- **Invariant 4 (lineage).** A kept extension is a commit with trailers. Trials are ephemeral and leave no lineage.
- **The contract is read-only in effect.** `body/wit/` must equal `kernel/wit/` (G1). The skill says so.
- **No `unsafe` needed in extensions.** The template has none (wit-bindgen's generated code has its own). Extension crates are not under the body's `forbid`, because they run inside Wasm, where `unsafe` cannot reach host memory. gate.md §3.3 step 3 exempts them (R1). That exemption must not become a way to put `unsafe` into the body binary, so the gate should also require extension crates to be cdylib-only and refuse any body-workspace dependency on one (follow-up E9).

## 8. Test plan

| Id | Case | Expected | Maps to |
|---|---|---|---|
| AU-1 | Copy the template unchanged into `body/ext/count-lines/`; `cargo test`; build; `init-body ext describe` | 3 tests pass; component builds; `describe` valid | template is known-good |
| AU-2 | AU-1 + `ext call` with `{"path":"README.md","needle":"INIT"}` on a fixture | `{"count":…,"lines":[…]}` | |
| AU-3 | Rename the template to `semver-sort` with testing.md's fixture logic | builds; A2 mock script passes | A2 |
| AU-4 | Warm/cold build timing per §3.6 | numbers recorded; C1 evaluated | M-2 |
| AU-5 | 10 agent authoring trials using only the skill | success rate and failure causes recorded; C2/C3 evaluated | M-3 |
| AU-6 | The process snippet from the skill, built in a scratch extension | compiles (or the `&` hint fires and is resolved in one edit) | skill accuracy |
| AU-7 | Skill frontmatter | `name` = `ext-authoring` = directory; `description` ≤ 1024 characters (`skills-ref validate`, if available) | Agent Skills spec |
| AU-8 | `members = ["*"]` with a stray non-crate directory in `body/ext/` | record Cargo's behavior; update §5 | UNVERIFIED item; settled at WP0: hard error |
| AU-9 | The template with `#![forbid(unsafe_code)]` added as its first line, built for wasm32 | record whether wit-bindgen's `export!` compiles under `forbid` (expected: no). Evidence for gate.md's exemption | E9 |
| AU-10 | `cd body/ext && cargo clippy --workspace --all-targets -- -D warnings` on the host with the template as a member | clean (the `cfg_attr(… allow(dead_code))` line covers the wasm-only code paths) | G0 `gate.g0.ext_clippy` |

Ids in "Maps to" are testing.md scenario and measurement ids (roadmap.md Q2).

## 9. Open questions

### 9.1 Local

- **Q-A1.** The real warm and cold build times and component sizes (§3.6). Phase 0 M-2. The warm median on `count-lines` is 380 ms (§3.10); cold builds and the `semver-sort` run are still open.
- **Q-A2.** Whether 0.63.0's generated import functions take `&RunRequest` and `Option<&str>`. Settled at WP14: they take borrowed arguments (`process::run(&RunRequest)`, `spawn(&SpawnRequest)`, `write(id, &[u8])`, `read(id, ms, Option<&str>)`), and the skill snippet compiles verbatim (AU-6 is in the test fixture).
- **Q-A3.** Disk cost of the `wasm32-wasip2` target and of the ext part of `work/target`. Phase 0 records both before the 20 GB precondition is checked.

### 9.2 Cross-doc

Settled in R1 and R2 (ledger docs/reconciliation/ext.md): the seed files (body/boot.md §3.16), G0's ext build and tests in the build slot (kernel/gate.md §3.3, kernel/supervise.md §3.6.2), `vendor --sync` (kernel/initctl.md), toolchain access (kernel/sandbox.md), testing.md's A2, M-2 and M-3 alignment, evolution.md's keep flow, and the template's contents (body/boot.md ships `skills/ext-authoring/` as this doc defines it, so `Cargo.toml.template` needs nothing there). Remaining:

- **→ kernel/gate.md (E9, E10; evolution-testing AU-3 asks the same for E10):**
  - compensating checks for the `forbid` exemption: every ext-workspace member has `crate-type = ["cdylib"]` only, and no body-workspace member depends on an ext crate; justify the exemption by the Wasm boundary alone, since "wit-bindgen's generated exports are not reported by the lint anyway" is UNVERIFIED and likely wrong for `export!` (AU-9 settles it);
  - an `ext/Cargo.lock` (or `Cargo.lock`) diff that only adds, removes or changes source-less (workspace-member) entries is not a dependency change: classify it `t1`.

## 10. Spec deltas

| Spec v0.2 | This doc | Why |
|---|---|---|
| §6.1: `cargo build --target wasm32-wasip2` | the same command, run in `body/ext/<name>/`; one ext workspace, dev profile tuned for small output | one artifact path; shared dependency builds; smaller components compile faster in the host |
| §4, §11: "a Luau or QuickJS lane" | if triggered, a JavaScript engine **inside a Wasm component**, loaded like any extension; QuickJS preferred, boa if no C toolchain | no new body dependency; the same sandbox and limits; better model fluency than Luau |
| §10 Phase 0 #2: threshold "warm build > 5 s → Luau lane" | kept as criterion C1, plus C2 (edit → usable > 10 s) and C3 (M-3 rerun < 6/10 with ≥ 3 toolchain failures) | the build is not the only source of authoring friction |
| — | template location `skills/ext-authoring/assets/template/`, with `Cargo.toml.template` | Agent Skills `assets/` convention; not a crate, so gate.md's every-`Cargo.toml`-is-a-member rule holds |
| §4 "`#![forbid(unsafe_code)]` in the body" | extension crates are exempt (they run in Wasm; gate.md R1), with cdylib-only and no-dependents checks requested | wit-bindgen's `export!` needs `unsafe` attributes; the body binary stays `unsafe`-free |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.18):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP0 D2 | a non-crate subdirectory under `members = ["*"]` is a hard cargo error, so `CARGO_TARGET_DIR` must point outside the ext workspace; the UNVERIFIED mark is gone | §3.3, §5, §8 (AU-8) |
| build: WP0 D6, WP14 Done | the template has two homes: `skills/ext-authoring/assets/template/` and the seed's reference extension `seed/ext/count-lines/`, byte-identical | §3.4 |
| build: WP14 F (Q-A2) | imports take borrowed arguments in wit-bindgen 0.63.0, and the skill snippet compiles verbatim; the question is settled | §9.1 |
| build: WP14 D6 | SKILL.md has two lines beyond the draft: the line-buffered stdout note and a failure-table row for runaway recursion | §3.9 |
| build: WP14 Measurements | M-2 warm rebuild median 380 ms (max 385 ms) on `count-lines`, so C1's 5.0 s is not triggered and the script lane is not needed; the `semver-sort` run is still to do | §3.10, §9.1 |

## 11. References

All checked 2026-10-19.

- Rust platform support (`wasm32-wasip2` Tier 2, std): https://doc.rust-lang.org/rustc/platform-support.html ; target spec: https://doc.rust-lang.org/stable/nightly-rustc/src/rustc_target/spec/targets/wasm32_wasip2.rs.html
- Tier 2 since 1.82: https://blog.rust-lang.org/2024/11/26/wasip2-tier-2/
- `wasm-component-ld` in host toolchains: https://github.com/rust-lang/rust/pull/126967 ; https://github.com/bytecodealliance/wasm-component-ld
- Component Model book, Rust (cargo-component deprecation, plain cargo flow, `mod bindings` pattern): https://component-model.bytecodealliance.org/language-support/building-a-simple-component/rust.html
- cargo-component releases: https://api.github.com/repos/bytecodealliance/cargo-component
- wit-bindgen README, `generate!` options, features and dependencies: https://github.com/bytecodealliance/wit-bindgen ; https://docs.rs/wit-bindgen/latest/wit_bindgen/macro.generate.html ; https://crates.io/api/v1/crates/wit-bindgen/0.63.0 ; https://crates.io/api/v1/crates/wit-bindgen-rust/0.63.0/dependencies
- Cargo workspaces (globs, config discovery): https://doc.rust-lang.org/cargo/reference/workspaces.html ; https://doc.rust-lang.org/cargo/reference/config.html
- Library file name dashes → underscores: https://doc.rust-lang.org/cargo/reference/cargo-targets.html
- Wasm size with and without stripping: https://docs.wasmtime.dev/examples-minimal-cwasm.html
- Agent Skills specification: https://agentskills.io/specification
- Zed's builder (plain cargo, `wasm32-wasip2`): https://raw.githubusercontent.com/zed-industries/zed/main/crates/extension/src/extension_builder.rs ; https://github.com/zed-industries/zed/pull/30953
- QuickJS (release 2026-06-04): https://bellard.org/quickjs/ ; mlua memory-limit abort: https://github.com/mlua-rs/mlua/issues/488 (both via research_notes/…/live_modification_isolation.md)
- Spec v0.2 §4, §10, §11; reports/Ngôn ngữ cho INIT Agent.md (T1 section).
