# The `init-agent` TUI: the owner's terminal client

```
Status: v0.1, built (WP26, build/status/WP26.md; as-built differences in §10) · 2026-10-20 · Area: tui (owner client; owner decision CONVENTIONS §6 OW-TUI)
        + v0.1.1 ADR-009 (workspace = `init-agent`'s cwd, `--full`, `/full`, `/undo`, the `FULL` badge; lane WIRE-009)
        + 2026-10-24 TUI-UX: tool activity blocks, the token/cache/cost segment, the heartbeat, `/details` (§3.6, §3.9, §3.12)
        + 2026-10-27 v0.5.0 TUI-BRAND: the semantic palette and tiers, the liveness row, the agent/Root status bar, tables, glyphs, identity (§3.4, §3.9, §3.9a, §3.10)
        + 2026-10-28 v0.5.1 TUI-FIX: what the v0.5.0 user sessions found (docs/user-sessions/): `/panic` asks first and the
          stopping commands complete last, answered questions stay answered, one money rule, self-change spend, gate waits,
          the no-workspace explanation, a clean quit (§3.7, §3.7.2, §3.8, §3.9, §3.10a, §3.8.1, §9 Q5)
Depends on: CONVENTIONS.md (§1 live-system safety, §2, §5), root/root.md (§3.1, §3.9, §3.15, §4.2, §4.3),
            kernel/supervise.md (§3.10, §4.5, §4.6), body/loop.md (§3.5, §4.1), body/log-context.md (§4.5),
            root/egress.md (§3.3, §4.2), awareness.md, testing.md (§3.1a, §3.5b), roadmap.md (WP26)
Defines:    the `tui/` workspace, crate and binary `init-tui` (opened by the owner's `init-agent` command); its
            trust rules T-1…T-5; the TTY layout, key map, rendering of owner-channel frames, notice classes
            and status bar; the text sanitizer; the client side of reconnect (Root epochs, replay overlap,
            never resending); line mode and its flags; the TUI's use of the `status`, `egress.stats` and
            `panic` requests; its exit codes; test ids TUI-1…TUI-14 and TUI-R1…TUI-R3 (proposed to testing.md)
```

---

## 1. Purpose

`init-agent` is the owner's everyday terminal for INIT, in the style of Claude Code: the agent's reply streams in, tool calls show as one line each and expand on demand, a multi-line input box sits at the bottom, and a status bar shows the level, model, spend against the daily ceiling, the Root's state and the running body.

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

**The `init-agent` command** is the launcher `scripts/init-agent` (lane REL, installed by `scripts/install.sh` into `INIT_BIN_DIR`, default `~/.local/bin`). It execs `init-tui` with its arguments when one is installed (looked up as `$INIT_TUI`, then `init-tui` next to the launcher, then `ROOT_HOME/bin/init-tui`), and otherwise falls back to `init-root attach`. So `init-agent` always opens an owner terminal, even before the TUI is installed or if it breaks. In this doc, `init-agent` means the TUI reached through the launcher.

Until v0.4.1 the launcher was named `init`. It was renamed because `~/.local/bin/init` shadows Linux's `/sbin/init` in user shells, where `~/.local/bin` often comes first in `PATH` (issue nagisanzenin/INIT-Agent#1). On macOS, v0.4.1 also installed `~/.local/bin/init` as a shim that printed a rename notice on stderr and ran `init-agent` with the same arguments; v0.5.0 drops it. No file named `init` is installed on either platform: `install.sh` removes INIT's own old `init` (the pre-v0.4.1 launcher or the v0.4.1 shim, checked by path, regular file and its content markers) and leaves a foreign `init` alone with a note. `install.sh` refuses a relative `INIT_BIN_DIR` and one that is, or resolves through symlinks to, `/sbin`, `/usr/sbin`, `/bin` or `/usr/bin`.

| Option | Trust | Cost | Verdict |
|---|---|---|---|
| **`tui/`, its own workspace** | outside every tree the agent can change; its own `Cargo.lock`, audited on its own; no path dependency on `init-root`, so it cannot be linked into the Root by accident | a fifth workspace: one `--sync tui/Cargo.toml` line in `ci/vendor.sh` and one step in `ci/local.sh`; the TUI repeats about 100 lines of protocol knowledge, and drift is caught by TUI-11 and TUI-R1, which run it against the real binaries | **recommended** |
| a crate in `kernel/` | the kernel workspace is agent code from L3 (root/root.md §3.8.1, `lineage/kernel.git`), so the agent could rewrite the owner's own client, which is a path around the Root | shares the kernel's lock, so kernel dependency moves would also move the TUI's | rejected |
| a crate in `root/` | trust-correct | puts crossterm and its tree into the Root's lockfile and audit surface, blurs the Root's size budget (R13, under owner review), and invites linking Root modules | rejected: the Root stays small |

**Trust model.** `init-agent` runs as the owner's user, unsandboxed, like any terminal program. It cannot confine itself: the Root rejects sandboxed peers on `owner.sock` (root/root.md §4.2, the peer check that keeps the zone out). So its code, including its dependencies, is part of the owner's trusted base, the same as the owner's shell. That is why the dependency set is kept small (§3.14). Its rules:

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
| `notice` | `level` ∈ `info` `warn` `alert`, `text`, `ts`; `code` (Root alerts); v0.5.0: `data` (kernel notices, kernel/supervise.md §4.6) | Root (alerts, replies to Root lines) or kernel (`[kernel] <kind>`, `/status`) | notices (§3.10); kernel `data` feeds the gate track (§3.10a) |
| `state` | `slot`, `mode` (`normal` `provisional` `safe`), `busy`, `sha12`, `queued`, `level`, `members` | kernel, on every busy↔idle change of the primary | spinner, body field, kernel mode |
| `replay_end` | — | Root, after the replay | switches from replay to live |

**Requests, on the poller's connection** (`{"v":1,"id","cmd","args"}`):

| `cmd` | Fields the TUI reads |
|---|---|
| `status` | `slot`, `mode`, `halted`, `panic{at, channel}`, `level`, `cap`, `max_level`, `tuples{current, lkg, audited}`, `kernel` (null when none runs, else `ready`, `boot_seq`), `queued`, `root{started_at, pid, version, test_mode}` |
| `egress.stats` | `money{day_spent_musd, day_reserved_musd, day_limit_musd, day_resets_at, total_spent_musd, total_limit_musd}`, `models[{model, routable}]` (root/egress.md §4.2) |
| `lineage.tail{n:8}` (v0.5.0, TUI-OBSERVE) | `rows[{seq, by, kind, hash, data}]`, `verify{ok, records, sigs_checked, head_seq, head_hash, key, error}` (root/root.md §4.2), asked on its own connection each time the transcript view opens; an older Root answers `root.unknown_cmd` and the panel says so |
| `panic` | the answer's `message` |

**Client → Root:** `{"client":"init-tui/<version>","replay":true,"t":"attach"}` first, then `{"t":"line","text"}`, `{"t":"interrupt"}`, `{"t":"panic"}`, `{"t":"detach"}`. The TUI writes ICJ (sorted keys, through `serde_json::Value`).

**v0.1.1 requests (ADR-009; root/root.md §4.2, §3.17), on the poller's connection:**

| `cmd` | When the TUI sends it | Fields the TUI reads |
|---|---|---|
| `workspace.set{cwd, mode?}` | once per launch, right after the first attach and before any owner line; `cwd` is `std::env::current_dir()` as the OS gives it (the Root canonicalizes and validates it); `mode:"workspace"` only with `--workspace` (ADR-012) | `access` (an `AccessView`, with `mode` from v0.3.1), `changed`, `refused{code, message}` |
| `full.set{on, confirm?}` | `--full` at launch, after `workspace.set`; `/full on`, `/full off` | `access`, `snapshot`; the `root.confirm` error (only under a `sandbox.mode = "workspace"` charter, ADR-012) and `root.mode` |
| `access.get` | after any notice with `code:"access"` | `AccessView` |
| `status` | as before | also `access` (`access`, `full`, `workspace`, `restart_pending`, `snapshot`) |

`/full on`, `/full off`, `/undo` and their `confirm` forms are ordinary `line` frames that the Root handles itself (§3.8), so the TUI needs no request for them.

### 3.4 Screen layout (TTY mode)

The conversation is written to the terminal's **main screen**, so it goes into the terminal's own scrollback and can be scrolled, searched and copied with the terminal's tools. Only the bottom **live region** is redrawn. The Ctrl-O transcript view is the only alternate-screen view.

```
■■■□ INIT 0.5.0 · L0 · root ok                                    ← identity, once at the first attach (v0.5.0)
› add a pty tool                                                  ← owner line (confirmed by its echo)
I'll check `SELF.md` first, then write the tool.                  ← assistant text (inline code: ink, muted backticks)
◇ ran 2 tools in 1.8 s · bash · edit                  ✕ 1 failed  ← a frozen tool activity (§3.6)
  ◇ edit   src/tools/pty.rs (2 edits)                 ✕ error: no_match · 0.0 s   ← failures stay visible
✓ [kernel] promoted                                               ← notice: state glyph, tag, text (§3.10)
──────────────────────────────────────────────────────────────── live region starts here
The anchor moved, so I'll read the file again and retry…           ← streaming text
◇ bash cargo test -p init-body pty            1 tool call · 1 running · 0 failed · 12 s   ← running activity
    running 12 s · still working… no output yet
⠹ running bash · 12 s                                                       run 17 s   ← liveness row (v0.5.0)
┌──────────────────────────────────────────────────────────────────────────────┐
│ › also add a timeout flag▏                                                   │
└──────────────────────────────────────────────────────────────────────────────┘
 busy 17 s · turn in 238k out 1.0k · cache 99.6% · $0.004 · $1.23/$5.00 today     ~/proj · ● FULL · L0 · root ok
 Enter send · Shift-Enter newline · Ctrl-C interrupt · Ctrl-O details · ? keys
```

- **The liveness row (v0.5.0, TUI-BRAND).** While the agent works (`state{busy:true}`), one row directly above the input box says what it is doing and for how long: `waiting for the model · 12 s` (after the run started or a request ended, until the next frame), `thinking · 4 s`, `writing`, `writing · no new text for 9 s` (a stream quiet for 3 s or more), or `running <tool> · 41 s`; on the right, `run 1 min 4 s`. It is never dropped for width and it carries the only spinner on screen; rows of calls in flight show a still `◇` in the rewrite colour instead. The words carry the state, so it reads the same under `NO_COLOR`. It shares the live region's height budget with the gate track (§3.10a), the tray and the popup.

- **Committing.** A finished block (an owner line, a frozen tool activity, a notice, a completed paragraph of text) is printed once above the live region and never redrawn. While a reply streams, complete lines are committed as soon as the live region would exceed `rows − input − 3`, so the live region always fits the screen.
- **Retry.** When the body reports a retry (`display{kind:"info", text:"retrying …"}`), the partial text still in the live region is discarded (body/loop.md §3.6). If part of it was already committed, a dim line `↺ retry: the partial reply above was discarded` is printed.
- **Redraw.** Each render moves to the top of the live region, rewrites only the rows that changed, and clears below. Output is wrapped in synchronized-update brackets (`BeginSynchronizedUpdate`, ignored by terminals that lack them). On resize the live region's top row is found again with one cursor-position query (`crossterm::cursor::position`) and the region is redrawn; committed text is the terminal's to reflow.
- **No mouse capture**, so native selection and wheel scrolling keep working.
- The hint line shows only while the input box is empty.

### 3.5 Rendering display frames

| Frame | TTY mode | Line mode (the v0.2 attach rules, kernel/supervise.md v0.2 §4.6) |
|---|---|---|
| `text` | appended to the streaming paragraph; rendered with the Markdown subset below | printed as it arrives |
| `thinking` | `✻ thinking…` with a spinner, one per block; full text when verbose (Ctrl-T or `--verbose`) | `[thinking…]` once per block; full text with `--verbose` |
| `tool_start` | a call in the turn's **tool activity** (§3.6), keyed by `call_id`: the activity's header shows its `name` and `text` (the body's one-line summary: the bash command up to 200 characters, `read <path>`, `edit <path> (n edits)`, `ext <name>`) while it runs | newline, then `» <name>: <text>` |
| `tool_end` | the call becomes `✓` or `✗` (`ok`), with the body's summary (`exit 0, 96 lines`, `error: no_match`) and the duration the TUI measured | `  « ok <text>` or `  « error <text>`; when the activity ends, `[tools] ran 3 calls (2 bash, 1 read), 1 failed` (only for 2+ calls) |
| `tool_progress` (proposed, §9 Q4) | the call's newest non-empty output line, dim, under the activity header; it also resets the heartbeat | not printed; it resets the heartbeat |
| `turn_end` | **not shown** in the main screen: its token counts go to the status bar's turn segment (§3.9) and the line is kept in the activity (or as a hidden block) for the transcript view and `/details`; a `; n display deltas dropped` tail stays visible as a `· n display deltas dropped` line | newline; the text with `--verbose`; when the run ends (`state{busy:false}`), `[turn] 3 requests: in 8.0k out 35 · cache 95.0% · $0.004 (session cache 97.1%)` |
| `info`, `error` | `· <text>` dim, or `✗ <text>` red | `[info] <text>`, `[error] <text>` |
| `info` with `ask` (TUI-PICKER) | the agent's question as a picker (§3.7.2); the `text` fallback is not shown | a numbered prompt under `[agent asks]` |
| a `boot_seq` different from the previous display frame's | a divider `── body restarted (boot <n>) ──` before the frame | the same divider as text |

**Markdown subset** (applied to `text` only, line by line, so streaming stays O(new bytes)): fenced code blocks (indented, dim background), inline code, `**bold**`, `#` headings, and `-`/`*`/`1.` lists. Links and URLs print as plain text, and there are no images, tables or HTML. This subset is the fixed vocabulary the agent can rely on (§3.15).

Parallel tool calls: the body sends every `tool_start` of a batch before the batch runs, and the `tool_end`s in completion order (body/loop.md §3.7). The TUI keeps one row per `call_id`, in start order.

### 3.6 Tool calls: collapsed and expanded

**Tool activity (TUI-UX, 2026-10-24).** Agents make many calls per turn, and one line per call (plus a token line per request) flooded the screen. The consecutive calls between two assistant messages are one **activity block** that updates in place in the live region:

```
⠸ bash cd wt-delegate && cargo build --offline 2>…  8 tool calls · 1 running · 1 failed · 1 min 16 s
    running 1 min 6 s · │ Compiling init-root v0.2.0 (/x/root/c… · still working… no output for 15 s
  … +3 more
  ▸ edit   src/delegate.rs (2 edits)                                       ✗ error: no_match · 0.4 s
  ▸ read   read src/delegate.rs                                                   ✓ 96 lines · 0.4 s
  ▸ edit   src/delegate.rs (1 edit)                                                     ✓ ok · 0.4 s
  ▸ bash   rg -n 'fn spawn' src                                            ✓ exit 0, 4 lines · 0.4 s
```

- **Header:** a spinner, the current action (the newest running call's name and text, truncated first; `thinking…` or `waiting for the model…` between calls), and on the right the counters and the activity's elapsed time (`8 calls · 1 failed · 1 min` when narrow).
- **The current call** gets a line under the header: its running time, its newest output line (dim, from `tool_progress`), and `still working… no output for 15 s` (or `no output yet`) once it has been silent for more than 10 s, so a long `cargo build` never looks hung.
- **Finished calls** shrink to one row each; only the last 3 show, older ones fold into `… +N more`. **Failed calls (red) and calls with no result are never folded.** Other running calls of a parallel batch keep their own rows.
- **Thinking** between calls folds into the activity (the header says `thinking…`; Ctrl-T shows its last 3 lines); its text is in the details.
- **Freezing.** When the reply's text arrives (non-blank `text`), the run ends (`state{busy:false}`), the owner sends a line, an error arrives or the body restarts, the block freezes and is printed once: one summary line `▸ ran 8 tools in 1 min 22 s · bash ×4 · read ×2 · edit ×2` with `✓ all ok`, `✗ 1 failed` or `? 1 no result` on the right, then one row per failed or unfinished call. A single call freezes to its own row, as before. Calls still running at the freeze end as `? no result`. **STEER (v0.2.1):** an owner line never freezes an activity while one of its calls is still running (the call keeps running in the body and its result still arrives); a line typed during a run waits in the pending tray (§3.7.1) and enters the transcript only once the body has read it, after the batch.
- **Replayed calls (v0.5.2).** The attach replay brings every frame at once, so its calls have no time here: a replayed activity reads `◇ ran 5 tools before this launch` (not `in 0.0 s`), its rows show no duration, and the details say `time: before this launch (not known here)` instead of the attach instant (user session v0.5.1-new: `ran 5 tools in 0.0 s` and `started 11:19:10Z · 0.0 s` for calls that ran at 10:39 for 3 min 46 s).
- **Details.** Ctrl-O opens the transcript view on the last activity, already expanded: every call with its fields (below), each request's token line, and each thought. v0.5.2 (user sessions v0.5.1: "in full" showed only `call: set -eu…`): `call:` is every line the agent sent (the body sends a shell command's first 3 lines, at most 400 characters, `…` when cut, and the details then add `(the agent's preview: the first lines of the call only)`); `output (last N lines):` is the call's `detail`, which the body now sends for every shell command that ran as well as for a failure (its last 10 lines); a shell call with none says `output: none sent (this agent's version sends output only for a failure)`. The collapsed row keeps to the call's first line. `/details` prints the same expansion of the last frozen activity into the main screen (and so into the terminal's scrollback); while an activity is still running it says to use Ctrl-O. The session log keeps everything for the agent either way; the TUI only folds the view.

Scrollback cannot be redrawn, so expanding happens in the **transcript view** (Ctrl-O, alternate screen), which shows every block of this TUI process:

- `j`/`k` or the arrow keys move the selection; PgUp/PgDn and `g`/`G` scroll; Enter or Space toggles the selected block; `e` expands all and `c` collapses all; `q`, Esc or Ctrl-O returns.
- An expanded tool call shows `name`, `call_id`, the full `tool_start` text wrapped, `ok`, the `tool_end` summary, the start time (UTC) and the duration. Thinking blocks expand to their full text.
- **What "result" means in v0.1.** The body sends only a one-line summary in `tool_end`, never the tool output (body/loop.md §4.1), and the kernel relay drops fields it does not know (§10, D7). Expanding therefore shows the full command and the summary, not the output. A larger result needs a body change plus §9 Q2's pass-through; the TUI already renders an optional `detail` string under the summary if one ever arrives.
- **The failed-call tail (v0.5.0, TUI-OBSERVE).** On a failed call the body's `tool_end` carries `detail`: the last 10 lines of the call's output, each cut to 200 characters, 2,000 bytes in all, without the bash trailer (`seed/src/tools/mod.rs` `fail_detail`); calls that succeed carry none, and the model's result is unchanged. The kernel relays it (§9 Q2(b)). The TUI shows it under the failed call, dim behind a `│` rule: the last 3 lines while the activity runs, all of them once it is frozen and in a single call's row, and in the transcript view as `detail:`. So "the output above says why" is true on screen.
- The transcript view keeps at most 50,000 blocks or 32 MiB of text; beyond that the oldest tenth is dropped from the view (the terminal's scrollback is unaffected).

### 3.7 Input box and keys

The input box is editable at all times, including while a reply streams. It grows to 10 rows (at most 40% of the screen) and then scrolls inside itself. Editing works on grapheme clusters (`unicode-segmentation`) and display widths (`unicode-width`): Backspace removes exactly one grapheme, which Vietnamese input methods that correct text by sending Backspace (Telex, VNI) rely on, and wide CJK and emoji characters move the cursor by two columns.

| Key | Effect |
|---|---|
| Enter | send the box as one `line` frame (multi-line text travels as one frame with `\n` inside) |
| Shift-Enter · Alt-Enter · Ctrl-J · `\` then Enter | insert a newline. Shift-Enter needs the kitty keyboard protocol: at start the TUI asks `crossterm::terminal::supports_keyboard_enhancement()` and, if it is supported, pushes `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES`. Terminal.app does not support it, so there the other three work. |
| paste | bracketed paste (`EnableBracketedPaste`): inserted verbatim, CRLF → LF, never sent by itself |
| **Ctrl-C** | send `interrupt` (the body pauses the current turn and keeps partial output; the owner's next message continues the task unless it changes it, body/loop.md §3.5, §3.5.1; harmless while idle). The draft in the box is kept. The hint shows `interrupt sent · Ctrl-C again within 2 s quits init-agent (the agent keeps running)`. |
| **Ctrl-C again within 2 s** | send `detach`, restore the terminal, exit 0. The agent is not touched. |
| Esc | while busy and no popup is open: send `interrupt`, a pause as for Ctrl-C (it does not arm quitting; hint `pausing · type to continue, or say what to do instead`); otherwise close the popup. **STEER:** with a steer still in the pending tray, Esc sends `{"t":"interrupt","then":"continue"}` (interrupt and steer: the body aborts as for a plain interrupt, then continues the run with the steer instead of going idle; hint `interrupting · your note goes in now (Esc again: just stop)`); a second Esc within 1 s, or Esc with no steer pending, is a plain `interrupt` |
| Tab (STEER) | while busy, with text in the box and no popup: send the box as `/later <text>`, the next task after this run (Codex's Tab); the tray shows it as `next task` |
| Up on an empty box with lines in the tray (STEER) | take back the newest line the body has not read: `{"t":"retract","seq"}` with the kernel echo's `seq`. On success its text returns to the box (or to the history if the box is not empty); if the agent already read it, the hint says so and it shows as seen. A line still unconfirmed or held by the Root cannot be taken back yet (the hint says why) |
| Ctrl-D on an empty box | detach and exit 0. v0.5.1: on the way out the live region (input box, status bar, hint line) is erased, every block not yet printed goes up once, and one line says that the agent keeps running under the Root, how to come back and how to stop it, with the ledger's total (`App::leave`; the same on Ctrl-C twice, SIGTERM and SIGHUP) |
| Up / Down | move between lines; on the first line, Up recalls earlier messages from this process's history |
| Left, Right, Home, End, Ctrl-A, Ctrl-E, Alt-Left, Alt-Right, Backspace, Delete, Ctrl-W, Ctrl-U, Ctrl-K | the usual line-editing moves and deletions |
| `/` at the start of an empty box | a completion popup of the owner commands (§3.8) with one-line descriptions; Tab completes the selected one, the first by default. v0.5.1: the safe, everyday commands come first (`/usage`, `/details`, `/model`, `/new`, …) and the ones that stop the agent last (`/resume`, `/root stop`, `/panic`, `proto::STOPPING`, their descriptions in the warn style), so Tab on a bare `/` is `/usage`, never `/panic`; the popup shows 8 rows, a window that follows the selection |
| `?` on an empty box | the key list overlay (as in Claude Code); `?` anywhere else is text |
| Ctrl-O | the transcript view (§3.6), opened on the last tool activity, expanded |
| Ctrl-T | show or hide thinking text |
| Ctrl-L | redraw |

- Ctrl-Z (suspend) is not supported in v0.1; it is ignored.
- **Size cap.** A `line` frame whose ICJ encoding exceeds 900 KiB is not sent. The box keeps the text and shows `too long to send (n KiB > 900 KiB); split it`. Both the Root and the kernel stop at 1 MiB per line, and the Root closes a connection that sends more (`FRAME_MAX`).
- While the agent is busy, the box's placeholder says `Enter steers · Tab queues the next task · Esc interrupts` (CONVENTIONS §6: a plain line during a run is a steer; v0.2.1 STEER).

#### 3.7.1 The pending tray (STEER, v0.2.1)

Steering is meant to feel like Claude Code and Codex: the owner keeps typing, and each line is picked up safely at the next turn boundary (after the running tool batch, before the next model request; body/loop.md §3.4) and visibly.

```
› fix the test name too                    after bash: cargo test -p ini… · 41 s
› then update the changelog                                            next task
› also run clippy …                                                     sending…
╭──────────────────────────────────────────────────────────────────────────────╮
```

- **Into the tray.** A line sent while the agent is busy (an input, not a Root or kernel command) is not pushed into the transcript: it waits in the tray, directly above the input box, one row per line (newest last, at most 4 rows, older ones fold into `… +N more waiting`; a multi-line message shows its first line and `…`).
- **When it will be read** (right-aligned, dim): `sending…` (no echo yet); `queued while INIT restarts` (a Root echo); `held while INIT updates` (`state.slot` is not `running`); `after <tool>: <call> · <elapsed>` (the oldest call still running: the whole batch's results come back first); `after this reply` (the model is writing or thinking); `starting a run` (the run ended first: a leftover steer starts the next run); `next task` (a follow-up); `taking back…` (a retract is in flight).
- **Seen.** The kernel's `state` frame carries `steer_pending`, `steer_seen` and `steer_retracted` (kernel/supervise.md §4.6), sent after every body ack. When a line's `seq` (from its kernel echo) is in `steer_seen`, or is no longer pending, it leaves the tray and is pushed into the transcript as `› <text> · seen` (a follow-up as a plain owner line), which is exactly where the model sees it. In `steer_retracted` it leaves the tray as taken back.
- **Older kernels** send no steer lists: a tray line then leaves at its kernel echo as delivered, as before v0.2.1, and the rest of the tray is flushed when the run ends.
- While lines wait, the hint line reads `Esc interrupt and send it now · ↑ take back the newest · Tab queue the next task`. The kernel's `[kernel] taken back: …` / `[kernel] not taken back: …` notices become hints, not transcript lines.

#### 3.7.2 The picker (TUI-PICKER, v0.5.0)

The owner chooses from a list with the arrow keys and Enter, never by typing a number back. An open picker takes the input box's place in the live region (`render/picker.rs`, wired in `app/pick.rs`):

```
── agent asks · Database ───────────────────────────────────────────────────────── waiting on you ──
 Which database should the orders service use? It stores about 2 GB and needs concurrent writers.
 › 1. Postgres  recommended
      a server; concurrent writers, the usual choice at this size
   2. SQLite
   3. Type your own answer…
────────────────────────────────────────────────────────────────────────────────────────────────────
 turn in 14.0k out 60 · cache 98.5% · $0.000 · $0.040/$5.00 today                       L0 · root ok
 ↑↓ move · Enter choose · 1–3 pick · or just type your answer · Esc dismiss
```

- **Keys.** ↑/↓ (also Tab, Shift-Tab, Ctrl-N, Ctrl-P) move and wrap; Home, End, PgUp, PgDn jump; Enter chooses; Esc cancels (a typed filter first); 1–9 choose by number while every row has one digit; with `multi`, Space or a number toggles and Enter confirms the checked options (none checked: the focused one). In the TUI's own pickers, typing filters the list (case-insensitive, label and description), and `/` on an empty filter leaves the picker for an owner command. Ctrl-C closes the picker and interrupts as usual; Ctrl-D closes it and quits; Ctrl-O and Ctrl-L work. No mouse capture: the inline layout, the terminal's selection and its scrollback stay.
- **Look.** A hairline rule with the TUI's own label (`setup · provider`, `agent asks · <header>`), the question, at most 8 option rows (the list scrolls inside the widget, with `↑ n more` and `↓ n more`), one description line under the focused option, a closing rule; the key hint goes on the hint line. The recommended option (or the current model) comes first and is marked `recommended` (`current`). Focus is ink weight: the focused row bold ink with `›`, the others dim; tags and numbers mute; rules `Line`. No picker is the Root's own, so the Root's amber never appears in one.
- **Closed.** A setup choice leaves one committed line, `› provider · DeepSeek`. An agent question leaves `· agent asked · <header>: <question>` (with `· dismissed` after Esc), followed by the owner's answer as an ordinary owner line.
- **Where.** First-run setup and `/setup` (provider, model), `/model` (the configured models, the current one first), `/key` without a provider when more than one configured provider takes a key. Free-text fields stay text inputs: the server URL, the model id, the price and the hidden key. The Root's typed confirmations (`root.confirm`: `/full on` under a workspace charter, `/undo`) stay typed: the phrase is the Root's deliberate friction, and a picker would make it one keystroke.
- **Line mode** (and so non-TTY and `TERM=dumb`) keeps the numbered prompts of v0.2.0, so scripts and pipes work unchanged.

**The agent's questions.** The body tool `ask_owner` (body/tools.md §3.8; 2–4 options, single or multi-select) makes the body send one extra display frame and end its turn without another model request: `display{kind:"info", text:<numbered fallback>, ask:{question, header?, multi, options:[{label, description?, recommended?}]}}`. `kind` stays `info`, so the kernel's closed `DisplayKind` accepts it and `ask` passes the relay as an unknown field (§10, Q2(b)); `init-root attach` and older TUIs print the fallback text. No Root or kernel change and no new privileged path exist: the owner's choice is sent as an ordinary `line` frame, the chosen labels as text (`, `-joined for multi), exactly as if typed, and reaches the body as the owner's next message.

- **Anti-spoofing (binding).** The body is agent-writable, so every safety property is here, keyed only on the frame being a `display` frame (which the Root always relays as `src:"kernel"`): whatever the frame carries, including `src:"root"`, `[root]` text or escape sequences, it renders under the TUI's own `agent asks` label in ink, dim, mute and hairlines only, never amber, harm or reverse video. Strings are cleaned (escape sequences dropped whole; control, bidi and zero-width characters dropped; whitespace collapsed) and capped: question 500 characters, header 24, label 80, description 200, at most 4 options (duplicates dropped). A label that starts with `/` is shown and sent quoted (`"/full on confirm"`), so no choice can be read as a Root, kernel or body command. A frame with no usable question or option renders as the plain `info` it also is. Tests: `render::picker::tests::agent_question_cannot_look_like_the_root`, `app::pick::tests::a_body_frame_never_renders_as_the_root`.
- **The owner decides, in their own time.** "Type your own answer", typing any other key, a paste, or Esc closes the picker; the owner's next message is the answer. Nothing has a timer: no default, no auto-pick. After "Type your own answer" or typing, until the owner sends a line, the hint line reads `waiting on you · the agent asked a question (above) · …`. v0.5.1: Esc dismisses the question and the waiting state with it; the hint says once `question dismissed · the agent stays idle until you send a message; your next message is its answer`.
- **Answered stays answered (v0.5.1).** A kernel echo of an owner input (`echo` with `seq`), live or in the attach replay, means the agent's question was answered (here, in another client, or before a relaunch): an open agent picker closes, leaving `· agent asked · <header>: <question> · answered` above the answer, and the waiting state ends. Before v0.5.1 the replay after a relaunch reopened an answered question. A question with no answer in the replay still opens. A newer question replaces an open one (the older stays in the transcript); a question that arrives while a setup picker or the hidden key field is open waits for it. In a `/goal` run the agent stays idle until the answer (the body asks the model nothing more; a restart while the question waits does not resume the run, body `has_pending_work`), and the Root stops the goal's wall clock until the owner's next line or the zone's next egress request (ADR-013, `goal.rs` `asked`/`resumed`).
- **Line mode** prints the question as a numbered prompt under the TUI's own `[agent asks]` label (not the body's fallback text); a reply of option numbers (`2`, or `1,3` for multi) is sent as the labels, any other line as typed.

### 3.8 Owner commands

The TUI sends every line unchanged; the Root and the kernel parse them as for `init-root attach` (exact match after trim). The one exception is `/panic`.

| Line | Handled by | TUI behaviour |
|---|---|---|
| `/panic` (alone) | Root, before anything else (root/root.md §3.9) | sends `{"t":"panic"}` on the attach connection. If that connection is down, it sends the request `panic` on a fresh connection and waits up to 2 s. If the socket is unreachable, it runs `ROOT_HOME/bin/init-root --root-home <ROOT_HOME> panic`, the Root's own fallback that writes the latch (root/root.md §3.9.6, as built). v0.5.1, TTY mode: `/panic` first prints what it does (every agent process and all model spend stop now, the level drops one step when above L0, `/resume` restarts on the audited-good version; `/stop` and `/root stop` are the milder stops) and the hint `type panic and press Enter to stop everything now · Esc or any other line cancels`; v0.5.2 (user session v0.5.1-new: "Panic quietly threw away my new tool"): when the Root's status says the running tuple is not the audited-good one, the text adds `What you lose: this agent runs version <current>, newer than the audited-good <audited>, so after /resume the self-changes kept since <audited> no longer run (they stay in the lineage; your files are not touched)`; the line `panic` or `/panic` (any case, trimmed) sends it. Esc, Ctrl-C or any other line cancels with `/panic cancelled: nothing was stopped` (that line is not sent; it goes back to the box). Nothing from the zone can cancel or pre-empt the confirmation: an agent question or a setup answer that arrives meanwhile waits and opens after the confirmation is sent or cancelled. Line mode keeps the v0.1 behaviour for scripts: it has no completion, so `/panic` is always typed in full. The Root's semantics are unchanged. |
| `/resume` | Root while it is panicked, halted or stopped; otherwise the kernel's own `Stopped` | line |
| `/root status` · `/root ledger` · `/root stop` · `/root help` | Root | line; the answer arrives as a `[root] …` notice |
| `/stop` · `/status` · `/kernel` · `/accept <task>` · `/bless <job>` · `/reject <job>` · `/later <text>` | kernel (kernel/supervise.md §4.6) | line |
| `/new` · `/compact [focus]` · `/branch <id> [note]` · `/label <id> <text>` | body (body/log-context.md §4.5) | line; when the echo of `/new` arrives, a divider `── new session ──` |
| `/clear` (v0.2.0, UX-KEYMODEL) | body, as `/new` | the TUI sends `/new` (TTY and line mode); the kernel never sees `/clear` |
| `/key [provider]` · `/model [n\|provider/model]` · `/setup` (v0.2.0, UX-KEYMODEL) | Root requests `providers.list`, `keys.set`, `providers.add`, `model.use` on their own connection | `/key`: a hidden field (the box shows `•`; never in the history, the transcript, a `line` frame or a log), sent once to the Root, which stores it 0600; the TUI zeroes its buffers. `/model`: the configured models in a picker (§3.7.2; line mode: a numbered list, then a number picks); the Root rewrites `kernel/config.toml` and restarts the kernel. `/setup`: the first-run flow again. First run: when `providers.list` says the selected model has no usable key, `init-agent` asks provider → key → model before the first chat. In line mode on a terminal the key cannot be hidden, so the TUI points to `init-root keys set <provider>` instead |
| `/help` (v0.5.2) | the TUI itself | not sent: a table of the commands below with their descriptions |
| an unknown `/word` (v0.5.2) | the TUI itself | not sent (user session v0.5.1-new: `/undo list` went to the model and was billed). A line whose first word is `/` and a letter, then letters, digits, `-` or `_`, and that no part of INIT reads (`proto::KNOWN_WORDS`: the TUI's, the Root's, the kernel's and the body's command words), gets `/word: unknown command — /help lists them; start the line with // to send it as a message (nothing was sent)` and stays in the box. A path (`/usr/bin/x …`) and a line starting `//` are sent unchanged; line mode prints the same refusal and drops the line |
| `/usage` · `/details` (TUI-UX) | the TUI itself | not sent. `/usage`: the Root's ledger and token totals plus what this TUI saw (requests, cache rate, the last turn). `/details`: the last tool activity in full (§3.6); in line mode, this turn's per-request token lines |
| anything else | body, as an input (a steer while busy) | line |

**v0.1.1 (ADR-009): Root lines for the workspace** (root/root.md §4.2, §3.17):

| Line | Handled by | TUI behaviour |
|---|---|---|
| `/workspace` · `/full` | Root | line; the answer is a `[root]` notice with the access state |
| `/full on` · `/full on confirm` · `/full off` | Root | line. The first `/full on` in a session gets a notice that says what full mode allows and asks for `/full on confirm`; the owner types that line. `/full off` acts at once. |
| `/undo list` · `/undo <id>` · `/undo <id> confirm` (v0.5.2, R-UNDO) | Root | line (never refused as unknown, never chat): the snapshots with id, time and trigger; `/undo <id>` restores one after its confirmation. In the completion popup. Plain `/undo` refused with `root.not_found` reason `unchanged` shows the Root's message (which snapshot, when, and `/undo list`) |
| `/undo` · `/undo confirm` | Root | line. `/undo` gets a notice naming the snapshot it would restore and asks for `/undo confirm`. While the agent is busy the Root refuses it (`root.busy`, `body_busy`) before asking; interrupt or wait, then retry. |

The completion popup lists exactly these. `/approve`, `/deny` and `/clear-safe` were removed in v0.3 (kernel/supervise.md §4.6) and are not listed. A command is recognized only when the whole box is a single line; a multi-line message that starts with `/stop` is ordinary text to the kernel too.

#### 3.8.1 The workspace and `--full` at launch (v0.1.1, ADR-009)

`init-agent`'s cwd is the workspace: the directory the owner runs `init-agent` in is the one the agent may edit.

1. After the first `attach`, before it sends any owner line, the TUI sends `workspace.set{cwd}` once. Reconnects do not resend it.
2. If the answer has `refused`, the Root's notice explains it (`[root] no workspace: cd into a project and run init-agent to let me edit it`), and the TUI shows it like any notice. The agent can still read the home; it just cannot edit outside `INIT_HOME`. v0.5.1: whenever the answer has no workspace the TUI adds its own notice (`access::no_workspace_text`): why (`<cwd> is not one (<the Root's refusal message>)`), that the agent works in its own folder instead (`INIT_HOME/work`, `~/.init/work` by default) so files it creates by name land there, what the session's access means (full: it can still change files by full path, with no undo; confined: only its own folder), that `/undo` has nothing to restore, and the rule (`a workspace is a folder inside your home, not your home itself and not INIT's own folders`). `/undo` refused with `no_workspace` says the same instead of the Root's circular advice.
3. With `--full`, the TUI then sends `full.set{on:true}`. On `root.confirm` it shows the error's `message` in a confirmation prompt in the input box, reads one line, and resends the same request with `"confirm": <that line>`. It takes the phrase from `data.phrase` and never hard-codes it. A second `root.confirm` (a mistyped phrase) leaves full mode off with a notice; nothing is retried automatically. On `root.mode` with `data.reason = "charter"`, it shows `full mode is disabled by your charter (sandbox.allow_full)`.
4. The Root applies a change to the running agent at its next idle boundary (root/root.md §3.17.3). Until then `status.access.restart_pending` is true, and the status bar's workspace field shows it (§3.9).

The TUI still opens no file and holds no secret (T-1, T-2): reading its own cwd is a system call, and the confirmation phrase is not a secret.

### 3.9 Status bar

```
 busy 41 s · deepseek-flash · $0.040 run · $1.23/$5.00 today · body 3f9c0a1b2c4d lkg · queued 1     ~/code/app · WORKSPACE · L0 · root ok
```

**v0.5.0 (TUI-BRAND): the agent on the left, the Root on the right.** The left side is the agent: busy time, model, the turn's tokens, cache and cost, money, the body. The right side is the Root, flush right: the workspace, the access mode, the level, and last the Root's own state, `root ok` in the Root's colour (§3.9a) while the Root answers and nothing is wrong. The Root's alarms replace `root ok` in the harm colour: `PANICKED 3 min ago via <channel> · /resume` (reverse video), `HALTED (<reason>)`, `Root unreachable · retry in 1.6 s`. A slot that is not plainly running adds a word before it (`stopped`, `starting`, `backoff`; `safe mode` in the warn style). `FULL` is ink with a harm dot (`● FULL`), `WORKSPACE` ink; a provisional body's `provisional` is in the rewrite colour (a change in flight), `lkg` muted. Separators and labels are muted, receipts dim.

| Field | Source | Refresh | Notes |
|---|---|---|---|
| level | Root `status.level`, `cap` | 2 s poll, and at once after any Root notice | `L0`; `L0 (cap L1)` when wide |
| model | kernel `state.model` (§9 Q2; absent as built) | each `state` frame | as built: the only routable model from `egress.stats.models` if exactly one is routable, shown dim; otherwise `model ?` |
| run cost | kernel `state.task_musd` (§9 Q2; absent as built) | each `state` frame | as built: `+$0.120 since attach`, the change in the Root's day spend since this TUI attached (it includes gate and probe spend) |
| self-change checks (v0.5.1) | the Root's ledger total across each self-change's flight (§3.10a) | each gate notice the track takes asks `egress.stats` at once | `checks $0.031 est`, only once this TUI has seen a self-change; the Root has no per-purpose split yet (§9 Q5), so this is the ledger's change from a change's `gate_start` to the first ledger answer after the last one ended, any other spend in that time included. Never the kernel's `evolution_checks_today_musd`, which comes from the last `state` frame and is older than a gate that runs after a reply |
| today against the ceiling | Root `egress.stats.money`: `day_spent_musd` of `day_limit_musd` | 5 s poll, and after each `turn_end` | the figure the hard ceiling governs (root/egress.md §3.3); yellow at ≥ 80%; red with `ceiling` once `day_spent_musd + day_reserved_musd ≥ day_limit_musd`, the Root's admission test with nothing more requested (v0.5.0: warn at ≥ 80 %, harm with `ceiling`) |
| Root state | Root `status.slot`, `mode`, `panic`, `halted`; the connection | 2 s poll, and at once after any Root notice | v0.5.0: `root ok` (root colour), preceded by the slot word when not running (`starting`, `backoff`, `stopping`, `stopped` dim; `safe mode` warn); `PANICKED 3 min ago via <channel> · /resume` (harm, reverse video), `HALTED (<reason>)` (harm), `Root unreachable · retry in 1.6 s` (harm) replace it. Before v0.5.0: `running`, … in yellow and red |
| body | kernel `state.sha12` and `mode`; Root `status.tuples` | each `state` frame; 2 s poll | `body <sha12>` plus `lkg` when `tuples.current == tuples.lkg`, `provisional` or `safe` from the kernel's mode. v0.5.2 (user session v0.5.1-new: after `/panic` and `/resume` it kept the dropped body's id while `/status` named the running one): not shown while the slot is panicked, halted or stopped, nor after a panic, resume, halt or crash-loop notice or a new kernel instance in the Root's status, until a fresh `state` frame or the kernel's `/status` line (`… · sha <sha12> · …`, which also sets it) names the body that runs |
| busy | kernel `state.busy` | each `state` frame | `busy 41 s` (elapsed by the TUI's clock); v0.5.0: no spinner here (the liveness row has it, §3.4), and it drops late |
| queued | Root `status.queued` and kernel `state.queued` | as above | only when > 0 |
| turn tokens (TUI-UX) | the body's `turn_end` lines (`tokens: in N (cached C), out M`; `in` counts every prompt token, cached ones included) and Root `egress.stats.money.total_spent_musd` | each `turn_end`; egress after each `turn_end` | `turn in 238k out 1.0k · cache 99.6% · $0.004`, then `session cache 97.1%`. Shown once a token line has been seen. A turn starts when the run starts (`state{busy:true}`) or an owner line is delivered while idle (a steer is the same turn). The **cache rate is token-weighted**: Σ cached / Σ input over the turn's (or the session's) requests, not a mean of per-request rates; one request's `119424/119586` is 99.86%, shown as `99.8%` (rounded down, never overstated). A request whose line has no cache field makes the rate `~` (computed over the requests that reported one); with none it is `cache ?`, never `0%`. The cost is the change in the Root's ledger total since the turn began (an estimate from usage and tariffs; it includes any other spend in that time, e.g. gate checks), shown to a tenth of a cent; `$?` without the ledger. v0.5.1: the turn's cost stops at the first ledger answer after its run ends (`state{busy:false}` asks for one), so a self-change's probes after the reply are not the turn's; when a self-change was in flight during the run the cost reads `$0.040 incl. checks`. v0.5.2: only when a step that makes model requests (G2, G3, G4) was in flight during the run (a G0 build spends nothing). A turn that ran, at least in part, before this TUI attached (it came in the attach replay, when the ledger does not move) reads `$? before this launch`, or `≥$0.002` when part of it ran since, never `$0.000`; `/usage`'s `last turn` says why, and a self-change seen only in the replay is not measured (`/usage`: `a self-change ran before this TUI attached: its cost is in the Root's total, not measured here`). Observe-only: no cap or ceiling is implied. "Session" means every token line this TUI process saw, the attach replay included |
| access (v0.1.1, ADR-009) | Root `status.access` | 2 s poll, and at once after a notice with `code:"access"` | the workspace with the home shown as `~` (`~/code/app`), or `no workspace` (dim) when there is none; after it the access mode: `● FULL` (ink, the dot in harm; v0.5.0, was red bold) while `access.full` is true, else `WORKSPACE` (ink; was yellow); v0.5.1: confined with no workspace, `agent's own folder only` instead of the contradictory `no workspace · WORKSPACE` (v0.3.1, ADR-012; `FULL` in reverse video under `NO_COLOR`); a trailing `↻` while `restart_pending` (the agent picks up the change at its next idle boundary). v0.5.0: on the Root's side, before the level. |

```
 busy 41 s · deepseek-flash · $0.040 run · $1.23/$5.00 today · body 3f9c0a1b2c4d lkg       ~/code/app · ● FULL · L0 · root ok
```

- When the terminal is narrow, fields are dropped in this order (v0.5.0): `/usage · /stop`, the observe-mode token and evolution receipts, the session cache rate, model and queued, run cost, the body, the workspace path and the turn tokens, busy, today (or the live total), and last the level (its short form first). The Root's state and the access mode (`FULL` or `WORKSPACE`) are never dropped.
- **Authority.** The level, money, access and Root state come from Root requests, which no zone process can forge. The body, mode, busy and model fields come from the kernel, which is maintainer code at L0–L2 and agent code from L3; they are informative, not authoritative.
- `NO_COLOR` set: no colours; states are shown with words, and the Root's alarms and `FULL` with reverse video.
- **Money (v0.5.1): one rule on every surface** (`render::usd`): below a dollar to a tenth of a cent (`$0.046`, `$0.000`), from a dollar up to the cent (`$1.23`), rounded half up; above zero but below a tenth of a cent `<$0.001`, never `$0.000`. Before v0.5.1 totals were in cents and turn costs in tenths, so the status bar's `$0.05` and `/usage`'s `$0.04` could stand for nearly the same ledger.
- **Slot words (v0.5.1):** `agent starting`, `agent failed to start, retrying` (warn; the Root's alert above says why), `agent stopping`, `agent stopped · /resume`, beside `root ok`.
- **Times.** std has no local time zone, and the TUI adds no crate for one, so it shows relative times (`3 min ago`, `resets in 5 h 12 min`) and, where an absolute time is needed, UTC with a `Z` (`21:04:05Z`).

### 3.9a The palette: colour means state (v0.5.0, TUI-BRAND)

The INIT brand's rule, "if a colour doesn't mean something, it isn't there", is the renderer's rule. The renderer never names a colour; it names a **role** (`render/palette.rs`), and the one place where styles become escape codes (`term.rs`) asks the palette what that role is on this terminal.

| Role | Meaning | Where |
|---|---|---|
| ink | primary text | the terminal's own foreground, in every tier |
| dim | secondary text | receipts, routine `✓`, finished rows, labels |
| mute | tertiary text | separators `·`, hints, times, the prompt `›`, tags other than the Root's |
| line | hairlines | the input frame, the output gutter `│`, the code-block background |
| root | **the Root, and only the Root**; never an accent, never animated | `root ok` on the status bar, the `[root]` tag (only on a frame the Root stamped `src:"root"`; a kernel's `[root]` look-alike stays muted), the Root's records |
| rewrite | a change in flight; outline glyphs | `◇` on a call or gate still running, `□` in the gate track, `provisional` |
| pass | kept | a gate passed, a change promoted or kept, level up; not every `✓` |
| harm | harm | `✕` failures, rollbacks, demotions, ceilings, HALTED, PANIC, the `●` before `FULL` |
| warn | attention that is not harm (ceiling ≥ 80 %, `? no result`, `queued`, a `warn` notice, an unconfirmed line) | no hue in any tier: ink in bold, the words carry it. Added beside the brand's eight because yellow is the Root's and red is harm. |

**Tiers.** One table per tier and background (`Palette::paint`):

| Role | truecolor dark | truecolor light | xterm-256 dark | xterm-256 light | 16 colours (dark / light) | `NO_COLOR` |
|---|---|---|---|---|---|---|
| ink | default fg | default fg | default | default | default | default |
| dim | `#8A96A3` | `#5A6470` | 246 | 241 | faint | faint |
| mute | `#6F7D8B` | `#75757D` | 244 | 243 | faint | faint |
| line | `#3A3F47` | `#C8CCD2` | 238 | 251 | 8 / 7 | faint |
| root | `#E6B450` | `#A16A0A` | 179 | 130 | 11 / 3 (yellow: the Root's alone) | default |
| rewrite | `#56D4DD` | `#0B8191` | 80 | 30 | 14 / 6 | default |
| pass | `#3FB950` | `#178646` | 71 | 29 | 10 / 2 | default |
| harm | `#F85149` | `#DD353A` | 203 | 160 | 9 / 1 | bold |
| warn | bold | bold | bold | bold | bold | bold |

- Contrast (WCAG, computed): every signal is ≥ 5.7:1 on the brand night `#0D0E11` in the dark variants and ≥ 4.5:1 on white in the light variants (xterm 30 is 4.36:1); dim ≥ 6:1, mute ≥ 4.5:1 on both. The hairline is lifted from the brand's `#24272D` so it shows on common terminal backgrounds, not only on `#0D0E11`; the brand's `n-mute` is lifted to `#6F7D8B` (4.58:1).
- **Tier choice:** `NO_COLOR` (non-empty) → `NO_COLOR`; `COLORTERM=truecolor|24bit` → truecolor; `TERM` containing `256color` or `direct` → xterm-256 (Terminal.app before macOS 26 has no truecolor); otherwise 16 colours.
- **Background:** `COLORFGBG` when set (background 7, 9–15 light; 0–6, 8 dark); else, in TTY mode, an OSC 11 query written right after raw mode is on and before anything reads the input, followed by a DA1 query that every terminal answers: the DA1 reply ends the wait (up to 1 s for a terminal that answers neither), and a reply's relative luminance above 0.179 means light. Hand-written (≈ 40 lines on `rustix` poll and read, already in the closure, §3.14). Otherwise dark. Nothing is read from a file (T-5); the TUI never paints the terminal's background.
- **Glyphs** are the brand's set: `›` owner, `◇` a call or change (rewrite while in flight, muted when done), `✓` / `✕` results, `↳` nesting and repeats, `●` a state dot, `■ □` cells (the gate track, the identity's Cell mark). `▲ ▼ ■ ✻ ▸ ✗` are retired. Bold is reserved for warn and the Root's alarms.
- **Identity.** At the first attach, one line above the Root's greeting: `■■■□ INIT <version> · L<n> · root ok`, the last cell outlined in the rewrite colour and `root ok` the only amber. No wordmark, no banner, no animation.

### 3.10 Notices

As built, notices carry only `level`, `text` and `ts`. The TUI classifies them by their text, for styling only; no TUI behaviour except an immediate status poll depends on a notice's text.

| Class | As-built text (source) | Rendered |
|---|---|---|
| promotion | `[kernel] promoted`, `[kernel] lkg`, `[kernel] provisional` (kernel `notice`) | `✓` pass (v0.5.0; was `▲` green) |
| level up | `[root] level L<a>→L<b>: …` (Root alert `level_up`) | `✓` pass, and the status bar's level flashes |
| rollback | `[root] … falling back to LKG`, `[root] kernel crash loop: rolling back to the LKG tuple` (Root alerts `rollback`, `kernel_crash_loop`); `[kernel] update_failed` | `✕` harm (v0.5.0; was `▼` yellow) |
| demotion | `[root] demoted L<a>→L<b> (…)` (Root alert `demoted`) | `✕` harm, and the level flashes |
| ceiling hit | none as built: the Root writes a `ceiling_hit` lineage record and no notice. The TUI raises its own notice when the polled spend meets the limit (§3.9): `■ daily ceiling reached: $5.00 of $5.00; requests are refused until the day resets (in 5 h 12 min)`, once per owner day, and the same for the total | `✕` harm |
| panic | `PANIC at …` (Root alert `panic`) | a full-width red line; the status bar turns red at once |
| resumed (v0.5.2) | `[root] resumed after panic …` (Root alert `resume`, sent at level `alert`) | `✓` pass: a resume that worked is not a failure (user session v0.5.1-new: it showed `✕`) |
| safe mode, halt | `[root] system safe mode…`, `[root] Halted{…}` | warn; `✕` harm |
| access (v0.1.1) | `[root] workspace: <path>`, `[root] no workspace: cd into a project and run init-agent to let me edit it`, `[root] full mode on: …`, `[root] full mode off …` (Root notice `code:"access"`) | info; `full mode on` in red; triggers an immediate status poll |
| snapshot (v0.1.1) | a snapshot taken, skipped (over 2 GiB, not APFS, another volume) or restored by `/undo` (Root notice `code:"snapshot"`) | info; a skip in yellow |
| gate fail (v0.5.0) | `[kernel] gate G<n> fail …` when the gate track did not take the notice (an older kernel sends no `data`) | `✕` harm |
| kernel rollback (v0.5.0) | `[kernel] rolled back <from> → <to>…` without `data` | as rollback |
| anything else | — | by `level`: info dim, warn in the warn style, alert with `✕` harm |

- **v0.5.0 (TUI-BRAND):** a notice is drawn as its state glyph in its signal colour, then the source tag, then the text in ink (dim for routine information). The `[root]` tag is in the Root's colour only on a frame the Root stamped `src:"root"`; any other tag (`[kernel]`, `[tui]`, a kernel's `[root]` look-alike) is muted. Identical notices in a row (same level, text and source) print once, followed by a live `↳ ×n in all` that is printed when something else follows; line mode prints `  ×n in all` the same way.
- **The Root's JSON answers** (`/root status`, `/root ledger`: `[root] {…}` from the Root) are drawn as aligned key/value tables (v0.5.1: the charter's dollar amounts are `dollar refs  $5.00 a day · $25.00 in all (charter records, not caps)`, since spending is metered, not capped; the word `ceilings` returns only if the Root sends `ceilings.enforced: true`) (`render/table.rs`): the known fields in a fixed order (state, level, tuples, kernel, instances, ceilings, lineage, mirror, last audit, charter, root; level, path, last change, next, incident, record), then every other field the Root sent, so nothing is hidden. `/usage` is a table the same way. Line mode keeps the raw `[root] {json}` line for scripts.

- The Root's greeting at each attach (`[root] attached: …`) is shown once at the first attach; after a reconnect the status bar shows `reconnected` instead.
- **Forgery.** A kernel can send a notice whose text starts with `[root]`, and as built a client cannot tell the two apart. From L3 that matters. §9 Q1 asks the Root to stamp `src` on every frame it relays; with it, the Root classes (level up, demotion, rollback of tuples, panic, ceiling) require `src:"root"`, and kernel-sourced look-alikes render as plain kernel notices.

### 3.10a The gate track (v0.5.0, TUI-OBSERVE)

A self-change is shown as what it is, not as a run of dim `[kernel] gate G2 pass` lines. The kernel's notices about a promote carry `data` (kernel/supervise.md §4.6): `gate_start` (the planned `steps`), one `gate` per stage (`stage`, `pass`, `reason`, `score`, `spent_musd`, `wall_ms`, `cache_permille`), `promoted` (`handoff_ms`), `lkg` and `rolled_back`. `render/gates.rs` folds them, keyed by the candidate's sha, into one block in the live region, above the popup and the tray:

```
◇ self-change 06be87a12345 · t2  G0 ■ G1 ■ G2 □ G3 · live · G4 ·            12 s
    G0  build + tests    ■ pass · 9.1 s
    G1  self-map         ■ pass · 0.4 s
    G2  boot probe       □ 2.9 s
    G3  self-edit probe  ·
    hand-off             ·
    G4  scored vs LKG    ·
```

- **At most 7 rows** (a header and one row per step; T2 and `dep` plan six steps), and at most a third of a small terminal, as a window around the step in flight, so the block never pushes the input box off. The running step's time ticks; times come from the notices' `ts`, so a replay after a reconnect shows true durations.
- **The dependency build (v0.5.2).** On a dependency-cache miss the kernel sends `gate_deps` (`gate_id`, `sha`) as G0 starts building the dependencies, and G0's owner notice adds `build_ms` (export, dependencies and body build before G0's tests), `deps_ms` and `deps_mode` (`hit`, `stub` built now, `cold`). The track shows the build as its own step before G0, `deps dependencies □ 12 min 3 s`, with the row `first build of dependencies (not cached yet): usually 10–20 min on an M2` under it while it runs, then `■ built · 17 min 16 s · not cached before; later changes reuse it`; G0's own row is the body build plus its tests. The rows then add up to the job record's G0 time. Before v0.5.2 a 17 min cold build read as G0 `waited 17 min 46 s`. A warm cache shows no `deps` row; an older kernel sends neither field and G0 shows as in v0.5.1.
- **Waits (v0.5.1).** A finished step's time is its own `wall_ms`; the rest of its span (from the step before, or the gate's start, to its notice) is shown as `· waited 6 min 54 s` from a second up: queued, waiting for a build slot, or kernel work outside the step. So the rows add up to the total. Before v0.5.1, G0's live timer said 8 min 33 s and its row then 1 min 39 s, with 8 minutes of a 17 min 47 s change on no row.
- **A G4 that scored nothing (v0.5.1)** (`reason` `no_instrument`: no tasks to score against; `free`: not run on the free path; or a pass with no score) shows `· not scored · <why>` with a `·` cell, not `■ pass · 0.0 s`, and the collapsed line counts it out: `✓ kept … · 4/4 gates · G4 not scored`.
- **Legend.** The `?` overlay explains the cells (`■` passed, `□` running, `·` to come or not run, `✕` failed) and LKG (last known good).
- **Collapse.** When the change ends, the block becomes one committed line: `✓ kept 06be87a12345 · 5/5 gates · 44 s · hand-off 4.6 s` (`· no restart` for a data-only change), `✕ refused at G2 06be87a12345 · probe_exit · 12 s · still on 3f9c0a1b2c4d` (a stage failed before it went live: nothing changed), or `✕ rolled back at G4 06be87a12345 · score_inferior · ↺ back on LKG 3f9c0a1b2c4d` (it went live, then the kernel rolled it back; until the rollback commits, the block says `↺ rolling back at the next idle point`). In the transcript view the line expands to every step with its time, spend, cache rate and score, and the full shas and gate id.
- **Colour** (§3.9a roles): `Rewrite` only for what is in flight (`◇`, `□`, the ticking time), `Pass` for a passed step and a kept change, `Harm` for a refusal or a rollback, `Mute` for steps to come. These rollbacks are the kernel's, so nothing here is in the Root's colour; the Root's own rollback is a Root notice (§3.10).
- **Trust.** Data from a frame the Root stamped `src:"root"` is never read; a `gate --quick` check (`gate_kind` other than `promote` or `g4`) is not a self-change and stays a plain notice, as do kinds the track does not know. A notice the track takes is not printed as a plain line. Zone text in a reason is sanitized like every frame string (§3.13).
- **Lineage panel.** The transcript view's foot (at most a third of the screen, from 12 rows) shows the Root's `lineage.tail`: v0.5.1 the rule names the records listed (`── lineage (signed by the Root) · newest 7 of 412: seq 405–411`) and the count its range (`verify ✓ 412 records (seq 0–411) · every signature · head 411 #0a1b2c3d4e5f · key 3f9c0a1b2c4d5e6f`; before, "9 records" over a list of seq 1–8 read as an off-by-one), or `verify ✕ at seq 407: 4: hash does not recompute`, then the newest records (`seq`, kind, what it says: `accept g0_g3 06be87a12345` in pass, `reject probe_exit …` in harm, a Root rollback in the Root's colour; signer and short hash). It is asked when the view opens; the TUI reads no file (T-2).

### 3.11 Reconnect, redelivery, no duplicates

**States:** `Connecting` → `Attaching` (replay until `replay_end`) → `Live` → on EOF or error `Reconnecting` (backoff 100 ms, doubling to 2 s, without end in TTY mode).

**Root epoch.** Before each attach the poller asks `status`; the epoch is `root.started_at` plus `root.pid`. If `status` lacks them (as in `init-fakeroot`), the epoch is unknown and treated as "same".

**Display frames, no duplicates.**
- **New epoch (the Root restarted):** the replay ring was in the old Root's memory and is gone, so nothing in the new ring was shown before. The TUI renders the replay after a divider `── Root restarted (21:04:05Z); output from while it was down is in the session log, not replayed ──`.
- **Same epoch (only the connection dropped):** the ring has no frame ids as built (§10, D3), so the TUI matches overlap. It keeps the exact bytes of the last 2,000 frames (at most 256 KiB, the ring's own bounds) received in this epoch. On re-attach it finds the largest index `j` in the replay `R` such that `R[j−m..j]` equals the last `m = min(64, kept, j)` kept frames, and renders `R[j..]` only. If no `j > 0` matches, the ring rotated past everything shown, and the TUI renders all of `R` after `── reconnected; some output may be missing ──`. If more than one `j` matches, it uses the largest and prints the same divider. With §9 Q1's `rseq`, this becomes `attach{since: last_rseq}` and the Root sends only newer frames.

**Owner input, never resent.**
- The TUI never resends a `line` by itself, so it cannot create a duplicate input. Exactly-once delivery into the log already follows from the Root's durable queue, its redelivery after each `ready`, the kernel's `qid` dedup and the body's `input_id` dedup (kernel/supervise.md §3.10 item 3).
- A sent line is shown at once as a dim pending line `› text` (while the agent is busy: in the pending tray instead, §3.7.1). It is confirmed by its echo, matched first-in first-out by text: an echo `E` matches a pending line `P` when `E == P.text`, or when `P.text.trim()` is `/later ` followed by `E` (the kernel echoes a follow-up without the prefix).
- A Root echo (with `qid`) marks the line `· queued` (no kernel was running). The later kernel echo (with `seq`) for the same text marks it delivered. The two become one transcript line, not two (§10, D8).
- An echo that matches no pending line (another client, or the replay) is shown as an ordinary owner line.
- Lines the Root or kernel handle as commands (§3.8) produce no echo and are shown at once as command lines, never pending.
- A line still unconfirmed 30 s after the kernel is ready again and the Root reports `queued: 0` is marked `no confirmation seen (it may have been delivered before the Root restarted); check before resending`. Up recalls it; the owner decides.
- `[root] the owner queue is full (200); your message was not queued` marks the newest pending line `not queued` in red.

**Interrupts are not queued.** As built, the Root delivers `interrupt` only to a ready kernel and drops it otherwise (§10, D6). The hint says `interrupt sent`, plus `(no body is running)` when the last `status` showed no ready kernel.

### 3.12 Line mode

Line mode is chosen when stdin or stdout is not a TTY, when `TERM=dumb`, or with `--plain` or `--raw`. It behaves like `init-root attach` should per the v0.2 rules (the as-built attach differs, §10 D1):
- one owner line per stdin line, sent as a `line` frame; rendering per the line-mode column of §3.5; echoes as `> <text>` (not `» `, which v0.2 uses for tool starts); notices as their text;
- **STEER (v0.2.1):** a kernel echo that arrives while the agent is busy prints `[steer queued] <text>` (`[next task queued] <text>` for a `/later` line) instead of `> <text>`; once the kernel's `state.steer_seen` reports the body's ack it prints `[steer seen] <text>` (`[next task started] <text>`), and `[steer retracted] <text>` when it was taken back. Plain, non-animated lines; an older kernel prints neither of the last two;
- **no animation** (this is also the screen-reader mode): one line per tool start and finish, a `[tools] …` summary line when an activity of 2+ calls ends, a `[turn] …` line with the turn's tokens, cache rate and cost when the run ends, and `  … <name> still working (1 min)` for a call silent for 30 s (then every 30 s); per-request token lines only with `--verbose` or `/details`;
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
rustix = { version = "=1.1.5", default-features = false, features = ["std", "event"] }   # v0.5.0: in the closure already; `event` = a safe poll for the OSC 11 query (§3.9a)

[dev-dependencies]
vt100 = "=0.18.1"         # v0.5.0: the renderer's bytes through a terminal emulator in tests only (TUI-B2)
```

- Golden files are compared by a 30-line helper (`UPDATE_GOLDEN=1` rewrites them), and the fixture server is std only. **v0.5.0:** one dev-dependency, `vt100` (with `vte` and `arrayvec`, new to `vendor/`), used only by the property tests; the binary's closure is still the 25 crates below (`rustix` was already in it).
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
    src/render/{live.rs, status.rs, transcript.rs, wrap.rs, md.rs, sanitize.rs, palette.rs (v0.5.0: roles and tiers), table.rs (v0.5.0: Root answers as tables), vt_tests.rs (v0.5.0: vt100 property tests)}
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
         [--timeout S] [--latency-report] [--full | --workspace] [--theme dark|light] [--version] [--help]
```

From v0.1.1 the TUI's cwd is the agent's workspace (§3.8.1, ADR-009): run `init-agent` in the project you want the agent to edit.

The launcher passes its arguments through unchanged (`init-agent --plain` runs `init-tui --plain`); `--which` belongs to the launcher, so `init-tui` defines no flag of that name. v0.5.2: a word starting with `#` and everything after it are ignored, as a shell comment that the shell passed on (macOS's zsh has `interactive_comments` off, so the README's `init-agent  # first run: …` ran `init-tui '#' first run: …` and failed with `unknown argument "#"`).

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
| `--full` | v0.1.1: ask the Root for full mode at launch (`full.set{on:true}`). v0.3.1 (ADR-012): full access is the charter default, so this matters only under `sandbox.mode = "workspace"`, where the Root asks for a typed confirmation (§3.8.1). The charter can forbid it (`sandbox.allow_full`). Excludes `--workspace`. |
| `--workspace` | v0.3.1 (ADR-012): confine this session to workspace mode (ADR-009: home reads minus secrets, writes only in the workspace and `INIT_HOME`, no network but the provider). Sent as `workspace.set{cwd, mode:"workspace"}`; without a cwd, as `full.set{on:false}`. Excludes `--full`. |
| `--theme dark\|light` | v0.5.0 (TUI-BRAND): the palette's variant for a dark or a light background (§3.9a), instead of the terminal's answer to the OSC 11 query or `COLORFGBG`; for a terminal that answers neither |
| `--version` | `init-tui (INIT Agent TUI) <version>`, plus `built from tui/ at <sha12>` when the build set `INIT_TUI_GIT_SHA` |

Environment read: `INIT_ROOT_HOME`, `HOME`, `TERM`, `NO_COLOR`, and from v0.5.0 `COLORTERM` and `COLORFGBG` (the palette tier and background, §3.9a; in TTY mode the background may also come from the terminal's answer to an OSC 11 query). Nothing else, in particular no `INIT_TEST*` and no `INIT_HOME`. From v0.1.1 the TUI also reads its cwd (`std::env::current_dir()`, not `$PWD`) and sends it as the workspace.

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
| Root not running at start | `connect` fails (ENOENT, ECONNREFUSED) | TTY: the status bar says `Root unreachable` and the screen shows how to start it; v0.5.1 by its path, which is not on the owner's `PATH` (`` `~/.init-root/bin/init-root start` ``, plus `--root-home <dir>` for another Root home; "leave that terminal open", or the launchd job); retries with backoff. Line mode: exit 69, unless `--wait`, with the same hint on stderr. |
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
| Before v0.4.1: the launcher `init` and the system's `init` shadow each other (Linux, where `/usr/bin/init` or `/sbin/init` exists) | `init --which` prints nothing useful, or `init --version` shows systemd | upgrade to v0.4.1 and run `init-agent` (§3.1); its `install.sh` removes INIT's old `init` on Linux |
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
| TUI-UX1 | activity goldens (`tuiux_activity`, `tuiux_failed`): running with the heartbeat (no output 52 s), with a fresh and a stale output line, narrow (40 cols); frozen summary with the failure row; expanded in the transcript view; `/details` with the first request's token line; failures never folded; thinking between calls (plain and verbose); an interrupted call as `no result`; a single call | golden text grids and style masks match | live, unit |
| TUI-UX2 | token math (`model.rs` tests, `tuiux_status`): parsing with and without the cache field and with the dropped tail; token-weighted turn and session rates (not the mean of rates); a missing cache field gives `?` or `~`, never `0%`; the turn boundary (busy, idle delivery, steer); the turn cost from the ledger and `$?` without it; the segment at 140/100/80/40 cols | as stated | live, unit |
| TUI-UX3 | line mode (`tests/proto.rs`): an activity of 3 calls with a failure and a `tool_progress`, three requests (one without a cache field), the run's end | `[tools] ran 3 calls (2 bash, 1 read), 1 failed` before the reply; one `[turn] 3 requests: … cache ~95.0% …` line; no `tokens:` line without `--verbose`, which prints them; no progress text | live |
| TUI-OB1 | gate track goldens (`gates_track`, `observe_gate_region`; `render/gates.rs`, `render/observe_tests.rs`): a T2 change live at 80 and 50 columns and on a 12-row terminal, G4 running after the hand-off, kept, refused at G2, G4 failed then rolled back, expanded; the block in the live region above the input box | golden grids and masks; ≤ 7 rows; one committed line per change; no plain notice for a taken one; a quick check, a promote the track never saw and an unknown kind stay plain; ESC in a reason is inert | live, unit |
| TUI-OB2 | failed-call tail (`observe_failed_tail`): live (3 lines), frozen (10), a single call, the transcript view | as stated; a call that succeeded shows no detail | live, unit |
| TUI-OB3 | lineage panel (`lineage_panel`): asking, verify ✓ with records, verify ✕ at a seq, an older Root (`root.unknown_cmd`), 60 columns | golden; ≤ 10 rows; zone text inert | live, unit |
| TUI-OB4 | protocol (`init-proto` `owner_notice_data_is_optional_and_round_trips`, kernel-supervise `notice_tests`, init-root `stamp_overwrites_src_and_rseq` and `lineage::owner_view` (test-fixtures), seed `failed_call_sends_its_output_tail_as_detail`, TUI `proto::notice_data`) | `data` round-trips and is omitted when absent; an older kernel's notice decodes; each gate outcome has its data and level; the Root relays `data` unchanged; `lineage.tail` rows and verify, ✕ after a record is altered | unit |
| TUI-OB5 | the recording kit (`scripts/demo/record-mock.sh` on an integration bundle): the real Root, kernel and body on `init-mockllm`, a kept T1 change and a change G0 refuses, recorded through `scripts/demo/rec.py` | the cast shows the caption `mock model` on every frame, `◇ self-change`, `✓ kept`, `✕ refused at G`, the failed promote's output and `verify ✓`; the owner's state is unchanged | owner-machine, temp home |
| TUI-B1 | v0.5.0 brand goldens (`brand_liveness`, `brand_root_answers`, `brand_identity`; palette and table unit tests): the liveness row waiting for the first token, thinking, writing, a quiet stream, a tool running, between requests, at 100 and 60 columns and gone when idle; `/root status`, `/root ledger`, `/usage` as tables at 100 and 60 columns with every field and a kernel's look-alike left plain; the identity line at 100/60/30; seven identical notices as one line and `↳ ×7 in all`; the palette per tier and background (yellow only for the Root, warn without a hue), OSC 11 replies, `COLORFGBG`, DA1 | golden grids and masks (`a` root, `w` rewrite, `p` pass, `h` harm, `n` warn, `m` mute, `l` line, `d` dim); every changed mask regenerated deliberately and reviewed | live, unit |
| TUI-B2 | renderer property test against a terminal emulator (`render/vt_tests.rs`, `vt100` dev-dependency): 24 random sessions of 120 events at random sizes from 20×6 to 140×40, the real renderer and writer with the TTY loop's cursor moves | no escape the emulator does not know (synchronized-update brackets aside); the live region on screen equals the renderer's rows, cell by cell, text and colour; amber only on `Role::Root` spans; the scrollback plus the screen hold every committed row once, in order | live, unit |
| TUI-R1 | the real Root in test mode with the mock: a session, `/panic`, `/resume`, a scripted demotion and a level change (test ledgers, root/root.md §3.16) | the status bar shows PANICKED within 1 s of `/panic`; the demotion notice and the new level appear | disposable, or owner-machine for v0.1 |
| TUI-R2 | Root restart: the runner kills its own test Root by held PID with three lines queued, then restarts it | the TUI re-attaches; each line appears exactly once in the session log; no `line` frame is resent | disposable, or owner-machine for v0.1 |
| TUI-R3 | ceiling: a test charter with `day_usd = "0.01"` and mock prices | the status bar turns red; the ceiling notice appears once | disposable, or owner-machine for v0.1 |

| TUI-15 | v0.1.1 against the fixture server: launch in a scratch project dir; with `--full`; with `--full` and a wrong phrase; a reconnect; a server without `workspace.set` | exactly one `workspace.set` with the TUI's cwd, before any `line`; `full.set{on:true}`, then the same request with `confirm` set to the typed line after `root.confirm`; a wrong phrase leaves it off with no further request; no resend after reconnect; `?` and the notice on an old server; status-bar snapshots with `FULL`, a workspace, `no workspace` and `↻` | live, unit |
| TUI-R4 | v0.1.1 end to end: the TUI, the real Root in test mode and the mock, with `init-agent` run in a fixture project directory on APFS under the test's scratch home; the mock's tool call writes a file in the workspace; then `/undo` and `/undo confirm` | the file appears in the workspace; after `/undo confirm` the workspace equals its session snapshot, a `snapshot` notice reports the restore, and a `pre_undo` snapshot holds the file | disposable, or owner-machine for v0.1 |

`ci/local.sh` runs TUI-1…TUI-10, TUI-14 and TUI-15 in the fast tier and TUI-11…TUI-13 in the full tier; TUI-R1…TUI-R3 go with the Root scenarios (disposable, or the owner-machine variant for v0.1).

## 9. Open questions

*Decided 2026-10-20 as recommended (the owner said proceed): Q1 and Q2 are built in WP26 (Root `4ce8dfe`, kernel `18a7ee6`; shapes in §10, "Built"); Q3 belongs to lane REL. The text below is kept as the record.*

**Cross-doc**
1. **→ root/root.md §3.15, §4.2: three small additive fields on the owner channel.** (a) `rseq`, a u64 counted per Root process, on every frame the Root emits, plus `attach{since}`, which replays only frames with a larger `rseq` and ends with `replay_end{epoch, gap}`; (b) `src` (`root` or `kernel`), set by the Root on every frame and overwriting any `src` from the kernel; (c) `code` on alert notices (the code it already writes to `alerts.jsonl`), and a notice for each `ceiling_hit`. **Recommended: yes, in WP26 by the Root lane**, about 30 lines in `link.rs` and `egress.rs`, with no new command, thread or file. It makes §3.11's dedup exact and makes Root notices unforgeable from L3. The TUI works without it.
2. **→ kernel/supervise.md §4.6: the `state` frame and the display relay.** (a) `state` gains `model` (the live scope's model) and `task_musd` (today's spend booked to the current task), and is also sent after each `turn_end`; (b) the display relay keeps fields it does not know, within the 1 MiB line. **Recommended: yes to both**, about 20 lines in `init-proto` and `kernel-supervise`. (a) fills the status bar's model and run cost; (b) is what lets the agent extend what the owner sees through body changes alone (§3.15). The TUI ignores fields it does not know, so nothing breaks either way.

4. **→ body/loop.md §4.1, kernel/supervise.md §4.6 (TUI-UX, open): `display{kind:"tool_progress", call_id, name, text}`.** While a call runs, the body sends its newest output (at most one frame a second per call, the text capped at one line of 200 characters). **Recommended:** yes, in the body's bash tool; it goes through the kernel's relay unchanged (Q2(b)). The TUI already renders it (the dim line under the activity header, §3.6) and falls back to the heartbeat without it, so nothing breaks either way.

5. **→ root/root.md §4.2, root/egress.md §4.2, kernel/supervise.md §4.6 (v0.5.1 TUI-FIX, open): what the TUI needs to stop guessing.**
   (a) **Spend by purpose** in `egress.stats.money`: `by_purpose{<x-init-purpose or "unlabelled">: {day_spent_musd, total_spent_musd, requests, input_tokens, output_tokens}}`, and the gate job id when the kernel names it (`by_gate{<gate_id>: spent_musd}`). The Root already reads `x-init-purpose` (`egress/lane.rs`); the gate's G2 probe and G3 requests must carry it (`probe`, and a `gate` value for G3), and requests without it count as `unlabelled`, never as `turn`. With it, `/usage`'s `self-changes` row and the status bar's `checks` field show the Root's own figure instead of the ledger's change while a change was in flight.
   (b) **The kernel's `state` frame after each gate notice** (or `evolution_checks_today_musd` on the gate notice's `data`), so the kernel's figure is not older than the gate it describes; as built it is sent only when a run starts or ends and after a turn.
   (c) **`/root status` `ceilings.enforced`** (false while money is observe-only), so the TUI labels the charter's dollar amounts from the Root's word rather than from the guide's.
   (d) **The no-workspace state from the Root**: `AccessView.work_dir` (the body's working directory when there is no workspace, `INIT_HOME/work`) so the TUI names the real path; the Root's refusal text (`[root] no workspace: cd into a project and run init-agent to let me edit it`, `access.rs` `no_ws_text`, `session_text`, and the `/workspace` answer `workspace none`) should state the rule and the reason (`refused.message`) instead of advice the owner has just followed; `full.set`/`workspace.set` with `mode:"workspace"` and no workspace should say the session can write only `INIT_HOME`.
   (e) **Commands printed to the owner that are not on `PATH`** (`init-root resume to restart` in `panic.rs`, `init-root start or /resume` in `link.rs`, `upgrade.rs`, `egress.rs`, `cli.rs`): the full path, as `install::start_hint` does. A body that cannot start (`queued: the body is backoff`) should carry the spawn failure's reason and next step in plain words (`status.kernel.last_failure{code, message, next}`); the TUI shows `agent failed to start, retrying` and the Root's alert as they are.

**Internal**
3. **→ lane REL (`scripts/release.sh`, `scripts/install.sh`, `scripts/init-agent`): building and installing `init-tui`.** **Recommended:** `release.sh` builds `tui/` (`cargo build --release --offline` inside `tui/`, so its `.cargo/config.toml` applies) and puts `init-tui` in the release directory; `install.sh` installs it next to the launcher in `INIT_BIN_DIR` (a new file, then a rename into place); the launcher ignores an `$INIT_TUI` inside `INIT_HOME` (T-4). `init-root install` does not change; `ROOT_HOME/bin/init-tui` stays a lookup location only.

## 10. Spec deltas

| Source | This doc | Why |
|---|---|---|
| spec v0.3 §13, §18; root/root.md §3.15: owner I/O through `init-root attach` | a second owner client, `init-tui` (opened by `init-agent`), on the same socket and frames, with no new trust | owner decision 2026-10-20 (CONVENTIONS §6 OW-TUI) |
| kernel/supervise.md v0.2 §4.6 rendering rules, kept by root/root.md §3.15 | line mode keeps them, except echoes print as `> ` (v0.2's `»` marks tool starts); TTY mode renders collapsible blocks | a richer owner view; scripts keep the old format |
| (new) | the owner's client sanitizes all zone text | zone text is untrusted from L0, and the kernel's from L3 |
| (v0.1.1) | `init-agent`'s cwd is the agent's workspace (`workspace.set`); `--full`, `/full`, `/undo` with the Root's typed confirmations; `FULL` and the workspace in the status bar | owner decision OW-PERM, ADR-009 (lane WIRE-009 builds it) |
| (v0.5.0, TUI-BRAND) | the INIT brand's grammar of colour as semantic roles in four tiers with dark and light variants (§3.9a); a liveness row (§3.4); the status bar split into the agent and the Root, ending in `root ok`; the Root's JSON answers and `/usage` as tables; the brand's glyphs; a one-line identity; `COLORTERM`, `COLORFGBG` and an OSC 11 query read; a `vt100` dev-dependency | the TUI showed none of the brand's meaning (cyan and green as decoration, no amber, yellow warnings that read as the Root, light-theme contrast near 1:1) and nothing while waiting for the model (research_notes `tui_audit.md`) |

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
