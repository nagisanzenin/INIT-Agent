# The `init` TUI: the owner's terminal client

```
Status: v0.1, built (WP26, build/status/WP26.md; as-built differences in §10) · 2026-10-20 · Area: tui (owner client; owner decision CONVENTIONS §6 OW-TUI)
        + v0.1.1 ADR-009 (workspace = `init`'s cwd, `--full`, `/full`, `/undo`, the `FULL` badge; lane WIRE-009)
Depends on: CONVENTIONS.md (§1 live-system safety, §2, §5), root/root.md (§3.1, §3.9, §3.15, §4.2, §4.3),
            kernel/supervise.md (§3.10, §4.5, §4.6), body/loop.md (§3.5, §4.1), body/log-context.md (§4.5),
            root/egress.md (§3.3, §4.2), awareness.md, testing.md (§3.1a, §3.5b), roadmap.md (WP26)
Defines:    the `tui/` workspace, crate and binary `init-tui` (opened by the owner's `init` command); its
            trust rules T-1…T-5; the TTY layout, key map, rendering of owner-channel frames, notice classes
            and status bar; the text sanitizer; the client side of reconnect (Root epochs, replay overlap,
            never resending); line mode and its flags; the TUI's use of the `status`, `egress.stats` and
            `panic` requests; its exit codes; test ids TUI-1…TUI-14 and TUI-R1…TUI-R3 (proposed to testing.md)
```

---

## 1. Purpose

`init` is the owner's everyday terminal for INIT, in the style of Claude Code: the agent's reply streams in, tool calls show as one line each and expand on demand, a multi-line input box sits at the bottom, and a status bar shows the level, model, spend against the daily ceiling, the Root's state and the running body.

It is a **client of the existing owner channel and nothing more**. It connects to `ROOT_HOME/run/owner.sock`, the socket `init-root attach` uses, and speaks the frames root/root.md §4.2 and kernel/supervise.md §4.6 already define. The Root does not change to serve it (small additive fields are proposed in §9 Q1–Q2; the TUI works without them). The TUI has no special trust: anything it can do, `init-root attach` can do.

## 2. Responsibilities / non-goals

**Responsibilities**
- Render the owner channel: display frames (text, thinking, tool start/end, turn end, info, error), echoes, notices and state frames.
- Send the owner's input as `line` frames, Ctrl-C as `interrupt`, `/panic` as `panic`, and leave with `detach`.
- v0.1.1 (ADR-009): report its cwd to the Root once per launch as the workspace (`workspace.set`), turn on full mode for `--full` (`full.set`), and carry the Root's typed confirmations (§3.8.1).
- Show a status bar from the Root's `status` and `egress.stats` requests and the kernel's `state` frames.
- Survive Root restarts and dropped connections: reconnect, replay without showing a frame twice, and never resend owner input.
- Fall back to a line mode when stdin or stdout is not a TTY, so scripts and tests can drive it.
- Make every byte that comes from the agent zone inert before it reaches the terminal.

**Non-goals**
- **Any authority.** The TUI parses no Root or kernel command itself (except mapping `/panic` to the `panic` frame, §3.8); the Root and the kernel parse lines exactly as they do for `init-root attach`.
- **Model calls, keys, spend.** The TUI never talks to a provider, the egress socket, the kernel's `kernel.sock` or `proxy.sock`, or any network host.
- **Reading agent-zone or Root files.** It reads no file under `INIT_HOME` (no session logs, no `state/`) and none under `ROOT_HOME` (no `alerts.jsonl`, no keys). Durable history is the session log, which the agent zone owns.
- **Replacing `init-root attach`.** That client stays in the Root as the minimal fallback.
- **Editing the charter, keys or config.** Those stay `init-root` commands typed in a shell.

## 3. Design

### 3.1 Placement and trust

**Decision: a separate Cargo workspace `tui/`**, with one crate `init-tui` that builds the binary `init-tui`. It is maintainer/owner code, like `root/`, and is never evolvable by the agent.

**The `init` command** is the launcher `scripts/init` (lane REL, installed by `scripts/install.sh` into `INIT_BIN_DIR`, default `~/.local/bin`). It execs `init-tui` with its arguments when one is installed (looked up as `$INIT_TUI`, then `init-tui` next to the launcher, then `ROOT_HOME/bin/init-tui`), and otherwise falls back to `init-root attach`. So `init` always opens an owner terminal, even before the TUI is installed or if it breaks. In this doc, `init` means the TUI reached through the launcher.

| Option | Trust | Cost | Verdict |
|---|---|---|---|
| **`tui/`, its own workspace** | outside every tree the agent can change; its own `Cargo.lock`, audited on its own; no path dependency on `init-root`, so it cannot be linked into the Root by accident | a fifth workspace: one `--sync tui/Cargo.toml` line in `ci/vendor.sh` and one step in `ci/local.sh`; the TUI repeats about 100 lines of protocol knowledge, and drift is caught by TUI-11 and TUI-R1, which run it against the real binaries | **recommended** |
| a crate in `kernel/` | the kernel workspace is agent code from L3 (root/root.md §3.8.1, `lineage/kernel.git`), so the agent could rewrite the owner's own client, which is a path around the Root | shares the kernel's lock, so kernel dependency moves would also move the TUI's | rejected |
| a crate in `root/` | trust-correct | puts crossterm and its tree into the Root's lockfile and audit surface, blurs the Root's size budget (R13, under owner review), and invites linking Root modules | rejected: the Root stays small |

**Trust model.** `init` runs as the owner's user, unsandboxed, like any terminal program. It cannot confine itself: the Root rejects sandboxed peers on `owner.sock` (root/root.md §4.2, the peer check that keeps the zone out). So its code, including its dependencies, is part of the owner's trusted base, the same as the owner's shell. That is why the dependency set is kept small (§3.14). Its rules:

- **T-1, one endpoint.** The only IPC endpoint is `ROOT_HOME/run/owner.sock`. The TUI opens no TCP or UDP socket and no other Unix socket.
- **T-2, no keys, no files.** It opens no file under `ROOT_HOME` or `INIT_HOME` and writes no file at all. It keeps history in memory only.
- **T-3, one exec.** It executes exactly one program, `ROOT_HOME/bin/init-root --root-home <ROOT_HOME> panic`, and only on the `/panic` fallback path when the socket is unreachable (§3.8). It never looks programs up in `PATH`.
- **T-4, installed outside the zone.** `init-tui` and the launcher are installed outside `INIT_HOME` (default `~/.local/bin/`, §9 Q3), where tail rule Z3 stops the zone from writing. A binary inside `INIT_HOME` (for example one the agent built under `work/`) is agent-writable and must never be run as the owner; the launcher must refuse an `$INIT_TUI` that lies inside `INIT_HOME`. **v0.1.1 (ADR-009):** T-4 still holds in full mode. Tail Z3a write-denies `~/.local/bin`, `~/bin` and `install.json`'s `bin_dir` (`init-root install --bin-dir`) in every mode, and the Root refuses a workspace inside them (root/root.md §3.3.2, §3.17.1).
- **T-5, nothing loaded at run time.** No config file, theme, plugin, script or key map is read from anywhere. All behaviour is in the binary.

### 3.2 Process model

std threads, no async runtime (as in the body, CONVENTIONS §5):

| Thread | Owns | Sends to the main loop |
|---|---|---|
| reader | the read half of the attach connection | `Frame(raw line, Value)`, `Disconnected` |
| poller | a second connection for request frames (`status` every 2 s, `egress.stats` every 5 s, or at once on demand) | `Status(Value)`, `Egress(Value)`, `RequestFailed` |
| input | `crossterm::event::read()` (TTY mode) or stdin lines (line mode) | `Key`, `Paste`, `Resize`, `Line`, `StdinEof` |
| signals | `signal_hook::iterator::Signals` for SIGINT (line mode), SIGTERM, SIGHUP | `Signal` |
| main | state, the write half of the attach connection, the terminal | — |

Two rules come from how the Root is built:
- **The reader never blocks on rendering.** The Root writes to each client with a 1 s write timeout and drops a client whose write fails (`link.rs` `attach` and `emit`), and macOS local stream sockets buffer 8 KiB each way (`net.local.stream.sendspace`). A client that stops reading for a second is cut off, and the Root's emit stalls for that second for everyone. The reader therefore drains the socket continuously into an unbounded channel.
- **Requests use their own connection.** On the attach connection, broadcast frames and request answers are written by different Root threads without a shared lock, so their bytes could interleave. Request frames go only on the poller's connection; the attach connection carries only `attach`, `line`, `interrupt`, `panic` and `detach`. An unknown `t` closes the attach connection (`link.rs` `conn`), so the TUI sends no other frame type there.

### 3.3 What the owner channel provides (as built)

**Root → client, on the attach connection:**

| `t` | Fields | Written by | TUI use |
|---|---|---|---|
| `display` | `kind` ∈ `text` `thinking` `tool_start` `tool_end` `turn_end` `info` `error`; `text`; optional `call_id`, `name`, `ok`; `boot_seq` | the primary body, relayed by the kernel | the transcript (§3.5) |
| `echo` | `text`, plus `qid` (Root: queued while no kernel runs) or `seq` (kernel: durable in its inbox) | Root or kernel | confirms the owner's lines (§3.11) |
| `notice` | `level` ∈ `info` `warn` `alert`, `text`, `ts` | Root (alerts, replies to Root lines) or kernel (`[kernel] <kind>`, `/status`) | notices (§3.10) |
| `state` | `slot`, `mode` (`normal` `provisional` `safe`), `busy`, `sha12`, `queued`, `level`, `members` | kernel, on every busy↔idle change of the primary | spinner, body field, kernel mode |
| `replay_end` | — | Root, after the replay | switches from replay to live |

**Requests, on the poller's connection** (`{"v":1,"id","cmd","args"}`):

| `cmd` | Fields the TUI reads |
|---|---|
| `status` | `slot`, `mode`, `halted`, `panic{at, channel}`, `level`, `cap`, `max_level`, `tuples{current, lkg, audited}`, `kernel` (null when none runs, else `ready`, `boot_seq`), `queued`, `root{started_at, pid, version, test_mode}` |
| `egress.stats` | `money{day_spent_musd, day_reserved_musd, day_limit_musd, day_resets_at, total_spent_musd, total_limit_musd}`, `models[{model, routable}]` (root/egress.md §4.2) |
| `panic` | the answer's `message` |

**Client → Root:** `{"client":"init-tui/<version>","replay":true,"t":"attach"}` first, then `{"t":"line","text"}`, `{"t":"interrupt"}`, `{"t":"panic"}`, `{"t":"detach"}`. The TUI writes ICJ (sorted keys, through `serde_json::Value`).

**v0.1.1 requests (ADR-009; root/root.md §4.2, §3.17), on the poller's connection:**

| `cmd` | When the TUI sends it | Fields the TUI reads |
|---|---|---|
| `workspace.set{cwd}` | once per launch, right after the first attach and before any owner line; `cwd` is `std::env::current_dir()` as the OS gives it (the Root canonicalizes and validates it) | `access` (an `AccessView`), `changed`, `refused{code, message}` |
| `full.set{on, confirm?}` | `--full` at launch, after `workspace.set` | `access`, `snapshot`; the `root.confirm` and `root.mode` errors |
| `access.get` | after any notice with `code:"access"` | `AccessView` |
| `status` | as before | also `access` (`access`, `full`, `workspace`, `restart_pending`, `snapshot`) |

`/full on`, `/full off`, `/undo` and their `confirm` forms are ordinary `line` frames that the Root handles itself (§3.8), so the TUI needs no request for them.

### 3.4 Screen layout (TTY mode)

The conversation is written to the terminal's **main screen**, so it goes into the terminal's own scrollback and can be scrolled, searched and copied with the terminal's tools. Only the bottom **live region** is redrawn. The Ctrl-O transcript view is the only alternate-screen view.

```
› add a pty tool                                                  ← owner line (confirmed by its echo)
I'll check SELF.md first, then write the tool.                    ← assistant text
▸ bash   cat SELF.md                                  ✓ exit 0, 96 lines · 0.3 s
▸ edit   src/tools/pty.rs (2 edits)                   ✗ error: no_match · 0.0 s
▲ [kernel] promoted                                               ← notice, styled by class
· tokens: in 12345 (cached 11002), out 321                        ← turn end (dim)
──────────────────────────────────────────────────────────────── live region starts here
The anchor moved, so I'll read the file again and retry…           ← streaming text
⠹ bash   cargo test -p init-body pty                  running · 12 s
╭──────────────────────────────────────────────────────────────────────────────╮
│ > also add a timeout flag▏                                                   │
╰──────────────────────────────────────────────────────────────────────────────╯
 L0 · deepseek-flash · $0.04 run · $1.23/$5.00 today · running · body 3f9c0a1b2c4d lkg · busy 41 s
 Enter send · Shift-Enter newline · Ctrl-C interrupt (twice: quit) · Ctrl-O transcript · ? keys
```

- **Committing.** A finished block (an owner line, a finished tool batch, a notice, a turn end, a completed paragraph of text) is printed once above the live region and never redrawn. While a reply streams, complete lines are committed as soon as the live region would exceed `rows − input − 3`, so the live region always fits the screen.
- **Retry.** When the body reports a retry (`display{kind:"info", text:"retrying …"}`), the partial text still in the live region is discarded (body/loop.md §3.6). If part of it was already committed, a dim line `↺ retry: the partial reply above was discarded` is printed.
- **Redraw.** Each render moves to the top of the live region, rewrites only the rows that changed, and clears below. Output is wrapped in synchronized-update brackets (`BeginSynchronizedUpdate`, ignored by terminals that lack them). On resize the live region's top row is found again with one cursor-position query (`crossterm::cursor::position`) and the region is redrawn; committed text is the terminal's to reflow.
- **No mouse capture**, so native selection and wheel scrolling keep working.
- The hint line shows only while the input box is empty.

### 3.5 Rendering display frames

| Frame | TTY mode | Line mode (the v0.2 attach rules, kernel/supervise.md v0.2 §4.6) |
|---|---|---|
| `text` | appended to the streaming paragraph; rendered with the Markdown subset below | printed as it arrives |
| `thinking` | `✻ thinking…` with a spinner, one per block; full text when verbose (Ctrl-T or `--verbose`) | `[thinking…]` once per block; full text with `--verbose` |
| `tool_start` | a running row: spinner, `name`, `text` (the body's one-line summary: the bash command up to 200 characters, `read <path>`, `edit <path> (n edits)`, `ext <name>`), elapsed time; keyed by `call_id` | newline, then `» <name>: <text>` |
| `tool_end` | the row becomes `✓` or `✗` (`ok`), with the body's summary (`exit 0, 96 lines`, `error: no_match`) and the duration the TUI measured; the batch is committed collapsed when its last call ends or the next text arrives | `  « ok <text>` or `  « error <text>` |
| `turn_end` | a dim `· <text>` line (token counts and dropped deltas) | newline; the text with `--verbose` |
| `info`, `error` | `· <text>` dim, or `✗ <text>` red | `[info] <text>`, `[error] <text>` |
| a `boot_seq` different from the previous display frame's | a divider `── body restarted (boot <n>) ──` before the frame | the same divider as text |

**Markdown subset** (applied to `text` only, line by line, so streaming stays O(new bytes)): fenced code blocks (indented, dim background), inline code, `**bold**`, `#` headings, and `-`/`*`/`1.` lists. Links and URLs print as plain text, and there are no images, tables or HTML. This subset is the fixed vocabulary the agent can rely on (§3.15).

Parallel tool calls: the body sends every `tool_start` of a batch before the batch runs, and the `tool_end`s in completion order (body/loop.md §3.7). The TUI keeps one row per `call_id`, in start order.

### 3.6 Tool calls: collapsed and expanded

In the main screen every tool call is one collapsed line. Scrollback cannot be redrawn, so expanding happens in the **transcript view** (Ctrl-O, alternate screen), which shows every block of this TUI process:

- `j`/`k` or the arrow keys move the selection; PgUp/PgDn and `g`/`G` scroll; Enter or Space toggles the selected block; `e` expands all and `c` collapses all; `q`, Esc or Ctrl-O returns.
- An expanded tool call shows `name`, `call_id`, the full `tool_start` text wrapped, `ok`, the `tool_end` summary, the start time (UTC) and the duration. Thinking blocks expand to their full text.
- **What "result" means in v0.1.** The body sends only a one-line summary in `tool_end`, never the tool output (body/loop.md §4.1), and the kernel relay drops fields it does not know (§10, D7). Expanding therefore shows the full command and the summary, not the output. A larger result needs a body change plus §9 Q2's pass-through; the TUI already renders an optional `detail` string under the summary if one ever arrives.
- The transcript view keeps at most 50,000 blocks or 32 MiB of text; beyond that the oldest tenth is dropped from the view (the terminal's scrollback is unaffected).

### 3.7 Input box and keys

The input box is editable at all times, including while a reply streams. It grows to 10 rows (at most 40% of the screen) and then scrolls inside itself. Editing works on grapheme clusters (`unicode-segmentation`) and display widths (`unicode-width`): Backspace removes exactly one grapheme, which Vietnamese input methods that correct text by sending Backspace (Telex, VNI) rely on, and wide CJK and emoji characters move the cursor by two columns.

| Key | Effect |
|---|---|
| Enter | send the box as one `line` frame (multi-line text travels as one frame with `\n` inside) |
| Shift-Enter · Alt-Enter · Ctrl-J · `\` then Enter | insert a newline. Shift-Enter needs the kitty keyboard protocol: at start the TUI asks `crossterm::terminal::supports_keyboard_enhancement()` and, if it is supported, pushes `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES`. Terminal.app does not support it, so there the other three work. |
| paste | bracketed paste (`EnableBracketedPaste`): inserted verbatim, CRLF → LF, never sent by itself |
| **Ctrl-C** | send `interrupt` (the body aborts the current turn and keeps partial output, body/loop.md §3.5; harmless while idle). The draft in the box is kept. The hint shows `interrupt sent · Ctrl-C again within 2 s quits init (the agent keeps running)`. |
| **Ctrl-C again within 2 s** | send `detach`, restore the terminal, exit 0. The agent is not touched. |
| Esc | while busy and no popup is open: send `interrupt` (it does not arm quitting); otherwise close the popup |
| Ctrl-D on an empty box | detach and exit 0 |
| Up / Down | move between lines; on the first line, Up recalls earlier messages from this process's history |
| Left, Right, Home, End, Ctrl-A, Ctrl-E, Alt-Left, Alt-Right, Backspace, Delete, Ctrl-W, Ctrl-U, Ctrl-K | the usual line-editing moves and deletions |
| `/` at the start of an empty box | a completion popup of the owner commands (§3.8) with one-line descriptions; Tab completes |
| `?` on an empty box | the key list overlay (as in Claude Code); `?` anywhere else is text |
| Ctrl-O | the transcript view (§3.6) |
| Ctrl-T | show or hide thinking text |
| Ctrl-L | redraw |

- Ctrl-Z (suspend) is not supported in v0.1; it is ignored.
- **Size cap.** A `line` frame whose ICJ encoding exceeds 900 KiB is not sent. The box keeps the text and shows `too long to send (n KiB > 900 KiB); split it`. Both the Root and the kernel stop at 1 MiB per line, and the Root closes a connection that sends more (`FRAME_MAX`).
- While the agent is busy, the box's placeholder says `Enter steers the current run · /later <text> queues a follow-up` (CONVENTIONS §6: a plain line during a run is a steer).

### 3.8 Owner commands

The TUI sends every line unchanged; the Root and the kernel parse them as for `init-root attach` (exact match after trim). The one exception is `/panic`.

| Line | Handled by | TUI behaviour |
|---|---|---|
| `/panic` (alone) | Root, before anything else (root/root.md §3.9) | sends `{"t":"panic"}` on the attach connection. If that connection is down, it sends the request `panic` on a fresh connection and waits up to 2 s. If the socket is unreachable, it runs `ROOT_HOME/bin/init-root --root-home <ROOT_HOME> panic`, the Root's own fallback that writes the latch (root/root.md §3.9.6, as built). No confirmation is asked. |
| `/resume` | Root while it is panicked, halted or stopped; otherwise the kernel's own `Stopped` | line |
| `/root status` · `/root ledger` · `/root stop` · `/root help` | Root | line; the answer arrives as a `[root] …` notice |
| `/stop` · `/status` · `/kernel` · `/accept <task>` · `/bless <job>` · `/reject <job>` · `/later <text>` | kernel (kernel/supervise.md §4.6) | line |
| `/new` · `/compact [focus]` · `/branch <id> [note]` · `/label <id> <text>` | body (body/log-context.md §4.5) | line; when the echo of `/new` arrives, a divider `── new session ──` |
| `/clear` (v0.2.0, UX-KEYMODEL) | body, as `/new` | the TUI sends `/new` (TTY and line mode); the kernel never sees `/clear` |
| `/key [provider]` · `/model [n\|provider/model]` · `/setup` (v0.2.0, UX-KEYMODEL) | Root requests `providers.list`, `keys.set`, `providers.add`, `model.use` on their own connection | `/key`: a hidden field (the box shows `•`; never in the history, the transcript, a `line` frame or a log), sent once to the Root, which stores it 0600; the TUI zeroes its buffers. `/model`: the configured models, then a number picks; the Root rewrites `kernel/config.toml` and restarts the kernel. `/setup`: the first-run flow again. First run: when `providers.list` says the selected model has no usable key, `init` asks provider → key → model before the first chat. In line mode on a terminal the key cannot be hidden, so the TUI points to `init-root keys set <provider>` instead |
| anything else | body, as an input (a steer while busy) | line |

**v0.1.1 (ADR-009): Root lines for the workspace** (root/root.md §4.2, §3.17):

| Line | Handled by | TUI behaviour |
|---|---|---|
| `/workspace` · `/full` | Root | line; the answer is a `[root]` notice with the access state |
| `/full on` · `/full on confirm` · `/full off` | Root | line. The first `/full on` in a session gets a notice that says what full mode allows and asks for `/full on confirm`; the owner types that line. `/full off` acts at once. |
| `/undo` · `/undo confirm` | Root | line. `/undo` gets a notice naming the snapshot it would restore and asks for `/undo confirm`. While the agent is busy the Root refuses it (`root.busy`, `body_busy`) before asking; interrupt or wait, then retry. |

The completion popup lists exactly these. `/approve`, `/deny` and `/clear-safe` were removed in v0.3 (kernel/supervise.md §4.6) and are not listed. A command is recognized only when the whole box is a single line; a multi-line message that starts with `/stop` is ordinary text to the kernel too.

#### 3.8.1 The workspace and `--full` at launch (v0.1.1, ADR-009)

`init`'s cwd is the workspace: the directory the owner runs `init` in is the one the agent may edit.

1. After the first `attach`, before it sends any owner line, the TUI sends `workspace.set{cwd}` once. Reconnects do not resend it.
2. If the answer has `refused`, the Root's notice explains it (`[root] no workspace: cd into a project and run init to let me edit it`), and the TUI shows it like any notice. The agent can still read the home; it just cannot edit outside `INIT_HOME`.
3. With `--full`, the TUI then sends `full.set{on:true}`. On `root.confirm` it shows the error's `message` in a confirmation prompt in the input box, reads one line, and resends the same request with `"confirm": <that line>`. It takes the phrase from `data.phrase` and never hard-codes it. A second `root.confirm` (a mistyped phrase) leaves full mode off with a notice; nothing is retried automatically. On `root.mode` with `data.reason = "charter"`, it shows `full mode is disabled by your charter (sandbox.allow_full)`.
4. The Root applies a change to the running agent at its next idle boundary (root/root.md §3.17.3). Until then `status.access.restart_pending` is true, and the status bar's workspace field shows it (§3.9).

The TUI still opens no file and holds no secret (T-1, T-2): reading its own cwd is a system call, and the confirmation phrase is not a secret.

### 3.9 Status bar

```
 L0 · deepseek-flash · $0.04 run · $1.23/$5.00 today · running · body 3f9c0a1b2c4d lkg · busy 41 s · queued 1
```

| Field | Source | Refresh | Notes |
|---|---|---|---|
| level | Root `status.level`, `cap` | 2 s poll, and at once after any Root notice | `L0`; `L0 (cap L1)` when wide |
| model | kernel `state.model` (§9 Q2; absent as built) | each `state` frame | as built: the only routable model from `egress.stats.models` if exactly one is routable, shown dim; otherwise `model ?` |
| run cost | kernel `state.task_musd` (§9 Q2; absent as built) | each `state` frame | as built: `+$0.12 since attach`, the change in the Root's day spend since this TUI attached (it includes gate and probe spend) |
| today against the ceiling | Root `egress.stats.money`: `day_spent_musd` of `day_limit_musd` | 5 s poll, and after each `turn_end` | the figure the hard ceiling governs (root/egress.md §3.3); yellow at ≥ 80%; red with `ceiling` once `day_spent_musd + day_reserved_musd ≥ day_limit_musd`, the Root's admission test with nothing more requested |
| Root state | Root `status.slot`, `mode`, `panic`, `halted`; the connection | 2 s poll, and at once after any Root notice | `running`, `starting`, `backoff`, `stopping`, `stopped` (dim), `safe mode` (yellow), `PANICKED 3 min ago via <channel> · /resume` (red, reverse video), `HALTED (<reason>)` (red), `Root unreachable · retry in 1.6 s` (red) |
| body | kernel `state.sha12` and `mode`; Root `status.tuples` | each `state` frame; 2 s poll | `body <sha12>` plus `lkg` when `tuples.current == tuples.lkg`, `provisional` or `safe` from the kernel's mode |
| busy | kernel `state.busy` | each `state` frame | `busy 41 s` (elapsed by the TUI's clock), spinner at 10 Hz |
| queued | Root `status.queued` and kernel `state.queued` | as above | only when > 0 |
| access (v0.1.1, ADR-009) | Root `status.access` | 2 s poll, and at once after a notice with `code:"access"` | the workspace with the home shown as `~` (`~/code/app`), or `no workspace` (dim) when there is none; `FULL` in red reverse video while `access.full` is true; a trailing `↻` while `restart_pending` (the agent picks up the change at its next idle boundary). Placed right after the level. |

```
 L0 · FULL ~/code/app · deepseek-flash · $0.04 run · $1.23/$5.00 today · running · body 3f9c0a1b2c4d lkg
```

- When the terminal is narrow, fields are dropped from the right in this order: queued, busy, model, run cost, body, today, the workspace path, level. The Root state and `FULL` are never dropped.
- **Authority.** The level, money, access and Root state come from Root requests, which no zone process can forge. The body, mode, busy and model fields come from the kernel, which is maintainer code at L0–L2 and agent code from L3; they are informative, not authoritative.
- `NO_COLOR` set: no colours; states are shown with reverse video and words.
- **Times.** std has no local time zone, and the TUI adds no crate for one, so it shows relative times (`3 min ago`, `resets in 5 h 12 min`) and, where an absolute time is needed, UTC with a `Z` (`21:04:05Z`).

### 3.10 Notices

As built, notices carry only `level`, `text` and `ts`. The TUI classifies them by their text, for styling only; no TUI behaviour except an immediate status poll depends on a notice's text.

| Class | As-built text (source) | Rendered |
|---|---|---|
| promotion | `[kernel] promoted`, `[kernel] lkg`, `[kernel] provisional` (kernel `notice`) | `▲` green |
| level up | `[root] level L<a>→L<b>: …` (Root alert `level_up`) | `▲` green, and the status bar's level flashes |
| rollback | `[root] … falling back to LKG`, `[root] kernel crash loop: rolling back to the LKG tuple` (Root alerts `rollback`, `kernel_crash_loop`); `[kernel] update_failed` | `▼` yellow |
| demotion | `[root] demoted L<a>→L<b> (…)` (Root alert `demoted`) | `▼` red, and the level flashes |
| ceiling hit | none as built: the Root writes a `ceiling_hit` lineage record and no notice. The TUI raises its own notice when the polled spend meets the limit (§3.9): `■ daily ceiling reached: $5.00 of $5.00; requests are refused until the day resets (in 5 h 12 min)`, once per owner day, and the same for the total | `■` red |
| panic | `PANIC at …` (Root alert `panic`) | a full-width red line; the status bar turns red at once |
| safe mode, halt | `[root] system safe mode…`, `[root] Halted{…}` | yellow, red |
| access (v0.1.1) | `[root] workspace: <path>`, `[root] no workspace: cd into a project and run init to let me edit it`, `[root] full mode on: …`, `[root] full mode off …` (Root notice `code:"access"`) | info; `full mode on` in red; triggers an immediate status poll |
| snapshot (v0.1.1) | a snapshot taken, skipped (over 2 GiB, not APFS, another volume) or restored by `/undo` (Root notice `code:"snapshot"`) | info; a skip in yellow |
| anything else | — | by `level`: info dim, warn yellow, alert red |

- The Root's greeting at each attach (`[root] attached: …`) is shown once at the first attach; after a reconnect the status bar shows `reconnected` instead.
- **Forgery.** A kernel can send a notice whose text starts with `[root]`, and as built a client cannot tell the two apart. From L3 that matters. §9 Q1 asks the Root to stamp `src` on every frame it relays; with it, the Root classes (level up, demotion, rollback of tuples, panic, ceiling) require `src:"root"`, and kernel-sourced look-alikes render as plain kernel notices.

### 3.11 Reconnect, redelivery, no duplicates

**States:** `Connecting` → `Attaching` (replay until `replay_end`) → `Live` → on EOF or error `Reconnecting` (backoff 100 ms, doubling to 2 s, without end in TTY mode).

**Root epoch.** Before each attach the poller asks `status`; the epoch is `root.started_at` plus `root.pid`. If `status` lacks them (as in `init-fakeroot`), the epoch is unknown and treated as "same".

**Display frames, no duplicates.**
- **New epoch (the Root restarted):** the replay ring was in the old Root's memory and is gone, so nothing in the new ring was shown before. The TUI renders the replay after a divider `── Root restarted (21:04:05Z); output from while it was down is in the session log, not replayed ──`.
- **Same epoch (only the connection dropped):** the ring has no frame ids as built (§10, D3), so the TUI matches overlap. It keeps the exact bytes of the last 2,000 frames (at most 256 KiB, the ring's own bounds) received in this epoch. On re-attach it finds the largest index `j` in the replay `R` such that `R[j−m..j]` equals the last `m = min(64, kept, j)` kept frames, and renders `R[j..]` only. If no `j > 0` matches, the ring rotated past everything shown, and the TUI renders all of `R` after `── reconnected; some output may be missing ──`. If more than one `j` matches, it uses the largest and prints the same divider. With §9 Q1's `rseq`, this becomes `attach{since: last_rseq}` and the Root sends only newer frames.

**Owner input, never resent.**
- The TUI never resends a `line` by itself, so it cannot create a duplicate input. Exactly-once delivery into the log already follows from the Root's durable queue, its redelivery after each `ready`, the kernel's `qid` dedup and the body's `input_id` dedup (kernel/supervise.md §3.10 item 3).
- A sent line is shown at once as a dim pending line `› text`. It is confirmed by its echo, matched first-in first-out by text: an echo `E` matches a pending line `P` when `E == P.text`, or when `P.text.trim()` is `/later ` followed by `E` (the kernel echoes a follow-up without the prefix).
- A Root echo (with `qid`) marks the line `· queued` (no kernel was running). The later kernel echo (with `seq`) for the same text marks it delivered. The two become one transcript line, not two (§10, D8).
- An echo that matches no pending line (another client, or the replay) is shown as an ordinary owner line.
- Lines the Root or kernel handle as commands (§3.8) produce no echo and are shown at once as command lines, never pending.
- A line still unconfirmed 30 s after the kernel is ready again and the Root reports `queued: 0` is marked `no confirmation seen (it may have been delivered before the Root restarted); check before resending`. Up recalls it; the owner decides.
- `[root] the owner queue is full (200); your message was not queued` marks the newest pending line `not queued` in red.

**Interrupts are not queued.** As built, the Root delivers `interrupt` only to a ready kernel and drops it otherwise (§10, D6). The hint says `interrupt sent`, plus `(no body is running)` when the last `status` showed no ready kernel.

### 3.12 Line mode

Line mode is chosen when stdin or stdout is not a TTY, when `TERM=dumb`, or with `--plain` or `--raw`. It behaves like `init-root attach` should per the v0.2 rules (the as-built attach differs, §10 D1):
- one owner line per stdin line, sent as a `line` frame; rendering per the line-mode column of §3.5; echoes as `> <text>` (not `» `, which v0.2 uses for tool starts); notices as their text;
- the sanitizer of §3.13 applies; ANSI styling only if stdout is a TTY and `NO_COLOR` is unset;
- a `> ` prompt only when stdin is a TTY and the body is idle;
- SIGINT sends `interrupt`; a second SIGINT within 2 s detaches and exits 0;
- `--raw` prints each frame as received, one JSON line each, after re-escaping U+007F–U+009F and the bidi controls as `\uXXXX` (serde_json already escapes C0);
- **`--wait-idle`** for scripts: after stdin EOF the TUI stays attached until every sent line is confirmed by a kernel echo, a `state{busy:false}` has arrived after the last confirmation, and 1 s has passed with no frame; then it detaches and exits 0. `--timeout` bounds it (default 600 s with `--wait-idle`).
- Without `--wait`, an unreachable Root at start exits 69 at once. After a successful attach, line mode reconnects like TTY mode, within `--timeout`.

### 3.13 Text safety

Every string taken from a frame (`text`, `name`, `call_id`, notice text, echo text, the `[root] {json}` answers) is agent-zone data from the TUI's point of view and passes one sanitizer before any terminal write. Terminal escape injection (an OSC 52 clipboard write, OSC 8 links, a window title, cursor moves that fake the status bar, `CSI 2J`) is the threat; the v0.2 `TIOCSTI` reasoning (kernel/supervise.md §3.10) is why no zone process gets a terminal in the first place.

| Input | Output |
|---|---|
| `\n` | a line break (the renderer's, not the byte) |
| `\t` | spaces to the next multiple of 4 columns |
| `\r\n` | a line break; a lone `\r` → `␍` |
| U+001B (ESC) | `␛` |
| other C0 (U+0000–U+001F), U+007F, C1 (U+0080–U+009F, including the one-byte CSI U+009B) | `<U+00XX>` in a dim style |
| bidi controls U+202A–U+202E, U+2066–U+2069, U+200E, U+200F, U+061C | `<U+XXXX>` (Trojan Source, CVE-2021-42574) |
| everything else | as is; width from `unicode-width` per grapheme |

The renderer writes styles only through crossterm commands built by the TUI, never by passing through bytes from a frame. TUI-4 checks that every ESC in the output belongs to a sequence the TUI itself produced.

### 3.14 Library choice and dependencies

**Decision: crossterm for terminal I/O, plus a small renderer of our own.** No ratatui.

| | ratatui 0.30.2 + crossterm 0.29.0 | **crossterm 0.29.0 + own renderer** | no crates (termios and key decoding by hand) |
|---|---|---|---|
| crates in the closure (macOS; Linux +1, `linux-raw-sys`) | 66 | **25** | 8 (`serde_json`'s 5, `libc`, the two `unicode-*`) |
| new to `vendor/` | 35 | **11** | 2 |
| proc-macro crates built | 8 (`darling_macro`, `derive_more-impl`, `document-features`, `indoc`, `instability`, `rustversion`, `strum_macros`, `thiserror-impl`) | 1 (`document-features`) | 0 |
| `#![forbid(unsafe_code)]` in `init-tui` | yes | yes | no: termios, `ioctl(TIOCGWINSZ)` and signals need `unsafe` |
| inline layout with native scrollback | `Viewport::Inline` + `insert_before` | about 250 lines (§3.4) | the same 250 lines |
| key decoding (CSI u, bracketed paste, resize, Esc ambiguity) | crossterm | crossterm | about 600 hand-written lines, the usual source of terminal bugs |
| snapshot tests | `TestBackend` | the renderer is a pure function of (state, width, height) → a text grid with a style mask, compared with golden files | the same |
| build risk tonight | low | low | high |

ratatui's value is full-screen cell layouts and widgets (a multi-line editor would come from `ratatui-textarea` 0.9.3, on top of the 66). A Claude-Code-style client writes most output once into the main screen and redraws only a small live region, which needs little of that; the input box (§3.7) is about 400 lines either way. The hard, bug-prone part is terminal input and modes, which crossterm covers. The hand-written option saves 17 crates but gives up `forbid(unsafe_code)` and adds the riskiest code, so it is worse.

**`tui/crates/init-tui/Cargo.toml` dependencies** (exact pins, vendored, offline):

```toml
[dependencies]
crossterm = { version = "=0.29.0", default-features = false, features = ["events", "bracketed-paste"] }
unicode-width = "=0.2.2"
unicode-segmentation = "=1.13.3"
serde_json = "=1.0.154"   # Value only, no derive; never `preserve_order` (CONVENTIONS §5)
signal-hook = "=0.3.18"   # already in crossterm's tree: SIGINT in line mode, SIGTERM/SIGHUP restore
```

- No dev-dependencies: golden files are compared by a 30-line helper (`UPDATE_GOLDEN=1` rewrites them), and the fixture server is std only.
- The closure: `bitflags`, `cfg-if`, `crossterm`, `document-features`, `errno`, `itoa`, `libc`, `litrs`, `lock_api`, `log`, `memchr`, `mio`, `parking_lot`, `parking_lot_core`, `rustix`, `scopeguard`, `serde_core`, `serde_json`, `signal-hook`, `signal-hook-mio`, `signal-hook-registry`, `smallvec`, `unicode-segmentation`, `unicode-width`, `zmij`. New to `vendor/`: `crossterm`, `document-features`, `litrs`, `lock_api`, `parking_lot`, `parking_lot_core`, `signal-hook`, `signal-hook-mio`, `signal-hook-registry`, `unicode-segmentation`, `unicode-width`. The other 14 are already vendored for the other workspaces. These counts come from the crates.io sparse index with the newest compatible versions (2026-10-20); the committed `tui/Cargo.lock` is the final word.
- **Vendoring.** WP26 adds `--sync tui/Cargo.toml` to `ci/vendor.sh` (all workspaces are synced together, because `cargo vendor` deletes what the given manifests do not need), and `tui/.cargo/config.toml` points at `../vendor` like the others (CONVENTIONS §2).
- RustSec advisory-db has no entry for `crossterm`, `signal-hook`, `signal-hook-mio`, `parking_lot`, `unicode-width` or `unicode-segmentation`; `mio`'s two advisories concern versions before 0.8.11 (checked 2026-10-20).
- Toolchain: rustc 1.97.1, edition 2024; crossterm's MSRV is 1.63.

**Workspace layout:**

```
tui/
  Cargo.toml              [workspace] resolver = "3", members = ["crates/init-tui"]; edition 2024; publish = false
  Cargo.lock
  .cargo/config.toml      vendored sources, offline (directory = "../vendor")
  crates/init-tui/
    src/main.rs           arguments, mode choice, exit codes
    src/conn.rs           attach connection, reader, poller, epochs, backoff, panic fallback
    src/proto.rs          frame classification over serde_json::Value; outgoing frames
    src/model.rs          blocks, tool rows, pending lines and echo merge, overlap dedup
    src/notice.rs         notice classes (§3.10)
    src/input.rs          editor, history, completion
    src/keys.rs           key map (§3.7)
    src/render/{live.rs, status.rs, transcript.rs, wrap.rs, md.rs, sanitize.rs}
    src/line.rs           line mode (§3.12)
    src/term.rs           raw mode, keyboard flags, paste, restore on every exit path, panic hook
    tests/                fixture server (std), golden/, sessions/, pty_drive.py
```

### 3.15 Evolvability

The agent may later improve the UI, and none of the routes gives the TUI trust or the agent a path to the owner's terminal:
- **Through the body (now).** The agent shapes what the owner sees by what its body sends: the text and its Markdown subset (§3.5), and, once the kernel keeps unknown display fields (§9 Q2), optional typed fields that the TUI renders if it knows them and ignores otherwise. These are ordinary body changes, gated like any other. A body change can never add a terminal capability, because the TUI interprets a fixed vocabulary and sanitizes everything else (§3.13).
- **A body-zone skill.** A T0 skill (for example `skills/owner-ui/SKILL.md`) can describe the subset and the frame kinds, so the agent learns how its output renders and refines its own style.
- **Changes to the TUI itself** are proposals. The agent files one with `proposal.file` (stored in `ROOT_HOME/state/proposals/`, alerted, never applied; root/root.md §4.1.3, FIX5) with a patch against `tui/`. It reaches the owner's terminal only as a maintainer release of `tui/`, like any other maintainer code.
- **Not an extension.** A T1 Wasm component runs in the body sandbox; a UI extension would be zone code with a channel to the owner's TTY, which the design rules out (kernel/supervise.md §3.10: no zone process has a terminal).

## 4. Interfaces

### 4.1 Command line

```
init-tui [--root-home DIR] [--plain | --raw] [--no-replay] [--verbose] [--wait] [--wait-idle]
         [--timeout S] [--latency-report] [--full] [--version] [--help]
```

From v0.1.1 the TUI's cwd is the agent's workspace (§3.8.1, ADR-009): run `init` in the project you want the agent to edit.

The launcher passes its arguments through unchanged (`init --plain` runs `init-tui --plain`); `--which` belongs to the launcher, so `init-tui` defines no flag of that name.

| Flag | Meaning |
|---|---|
| `--root-home DIR` | else `INIT_ROOT_HOME`, else `~/.init-root` (the rule `init-root` uses) |
| `--plain` | line mode even on a TTY |
| `--raw` | line mode printing frames as JSON lines (§3.12) |
| `--no-replay` | the first attach asks `replay:false`; reconnects still replay, for the dedup of §3.11 |
| `--verbose` | thinking text and turn-end lines (TTY mode: Ctrl-T toggles) |
| `--wait` | line mode: wait for an unreachable Root instead of exiting 69 (TTY mode always waits) |
| `--wait-idle` | line mode: stay attached after stdin EOF until idle (§3.12) |
| `--timeout S` | line mode: overall bound in seconds |
| `--latency-report` | at exit, print the latency histograms of §6 to stderr |
| `--full` | v0.1.1: ask the Root for full mode at launch, which lets the agent write the whole home except secrets and the Root's paths; the Root asks for a typed confirmation the first time (§3.8.1). The charter can forbid it (`sandbox.allow_full`). |
| `--version` | `init-tui (INIT Agent TUI) <version>`, plus `built from tui/ at <sha12>` when the build set `INIT_TUI_GIT_SHA` |

Environment read: `INIT_ROOT_HOME`, `HOME`, `TERM`, `NO_COLOR`. Nothing else, in particular no `INIT_TEST*` and no `INIT_HOME`. From v0.1.1 the TUI also reads its cwd (`std::env::current_dir()`, not `$PWD`) and sends it as the workspace.

### 4.2 Exit codes

| Code | Meaning |
|---|---|
| 0 | detached (Ctrl-C twice, Ctrl-D, SIGHUP, SIGTERM, `--wait-idle` done) |
| 1 | internal error (message on stderr) |
| 2 | `--timeout` elapsed |
| 64 | usage |
| 69 | the Root is unreachable (line mode without `--wait`); same code as `init-root attach` |
| 76 | protocol: the first frames from the socket are not an attach exchange |
| 101 | a Rust panic, after the terminal is restored |

### 4.3 Frames and requests

§3.3 lists them. The attach frame's `client` is `init-tui/<version>`; the Root treats it as information only. Request ids are `t-<n>`, counted per connection.

## 5. Failure modes and recovery

| Failure | Detection | Behaviour |
|---|---|---|
| Root not running at start | `connect` fails (ENOENT, ECONNREFUSED) | TTY: the status bar says `Root unreachable` and the screen shows how to start it (`init-root start`, or the launchd job); retries with backoff. Line mode: exit 69, unless `--wait`. |
| Root restarts (crash, launchd, upgrade) | EOF; the next `status` shows a new epoch | divider, re-attach with replay; pending lines stay pending until their echo; the Root's queue is durable |
| Root drops this client (write blocked ≥ 1 s) | EOF with the same epoch | overlap dedup on re-attach (§3.11); counted in `--latency-report`; should not happen with the reader thread |
| Kernel restart, handoff, rollback | `boot_seq` changes; `state` frames; Root notices | divider `── body restarted (boot <n>) ──`; the conversation continues |
| Root panicked | the PANIC notice; `status.slot == "panicked"` | red status bar; input still goes to the Root's queue (200 lines), which says so |
| Owner queue full | the Root's notice | the newest pending line is marked `not queued` |
| Echo lost (Root restart between queue and echo, or the ring rotated) | no echo 30 s after the kernel is ready with `queued: 0` | marked `no confirmation seen`; never resent (§3.11) |
| Malformed frame | JSON error | ignored and counted; `[tui] malformed frame ignored` at most once a minute |
| Frame over 2 MiB | the reader's limit | the connection is dropped and re-attached (the Root never sends more than its 1 MiB lines plus its own fields) |
| Terminal under 20 columns or 5 rows | size | only the status bar and a one-line input, plus `terminal too small` |
| Terminal closed | SIGHUP or EOF on the TTY | `detach`, exit 0 |
| SIGTERM | signal | restore the terminal, `detach`, exit 0 |
| Rust panic inside the TUI | panic hook | restore the terminal (raw mode off, keyboard flags popped, paste off, alternate screen left, cursor shown), print the message, exit 101; the agent is unaffected |
| Kitty keyboard query unanswered | crossterm's timeout | treated as unsupported; Alt-Enter, Ctrl-J and `\` Enter still give newlines |
| `init` resolves to the system's `init` (Linux, where `/usr/bin/init` or `/sbin/init` exists) | `init --which` (the launcher) prints nothing useful, or `init --version` shows systemd | put `INIT_BIN_DIR` first in `PATH`, or run `init-tui` directly |
| `init-tui` missing or broken | the launcher finds no `init-tui` | the launcher runs `init-root attach`, the plain owner terminal |
| The egress is not running | `egress.stats` refused (`root.mode`) | the money fields show `?` |
| `init-fakeroot` lacks `egress.stats` | `root.unknown_cmd` | the money fields show `?` (tests) |
| v0.1.1: a Root without ADR-009 (`workspace.set` → `root.unknown_cmd`) | the error | the access field shows `?`; `--full` prints `this Root does not support full mode` and continues |
| v0.1.1: the cwd is refused (the home, outside the home, a Root path, the deny list) | `refused` in the answer, and the Root's notice | the notice is shown; the access field says `no workspace`; the agent can read but not edit the home |
| v0.1.1: the `--full` confirmation is mistyped or cancelled (Esc) | a second `root.confirm`, or no line | full mode stays off; a notice says so; `/full on` can be used later |

## 6. Performance notes (spec §9)

| Metric | Target | Measured by |
|---|---|---|
| keystroke → frame flushed (TTY) | p50 ≤ 4 ms, p99 ≤ 16 ms | TUI-13, `--latency-report` |
| display frame received → flushed | p99 ≤ 16 ms at 200 frames/s; p99 ≤ 33 ms at 2,000 frames/s | TUI-13, fixture server |
| body stdout → screen, end to end (body → kernel → Root → TUI) | p99 ≤ 25 ms on the M2 | TUI-11 with a paced mock stream; needs the mock's send times (UNVERIFIED that `init-mockllm` logs them; WP26 adds that to the testkit if not) |
| never dropped by the Root | 0 disconnects in 60 s at 5,000 frames/s | TUI-13; the fixture server uses the Root's 1 s write timeout and 8 KiB buffers |
| start → first paint, with a full 256 KiB replay | ≤ 150 ms, excluding the terminal's own paint time | TUI-13 |
| resize → repaint | ≤ 33 ms | TUI-12 |
| idle CPU | < 0.5% of one core; no timer faster than 10 Hz, and that one (the spinner) only while busy | TUI-13 |
| memory | ≤ 64 MiB RSS with 50,000 blocks | TUI-13 |
| load on the Root | about 0.7 requests/s (`status` every 2 s, `egress.stats` every 5 s) | Root logs in TUI-R1 |

- **Budget.** The two relay hops cost about 50 µs each (root/root.md §3.15). The main loop renders at most once per 8 ms when something changed, so a keystroke waits at most 8 ms, and a render of a live region of up to 60 rows takes about 2 ms (UNVERIFIED until TUI-13). Text deltas are coalesced between renders, so a flood costs one render per 8 ms, not one per frame.
- **Cheap redraws.** Only changed rows of the live region are rewritten: a keystroke usually rewrites one or two rows, about 200 bytes. Streaming text rewraps only the current paragraph.
- Disk: the TUI writes no files. Building it adds about 0.2–0.4 GB of target directory (UNVERIFIED; measured in WP26).

## 7. Invariants and security (spec §2.1)

| Invariant | How the TUI keeps it | Test |
|---|---|---|
| Not a path around the Root | T-1: the only endpoint is `owner.sock`; no model call, no other socket, no network | TUI-14 |
| Keys never leave the Root | T-2: it opens no file under `ROOT_HOME` or `INIT_HOME` and holds no secret | TUI-14 |
| No special trust | an ordinary owner-socket client: the frames and lines `init-root attach` uses, nothing more; never linked into `init-root` (separate workspace, no path dependency) | review; TUI-5 |
| The agent cannot change the owner's client | source in `tui/` (maintainer repo, in no lineage the agent writes); installed outside `INIT_HOME` (T-4); nothing loaded at run time (T-5) | TUI-14 |
| Zone text is inert | the sanitizer (§3.13) on every frame string; styles only from the TUI's own commands | TUI-4 |
| Authority is visible | Root-sourced status fields are separated from kernel-reported ones (§3.9); notice text affects styling only (§3.10) | TUI-1 |
| No duplicate input | the TUI never resends; delivery is exactly once by the Root and kernel dedups (§3.11) | TUI-7, TUI-R2 |
| `/panic` never depends on the zone | frame → request → `init-root panic` latch fallback, none of which passes through the zone (T-3) | TUI-9, TUI-R1 |
| Quitting never touches the agent | quitting sends `detach` only | TUI-3 |
| The terminal is always restored | every exit path, including panics and signals, runs the restore | TUI-3, TUI-12 |
| Writes no files | T-2 | TUI-14 |

## 8. Test plan

New ids, proposed to testing.md. Every test runs at $0 with the mock LLM. Live-environment tests stop only processes they started, by held PID (CONVENTIONS §1). Tests that start the real `init-root` run in the disposable environment (testing.md §3.1a), or, for v0.1, in the owner-machine variant: a test Root built without `live-sweep`, in scratch homes, which signals only the PIDs it holds (provisional decision, reconciliation/build-deviations.md §3.14, RELEASE_PLAN.md). No test connects to the owner's own `ROOT_HOME` or `INIT_HOME`: the harness always passes `--root-home <scratch>` and asserts the path lies under the test's scratch directory.

| Id | Case | Expectation | Env |
|---|---|---|---|
| TUI-1 | render snapshots at 80×24, 120×40 and 40×12: streaming text, thinking collapsed and verbose, a parallel batch running and done (ok and error), turn end, each notice class, the status bar in each Root state (running, safe mode, panicked, halted, unreachable), the ceiling at 80% and 100%, an `lkg` and a `provisional` body | golden text grids and style masks match | live, unit |
| TUI-2 | editor: Shift-Enter (with kitty flags), Alt-Enter, Ctrl-J, `\` Enter; bracketed paste with CRLF; Backspace over Vietnamese text in precomposed and combining forms; wide CJK and emoji cursor positions; history; the 900 KiB cap | one grapheme per Backspace; the cursor column equals the display width; an over-size line is not sent | live, unit |
| TUI-3 | keys: Ctrl-C; Ctrl-C again after 1.9 s and after 2.1 s; Esc while busy and idle; Ctrl-D on an empty box | exactly one `interrupt` per Ctrl-C; the second within 2 s sends `detach` and exits 0 with the terminal restored; after 2.1 s it is another `interrupt` | live, unit |
| TUI-4 | sanitizer: every C0, C1, DEL, ESC and bidi code point in `text`, `name`, `call_id`, notice and echo text; OSC 52, OSC 8, `CSI 2J`, DECSET; a fuzz of 10⁵ random frames | visible escapes only; every ESC in the output starts a sequence the TUI produced | live, unit |
| TUI-5 | protocol against the fixture server: `attach` first; replay, `replay_end`, then live; unknown `t` and unknown fields ignored; requests only on the second connection; no frame other than the five client frames on the attach connection | as stated | live |
| TUI-6 | reconnect with the same epoch: the server drops the connection after n frames, at many n; the ring rotates past the last seen frame; a repeated frame block makes the overlap ambiguous | no frame rendered twice; the `may be missing` divider exactly when the ring rotated or the match was ambiguous | live |
| TUI-7 | reconnect with a new epoch (fixture "Root restart") with three pending lines | divider; lines confirmed by the later kernel echoes; the server counts exactly one `line` frame per line typed | live |
| TUI-8 | echo merge: a Root echo (`qid`) then a kernel echo (`seq`) for one line; `/later x`; an echo from another client; `/new` | one transcript line per input, `queued` then delivered; the `/later` echo matches; the foreign echo is shown; the `/new` divider | live |
| TUI-9 | `/panic` paths: attach up; attach down but the socket up; socket absent, with a stub `init-root` at `<scratch>/bin/init-root` that records its argv | one `panic` frame; one `panic` request; one stub run with `--root-home <scratch> panic`. Never the real `init-root`. | live |
| TUI-10 | line mode with pipes: v0.2 rendering golden; `--raw` exact frames with C1 re-escaped; `--wait-idle`; exit codes 0, 2, 64, 69, 76 | as stated | live |
| TUI-11 | scripted owner session, full stack: `init-fakeroot` + the real `init-kernel` + the real `init-body` + `init-mockllm` (text, a bash call, a failing edit, a turn end); `init-tui --plain --wait-idle < session.txt` with a message, a steer, `/status`, `/new`, a message, `/stop` | golden output after normalizing ids and times; end-to-end latency of §6; the session log has each input exactly once | live (under `init-fakeroot`, every process held by the runner) |
| TUI-12 | TTY end to end through a pty (`python3 -I`, stdlib `pty` and `os.forkpty`) on the TUI-11 stack: type, stream, Ctrl-O and expand a tool call, resize 80 → 120 → 60, Ctrl-C twice | the screen matches golden snapshots; `stty -a` is identical before and after | live |
| TUI-13 | latency and load: keystroke and frame latencies, a 60 s flood at 5,000 frames/s with the Root's write timeout and buffer sizes, first paint with a full replay, idle CPU, memory | the §6 targets | live |
| TUI-14 | confinement: run TUI-5's and TUI-11's sessions with the TUI under `sandbox-exec` with a profile that allows only its binary, system libraries, the pty, `/dev/null` and a connection to the test socket, and denies every `file-write*`, every read under the scratch `ROOT_HOME` except the socket, and all network | both sessions pass unchanged, which shows T-1, T-2 and "writes no files"; this runs against the fixture server and `init-fakeroot`, because the real Root rejects sandboxed peers | live |
| TUI-R1 | the real Root in test mode with the mock: a session, `/panic`, `/resume`, a scripted demotion and a level change (test ledgers, root/root.md §3.16) | the status bar shows PANICKED within 1 s of `/panic`; the demotion notice and the new level appear | disposable, or owner-machine for v0.1 |
| TUI-R2 | Root restart: the runner kills its own test Root by held PID with three lines queued, then restarts it | the TUI re-attaches; each line appears exactly once in the session log; no `line` frame is resent | disposable, or owner-machine for v0.1 |
| TUI-R3 | ceiling: a test charter with `day_usd = "0.01"` and mock prices | the status bar turns red; the ceiling notice appears once | disposable, or owner-machine for v0.1 |

| TUI-15 | v0.1.1 against the fixture server: launch in a scratch project dir; with `--full`; with `--full` and a wrong phrase; a reconnect; a server without `workspace.set` | exactly one `workspace.set` with the TUI's cwd, before any `line`; `full.set{on:true}`, then the same request with `confirm` set to the typed line after `root.confirm`; a wrong phrase leaves it off with no further request; no resend after reconnect; `?` and the notice on an old server; status-bar snapshots with `FULL`, a workspace, `no workspace` and `↻` | live, unit |
| TUI-R4 | v0.1.1 end to end: the TUI, the real Root in test mode and the mock, with `init` run in a fixture project directory on APFS under the test's scratch home; the mock's tool call writes a file in the workspace; then `/undo` and `/undo confirm` | the file appears in the workspace; after `/undo confirm` the workspace equals its session snapshot, a `snapshot` notice reports the restore, and a `pre_undo` snapshot holds the file | disposable, or owner-machine for v0.1 |

`ci/local.sh` runs TUI-1…TUI-10, TUI-14 and TUI-15 in the fast tier and TUI-11…TUI-13 in the full tier; TUI-R1…TUI-R3 go with the Root scenarios (disposable, or the owner-machine variant for v0.1).

## 9. Open questions

*Decided 2026-10-20 as recommended (the owner said proceed): Q1 and Q2 are built in WP26 (Root `4ce8dfe`, kernel `18a7ee6`; shapes in §10, "Built"); Q3 belongs to lane REL. The text below is kept as the record.*

**Cross-doc**
1. **→ root/root.md §3.15, §4.2: three small additive fields on the owner channel.** (a) `rseq`, a u64 counted per Root process, on every frame the Root emits, plus `attach{since}`, which replays only frames with a larger `rseq` and ends with `replay_end{epoch, gap}`; (b) `src` (`root` or `kernel`), set by the Root on every frame and overwriting any `src` from the kernel; (c) `code` on alert notices (the code it already writes to `alerts.jsonl`), and a notice for each `ceiling_hit`. **Recommended: yes, in WP26 by the Root lane**, about 30 lines in `link.rs` and `egress.rs`, with no new command, thread or file. It makes §3.11's dedup exact and makes Root notices unforgeable from L3. The TUI works without it.
2. **→ kernel/supervise.md §4.6: the `state` frame and the display relay.** (a) `state` gains `model` (the live scope's model) and `task_musd` (today's spend booked to the current task), and is also sent after each `turn_end`; (b) the display relay keeps fields it does not know, within the 1 MiB line. **Recommended: yes to both**, about 20 lines in `init-proto` and `kernel-supervise`. (a) fills the status bar's model and run cost; (b) is what lets the agent extend what the owner sees through body changes alone (§3.15). The TUI ignores fields it does not know, so nothing breaks either way.

**Internal**
3. **→ lane REL (`scripts/release.sh`, `scripts/install.sh`, `scripts/init`): building and installing `init-tui`.** **Recommended:** `release.sh` builds `tui/` (`cargo build --release --offline` inside `tui/`, so its `.cargo/config.toml` applies) and puts `init-tui` in the release directory; `install.sh` installs it next to the launcher in `INIT_BIN_DIR` (a new file, then a rename into place); the launcher ignores an `$INIT_TUI` inside `INIT_HOME` (T-4). `init-root install` does not change; `ROOT_HOME/bin/init-tui` stays a lookup location only.

## 10. Spec deltas

| Source | This doc | Why |
|---|---|---|
| spec v0.3 §13, §18; root/root.md §3.15: owner I/O through `init-root attach` | a second owner client, `init-tui` (opened by `init`), on the same socket and frames, with no new trust | owner decision 2026-10-20 (CONVENTIONS §6 OW-TUI) |
| kernel/supervise.md v0.2 §4.6 rendering rules, kept by root/root.md §3.15 | line mode keeps them, except echoes print as `> ` (v0.2's `»` marks tool starts); TTY mode renders collapsible blocks | a richer owner view; scripts keep the old format |
| (new) | the owner's client sanitizes all zone text | zone text is untrusted from L0, and the kernel's from L3 |
| (v0.1.1) | `init`'s cwd is the agent's workspace (`workspace.set`); `--full`, `/full`, `/undo` with the Root's typed confirmations; `FULL` and the workspace in the status bar | owner decision OW-PERM, ADR-009 (lane WIRE-009 builds it) |

**Docs and code disagree** (the code is what exists; noted here, raised where they matter):

| # | Docs | As built | Effect on the TUI |
|---|---|---|---|
| D1 | root/root.md §3.15, §4.3: `init-root attach` renders with the v0.2 rules (`»`/`«`, `[thinking…]`, `[info]`/`[error]`) | `cli.rs` `attach` prints `display.text` raw (a tool start prints its command with no marker or newline), echoes as `» text`, notices as text, and ignores `state` | none; the TUI implements the rules itself |
| D2 | v0.2 rule: a second Ctrl-C within 2 s detaches | every Ctrl-C sends `interrupt`; the owner leaves with Ctrl-D | the TUI implements the rule |
| D3 | root/root.md §3.15: a replay ring of 2,000 frames or 256 KiB | the ring has no frame ids or sequence numbers (`link.rs` `emit`) | overlap matching (§3.11); Q1(a) |
| D4 | — | notices carry `level`, `text`, `ts` only; kernel notices use the same frame, so `[root]` text can come from the kernel | text-based styling only; Q1(b), (c) |
| D5 | root/egress.md §3.6: hits are recorded | a `ceiling_hit` is a lineage record; no owner notice | the TUI derives the ceiling notice from `egress.stats`; Q1(c) |
| D6 | kernel/supervise.md §3.10 item 7: inputs queue while no kernel runs | `interrupt` is delivered only to a ready kernel and otherwise dropped (`link.rs` `deliver`); `init-fakeroot` queues it | the hint says so |
| D7 | — | the kernel relays display frames through a typed struct (`init-proto` `Display`), so fields the body adds are dropped | expansion shows summaries only; Q2(b) |
| D8 | root/root.md §3.15: "no double echo" | a line queued while no kernel runs is echoed by the Root (`qid`) and again by the kernel (`seq`) after redelivery | merged into one line (§3.11) |
| D9 | — | the Root writes to clients with a 1 s timeout while holding the ring and client locks, so a stalled client delays every emit by up to 1 s and is then dropped | the reader thread never stalls (§3.2) |
| D10 | root/root.md §3.15 lists `/approve`, `/deny`, `/clear-safe` among the kernel's lines | removed in v0.3 (kernel/supervise.md §4.6) | not in the completion list |
| D11 | root/root.md §4.2's request list | the Root also answers `egress.stats` (`link.rs`), as root/egress.md §4.4's `init-root egress stats` uses; `init-fakeroot` does not | the TUI uses it and tolerates its absence |
| D12 | — | the kernel's `state` frame carries no model and no cost, and is sent only on busy↔idle changes | fallbacks in §3.9; Q2(a) |

**Built (WP26, 2026-10-20).** Q1 and Q2 close D3, D4, D5, D7 and D12 as follows; the TUI still handles the as-built shapes (fixture tests run both).

| Item | As built |
|---|---|
| Q1(a) `rseq` | every ring frame carries `rseq` (u64 per Root process, from 1, assigned under the ring lock). `attach{since, epoch}`: `since` is honoured only when `epoch` is absent or equals the Root's epoch, and is ignored with `replay:false`. `replay_end{epoch, gap, src}`, with `epoch = "<root.started_at>/<root.pid>"`; `gap` is true when frames after `since` rotated out. `status.root.epoch` carries the same string. The TUI sends `epoch` with `since` (an addition to §9 Q1: a Root restart between the `status` and the attach cannot match a stale `since`), still drops any `rseq ≤ last`, and keeps the overlap match for Roots without `rseq`. |
| Q1(b) `src` | `src:"kernel"` on every frame relayed from `owner.out` (overwriting the kernel's own), `src:"root"` on the Root's notices, alerts and queued-line echoes, and on the per-connection greeting and `replay_end` (which carry no `rseq`). An alert the Root relays for the kernel (`source:"kernel"`) is `src:"kernel"`. |
| Q1(c) `code`, ceiling notice | alert notices carry `code` (as in `alerts.jsonl`). Each `ceiling_hit` record gives a notice `{code:"ceiling_hit", level:"alert", src:"root", text:"[root] ceiling reached: <ceiling> (<code>); attempted <a> of <l> micro-USD; refused[ (counts toward demotion)]"}`, at most one per ceiling a minute, with no `alerts.jsonl` line and no `notify.cmd`. |
| Q2(a) `state` | `state{busy, level, members, mode, model, queued, sha12, slot, t, task_musd}`; `model` is the configured `[model] id`, `task_musd` today's spend of the current task from the kernel proxy's accounting (`x-init-task`); both null when unknown. Also sent right after each relayed `turn_end`. |
| Q2(b) relay | display fields the kernel does not know (e.g. `detail`) pass through; a body's own `boot_seq`, `rseq`, `src`, `t`, `v` are dropped; all extra fields are dropped if they would push the line past 1 MiB − 1 KiB. |

**TUI as built, where it differs from §3–§8** (the simplest correct option; recorded in build/status/WP26.md):

| # | This doc | As built | Why |
|---|---|---|---|
| B1 | §3.7: renders at most once per 8 ms | frames coalesce per 8 ms; a keystroke renders at once | §6 keystroke p50 ≤ 4 ms; keys are human-rate, so the render rate stays bounded |
| B2 | §3.12 `--wait-idle`: a `state{busy:false}` after the last confirmation | required only once a `state` frame has been seen in the session; 1 s with no frame is always required | the Root's fake kernel (TUI-R) sends no `state` frames |
| B3 | §3.8 `/panic` | in line mode, a `/panic` read while the attach is not (yet) live goes straight to the request and then the `init-root panic` fallback | `/panic` never waits behind an attach |
| B4 | §4.2 exit 76 | a connection the Root closes before the first `replay_end` counts as unreachable (69 without `--wait`); 76 stays for non-JSON or a request answer as the first frames | a closed socket is not a foreign protocol |
| B5 | §3.11 "never resend" | TTY mode does not send a line typed while no attach connection is up: the box keeps it with a hint | nothing is queued by the TUI, so nothing can be resent |
| B6 | §3.5 tool rows | a call still running at `turn_end` or a body restart ends as `? no result` | it would hold every later block in the live region |
| B7 | §3.6 | in the transcript view a text block over 6 rows shows 5 rows and `… n more rows` until expanded | long replies stay navigable |
| B8 | §8 TUI-11 | not built: TUI-R runs the real Root in test mode with its fake kernel (`/bin/cat` body) and `init-mockllm` behind the egress, through a pty and in line mode | the real-kernel + real-body stack under `init-fakeroot` needs the WP15 template homes; disk was at the 5 GB floor. Body stdout → screen through the real body stays UNVERIFIED (§6) |
| B9 | §8 TUI-12 | the pty run checks the output stream and `stty -g` before/after, not golden screens | no terminal emulator in the stdlib; the screens are golden-tested at the renderer (TUI-1) |
| B10 | §8 TUI-13 | 5,000 frames/s for 10 s (`TUI13_FLOOD_SECS=60` for the full minute); ignored test, run by `ci/local.sh` with `INIT_CI_TIER=full` on a release build | CI time on a shared, loaded machine |

**§6 measured** (M2, release `init-tui`, machine load 25–45 from parallel lanes, TUI-13 twice): keystroke → flush p50 0.5 ms, p99 1.3–12.9 ms; frame → flush p99 9.6–16.6 ms at 200 frames/s (one run 0.6 ms over target), 12.9–22.1 ms at 2,000 frames/s; first paint with a full 256 KiB replay 41–50 ms; idle CPU 0.0–0.2% of a core; RSS 7–18 MiB with 50,000 blocks; 0 disconnects at 5,000 frames/s for 10 s. Real Root (TUI-R, debug builds): PANICKED in the status bar 50 ms after `/panic`; an owner line → its `/bin/cat` echo on screen through Root → fake kernel → body → Root → TUI p50 13.9 ms, p99 23.9 ms. Disk: `tui/target` 0.17 GB (debug + release). Closure: 25 crates on macOS, as §3.14 predicted (`tui/Cargo.lock`); `vendor/` went from 265 to 277 crates in the WP26 run (`ci/vendor.sh --no-delete`, which may include other lanes' manifest changes).

## 11. References

All checked 2026-10-20.
- Code read: `root/crates/init-root/src/link.rs` (owner socket, `attach`, `emit`, `line`, `deliver`, `enqueue`, `conn`, `request`), `cli.rs` (`attach`, `panic_cmd`), `sup.rs` and `egress.rs` (alerts, `ceiling_hit`, `ceilings()`), `panic.rs`; `kernel/crates/kernel-supervise/src/supervisor.rs` (`owner_in`, `owner_out`, `notice`, display and state relay); `kernel/crates/init-proto/src/stdio.rs` and `root_link.rs` (`Display`, `OwnerOutFrame`); `kernel/crates/init-testkit/src/fakeroot/mod.rs` (owner socket); `seed/src/loop/frames.rs`, `seed/src/loop/mod.rs`, `seed/src/tools/mod.rs` (display emission, `start_text`, summaries); the v0.2 attach rules in git history (`63d2c8f:docs/kernel/supervise.md` §4.6).
- `sysctl net.local.stream.sendspace net.local.stream.recvspace` on the owner's M2: 8192 and 8192.
- crates.io API and sparse index (generic User-Agent): ratatui 0.30.2 (2026-06-19, MSRV 1.88), ratatui-core 0.1.2, ratatui-widgets 0.3.2, ratatui-crossterm 0.1.2, crossterm 0.29.0 (2025-04-05, MSRV 1.63), unicode-width 0.2.2, unicode-segmentation 1.13.3, signal-hook 0.3.18 (newest 0.3.x; 0.4.5 exists, but crossterm 0.29 needs ^0.3.17), ratatui-textarea 0.9.3 (2026-10-05, ratatui-core ^0.1.1) — https://crates.io/api/v1/crates/<name> ; https://index.crates.io/ . Closure counts: a small resolver over the sparse index (normal and build dependencies, `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`, newest compatible versions), compared with the repo's `vendor/` (265 crates).
- crossterm 0.29.0 API: `terminal::enable_raw_mode`, `terminal::supports_keyboard_enhancement`, `event::{read, poll, Event, PushKeyboardEnhancementFlags, KeyboardEnhancementFlags, EnableBracketedPaste}`, `terminal::{size, BeginSynchronizedUpdate, EnterAlternateScreen, ClearType}`, `cursor::MoveToPreviousLine`, `tty::IsTty` — https://docs.rs/crossterm/0.29.0/crossterm/
- signal-hook 0.3.18 `flag::register` (a safe function) — https://docs.rs/signal-hook/0.3.18/signal_hook/flag/fn.register.html ; unicode-width 0.2.2 — https://docs.rs/unicode-width/0.2.2/unicode_width/
- ratatui 0.30.2 `Viewport`, `TerminalOptions`, `TestBackend` — https://docs.rs/ratatui/0.30.2/ratatui/
- RustSec advisory database — https://github.com/rustsec/advisory-db (`crates/<name>`)
- kitty keyboard protocol — https://sw.kovidgoyal.net/kitty/keyboard-protocol/ ; synchronized output — https://gist.github.com/christianparpart/d8a62cc1ab659194337d73e399004036 ; xterm control sequences — https://invisible-island.net/xterm/ctlseqs/ctlseqs.html ; NO_COLOR — https://no-color.org/ ; Trojan Source (CVE-2021-42574) — https://trojansource.codes/
