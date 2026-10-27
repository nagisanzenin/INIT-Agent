# Body boot (body side)
Status: v0.3 + build sync 2026-10-20 + ADR-009 (v0.1.1) · Area: body
Depends on: CONVENTIONS.md, kernel/supervise.md, kernel/selfmap.md, kernel/gate.md, body/provider.md, body/loop.md, body/tools.md, body/log-context.md, ext/host.md, awareness.md, testing.md, evolution.md, ext/authoring.md   Defines: the body-side boot order (which component runs when), the boot-time `body/` git sync (contract: kernel/supervise.md §3.11.5), the seed crate build configuration (`seed/Cargo.toml`, `seed/.cargo/config.toml`), the body's validation of env, `boot.json` and process facts, the three body modes (normal, probe, safe) and what each does, the probe round trip and its instruction, the body's extra fields in `ready` and `heartbeat` (base formats: kernel/supervise.md §4.4), `init-body --manifest` (tool registry for kernel/selfmap.md) and `--version`, argv dispatch of `init-body ext …` (commands defined by ext/host.md §4.5), body exit sub-codes, and the boot facts handed to log-context's `boot` event

---

## 1. Purpose

Every body process must turn kernel-provided facts (env vars, `state/kernel/boot.json`) and a durable log into a running agent that continues the same conversation. The cases are:
- a cold start;
- a crash restart;
- a rollback to LKG;
- the overlapped handoff after a promote;
- a G2/G3 probe in a staging root.

Boot is where the body tells the kernel, through `ready` and the heartbeat, that it is alive and what it is. It is also the path a candidate must survive to pass G2 (spec §6.3). This doc fixes the order of the steps, the owner of each step, its failure code and its time budget.

## 2. Responsibilities / non-goals

**Responsibilities**
- Argv dispatch (`--manifest`, `--version`, `ext …`) and the panic hook.
- Validating env, `boot.json` and process facts (cwd, bash, kill, socket path lengths).
- The model check.
- Ordering: extension host start, housekeeping, log-context rehydration, the probe, `ready`, the heartbeat, serving.
- Mode behavior: normal, probe, safe.
- The probe round trip (probe mode).

**Non-goals** (owned elsewhere; this doc only calls them)
- The env contract, `boot.json`, `ready`/`heartbeat` base formats, exit codes 0/75, rollback, LKG, binary swaps, crash backoff, safe-mode entry: kernel/supervise.md. The body never touches `bin/` or `state/kernel/`.
- Session selection, the lock, scanning, torn-tail repair, interrupted-call repair, the head and `prefix_hash`, the `boot` event schema and its durability: body/log-context.md §3.4–§3.6.
- The boot block text and the environment snapshot: awareness.md.
- The Wasmtime host, and the `ext describe|call|inspect` subcommands themselves: ext/host.md.

## 3. Design

**Execution order** (the section numbers group topics; this list is the order the code runs):

| # | Step | § | Fatal? |
|---|---|---|---|
| 1 | argv dispatch, panic hook | 3.1 | 64 on bad argv |
| 2 | env and process facts | 3.2 | 78 |
| 3 | `boot.json` | 3.3 | 78 |
| 4 | model check | 3.4 | 78 |
| 5 | `body/` git sync, then housekeeping | 3.6 | no (warnings) |
| 6 | `ExtHost::new` (queues the Boot load; not awaited) | 3.5 | no |
| 7 | rehydration: lock, scan, repair, head, `boot` event | 3.7, 3.8 | 1 (lock, log I/O) |
| 8 | probe (probe mode only) | 3.9 | 70 |
| 9 | `ready` | 3.11 | 70 |
| 10 | heartbeat thread | 3.12 | no |
| 11 | serve | 3.13 | — |

The sync runs before the extension host and before the head is built, because both read `body/`.

### 3.1 Entry, argv, panic hook

```rust
#![forbid(unsafe_code)]                                   // crate root: src/main.rs
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        None => {}
        Some("--manifest") if args.len() == 1 => return manifest::print(),     // §4.3: no env, no fs, no sockets
        Some("--version")  if args.len() == 1 => { println!("init-body {}", env!("CARGO_PKG_VERSION")); return ExitCode::SUCCESS; }
        Some("ext") => return ext_host::cli(&args[1..]),                      // ext/host.md §4.5: describe | call | inspect
        _ => { eprintln!("usage: init-body [--manifest | --version | ext <describe|call|inspect> …]"); return ExitCode::from(64); }
    }
    std::panic::set_hook(Box::new(|info| {                 // one JSON line on stderr, then unwind as usual
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let loc = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let th = std::thread::current().name().unwrap_or("?").to_string();
        eprintln!("{}", serde_json::json!({"type":"panic","message":msg,"location":loc,"thread":th}));
    }));
    match boot::run() {
        Ok(code) => code,                                  // from Session::serve(): 0 or 75
        Err(e) => { eprintln!("{}", e.to_json_line()); ExitCode::from(e.exit_code()) }   // §4.4
    }
}
```

- The hook receives `&PanicHookInfo`, which has `payload()` and `location()`.
- A panic in a tool thread is caught by the loop (`catch_unwind`, loop.md §3.7), logged, and survived. A panic on the main thread exits with Rust's default 101, which is a crash.
- supervise copies stderr to `state/kernel/logs/body-<boot_seq>.stderr`, so the panic line is there for the agent to read after a restart.

### 3.2 Environment and process facts

kernel/supervise.md §4.1 builds the whole environment (`env_clear()` first). The body validates what it uses (step budget < 5 ms). A failure exits 78 with `{"type":"boot_error","step":"env",…}`.

| Var | Check |
|---|---|
| `INIT_HOME` | absolute path to an existing directory. supervise caps it at 64 bytes, so socket paths fit |
| `INIT_SESSION` | `^[0-9A-Za-z._-]{1,64}$`. The production kernel always sends an id, choosing a new one when `state/session` is missing or invalid (supervise §3.4). Empty happens only in tests and manual runs; log-context then generates a new id `YYYYMMDD-HHMMSS-<6 hex>` (UTC) at rehydration and writes `state/session` (§3.7). |
| `INIT_SHA` | `^([0-9a-f]{40}\|[0-9a-f]{64})$` |
| `INIT_BOOT_REASON` | `cold` · `promote` · `rollback` · `crash` · `handoff` |
| `INIT_MODE` | `normal` · `probe` · `safe` |
| `INIT_BOOT_ID` | `^[0-9a-f]{16}$`; must equal `boot.json.boot_id` (§3.3) |
| `INIT_UI` | must be `stdio-jsonl/1`, the only protocol this body speaks (loop.md §4.1) |
| `INIT_KERNEL_SOCK`, `INIT_PROXY_SOCK` | absolute; byte length ≤ 103 on macOS (`sun_path` is 104 bytes with the NUL), ≤ 107 on Linux. Verified locally: a 249-byte path fails `bind` with "path must be shorter than SUN_LEN". Existence is **not** checked: the proxy may be restarting, and provider.md retries `Connect` |
| `INIT_KERNEL_VERSION` | informational; copied into the boot facts |
| `INIT_WORKSPACE` (v0.1.1, ADR-009) | informational, never exit 78. Empty or absent (an older kernel) means no workspace. A value that is not absolute is ignored with a boot-facts warning. Set by the Root on every `body`-profile spawn, overwriting the kernel's value (root/root.md §3.17.2): the workspace the owner ran `init-agent` in, or `""` |
| `INIT_ACCESS` (v0.1.1, ADR-009) | informational, never exit 78: `none` · `read_home` · `workspace` · `full`, or a newer value kept verbatim. It must match `^[a-z_]{1,32}$`; otherwise it is ignored with a warning. Absent (an older kernel) means no access line in the boot block. Set by the Root with `INIT_WORKSPACE`, so both always match the instance's sandbox (kernel/sandbox.md §3.1.1) |

The body cannot `stat` `$INIT_KERNEL_SOCK`: its sandbox profile grants `file-read-metadata` on `RUN` only, not on the socket file, so `stat` fails with EPERM and `[ -S "$INIT_KERNEL_SOCK" ]` is always false. Connecting works, so probe the socket by connecting.

Process facts, recorded in the boot facts (§3.8) and the environment snapshot (awareness.md):
- **cwd** = `std::env::current_dir()`: `$INIT_HOME/work` for the live body, `<probe root>/work` for a probe (supervise §3.4 `LaunchSpec.cwd`). It is captured once and passed to the tools as `ToolCx.paths.cwd` (`ToolCx.paths: PathCx`, body/loop.md §4.2). The body never calls `set_current_dir`. **v0.1.1 (ADR-009):** when `INIT_WORKSPACE` names an existing directory, the cwd fact, and so the tools' starting directory, is the workspace instead. The spawn's process cwd stays under `INIT_HOME` (root/root.md V4), and is used only when the workspace is not a directory at boot, with a warning in the boot facts. `HOME` stays `$INIT_HOME/work/home`, so the owner's home is reached by its absolute path.
- **bash** = the first absolute `PATH` entry that contains a regular file `bash` with an execute bit (`std::os::unix::fs::PermissionsExt::mode() & 0o111 != 0`); else `/bin/bash`; else none, which becomes a warning and makes the bash tool return `spawn_failed`. supervise's macOS `PATH` puts `/opt/homebrew/bin` before `/bin`, so on the owner's Mac this resolves to GNU bash 5.3.20 rather than `/bin/bash` 3.2.57 (both seen locally), **provided the sandbox lets the body read `/opt/homebrew`**: PENDING OWNER (CONVENTIONS §6 ESC-2), recommended yes and used here as the provisional default. With "no", the `stat` of that entry fails, the lookup moves on, and the facts say `/bin/bash` 3.2.57. Its version comes from one `bash --version` (first line, ~3 ms).
- **kill** = the first of `/bin/kill`, `/usr/bin/kill` that exists (metadata only); else `bash-builtin` (tools.md §3.5.4).

### 3.3 `boot.json` (the body side of "the rollback check")

`$INIT_HOME/state/kernel/boot.json` is **required**, written by the kernel before every spawn, including into each probe root (supervise §4.3). Missing, unparsable or inconsistent gives exit 78. The body reads it and never writes it.

| Field | Body use |
|---|---|
| `boot_id` | must equal `INIT_BOOT_ID`; echoed in `ready` and `heartbeat` |
| `mode`, `reason` | must equal `INIT_MODE`, `INIT_BOOT_REASON` |
| `model` = `{provider, model, effort}` | selects the adapter and model (§3.4); `effort: null` means `ModelInfo.default_effort` |
| `update_failed` | the latest failure not yet delivered to a body, or null. Mirrored into the boot event and boot block (log-context, awareness); deduplicated by its `seq` |
| `lkg_tree` | safe mode reads T0 data (genesis, `SELF.md`, skills) from `$INIT_HOME/<lkg_tree>/` instead of `body/` (§3.14) |
| `provisional`, `safe`, `panic`, `pending`, `selfmap`, `evolution`, `restarts_since_lkg`, `from_sha`, `lkg_sha`, … | carried through verbatim in the boot facts (`kernel` object) for log-context and awareness |

Spec §4 says boot "runs the rollback check before anything that can fail". Restoring `init-body.prev`, choosing LKG and rolling back are all done by the kernel before it spawns the body (supervise §3.9). The body's part is to *read the outcome first*: `boot.json` is read before the lock, the log or the network. The separate `update-failed.json` is not read: `boot.json.update_failed` carries the undelivered one.

### 3.4 Model check

`(boot.model.provider, boot.model.model)` must be a row of provider.md's `ModelInfo` table. That table has the six frontier models and `mock/mock-frontier-1` for testing.md scenarios. Otherwise the body exits 78 with `unknown model "<provider>/<model>"; known: deepseek/deepseek-flash, …`. `effort`, when set, must be one of `low|medium|high|xhigh|max`, else 78. (Config `model.id` reaches the body as `boot.json.model.model`, supervise §4.3.)

This is "the model id is checked at boot" (CONVENTIONS §5; spec §11: weak models get worse at self-modification). A bad owner configuration then exits 78 at every boot. Two consecutive exits 78 at boot put the supervisor in `Halted{config}` with an owner alert, and they are not counted as crashes (supervise §3.3, CONVENTIONS §4), so a config error never walks the crash ladder into safe mode. The check is agent-editable, so only the kernel can *enforce* "frontier only": the proxy's price table doubles as the model allowlist and refuses a model it cannot price (kernel/proxy.md, kernel/config.md §4.5).

### 3.5 Extension host

- **Normal and probe mode:** `ExtHost::new(paths)` (ext/host.md §4.1) runs here, before rehydration. It queues the `Boot` load, which compiles the kept components on the host's loader thread. `ready` does **not** wait for it (host.md §3.9 item 4).
- An `ext` call before the load finishes waits through the loop's `commit_reload(60 s)` rule (loop.md §3.7).
- Load failures are not fatal. They appear in the first `reload` event and in `$INIT_HOME/state/ext/catalog.json`.
- **Safe mode:** no host at all (§3.14).
- When log-context creates a **new** session at rehydration, the loop calls `ext.reset_trials()` (host.md §3.9 item 5).

### 3.6 `body/` sync and housekeeping (non-fatal)

**`body/` git sync** (kernel/supervise.md §3.11.5 defines the contract; evolution.md §3.6 owns the procedure, the "body resync procedure" of CONVENTIONS §4). The kernel never runs git inside `body/`, so the body does it here:
- each git command is one `Command` (no shell), run through tools.md's `run_group` helper (§3.5.7) with a 30 s timeout and `CaptureSpec::Merged`;
- the environment is the bash tool's (`GIT_TERMINAL_PROMPT=0`, `GIT_EDITOR=true`);
- every command gets `-C $INIT_HOME/body`;
- results go into the boot facts as `git_sync: {fetched, action, drift, stash, rolled_back, error}`.

| Mode | What runs |
|---|---|
| probe | nothing: the probe root's `body/` is a fresh kernel checkout at the candidate's sha (supervise §3.6.3) |
| safe | step 1 only (fetch). Safe mode reads T0 from `lkg_tree`, never from `body/`, and must not touch the agent's work |
| normal, **any reason** | evolution.md §3.6's script, step for step (below). It is reason-agnostic: an ancestry test, not `INIT_BOOT_REASON`, decides whether lineage `main` moved forward or backward, so promote, rollback, a panic that dropped a provisional chain, and an agent that committed on its own `main` all take the right branch |

The script as the body runs it (`B` = `$INIT_HOME/body`; `origin` = `$INIT_HOME/lineage/body.git`, read-only, so fetching writes nothing there):

1. `git fetch --quiet origin` (the install-time refspecs bring `main`, the notes and `refs/init/*`). Failure → `fetched: false`, a warning, and **stop**: never move `main` against a stale `origin/main`.
2. `on_main` = (`git symbolic-ref -q --short HEAD` prints `main`). `rolled_head` (ADR-013 G2) = not `on_main`, and `HEAD` is a commit the kernel rolled back (`refs/remotes/origin/init/rolled-back/<HEAD sha>` exists): the agent's `evo/<id>` branch at a change it promoted data-only and the kernel then rolled back. With `rolled_head`, steps 3 and 4 take the "on `main`" resync below (stash, `switch -C main origin/main`, pop; `action: "resync"`), so the rollback reaches the T0 data the head is built from; the branch keeps the commit.
3. If `git merge-base --is-ancestor main origin/main` exits 0 (lineage only moved forward):
   - on `main`: `git merge --ff-only --quiet origin/main` → `action: "ff"` (or `"none"` when already equal). If it fails (dirty paths it would touch), leave the tree and set `drift: true`;
   - otherwise: `git branch -f main origin/main` → `action: "ref"`; failure → `drift: true`. The working tree is not touched.
4. Otherwise (lineage `main` moved backward, or the agent committed on its own `main`):
   - `git branch -f rolled-back/<sha12 of main> main` → `rolled_back: "rolled-back/<sha12>"`;
   - on `main`: record `before` = `git rev-parse -q --verify refs/stash`; `git stash push --include-untracked -m "init: resync to <sha12 of origin/main>"`; record `after` the same way; `git switch -C main origin/main`; then **only if `after != before`** `git stash pop --index`. A failed pop keeps the stash and sets `stash: "stash@{0}"`. With no local changes `stash push` creates nothing, and an older, unrelated stash is never popped. `action: "resync"`;
   - otherwise: `git branch -f main origin/main` → `action: "ref"`; failure → `drift: true`.

A G4 task run is a normal-mode body in a probe root (gate.md §3.7.3). Its `body/` is a detached `--shared` kernel checkout (supervise §3.6.3), so step 2 finds `on_main` false and at most the ref `main` moves; the working tree and the task workspace are untouched.

Nothing is ever deleted. A failure (timeout, a lock left by a crashed git, a corrupt repository) is a warning, and boot continues on whatever `body/` contains. The agent sees the `git_sync` facts in its boot block and can repair the clone. log-context's `boot.stash` field is `git_sync.stash` (log-context.md §4.2 field sources).

**Housekeeping** (≤ 100 ms):
- **Spill cleanup.** For each directory `state/spill/<id>/` with `id != INIT_SESSION`: if the newest mtime among its files is older than 14 days, remove that directory by its exact path (`remove_dir_all(<exact path>)`). Never a glob, never the parent.
- **Spill budget.** Sum the file sizes in `state/spill/<INIT_SESSION>/` to seed the 1 GiB per-session counter (tools.md §4.4).
- Errors become `warnings` in the boot facts.

### 3.7 Rehydration (log-context.md §3.6)

`log_context::rehydrate(&env, &boot_facts, head_root)` performs these steps; the body only orders them:
1. Select the session: `INIT_SESSION` if non-empty (a missing file under that id is created, `origin.reason:"cold"`); if empty, generate `YYYYMMDD-HHMMSS-<6 hex>` (UTC), create the file and write `state/session` (log-context §3.4.3, §3.6 step 1). The empty case only arises in tests and manual runs: at spawn, the production kernel chooses the id whenever `state/session` is missing or invalid, and never sends `INIT_SESSION` empty (kernel/supervise.md §3.4). The body itself generates ids only for `/new`.
2. Lock it: `try_lock` every 50 ms for up to 10 s, else exit 1 with stderr `log: session <sid> locked by another writer`.
3. Scan the file and repair a torn tail.
4. Append synthetic results for interrupted tool rounds, in block order:
   - `INTERRUPTED_TEXT` where a `tool_call` marker exists;
   - `NOT_STARTED_TEXT` where it does not.
5. Build the head from `head_root` (`$INIT_HOME/body` normally, `$INIT_HOME/<lkg_tree>` in safe mode) and compute `prefix_hash`.
6. Append the `boot` event with the boot facts (§3.8), then sync (D3).

The body never re-runs a tool by itself. Whether to redo a side effect is the model's call, and it can check first (spec §7.1, ACRFence: irreversible actions are never replayed).

A reply that was mid-stream at the crash was never logged (provider.md §3.5). The leaf is then the preceding owner message or result set, which counts as pending work (§3.13).

### 3.8 Boot facts (input to the `boot` event)

log-context.md owns the `boot` event schema and awareness.md its text. The body hands over these facts, all sorted keys:

```json
{"bash":{"path":"/opt/homebrew/bin/bash","version":"GNU bash, version 5.3.20(1)-release (aarch64-apple-darwin23.6.0)"},
 "body_version":"0.1.0","boot_id":"a3f09c1e5b7d2468","cwd":"/Users/o/.init/work",
 "ext":{"status":"loading"},"git_sync":{"action":"ff","drift":false,"error":null,"fetched":true,"rolled_back":null,"stash":null},
 "kernel":{"…":"boot.json verbatim"},"kernel_version":"0.1.0","kill":"/bin/kill",
 "mode":"normal","model":{"effort":"high","model":"deepseek-flash","provider":"deepseek"},"pid":48121,
 "reason":"promote","sha":"4b1e0c2d9a7f…","spill_bytes":1048576,"warnings":[]}
```

log-context adds `prefix_hash` and the repair count, and takes its top-level `boot.stash` from `git_sync.stash`. Spec Appendix B's fields (`sha, from_sha, reason, update_failed, model, body_tokens, prefix_hash, retire_candidates`) are all covered: `from_sha`, `update_failed`, `body_tokens` (`selfmap`) and `retire_candidates` (`evolution`) come through the verbatim `kernel` object. A `boot.cause` object is logged as its `kind` string (the schema's enum), and `from_sha` and `lkg_sha` are copied only when they are valid shas.

The Root status the model sees (`boot.json.env.root`) is rendered through `src/boot/rootview.rs`. It maps the Root's real shapes (`ceilings.money.day_limit_musd`, `total_limit_musd`, `total_spent_musd`, `parallel_bodies.limit`, class objects) onto awareness.md §4.5's names. For `cap` and `supported_max_level` it falls back to the Root's published view in `INIT_HOME/root/` (`ledger.json`, `ceilings.json`, `charter.json`) when the body may read it (root.md X2). It fills `selfmap.body_tokens` from `body/self.json` when the kernel sent none. A part that stays unknown is left out of the text instead of printed as `?`. This view is for rendering only: the boot facts and `env.kernel` keep `boot.json` verbatim.

### 3.9 Probe round trip (probe mode only)

The probe proves the following about the candidate, which kernel/gate.md §3.5 checks from the proxy's side (criteria 3–7):
- it can render its **real** head and conversation, send them through the proxy, and parse the reply;
- it can run a real tool and send the result back, a *tool round trip* (proxy `tool_round_trip`, gate criterion 3);
- the proxy sees its tool list (`tools_sha256`, criterion 4) and its head (`prefix_hash`, criterion 5);
- its first request continues the parent's last request element for element (continuity, criterion 7).

Gate Q-BT1 (R1): probe mode changes **nothing** in the head or the loop.
- The probe uses the session's normal `effort` and `ModelInfo.default_max_tokens`. Effort is a head parameter (log-context §3.10.4); only the purpose header differs.
- The probe driver is built in boot (`src/boot/probe.rs`) from the loop's public seams: `boundary_append`, `next_request` and `on_response`, `ModelClient::stream` with `Purpose::Probe`, and `Tools::prepare`/`run` inside `catch_unwind`. It does not use `Session` internals. Only the batch policy differs.

Steps:
1. A watchdog thread calls `Abort::trigger()` after **60 s** (`INIT_TEST_PROBE_WATCHDOG_MS` under `INIT_TEST=1`). G2's own limit is 90 s to `ready` (supervise §3.5).
2. **Request 1.** A boundary appends one harness user-side item after the `boot` block, which awareness.md renders in its probe variant ("GATE PROBE: you are a staging copy…"). The text is fixed here; awareness.md §3.6 quotes it in its probe row (R2):
   ```
   [init:probe] Gate probe: call the bash tool once with the command "true", then reply with the single word ready.
   ```
   - The request is the next live turn exactly: the head, the projected conversation from the staged log copy, the boot block, this item. Purpose `Probe` (header `x-init-purpose: probe`, billed to the probe arm's cap).
   - Head and history are byte-identical to the parent's last request (unless `Prefix-change: yes`), so everything before the tail is a cache hit. At DeepSeek's cache-hit price ($0.003–0.006 per 1M tokens), a 200k-token history costs under $0.002.
3. **Tools.** Each tool call in the reply goes through the probe policy:
   - exactly `bash {"command":"true"}` runs through the normal pipeline;
   - any other call gets `error: probe_policy: not executed in probe mode` (is_error), so the probe cannot change the staging root.

   Every emitted call still gets a result, which is what makes the round trip. The policy applies only to this exchange: after `ready`, G3 instructions (`input` frames) run with the normal tools, since G3 must be able to self-edit.
4. **Request 2** sends the results (boundary step 1) and reads the reply (`ready` or anything else).
   - If request 1 had **no** tool call, the body sends one nudge instead, `[init:probe] Call the bash tool with the command "true".`, and repeats steps 3–4 once.
   - At most 3 model requests in total. If the reply to the nudge has no tool call either, there are no results to send, so the probe ends after 2 requests (a third would end on an assistant turn). `bash true` then runs directly, unlogged (§5).
5. **Outcome.**
   - If every request ended `Done` (not `Failed`), the body writes `ready` with `probe = {ok, requests, tool_round_trip, model_called_tool, model_ms, tool_ms, usage, request_id, head_hash}` (§4.2).
   - `tool_round_trip` is true when a request that carried tool results came back `Done`.
   - `tool_round_trip:false` (the model never called a tool) still writes `ready`: the gate then reports the precise cause, `probe_no_round_trip`, rather than a boot failure.
   - A `Failed` request (provider, timeout, watchdog) means no `ready`, stderr `{"type":"boot_error","step":"probe",…}`, and exit 70. supervise copies the stderr and stdout tails to `state/kernel/probe/<sha12>/` for the agent.
6. The exchange is appended to the **staging copy** of the log by the normal boundary code. Probe mode never runs on a live log (supervise §3.6.3), and the copy is deleted with the probe root.

### 3.10 Normal and safe boots make no model call

There is no round trip on normal or safe boots. The first real turn is the check, and G2 has already probed the same binary before any handoff. This is a spec delta (§10 item 1). **Settled by the main session in R2** (CONVENTIONS §6 body-core E1: probe-mode boots only); testing.md §3.4.4 and §3.10 E2E-1, log-context.md §3.4.1 and awareness.md §3.6 follow it.

### 3.11 Write `ready`

`state/run/ready` is written atomically: temp file `state/run/.ready.<pid>`, then `sync_all`, then `rename` (supervise §4.4). The format is in §4.2.

`ready` means:
- the lock is held;
- the log is rehydrated and repaired;
- the `boot` event is durable;
- in probe mode, the probe passed;
- the body is serving its control channel within milliseconds (supervise §3.5: "a body that is `ready` can take owner input immediately").

The kernel matches `ready` on `boot_id`.

### 3.12 Heartbeat thread

Started right after `ready`:
- It writes `state/run/heartbeat` atomically (temp + rename, no fsync) immediately, then every **5 s**, and does nothing else (supervise §3.5). The format is in §4.2.
- It **keeps writing while the body is paused**, with `busy:false` and `phase:"paused"`. The incoming body writes the same file with its own `boot_id`. supervise keeps `last_seen[boot_id]` per instance, does not evaluate staleness for A (paused) or B (before commit) during a handoff, and no longer needs a heartbeat from B to settle (supervise §3.5).
- A write failure becomes one stderr warning per minute, and the thread keeps trying.
- Heartbeats are a liveness hint, not a security property (supervise §3.5).

### 3.13 Serve

1. Start the **output thread** (stdout frames, `sync_channel(1024)`) and the **input thread** (stdin frames; loop.md §3.1). Lines that are not valid frames produce a stderr warning and are ignored.
2. `Session::serve()` (loop.md §3.3), with these mode differences:
   - **normal:** if `has_pending_work()`, the run continues at once. This is how a promoted body "continues the same task in the same conversation" (spec §6.2 step 4; A3).
   - **probe:** no auto-continue. The kernel either sends `stop` after the 15 s probe window (G2), or drives the body with `input` frames through `ProbeHandle::send_input` for G3 (supervise §3.6.3; gate.md §3.6).
   - **safe:** no auto-continue; the body waits for the owner.
3. On stdin EOF in any mode, the body interrupts, syncs, calls `ext.shutdown()` and exits 0 within 10 s (supervise §4.5; loop.md §4.1).

### 3.14 Modes

| | normal | probe | safe |
|---|---|---|---|
| Used for | live body: cold, crash restart, rollback, after handoff | G2/G3 candidate in `probe/<id>/` with a copy of the log (supervise §3.6.3) | LKG after a crash loop (> 4 crashes in 600 s on LKG, supervise §3.7) |
| Log | live | staging copy | live |
| Lock, rehydrate, repair, `boot` event | yes | yes (on the copy) | yes |
| Head / T0 data from | `body/` | `body/` (the candidate checkout) | `lineage/trees/<lkg12>/` (`boot.json.lkg_tree`) |
| Extension host | yes | yes (proves it starts) | **none** (`ext` returns `ext_disabled`) |
| Probe round trip | no | **yes** | no |
| Auto-continue pending work | yes | no | **no** |
| Reload | yes | as the kernel drives it | refused (supervise refuses `initctl reload` with `E_SAFE_MODE`) |
| Heartbeat | yes | yes (G2 needs ≥ 2 updates in the 15 s window) | yes |

Safe mode keeps the smallest working surface: model, four core tools, log. Extensions, agent-edited T0 files and auto-continuation are the likeliest causes of a crash loop on an otherwise good LKG, so all three are off. The owner can still talk to the agent and have it diagnose from the log. Leaving safe mode is the owner's `/clear-safe` (supervise).

### 3.15 Sequences

**Cold boot:** argv → env → `boot.json` → model check → `body/` git sync → housekeeping → `ExtHost::new` → rehydrate (new or existing session; `boot` event) → `ready` → heartbeat → serve.

**Overlapped T2 handoff.** supervise §3.6.4 owns this; it is shown from the body's side:

```
kernel                          A (old body, normal)                 B (new body, INIT_BOOT_REASON=promote)
  │── pause{token} ────────────────▶│
  │                                 │ next boundary: steps 1–4 appended, sync, unlock (loop.md §3.8.1)
  │◀──── paused{token,log_len,leaf}─│ (heartbeat continues, busy:false)
  │ fence (try_lock ≤ 5 s), write boot.json(B), spawn B, unlock fence
  │ ─────────────────────────────────────────────────────────────────▶│ §3.2 env · §3.3 boot.json · §3.4 model
  │                                                                   │ §3.6 git sync · housekeeping · §3.5 ExtHost::new
  │                                                                   │ §3.7 rehydrate: lock (≤ 10 s) · repair · boot event · sync
  │◀────────────────────────────────────────────────────── ready ────│ §3.11 (boot_id = B's)
  │ settle 3 s (B alive) · commit (bin renames, update-ref)
  │── exit{token, code:75} ─────────▶│ ext.shutdown(); exit(75)
  │                                                                   │ §3.12 heartbeat · §3.13 serve → pending → continue run
```

**Failed new boot** (B exits before `ready`, misses 90 s, or dies while settling):
1. The kernel sweeps B.
2. The kernel sends `resume{token, update_failed}` to A.
3. A re-locks, rescans from `log_len`, and appends `update_failed` with `parent_id = leaf`. B's `boot` event, if any, is left on an abandoned branch.
4. A continues. A failed swap is a non-event (A4).

**Stop while paused (v0.3).** If the kernel sends `stop` to a paused A instead of `exit` or `resume` (the Root's `stop` or `prepare_swap`, kernel/supervise.md §3.8.3), A exits 0 at once without writing to the log (loop.md §3.8.1). The session continues in the body that the next kernel starts.

**Time budget (normal boot, target < 2 s):**

| Step | Budget |
|---|---|
| argv, env, `boot.json`, model check | < 10 ms |
| `ExtHost::new` | engine and linkers only; compiles run on the loader thread (host.md) |
| housekeeping | < 100 ms |
| rehydration | log-context.md (lock ≤ 10 s when contended; scan target < 500 ms for a 50 MB log) |
| `boot` event + `sync_data` | 5–30 ms (`F_FULLFSYNC` on macOS; log-context T-LOG-9 measures it) |
| `ready` write + `sync_all` | 5–30 ms |
| probe (probe mode) | ≤ 60 s watchdog; typically one cached model call |

### 3.16 Seed crate build configuration

The seed crate's manifest and Cargo config are boot-critical: a wrong setting fails G0, breaks the cache contract, or turns tool panics into crashes. CONVENTIONS §4 registers this doc as their owner. Sources of each requirement:
- ext/host.md §3.16 (dependency opt-level, `exclude = ["ext"]`);
- ext/authoring.md §3.3 (vendored offline sources);
- log-context.md §3.8.1 and T-CTX-3 (no `preserve_order`);
- loop.md §3.7 (unwind);
- kernel/gate.md G0 (build-config policy).

`seed/Cargo.toml`, installed as `body/Cargo.toml`:

```toml
[package]
name = "init-body"
version = "0.1.0"
edition = "2024"
publish = false

[[bin]]
name = "init-body"
path = "src/main.rs"

[dependencies]
serde = { version = "=1.0.229", features = ["derive"] }
serde_json = "=1.0.154"          # never the `preserve_order` feature: ICJ needs BTreeMap order
wasmtime = "=48.0.6"             # LTS; CONVENTIONS §5. Features are ext/host.md §3.2's
wasmtime-wasi = "=48.0.6"

[workspace]                      # the body is its own workspace, never a member of kernel/ (CONVENTIONS §2)
exclude = ["ext"]                # the extension workspace (ext/authoring.md §3.3)

[profile.dev]
panic = "unwind"                 # the default, stated so a self-edit cannot flip it silently

[profile.dev.package."*"]        # dependencies only: Cranelift built at opt-level 0 makes reload compiles slow
opt-level = 2
debug = false
```

`seed/.cargo/config.toml` (ext/authoring.md §3.3, verbatim). Nothing else may be added: no `build.rustflags`, `target.*.runner`, `rustc-wrapper` or lint caps, because gate.md G0's build-config policy rejects them.

```toml
[source.crates-io]
replace-with = "vendored-sources"
[source.vendored-sources]
directory = "vendor"
[net]
offline = true
```

- `#![forbid(unsafe_code)]` is the first line of `src/main.rs` (§3.1). G0 checks it on every target root.
- The seed tree (CONVENTIONS §2) also ships `GENESIS.md` and `SELF.md` (awareness.md), `ext/Cargo.toml` and `ext/Cargo.lock` (the separate extension workspace, `members = ["*"]`), `wit/init-ext/v1/init-ext.wit` (a byte-identical copy of the kernel's, G1 `gate.g1.wit_copy_drift`), and `skills/ext-authoring/` with `SKILL.md` and `assets/template/{Cargo.toml.template, src/lib.rs}`. The template manifest is named `Cargo.toml.template`, never `Cargo.toml`, so that it is not an unlisted crate under gate.md §3.3 step 3 (ext/authoring.md §3.4). ext/authoring.md and ext/wit.md own those contents.
- `vendor/` holds both workspaces' dependencies (`cargo vendor --locked --sync ext/Cargo.toml vendor/`, run only by the kernel through `initctl vendor`). Whether a *new* crate also needs owner approval is PENDING OWNER (CONVENTIONS §6 ESC-1); the provisional default is no (crates.io only, exact versions, 20 per day, full G4).

## 4. Interfaces

### 4.1 Environment (consumed)

kernel/supervise.md §4.1 is the full list. The body reads `INIT_HOME`, `INIT_SESSION`, `INIT_SHA`, `INIT_BOOT_REASON`, `INIT_MODE`, `INIT_BOOT_ID`, `INIT_UI`, `INIT_KERNEL_VERSION`, `INIT_KERNEL_SOCK`, `INIT_PROXY_SOCK`, `PATH` (bash lookup) and `HOME` (tools). From v0.1.1 it also reads `INIT_WORKSPACE` and `INIT_ACCESS` (ADR-009), which the Root, not the kernel, sets for every `body`-profile spawn (§3.2). `INIT_TEST_*` overrides (testing.md §3.4.6) are honored only when `INIT_TEST=1` is set. This doc adds `INIT_TEST_PROBE_WATCHDOG_MS` (default 60,000) and `INIT_TEST_LOCK_WAIT_MS` (the lock wait of §3.7 step 2, default 10,000; the "test override" of §8 item 3). `INIT_FAULT` (testing.md §3.8.1) is also read, only under `INIT_TEST=1`. This doc adds the action `panic` (a main-thread panic, exit 101) and the points `boot.after_env`, `boot.after_boot_event` and `boot.before_ready`. Everything else the kernel sets is simply inherited by tool children (tools.md §3.5.5).

### 4.2 Files written

`$INIT_HOME/state/run/ready`. The base fields come from supervise §4.4; the body adds `mode`, `session`, `leaf`, `boot_event_id` and `probe`, which supervise ignores as unknown fields:

```json
{"boot_event_id":"3f9a0c1d2e4b5a69","boot_id":"a3f09c1e5b7d2468","leaf":"7d1e44c09a2b3f10","mode":"probe","pid":48211,
 "prefix_hash":"sha256:9b1f…","probe":{"head_hash":"sha256:9b1f…","model_called_tool":true,"model_ms":2140,"ok":true,
 "request_id":"1f633d8b…","requests":2,"tool_ms":4,"tool_round_trip":true,
 "usage":{"cache_read":23552,"cache_write":0,"input":24080,"output":64,"reasoning":24}},
 "session":"20261019-120000-3fa9c1","sha":"4b1e0c2d9a7f…","ts":"2026-10-19T13:02:11.512Z","v":1}
```

`probe` is `null` outside probe mode.

`$INIT_HOME/state/run/heartbeat`. The base fields are `boot_id`, `busy`, `ts` and `v`; the body adds the rest:

```json
{"boot_id":"a3f09c1e5b7d2468","busy":true,"last_progress_ms":51234,"leaf":"7d1e44c09a2b3f10","phase":"tools","pid":48211,
 "run":3,"session":"20261019-120000-3fa9c1","tool_pgids":[48390],"ts":"2026-10-19T13:04:01.007Z","turn":17,"uptime_ms":112004,"v":1}
```

- `phase` is one of `booting|idle|model|tools|paused|stopping`; `busy` = `phase ∈ {model, tools}`.
- `last_progress_ms` (ms since boot of the last stream byte, tool event or log append) lets the kernel tell a long tool or a long generation from a hang.
- `tool_pgids` lists the live bash process groups. supervise's sweep identifies sandbox members on its own (§3.8.2), so this is advisory.

Temp names: `state/run/.ready.<pid>`, `state/run/.heartbeat.<pid>`.

### 4.3 `init-body --manifest`

This prints the body's static self-description for kernel/selfmap.md, as one line of JSON (sorted keys) plus `\n`, then exits 0. It reads **no** env vars and touches no file or socket, so selfmap can run it as `env -i <binary> --manifest` in a minimal sandbox and trust that the output comes from the binary itself.

```json
{"body_version":"0.1.0","control_channel":"stdio-jsonl/1","ext_api_majors":[1],
 "env":["HOME","INIT_BOOT_ID","INIT_BOOT_REASON","INIT_HOME","INIT_KERNEL_SOCK","INIT_KERNEL_VERSION","INIT_MODE","INIT_PROXY_SOCK","INIT_SESSION","INIT_SHA","INIT_UI","PATH"],
 "manifest":1,"modes":["normal","probe","safe"],
 "models":[{"api":"chat_completions","id":"deepseek-flash","provider":"deepseek"},{"api":"chat_completions","id":"deepseek-v4-pro","provider":"deepseek"},
           {"api":"chat_completions","id":"mock-frontier-1","provider":"mock"},
           {"api":"messages","id":"claude-opus-5-5","provider":"anthropic"},{"api":"messages","id":"claude-sonnet-5-5","provider":"anthropic"},
           {"api":"responses","id":"gpt-6-astra","provider":"openai"},{"api":"responses","id":"gpt-6.1-sol","provider":"openai"}],
 "subcommands":["--manifest","--version","ext call","ext describe","ext inspect"],
 "tools":[ …the five objects of tools.md §4.1, verbatim, sorted byte-wise by name… ],
 "tools_sha256":"sha256:<init-tools-v1, below>"}
```

- `tools` is byte-for-byte the registry that provider.md renders into every request. selfmap and the proxy compare it per API after extracting name, description and schema from each request's `tools`. The extraction rules belong to kernel/proxy.md and kernel/selfmap.md.
- **`tools_sha256` uses algorithm `init-tools-v1`** (kernel/proxy.md §3.12.2; adopted in R1 for proxy Q-P6 and selfmap Q-B1). That is `"sha256:" + hex(SHA-256(ICJ([tools sorted byte-wise by name])))`, where each tool is `{"description","input_schema","name"}` and ICJ is log-context §3.8.1. The proxy and selfmap compute the same function, so the three values are directly comparable, and selfmap's mismatch check can become a G1 failure.
- SHA-256 is log-context's std-only `src/sha256.rs` (log-context §3.10.4), the same function as the head hash. It is not `DefaultHasher`, whose algorithm std leaves unspecified across releases.
- Any change to `tools` changes the cached head, so the commit must carry `Prefix-change: yes` (spec §6.3 G2).
- `ext_api_majors` lists the `init:ext` WIT majors the built-in host accepts (`[1]` at seed; ext/wit.md §3.6). selfmap copies the manifest minus `tools` verbatim (kernel/selfmap.md §3.5), so this field needs no schema change there.

### 4.4 Exit codes

The contract owned by kernel/supervise.md §4.2 (CONVENTIONS §4): `0` = stop, `75` = handoff complete (only after `exit{code:75}`), `78` at boot twice in a row = `Halted{config}`, anything else = crash. Within these, the body uses the following codes, always with a JSON line on stderr:

| Code | Meaning | Owner |
|---|---|---|
| 0 | `stop`, stdin EOF, or probe finished | supervise |
| 75 | only on the kernel's `exit{code:75}`; never otherwise | supervise |
| 1 | session log locked by another writer; fsync or write error on the log | log-context §3.4.1, §3.4.3 |
| 64 | bad argv | this doc |
| 70 | internal boot error: probe failed, `ready` write failed, a head file (`GENESIS.md`, `SELF.md`, `kernel/CONSTITUTION.md`, a skill) missing, unreadable or not valid UTF-8, so supervise's crash and rollback logic runs | this doc |
| 78 | config error: env, `boot.json`, unknown model, bad effort. Two in a row at boot → supervise `Halted{config}` (supervise §3.3; CONVENTIONS §4) | this doc |
| 101 | Rust panic on the main thread (std default) | — |
| 0 / 1 / 2 | `init-body ext …` subcommands (ran / load error / usage) | ext/host.md §4.5 |

### 4.5 stderr lines

```json
{"type":"boot_error","code":78,"step":"model","message":"unknown model \"deepseek/deepseek-chat\"; known: deepseek/deepseek-flash, deepseek/deepseek-v4-pro, …"}
{"type":"panic","location":"src/loop.rs:212:17","message":"index out of bounds","thread":"main"}
{"type":"warning","step":"heartbeat","message":"write failed: No space left on device (os error 28)"}
```

## 5. Failure modes and recovery

| Failure | Exit / effect | Who recovers |
|---|---|---|
| Missing or invalid env, `boot.json`, unknown model | 78 | two in a row → supervise `Halted{config}` + owner alert, not a crash (supervise §3.3) |
| `boot_id` mismatch between env and `boot.json` | 78 | indicates a kernel bug or a stale `boot.json` |
| Log locked by another live body | 1 after 10 s (log-context) | supervise counts a crash in `Starting`, or an abort reason in `AwaitReady` |
| Torn last log line | repaired by log-context, not an exit | — |
| Unreadable or corrupt log | log-context's policy (skip middle corruption, warn) | the owner inspects |
| Disk full writing the log or `ready` | 1 (log) or 70 (`ready`) | the kernel's `DiskFull` state; the owner frees disk |
| Proxy down during the probe | transport retries until the 60 s watchdog, then 70 | G2 fails; the old body continues (A4) |
| Model ignores the probe instruction | `bash true` runs directly; probe still ok; `model_called_tool:false` | — |
| Extension host fails to start | warning; tools run without `ext` (`ext_host` errors) | the agent sees it in the boot facts |
| No bash on PATH and no `/bin/bash` | warning; the bash tool returns `spawn_failed` | the owner's host config |
| No `kill` binary | warning; bash-builtin fallback | — |
| Heartbeat write fails | warning, retried | — |
| New body fails after A paused | A resumes with branch-back `update_failed` | loop.md §3.8.1 |

## 6. Performance notes (spec §9)

- **Rank 1 (cache).**
  - The boot block is volatile and goes at the tail, never in the head (log-context, awareness), so a restart keeps its cache hits.
  - Phase 0 measurement 4, "the prefix hash stays identical across a T2 restart", is a comparison of two `boot` events' `prefix_hash`.
  - The probe request reuses the parent's cached prefix.
- **Rank 2 (round-trips).** No model call on normal boots. One cached call per G2 probe.
- **Rank 4 (self-edit loop).** Boot is a small part of a handoff (the gate takes minutes). A normal boot targets < 2 s, and extension compiles run in the background.
- **Rank 5 (footprint).** The idle body has 4 threads (main, input, output, heartbeat) plus the extension host's ticker and loader.

## 7. Invariants and security (spec §2.1)

- **Invariant 1 (recoverability).**
  - The body performs no rollback and never writes `bin/` or `state/kernel/` (sandbox-enforced).
  - `ready` is written only after the `boot` event is durable.
  - A paused body writes nothing to the log.
- **Invariant 4 (lineage).** Every boot appends exactly one `boot` event, with sha, reason, mode and `prefix_hash`. Repairs are appended as results, never as rewrites.
- **Single writer.** No log write happens before log-context's lock, which is released only at pause or exit.
- **Probe isolation.**
  - The probe runs where the kernel put it (a staging root with a copy of the log).
  - It appends its exchange only to that staging copy, never to the live log (§3.9 step 6).
  - It never executes a model-chosen command other than exactly `true`.
- **`--manifest` integrity.** It reads no env and no files, so its output depends only on the binary.
- **Compiler-checked memory safety.**
  - `#![forbid(unsafe_code)]` at the crate root (G0 checks that it is unchanged, spec §6.3).
  - The seed `Cargo.toml` keeps `panic = "unwind"`, so tool panics stay recoverable (loop.md §3.7).
  - It also keeps the build settings host.md §3.16 requires (`[profile.dev.package."*"]` and `exclude = ["ext"]`).

## 8. Test plan

1. **Env table:** each var missing or invalid gives 78 naming the var; `INIT_UI=stdio-jsonl/2` gives 78; a 104-byte socket path on macOS gives 78.
2. **`boot.json`:** missing or invalid gives 78; `boot_id` ≠ `INIT_BOOT_ID` gives 78; an unknown model gives 78 listing the known ids; a bad effort gives 78; extra fields appear in the boot facts' `kernel` object.
3. **Lock** (log-context's mechanism, exercised from boot):
   - a second body on the same session exits 1 after 10 s (shortened by a test override);
   - if the first body unlocks at 2 s (simulated pause), the second acquires the lock and proceeds.
4. **Repair through rehydrate:** logs ending in (a) an owner message, (b) an assistant message with two calls, one with a marker and one without, (c) a complete result set.
   - (a) and (c) are pending with no repair.
   - (b) gets `INTERRUPTED_TEXT` and `NOT_STARTED_TEXT`, in block order, *before* the `boot` event, and is then pending.
5. **`ready`:** atomic (a polling reader never sees partial JSON); `boot_id` and `pid` match; the `boot` event is on disk before `ready` (kill -9 right after `ready` appears, then re-read the log).
6. **Heartbeat:** the first write within 100 ms of `ready`, then every 5 s ± 0.5 s; it continues while paused with `busy:false`; `tool_pgids` lists a running bash group.
7. **Probe** (mock, $0):
   - (a) the model calls `bash true` and then replies `ready`: two requests, the second carrying the result for the emitted call id; `tool_round_trip:true`, `ok`;
   - (b) a text-only reply, then a text-only reply to the nudge: two requests, because there are no results to send and a third request would end on an assistant turn; `bash true` runs directly, unlogged; `ready` written with `model_called_tool:false` and `tool_round_trip:false` (the gate reports `probe_no_round_trip`);
   - (c) the model calls `bash rm -rf x`: not executed; it gets `error: probe_policy: …` as its result; the round trip still counts;
   - (d) the mock returns 500 forever: no `ready`, exit 70 within 61 s;
   - (e) a candidate seeded to panic at boot (`INIT_FAULT=boot.after_env:panic`): exit 101, no `ready` (A4 building block);
   - (f) the first probe request carries the real tools, the session's normal effort, `x-init-purpose: probe`, and the full projected conversation, ending with the `[init:probe]` item; its head hash equals the live body's (gate criterion 5).
8. **Safe mode:** no extension host (any `ext` call gives `ext_disabled`); the head is read from `lkg_tree` (a broken `body/SELF.md` does not reach the prompt); pending work is *not* continued; `reload` gives `reload_done{ok:false}`; the boot facts have `mode:"safe"` (A5 body side).
9. **Handoff overlap** with the scripted kernel side of §3.15: B's `boot` event follows A's last event; A exits 75 after `ext.shutdown()`; B continues the pending run (A3 body side).
10. **Failed handoff:** B exits 70; A gets `resume`, appends `update_failed` with `parent_id` = A's leaf, and continues (A4).
11. **`--manifest`:** `env -i ./init-body --manifest` exits 0 with one line; its `tools` equals tools.md §4.1 byte-for-byte (golden); `tools_sha256` matches; it still succeeds with `INIT_HOME` pointing at a nonexistent path.
12. `--version`; bad argv gives 64; `ext describe` is dispatched to the host CLI; the panic-line format.
13. **`body/` sync** (a temp clone of a fixture lineage repo; every case runs with each `INIT_BOOT_REASON`, since the script ignores it):
    - lineage `main` moved forward, clean tree on `main`: `action:"ff"`;
    - forward, but the fast-forward touches a dirty path: `drift:true`, tree untouched;
    - forward, `body/` on branch `evo/x`: only the ref `main` moves (`action:"ref"`), the `evo/x` tree is untouched;
    - lineage `main` moved backward (rollback), dirty tree on `main`: branch `rolled-back/<sha12 of old main>` exists, the stash is popped, `main` = `origin/main` with the local edits restored;
    - the agent committed on its own `main` (not an ancestor of `origin/main`): the commit survives on `rolled-back/<sha12>`, and `main` = `origin/main`;
    - a conflicting pop: the stash is kept and reported as `stash:"stash@{0}"`, and the boot event's `stash` carries it;
    - with no local changes, no unrelated older stash is popped (`refs/stash` unchanged);
    - the fetch fails (origin missing): `fetched:false`, nothing else runs;
    - safe mode only fetches; probe mode runs nothing;
    - a `.git/index.lock` left behind: a warning, and boot continues.
14. **Seed build configuration** (§3.16): `cargo metadata` shows no `preserve_order` feature on `serde_json`; `[profile.dev.package."*"] opt-level = 2`; `ext` excluded from the workspace; the build succeeds offline from `vendor/`.

Scenario IDs from testing.md (roadmap.md Q2):
- E2E-1 (§3.10): cold boot on the real model. A cold *normal* boot makes no model call (settled), so E2E-1 checks the boot round trip through a probe-mode boot (`gate --quick`, testing.md §3.10).
- F-crash-boot (a): items 7(e), 10. F-corrupt-tail: items 4, 5 (log-context's repair through boot). F-hang (a): the probe watchdog of §3.9.
- A3 / A3-real: items 9, 13 (promote fast-forward). A4: items 7(e), 10. A5 / A5-real / A5-fake: item 8.
- M-4: the `boot` events' `prefix_hash` across a no-op T2 handoff.

## 9. Open questions

Resolved in R1/R2 and removed:
- Q-B1 (supervise env, `boot.json`, `ready`/`heartbeat`, the example model id; supervise §4.1–§4.4);
- Q-B2 (probe on probe-mode boots only; settled, §3.10);
- Q-B3 (cwd = `$INIT_HOME/work`, supervise §3.4);
- Q-B4 (settle and staleness during a handoff, supervise §3.5);
- Q-B5 (the proxy's price table is the kernel-side allowlist);
- Q-B6 (G3 drives the probe with `send_input`; G4 task runs use `mode: "normal"`, gate.md §3.7.3, so they pay no probe pair);
- Q-B7 (the manifest is what selfmap needs; `ext_api_majors` added, selfmap §3.5).

**Cross-doc:** none.

**Owner decisions** (not in CONVENTIONS §6; the stated default ships unless the owner objects; reconciliation/body-core.md R2 asks the main session to list it)
- **Q-B8.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* The probe uses one real, mostly cached model call pair per G2 arm. The alternative is the mock in G2: $0, but it proves less. The current choice follows spec §6.3.

## 10. Spec deltas

1. **Spec §4 Boot: "does one real model round trip and one tool call, writes ready" on every boot.** Here: only in **probe** mode (G2/G3), settled by the main session in R2 (CONVENTIONS §6 body-core E1). Why:
   - a handoff boot runs on the live log, where a probe exchange would pollute the conversation or need special-casing;
   - G2 has just probed the same binary;
   - crash restarts would pay a model call each time;
   - the first real turn detects a broken provider just as well.
2. **Spec §4 Boot: "runs the rollback check before anything that can fail".** The kernel performs the rollback. The body reads its outcome (`boot.json`, including `update_failed`) before any fallible step (lock, log, network).
3. **The probe is a real tool round trip over the full projected conversation**: 2 requests (3 with one nudge), the second carrying the tool result, not a minimal one-shot prompt. This exercises the candidate's rendering and meets gate.md §3.5 criteria 3 (tool round trip) and 7 (continuity). The cost is a cached prefix plus a few hundred output tokens.
4. **New:**
   - `init-body --manifest`, `--version` and `ext …` dispatch;
   - the body's extra fields in `ready` and `heartbeat`;
   - safe-mode body behavior (no extensions, T0 from `lkg_tree`, no auto-continue);
   - the body exit sub-codes (within "anything else = crash", except that two consecutive 78s at boot mean `Halted{config}`, supervise §3.3).

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.13 and §1.9):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP13 D1 | probe 7(b) makes 2 requests, not 3: after a text-only reply to the nudge there are no results to send, and a third request would end on an assistant turn | §3.9, §8 item 7(b) |
| build: WP13 D2 | the probe driver is built in boot from the loop's public seams, not `Session` internals; `tool_round_trip` means a request that carried tool results came back `Done` | §3.9 |
| build: WP13 D3 | test-only knobs under `INIT_TEST=1`: `INIT_TEST_LOCK_WAIT_MS`, the `INIT_FAULT` action `panic`, and the points `boot.after_env`, `boot.after_boot_event`, `boot.before_ready` | §4.1, §8 item 7(e) |
| build: WP13 D5 | the Root status is rendered through `rootview.rs`; `boot.cause` objects are logged as their `kind`, and `from_sha`/`lkg_sha` are copied only when valid shas | §3.8 |
| build: WP13 D8 | a head error exits 70 (crash path) | §4.4 |
| build: FIX1 N | the body cannot `stat` `$INIT_KERNEL_SOCK` (`file-read-metadata` on `RUN` only); probe the socket by connecting | §3.2 |
| build-deviations §5 L1 (WP11 D5/D6) | the cwd reaches the tools as `ToolCx.paths.cwd` (`ToolCx.paths: PathCx`), not `ToolCx.cwd` | §3.2 |
| ADR-009 (owner decision 2026-10-20, v0.1.1; lane WIRE-009) | the body reads `INIT_WORKSPACE` and `INIT_ACCESS`, leniently (a bad value is a warning, never exit 78); the cwd fact, where the tools start, is the workspace when it is a directory | §3.2, §4.1 |

## 11. References (checked 2026-10-19)

- Rust std `File::lock`/`try_lock`/`unlock`, `TryLockError` (stable since 1.89.0): https://doc.rust-lang.org/stable/std/fs/struct.File.html
- Rust std `panic::set_hook`, `PanicHookInfo`: https://doc.rust-lang.org/stable/std/panic/fn.set_hook.html
- Rust std `process::ExitCode`: https://doc.rust-lang.org/stable/std/process/struct.ExitCode.html
- Rust std `os::unix::fs::PermissionsExt`: https://doc.rust-lang.org/stable/std/os/unix/fs/trait.PermissionsExt.html
- Rust std `hash::DefaultHasher` ("the internal algorithm is not specified"): https://doc.rust-lang.org/stable/std/hash/struct.DefaultHasher.html
- sysexits(3) codes 64/70/78 (FreeBSD/macOS man page): https://man.freebsd.org/cgi/man.cgi?query=sysexits
- spec v0.2 §4 (Boot row), §6.2, §6.3 (G2: ready within 90 s, prefix hash), §7.1, Appendix B (`boot` fields)
- Sibling docs read 2026-10-19 (R2 re-read): kernel/supervise.md §3.1–§3.10, §4.1–§4.5; body/log-context.md §3.4, §3.6, §3.10.4, §4.2; ext/host.md §3.9, §3.13, §4.1, §4.5; ext/authoring.md §3.4; evolution.md §3.6; kernel/gate.md §3.6, §3.7.3; awareness.md §3.6; testing.md §3.4.4, §3.10; CONVENTIONS.md v0.2
- Local verification, macOS 14.6 (Darwin 23.6.0), rustc 1.97.1, 2026-10-19:
  - `File::try_lock` returns `WouldBlock` on a second handle, and `Ok` after the first is dropped;
  - the SUN_LEN limit on socket paths;
  - `/bin/bash` 3.2.57 and `/opt/homebrew/bin/bash` 5.3.20 on PATH.
