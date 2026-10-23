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
A trial lasts until the next session. To keep it, refresh `self.json` with
`"$INIT_HOME/kernel/bin/init-kernel" selfmap --root "$INIT_HOME/body" --write`,
then commit it with `body/ext/<name>/` and `body/ext/Cargo.lock` on `evo/<id>`.
This T1 self-map/build requirement applies to extension code; a free T0 change
confined to skills or prompt files uses the direct route in `skills/extend/SKILL.md`.
On the free path, `Why:` and `Prefix-change:` are required; `Predicts:`,
`Compensates-for:` and `Repro:` are optional. The gated path requires the full
trailer matrix in `skills/evolve/SKILL.md`. Use `Prefix-change: yes` if the change
also alters a prompt file or a skill's name/description, otherwise `no`.
Run `initctl promote evo/<id> --wait`. Never commit the .wasm. The kernel rebuilds
the component from source, runs the scope's checks and publishes it. Free admission
opens the Root's harm window; it has no mandatory reward gate. Unused extensions
are listed for retirement later.

## Don't
- Don't put state in statics or expect it to survive a call.
- Don't sleep or poll in a loop waiting for time to pass.
- Don't read anything outside `/work` and `/data`; it does not exist for you.
- Don't edit `body/wit/`: it must equal `kernel/wit/` or the gate fails.
- Don't add crates casually: `initctl vendor` forces the full gate.
  <!-- PENDING OWNER (CONVENTIONS §6 ESC-1): whether vendoring also needs owner approval; default no. -->
- Don't make the body depend on an extension crate, and keep its crate type
  cdylib: extension code must never end up inside the body binary.
