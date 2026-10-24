# Tools: read, write, edit, bash, ext
Status: v0.3 + build sync 2026-10-20 · Area: body
Depends on: CONVENTIONS.md, body/provider.md, body/loop.md, body/log-context.md, ext/host.md, kernel/sandbox.md, kernel/supervise.md   Defines: the five tool schemas and their final description text (the single source of truth), tool behaviors and limits, the edit matching algorithm and diff output, bash process-group spawning, output capture, timeouts, process-tree kill, the bash child environment, spill files, the ext dispatch contract (body side), and the error format returned to the model

---

## 1. Purpose

The seed body gives the model exactly five tools.
- `read`, `write` and `edit` run in-process and need no subprocess (spec §9 rank 3).
- `bash` is the universal tool: anything the other four cannot do, a command can.
- `ext` routes to Wasm extensions. Its schema never changes, so growing a new capability never invalidates the prompt cache (spec §4).

This doc fixes the schemas and the description text the model sees, every limit, and every message the tools return. The tool description is the only place the tools are explained to the model. Nothing about them is repeated in the system prompt.

## 2. Responsibilities / non-goals

**Responsibilities**
- The registry: names, schemas, descriptions, order (§4.1).
- Argument parsing and validation, with errors that teach (§4.6).
- Behaviors and limits for each tool, plus the spill files (§3, §4.5).
- The edit matcher: exact first, then normalized fallbacks, uniqueness, overlap, the diff (§3.4).
- bash: process-group spawn, merged capture, timeouts, tree kill without `unsafe`, env (§3.5).
- The `ext` tool contract on the body side (§3.6).

**Non-goals**
- Path or command restrictions: the sandbox enforces them (kernel/sandbox.md). There are no permission prompts (spec §2.1).
- Parallelism and ordering of calls: loop.md §3.7.
- Wasmtime, component loading, WIT-to-JSON-Schema conversion: ext/host.md.
- Per-model tool profiles. The seed ships one tool set for every model. If the agent grows a model-specific variant (spec §6.6: "the per-model tool profile is subject to the same loop"), it lives in `src/profiles/<family>.rs`, the location kernel/selfmap.md inventories as a `profile/<family>` component (selfmap Q-P1, settled in R1).
- A persistent shell or PTY, `grep`/`find`/`ls` tools, an image resizer: growth patterns, not seed (spec §4). Search goes through bash, as in Claude Code, where `Glob` and `Grep` are now absent by default and search runs through the shell.

## 3. Design

### 3.1 The common pipeline

```rust
pub struct Tools { specs: Vec<ToolSpec>, bash: PathBuf, ext: Option<Arc<dyn ExtHostApi>>, env: BTreeMap<String, String>, limits: Limits }  // ext None in safe mode

pub enum Prepared {
    Ready { idx: usize, call_id: String, name: String, job: Job },
    Done  { idx: usize, call_id: String, name: String, result: ToolResult },   // validation failed: the error result is already built
}
pub enum Job { Read(ReadArgs), Write(WriteArgs), Edit(EditArgs), Bash(BashArgs), Ext(ExtArgs) }

pub struct PathCx<'a> {
    pub cwd: &'a Path,                     // body working directory (boot.md §3.2), never changed at runtime
    pub home: Option<&'a Path>,            // $HOME, for a leading "~/" in paths
    pub init_home: Option<&'a Path>,       // $INIT_HOME, for a leading "$INIT_HOME/"
}
pub struct ToolCx<'a> {
    pub paths: PathCx<'a>,
    pub spill_dir: &'a Path,               // $INIT_HOME/state/spill/<INIT_SESSION>/
    pub call_id: &'a str,
    pub abort: &'a Abort,
    pub images: bool,                      // does the model take image input?
    pub model_id: &'a str,                 // for the no-image note
    pub mode: Mode,                        // Safe ⇒ ext disabled
    pub reg: Option<&'a Snapshot>,         // ext/host.md snapshot for this batch, type-erased (§3.6); None in safe mode
}
pub struct Ran { pub result: ToolResult, pub summary: String }   // summary = the one-line tool_end text (loop.md §4.1)

impl Tools {
    pub fn specs(&self) -> &[ToolSpec];                         // [bash, edit, ext, read, write] (by name): head + --manifest
    pub fn prepare(&self, idx: usize, c: &ToolCall, cx: &PathCx) -> Prepared;
    pub fn path_key(&self, p: &Prepared) -> Option<PathBuf>;     // write/edit target, for loop.md unit grouping
    pub fn run(&self, p: &Prepared, cx: &ToolCx) -> Ran;         // never panics on purpose; loop wraps catch_unwind
}
```

**Argument parsing** is typed, field by field, from the `serde_json::Map` the call carries.
- Missing required fields, wrong JSON types and **unknown fields** are `invalid_input` errors. Unknown fields catch habits from other harnesses (`file_path`, `old_string`). The error names the expected field (`hint: did you mean "old_text"?`) instead of silently accepting an alias, so the description stays the single source of truth.
- Integers accept JSON numbers with no fractional part (`120.0` is read as 120). `timeout` is such an integer: a fractional value is `invalid_input`, because every number the harness writes to the log is an integer (ICJ, CONVENTIONS §5).

**Path resolution** (`resolve(p, cx)`, where `cx` is a `PathCx`), with no filesystem access:
1. `""` is `invalid_input`.
2. A leading `~/` (or a bare `~`) is replaced with `$HOME`, and a leading `$INIT_HOME/` with the value of `INIT_HOME`. Models often write both (the `ext` description names `$INIT_HOME/state/ext/catalog.json`), and expanding them is unambiguous. No other variable is expanded.
3. Relative paths are joined to `cx.cwd`.
4. Lexical normalization: drop `.`, resolve `..` against the joined path, collapse repeated `/`.

The result is used both for I/O and as the `path_key` for grouping. The body never restricts paths: a write outside the writable roots fails at the OS, and the tool reports `permission_denied` (§4.6).

**Output finalization** applies to every tool result:
- text over 51,200 bytes (50 KiB) is cut to head + tail with a spill file (§3.5.2 algorithm, §4.4 location);
- `read` and `edit` are bounded by their own limits. `read` skips this cap, because its trailer can pass 51,200 bytes by a few bytes;
- `bash` uses its own lower thresholds (30 KB);
- `ext` relies on this cap.

### 3.2 `read`

Behavior, in order:
1. `metadata(path)`.
   - Not found: `not_found`. The hint says whether the parent directory exists, and lists up to 5 entries of the parent whose names share the first 3 characters or are within edit distance 2 of the name (Levenshtein; `read_dir`, capped at 1,000 entries scanned), nearest first. The distance rule is what finds `src/loop.rs` for `src/lop.rs`, which share only two leading characters.
   - A directory: `is_directory`, with `hint: use bash: ls -la <path>`.
2. Read the first 8,192 bytes and sniff the type:
   - **Image** by magic number:
     - PNG `89 50 4E 47 0D 0A 1A 0A`;
     - JPEG `FF D8 FF`;
     - GIF `47 49 46 38 (37|39) 61`;
     - WebP `52 49 46 46 ?? ?? ?? ?? 57 45 42 50`.

     These are the four formats all three providers accept.
     - If `!cx.images`, return a non-error text: `[image file: image/png, 120,345 bytes; the current model (deepseek-v4-pro) has no image input]`.
     - If the file is over 3,750,000 bytes (base64 stays ≤ 5,000,000 bytes), return the `too_large` error with `hint: downscale with bash, e.g. sips -Z 1568 <in> --out <out> (macOS) or magick <in> -resize 1568x1568 <out>`.
     - Otherwise the result is `[Text("[image: image/png, 120,345 bytes]"), Image{media_type, base64}]`. Base64 is a std-only encoder (RFC 4648 alphabet, with padding).
   - **Binary**: a NUL byte in the sniffed bytes, and not an image. Return `binary_file` with `hint: inspect with bash: file <path>; xxd <path> | head -50`.
3. **Text.** Stream the file with `BufReader::with_capacity(64 KiB)` and `read_until(b'\n')`:
   - Strip one trailing `\n`, and one `\r` before it, for display.
   - Decode each line with `String::from_utf8_lossy`. If any line was lossy, add one note at the end: `[file is not valid UTF-8; invalid bytes shown as U+FFFD]`.
   - Emit lines `offset ..= offset+limit-1`, with `offset` defaulting to 1 and `limit` defaulting to 2,000 (a larger `limit` is clamped to 2,000).
   - Each line is formatted as `format!("{:>6}\t{}\n", n, line)` (the `cat -n` layout models know).
   - A line over 2,000 bytes is cut at the last char boundary ≤ 2,000 and suffixed `… [line truncated: <total> bytes; see it with bash: sed -n '<n>p' <path> | cut -c 2001-4000]`.
   - Stop early when the next formatted line would push the output past 51,200 bytes. The first line is always emitted (it is already ≤ 2,000 bytes).
   - After the last emitted line, keep counting `\n` to get the total line count `T`, scanning at most 256 MiB. Past that, T is reported as `more than <counted>`.
   - If lines remain, append `\n[Showing lines S-E of T. Use offset=E+1 to continue.]` (Pi's hint). If the file is empty, return `[empty file]`. If `offset > T`, return `invalid_input` with `offset 500 is past the end of the file (212 lines)`.

There is no read-before-edit rule; Claude Code relaxed it for newer models, and Pi never had it. There is no file cache: reads are cheap, and a stale cache could silently mislead.

### 3.3 `write`

1. `content` is at most 4 MiB. The call's arguments live inside the assistant `message` event, and log-context.md caps one log line at 8 MiB (`MAX_LINE_BYTES`). A model reply cannot get near 4 MiB anyway: 384K output tokens is about 1.5 MB. A larger value is `too_large`, with `hint: write the file in parts (write the first part, then append with bash: cat >> or edit)`.
2. If `path` exists and is a directory: `is_directory`.
3. `create_dir_all(parent)`.
4. If `symlink_metadata(path)` is a symlink, write *through* it to `canonicalize(path)`. If the link dangles, use `parent.join(read_link(path))`. The link itself is kept.
5. **Atomic replace:**

```rust
let tmp = dir.join(format!(".{}.init-tmp.{}.{}", name, std::process::id(), SEQ.fetch_add(1, Relaxed)));
let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
f.write_all(content.as_bytes())?;
drop(f);
if let Ok(old) = std::fs::metadata(&target) { std::fs::set_permissions(&tmp, old.permissions())?; }  // keep mode bits
std::fs::rename(&tmp, &target)?;      // atomic within one filesystem; on any error: let _ = remove_file(&tmp)
```

- There is no `fsync`. `rename` makes the replace atomic for readers and for a process crash. A power loss could lose the newest content, but git (self-edits) and the event log (the content is in the call args) are the durable records. On macOS, `File::sync_all` uses `F_FULLFSYNC`, which costs milliseconds on every write, on the hottest tool path (spec §9 rank 3).
- Result: `Wrote 1,234 bytes (56 lines) to src/main.rs (new file).` or `… (replaced 1,100 bytes).`

### 3.4 `edit`

**Input:** `{path, edits: [{old_text, new_text}, …]}`, 1 to 64 edits. All edits are matched against the *original* file; they are not applied one after another (Pi's contract).

**Matching algorithm:**

```
read bytes (≤ 16 MiB, else too_large); must be UTF-8 (else binary_file: "edit needs a UTF-8 text file")
bom   := strip one leading U+FEFF
eol   := "\r\n" if the first '\n' in the file is preceded by '\r', else "\n"
text  := replace every "\r\n" with "\n"          (lone '\r' is left alone)
for each edit i:
    o := old_text with "\r\n" → "\n";  n := new_text with "\r\n" → "\n"
    if o == "" → invalid_input "edits[i].old_text is empty"
    m_i := find(text, o)                      // tiers below; first tier with ≥1 match decides
    if m_i has 0 matches in every tier → no_match(i) with hint
    if m_i has ≥2 matches in its tier   → ambiguous_match(i, count, first 5 line numbers)
sort the matches by start; if any next.start < prev.end → overlapping_edits(i, j)
out := splice text with each replacement (tier 3 re-indents n, see below)
if out == text → no_change
restore: if eol == "\r\n" { out := out with "\n" → "\r\n" };  prepend bom if it had one
atomic write (same procedure as write, §3.3)
```

**Tiers.** All three count every occurrence, overlapping ones included, so `aa` in `aaa` is ambiguous.
- **Tier 1, exact.** Substring search on `text`. Advance by one char after each hit to count overlaps. The range is `[hit, hit + o.len())`.
- **Tier 2, line-aligned, trailing whitespace and Unicode punctuation ignored.**
  - Split `text` into lines with byte offsets.
  - Split `o` into lines, first removing one trailing `\n` and recording `o_ends_nl`.
  - Compare windows of `k = o_lines.len()` consecutive file lines with `norm2(file_line) == norm2(o_line)`, where `norm2(s) = map_punct(s).trim_end()`.
  - Range = start of the first window line to the end of the last window line, plus its `\n` if `o_ends_nl` and that line has one.
- **Tier 3, line-aligned, also ignoring a uniform indentation difference.** Compare with `norm3(s) = map_punct(s).trim()`. A window also needs a **consistent indent delta** over all its non-blank line pairs `(f, p)`, where `lead(x)` is the leading run of spaces and tabs:
  - `lead(f) == D + lead(p)` for one fixed string `D`: the model left out a common indent. The replacement re-indents: `D` is prepended to every non-blank line of `n`.
  - `lead(p) == E + lead(f)` for one fixed `E`: the model added an extra indent. `E` is removed from the start of every non-blank line of `n` that starts with it.
  - Any other combination does not match.
  - This is Aider's "missing leading whitespace" repair, made symmetric. A replacement without the re-indent would break Python and YAML.
- `map_punct` maps chars, exactly as Codex `seek_sequence::normalise` and Pi `normalizeForFuzzyMatch` do:
  - U+2010–U+2015 and U+2212 → `-`;
  - U+2018–U+201B → `'`;
  - U+201C–U+201F → `"`;
  - U+00A0, U+2002–U+200A, U+202F, U+205F, U+3000 → space.
- Pi also applies NFKC normalization. Rust std has no Unicode normalization, and a crate for it would break the dependency rule, so the body leaves it out; this is a documented difference.

**Why not more tiers.** Each fallback can only make a replacement that the exact text alone would have been unique for, or fail; it never guesses between candidates. Aider reports that turning off flexible patch application raised editing errors 9× (production_harness_internals.md §2). Pi and Codex use the same tiers. Fuzzy (Levenshtein) matching and LLM repair (Gemini CLI's `llm-edit-fixer`) are left out: the first can silently edit the wrong place, and the second spends a model call. Either can be grown later through the gate.

**Hints:**
- `no_match`: if the first non-blank line of `o`, trimmed, equals some trimmed file line, the hint is `the first line of old_text appears (ignoring indentation) at line 212, 340; the lines after it differ. Re-read that region and copy the text exactly.` Otherwise: `no line of the file matches the first line of old_text; the file may have changed since you read it. Re-read it.`
- `ambiguous_match`: `found 3 matches (lines 12, 48, 97); add surrounding lines to old_text so it matches one place.`
- `overlapping_edits`: `edits[1] (lines 40-44) and edits[3] (lines 43-50) overlap; merge them into one edit.`

**Diff output.** The diff is built straight from the matched ranges, so no diff algorithm is needed:
1. For each match, in start order, take the original lines `a..=b` that the range touches.
2. Build the new block as (line `a` up to the match start) + replacement + (line `b` after the match end), split into lines.
3. Merge hunks that are closer than 2 × 2 context lines. Emit a unified diff with 2 context lines:

```
Edited src/loop.rs: 2 replacements; edits[1] matched ignoring indentation (re-indented by 4 spaces).
--- a/src/loop.rs
+++ b/src/loop.rs
@@ -40,4 +40,5 @@
         let calls = msg.tool_calls();
-        if calls.is_empty() { return; }
+        if calls.is_empty() {
+            return RunEnd::Idle;
+        }
         self.execute_batch(&calls);
```

- New-file line numbers shift by the line delta of the earlier hunks.
- The diff is capped at 120 lines after the header, then `[diff truncated: 340 more lines; read the file to check the result]`.
- The path in the header is as the model wrote it.
- The diff confirms what actually happened, which matters most when tier 2 or 3 matched. It also gives the model the new line numbers for its next `read`.

### 3.5 `bash`

#### 3.5.1 Spawn

```rust
use std::os::unix::process::CommandExt;         // process_group: stable since 1.64.0
use std::process::{Command, Stdio};

let (reader, writer) = std::io::pipe()?;        // stable since 1.87.0; one pipe for stdout+stderr
let writer2 = writer.try_clone()?;
let mut cmd = Command::new(&self.bash);         // resolved once at boot (boot.md §3.2)
cmd.arg("-c").arg(&args.command)
   .current_dir(cx.paths.cwd)
   .env_remove("BASH_ENV")                      // non-interactive bash would source it; keep runs deterministic
   .envs(&self.env)                             // §4.3
   .env("INIT_CALL_ID", cx.call_id)
   .stdin(Stdio::null())
   .stdout(writer).stderr(writer2)              // PipeWriter: Into<Stdio>
   .process_group(0);                           // setpgid(0,0) in the child: pgid = child pid
let spawned = cmd.spawn();
drop(cmd);                                      // REQUIRED: Command owns the parent's copies of the writer; until it is
                                                // dropped, the reader never sees EOF (verified locally)
let mut child = spawned.map_err(|e| spawn_failed(e))?;
let pg = child.id() as i32;
if !cx.abort.register_group(pg) { kill_group(pg, Signal::Kill); }    // abort raced the spawn
```

- One merged pipe keeps stdout and stderr interleaved in the order the command wrote them. That was verified: `echo out1; echo err1 >&2; echo out2` read back as `out1\nerr1\nout2\n`. It needs only one reader thread.
- stdin is `/dev/null`. Interactive programs see EOF and exit or fail fast, instead of hanging until the timeout. A PTY is the first growth step (spec §4).
- A child spawned by `Command` gets the default `SIGPIPE` disposition even though Rust ignores SIGPIPE in its own process. Checked: `yes | head -c 10` terminated normally inside the bash tool.
- Shell: the first `bash` found on `PATH` at boot, else `/bin/bash` (boot.md §3.2). On the owner's Mac that is `/opt/homebrew/bin/bash` (GNU bash 5.3.20) **only if the body's sandbox can read `/opt/homebrew`**: PENDING OWNER (CONVENTIONS §6 ESC-2), recommended yes and assumed here as the provisional default. If the owner says no, the lookup cannot stat that entry, skips it, and the tool runs `/bin/bash` 3.2.57; nothing else changes. The path and version go into the `boot` event, so the agent knows which one it has (awareness.md). bash runs with `-c` only: no login shell, no rc files.

#### 3.5.2 Output capture (reader thread)

```rust
struct Capture { mem: Vec<u8>, tail: Ring<26_000>, total: u64, newlines: u64,
                 spill: Option<(PathBuf, File)>, spilled: u64, spill_full: bool, read_err: Option<io::Error> }
```

The reader thread loops on `reader.read(&mut [0u8; 65_536])` until `Ok(0)`. An `Err` other than `Interrupted` is noted and ends the loop. For each chunk:
1. `total += n`; count `\n` into `newlines`.
2. While `total ≤ 30,000`, append to `mem`.
3. On the first byte past 30,000:
   - create `spill_dir` (`create_dir_all`);
   - open `<spill_dir>/<call_id_sanitized>.txt` with `create_new`, adding `-2`, `-3` … on collision;
   - write `mem` to it.
4. After that, append each chunk to the spill file until it holds 64 MiB (then `spill_full`; keep draining the pipe and drop the bytes, because a writer must never block on a full pipe), and keep the last 26,000 bytes in `tail`.

At EOF, build the model-facing text:
- If `total ≤ 30,000`: `text = mem`.
- Otherwise:
  - `head` = `mem[..4,000]`, cut back to its last `\n` if one exists;
  - `tail` = the ring, cut forward past its first `\n` if one exists within its first 1,000 bytes;
  - `text = head + "\n[... {omitted_lines} lines ({omitted_bytes} bytes) omitted; full output ({total} bytes) in {spill_path} ...]\n" + tail`.
- Clean the text sent to the model (the spill file stays raw):
  - remove ANSI CSI sequences (`ESC [` … final byte `0x40–0x7E`) and OSC sequences (`ESC ]` … `BEL` or `ESC \`);
  - drop one `\r` before each `\n`, so CRLF becomes LF in the model text (the spill file keeps CRLF);
  - inside each line, keep only the text after the last lone `\r`. This collapses progress bars that redraw with carriage returns;
  - decode with `from_utf8_lossy`.
- If `spill_full`: add `[spill file capped at 64 MiB]`. If the session spill total is over 1 GiB, no spill file is created and the marker says `[output omitted; session spill limit reached]`.

Head + tail, not tail only: compiler errors print their first, most specific error at the top, and the summary is at the end. mini-swe-agent (first and last 5,000 characters) and Claude Code (head-and-tail excerpt on failure) do the same. Where Pi keeps only the tail, this is a spec delta (§10).

#### 3.5.3 Waiting, timeout, drain

```rust
std::thread::scope(|s| {
    let (ctx, crx) = std::sync::mpsc::channel();
    s.spawn(move || { let _ = ctx.send(capture(reader, spill)); });        // reader thread
    let (wtx, wrx) = std::sync::mpsc::channel();
    s.spawn(move || { let _ = wtx.send(child.wait()); });                  // waiter thread (owns `child`)
    let t0 = Instant::now();
    let exit = match wrx.recv_timeout(timeout) {                           // Receiver::recv_timeout: since 1.12.0
        Ok(st) => Exit::Normal(st),
        Err(RecvTimeoutError::Timeout) => {
            kill_group(pg, Signal::Term);
            match wrx.recv_timeout(Duration::from_secs(2)) {               // TERM → KILL grace
                Ok(st) => Exit::TimedOut(st, Signal::Term),
                Err(_) => { kill_group(pg, Signal::Kill); Exit::TimedOut(wrx.recv().unwrap(), Signal::Kill) }
            }
        }
        Err(RecvTimeoutError::Disconnected) => unreachable!(),             // waiter always sends
    };
    let wall = t0.elapsed();
    // Leader gone. EOF arrives once every holder of the pipe's write end has exited.
    let out = match crx.recv_timeout(Duration::from_secs(2)) {             // drain grace
        Ok(c) => c,
        Err(_) => { kill_group(pg, Signal::Kill); bg_killed = true; crx.recv().unwrap() }  // a background job still holds the pipe
    };
    (exit, wall, out)
});
cx.abort.unregister_group(pg);
```

- **Abort** needs no code here. `Abort::trigger`, on the input thread, SIGKILLs every registered group. The leader exits, `wrx` fires, the pipe reaches EOF, and the result is marked aborted because `cx.abort.is_set()`.
- **Background jobs.**
  - A job that redirects its output (`server > log 2>&1 &`) does not hold the pipe. The call returns as soon as the leader exits, and the job keeps running.
  - A job that does not redirect keeps EOF from arriving. After the 2 s drain grace its process group is killed, with the note `[background processes still writing to this command's output were killed; redirect their output (cmd > file 2>&1 &) to keep them running]`. Verified locally: the reader stayed blocked until the group kill and then returned at once.
- **Group-id reuse after the leader is reaped.** Killing after `wait()` targets a group whose leader is gone. POSIX does not reuse a process id while a process group with that id still has members, so the kill reaches the right group while any member lives. If none is left, `kill` fails with exit status 1 (verified for a nonexistent group). The remaining window is narrow: every member exits *and* an unrelated new process gets the same pid *and* makes itself a group leader, all within the 2 s grace. It is accepted, and bounded by the sandbox: kernel/sandbox.md limits `signal` to processes in the same sandbox.

#### 3.5.4 Killing the process tree without `unsafe`

The body has `#![forbid(unsafe_code)]`, and std has no `killpg`. There are two options:

| Option | Cost | Dependencies | Risk |
|---|---|---|---|
| **A. Spawn `kill -s <SIG> -- -<pgid>`** | measured 1.44 ms p50, 3.2 ms max (20 runs, M2, 2026-10-19) per call; only on the cold path (timeout, abort, drain expiry) | none | needs a `kill` binary or bash |
| B. A safe dependency (`rustix::process::kill_process_group`, or `nix`) | ~µs | a new direct dependency. `rustix` is already vendored as a wasmtime dependency, but its `process` feature and a direct dependency would change `Cargo.toml` and force full G4 (spec §6.3) | API drift the agent must track; breaks the CONVENTIONS §5 dependency list |

**Decision: A.**
- The kill path runs only on timeout, abort or drain expiry. 1.4 ms is noise against a timeout of seconds.
- It keeps the global dependency decision (serde, serde_json, wasmtime, wasmtime-wasi).
- It is the exact command an owner would type, so it is easy to audit and reproduce.
- B remains a growth option the agent can take through the gate if it ever measures a reason to.

```rust
pub enum Signal { Term, Kill }
/// Returns true if the signal was delivered to at least one process in the group.
pub fn kill_group(pgid: i32, sig: Signal) -> bool {
    debug_assert!(pgid > 1);                                     // never 0, 1 or negative: kill(-1) would target everything
    let name = match sig { Signal::Term => "TERM", Signal::Kill => "KILL" };
    let target = format!("-{pgid}");
    for bin in ["/bin/kill", "/usr/bin/kill"] {
        if let Ok(st) = Command::new(bin).args(["-s", name, "--", &target])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status() {
            return st.success();                                 // 1 = no such group
        }
    }
    // Fallback: bash's builtin kill (bash is required for the bash tool anyway)
    Command::new(bash_path()).args(["-c", &format!("kill -s {name} -- {target}")])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}
```

Verified on macOS 14.6 (2026-10-19), group of `bash -c 'sleep 300 & sleep 300 & …; wait'`, 3 members:
- `/bin/kill -s TERM -- -PGID`: leader signal 15, no members left.
- `/bin/kill -s KILL -- -PGID`: signal 9, none left.
- `/bin/kill -KILL -- -PGID`: same.
- `bash -c 'kill -s KILL -- -PGID'`: same.

`/usr/bin/kill` does not exist on macOS, which is why `/bin/kill` is tried first. On Linux, procps-ng and util-linux `kill` accept `-s SIG -- -PGID` per their man pages; this is UNVERIFIED on the target distro (Phase 0 check T-L1). Boot records which kill path works (boot.md §3.2).

**Why not `Child::kill()`:** it signals only the leader (one pid). `bash -c 'make test'` would leave `make`'s children running.

#### 3.5.5 The bash child environment

The body's whole environment is inherited. kernel/supervise.md §4.1 builds it from scratch with `env_clear()`, and it already contains:
- the `INIT_*` variables;
- `PATH`, `HOME=$INIT_HOME/work/home`, `TMPDIR`, `CARGO_HOME`, `CARGO_TARGET_DIR=$INIT_HOME/work/target`, `CARGO_NET_OFFLINE=true`;
- the git identity, `GIT_CONFIG_NOSYSTEM=1`, `GIT_TERMINAL_PROMPT=0`;
- `LANG`, `TERM=dumb`, `NO_COLOR=1`.

The body adds the following per call. "If unset" means the kernel's value wins, which keeps tests that run the body without the kernel equivalent.

| Var | Value | Why |
|---|---|---|
| `INIT_CALL_ID` | the tool call id | lets scripts and `initctl` tag work with the call |
| `INIT_SPILL_DIR` | `$INIT_HOME/state/spill/<INIT_SESSION>` | where long outputs go |
| `INIT_PROVIDER`, `INIT_MODEL` | e.g. `deepseek`, `deepseek-flash` | self-knowledge for scripts (Pi exports `PI_PROVIDER` and `PI_MODEL`) |
| `PAGER`, `MANPAGER`, `GIT_PAGER` | `cat` | no pager hang (mini-swe-agent sets `PAGER=cat`) |
| `GIT_EDITOR` | `true` | `git commit` without `-m` aborts ("empty message") instead of waiting for an editor until the timeout |
| `CLICOLOR` | `0` | BSD tools (macOS `ls`) |
| `CARGO_TERM_COLOR` | `never` | cargo ignores `TERM` for color |
| `TERM`, `NO_COLOR`, `GIT_TERMINAL_PROMPT` | `dumb`, `1`, `0`, **if unset** | same values as the kernel's |
| `CARGO_TARGET_DIR` | `$INIT_HOME/work/target`, **if unset** | the shared build cache (spec §7.4) |

Removed: `BASH_ENV`. Nothing secret is in the body environment, because keys live only in the proxy (invariant 3), so nothing else needs scrubbing.

#### 3.5.6 Result

- **Text:** the cleaned output (§3.5.2), then one trailer line:
  - `[exit_code=0 wall_time=1.42s]`
  - `[exit_code=none signal=9 wall_time=0.31s]`
  - `[timeout after 120s: process group killed (TERM) wall_time=120.31s]`, when TERM was enough
  - `[timeout after 120s: process group killed (TERM, then KILL after 2s) wall_time=122.01s]`, when the group needed KILL
  - `[aborted after 4.12s: process group killed]`
- `is_error` is true for a non-zero exit, a signal, a timeout, an abort or a spawn failure (Pi: a non-zero exit is an error result).
- `ToolMeta` (provider.md §3.2), logged in `tool_result` (log-context X8), not sent to the model: `spill_path` when a spill file was written; `exit_code`; `signal`; `wall_ms`.

`timeout` must be an integer with `1 ≤ t ≤ 3600`. Otherwise it is `invalid_input` with `timeout must be an integer from 1 to 3600 (seconds)`. Pi rejects out-of-range timeouts the same way. The default is 120 s, Claude Code's default. Factory reports that short default timeouts did better on average (production_harness_internals.md §2).

#### 3.5.7 Shared helpers (also used by ext/host.md's `init:ext/process` and boot.md's git sync)

host.md §3.13 runs extension-spawned commands (`process.run`) with the same process handling, and boot.md §3.6 runs its git commands with it. The bash internals are therefore exported as two functions:

```rust
pub fn kill_group(pgid: i32, sig: Signal) -> bool;                         // §3.5.4

pub struct RunSpec<'a> {
    pub stdin: Option<&'a [u8]>,           // None: /dev/null. Some: piped; a writer thread writes it, then closes the pipe
    pub timeout: Duration,                 // computed by the caller: bash `timeout`; host min(timeout-ms, deadline − now)
    pub cancel: Option<&'a AtomicBool>,    // a plain flag: the host's per-call `cancel` (host.md §3.10), or None
    pub on_spawn: Option<&'a dyn Fn(i32) -> bool>,   // bash: |pg| cx.abort.register_group(pg); false ⇒ kill at once
    pub capture: CaptureSpec,
}
pub enum CaptureSpec {
    Merged { spill_dir: Option<PathBuf>,   // one pipe for both streams, §3.5.2 (30,000 inline, head 4,000 + tail 26,000);
             name: String },               //   None = no spill file (boot's git sync), the marker then says "not saved"; name = the spill file's
                                           //   name before sanitizing (bash: the call id; boot's git sync: "git")
    Separate { max_each: usize },          // two pipes; keep the LAST max_each bytes of each (host: 1,048,576)
}
pub enum Captured {
    Merged { text: String, total: u64, spill_path: Option<PathBuf>, spill_full: bool },
    Separate { stdout: Vec<u8>, stderr: Vec<u8>, stdout_total: u64, stderr_total: u64 },  // truncated ⇔ total > kept
}
pub struct GroupOutcome { pub pgid: i32, pub status: Option<ExitStatus>, pub timed_out: bool, pub cancelled: bool,
                          pub bg_killed: bool, pub output: Captured, pub wall: Duration }

/// The caller sets program, args, cwd and env. run_group sets stdin/stdout/stderr and process_group(0) itself,
/// overriding anything the caller set. Err only when spawn fails (the caller maps it: bash → spawn_failed,
/// host → not-found / io).
pub fn run_group(cmd: Command, spec: RunSpec) -> std::io::Result<GroupOutcome>;
```

Behavior, common to both capture modes (§3.5.1–§3.5.3):
- **Spawn** with `process_group(0)`, then drop the `Command` so the parent's pipe ends close. `on_spawn(pgid)` returning false kills the group at once (abort raced the spawn).
- **Wait** with the waiter thread blocked in `wait()` and the caller in `recv_timeout`; there is no `try_wait` polling. With `cancel: None` it is one `recv_timeout(timeout)`. With `cancel: Some(flag)` the wait is cut into slices of at most **10 ms**, and the flag is read between slices, so a cancel is seen within 10 ms; the cost is about 100 wake-ups per second while a host command runs, and none when it does not.
- **Timeout or cancel:** `kill_group(TERM)`, wait up to 2 s, then `kill_group(KILL)`, then reap. `timed_out` or `cancelled` is set; a cancel that lands after the leader exited only skips the drain grace.
- **Drain:** after the leader exits, wait up to 2 s for EOF on every pipe; if a background job still holds one, kill the group and set `bg_killed`.
- **stdin:** the writer thread ignores `BrokenPipe` (the child exited without reading). Rust ignores `SIGPIPE` in its own process, so a write to a closed pipe returns an error and never kills the body.

`Separate` differs from `Merged` only in capture: two pipes (`std::io::pipe()` twice, or `Stdio::piped()`), two reader threads, each keeping a ring of the last `max_each` bytes and a byte count. No ANSI stripping, no spill file: the bytes go back to the extension unchanged, and the extension's result text is capped by the host (1 MiB) and by §3.1. Threads per call: `Merged` 2 (reader, waiter), `Separate` 3, plus 1 when `stdin` is given.

How each caller uses it:

| Caller | `capture` | `cancel` | `on_spawn` | timeout |
|---|---|---|---|---|
| `bash` tool | `Merged { spill_dir: Some(cx.spill_dir), name: call_id }` | None: `Abort::trigger` SIGKILLs every registered group at once (loop.md §3.5) | `register_group`; `unregister_group(outcome.pgid)` after | the call's `timeout` |
| host `process.run` | `Separate { max_each: 1_048_576 }` | the call's `cancel` flag (host.md §3.10, §3.13) | None | `min(timeout-ms, deadline − now)` |
| boot git sync | `Merged { spill_dir: None, name: "git" }` | None | None | 30 s per command |

#### 3.5.8 The Root's memory cap and disk floor (MEM-RUNAWAY, 2026-10-22)

The Root polices every tool process: root/root.md §3.14.1 kills a call's process tree whose memory footprint passes the charter's `sandbox.mem_cap_mib`, and every workload while free disk is below `sandbox.disk_floor_gib`. It writes each kill, before the signal, to a file the body may read and the agent may not write. The Root sets the body's `INIT_MEM_EVENTS` (the file's absolute path) and `INIT_MEM_CAP_MIB` at spawn. The body only reads that file (`tools/memcap.rs`); enforcement is the Root's alone, so an evolved body cannot lift it.
- **After a call dies by a signal** that the body did not send (not a timeout, not an abort), the body looks for a record whose `pgid` is the call's process group and whose `wall_ms` is no earlier than 2 s before the spawn. The trailer is then `[process exceeded memory cap <N> MiB, killed (its process tree reached <F> MiB) wall_time=<s>]`, or `[free disk (<F> MiB) fell below the workload floor (<L> MiB), process killed wall_time=<s>]`. The result is an error, and its `tool_end` summary is `memory cap` or `disk floor`. `init:ext/process` `run` appends the same sentence to stderr.
- **Before a bash call**, if the newest disk record is a `disk_low` at most 15 s old, nothing is spawned. The result is `error: disk_floor: free disk (<F> MiB) is below the workload floor (<L> MiB): no new processes until disk space is freed`, with a hint to tell the owner.
- Without `INIT_MEM_EVENTS`, as in tests and hand runs, nothing changes.

### 3.6 `ext`

The body side is thin. ext/host.md owns everything below the call, through its §4.1 API:

```rust
// ext/host.md §3.10, §4.1, as the trait the body reaches the host through (src/tools/ext.rs; the host implements it)
pub struct CallCtx<'a> { pub call_id: &'a str, pub workspace: &'a Path, pub abort: &'a AtomicBool }
pub type Snapshot = Arc<dyn Any + Send + Sync>;   // host.md's Arc<Registry>, type-erased: no Wasmtime types reach the loop or tools
pub trait ExtHostApi: Send + Sync {
    fn snapshot(&self) -> Snapshot;            // taken once per tool batch by the loop (loop.md §3.7)
    fn call(&self, reg: &Snapshot, name: &str, input: &serde_json::Value, cx: &CallCtx) -> ExtOutcome;
    // plus begin_reload, commit_reload, reload_pending, reset_trials and shutdown (loop.md §4.2)
}
pub struct ExtOutcome { pub text: String, pub is_error: bool, pub kind: OutcomeKind,
                        pub version: Option<String>, pub elapsed_ms: u64, pub truncated: bool }
```

Dispatch:
1. **Validate.** `input` must be a JSON object of at most 1 MiB once serialized (ext/wit.md §3.7 item 2); otherwise `invalid_input`. `name` must be non-empty.
2. **Call.** `ExtHostApi::call(reg, name, input, &CallCtx { call_id, workspace: cx.paths.cwd, abort: cx.abort.flag() })`.
   - **The task workspace** (`CallCtx.workspace`, mounted as `/work` in the component, ext/wit.md D2) is the body's working directory, `ToolCx.paths.cwd`. That is the directory every `bash` call starts in (§3.5.1). `cd` does not persist between calls, so the seed has no other notion of a per-task directory. A G4 run gets its task directory as the body's cwd from the kernel (kernel/gate.md, supervise `LaunchSpec.cwd`).
   - The host enforces the per-extension deadline: `timeout_ms` from the component's `describe`, default 30 s, range 100 ms–600 s (host.md §4.1). It also enforces the 1 MiB output cap and the 2 s watchdog.
   - The outcome maps to a tool result. On success: `text`, `is_error: false`. On error: `error: <code>: <host text>`, where the code comes from `kind`: `InvalidInput` → `invalid_input`, `Unknown` → `ext_unknown`, `Timeout` → `timeout`, `Aborted` → `aborted`, `Failed` → `ext_failed`, `Busy` → `ext_busy`, `Host` → `ext_host`, and the other kinds (`Oom`, `StackOverflow`, `Panic`, `Trap`, `Exit`) → `ext_trap`.
   - For an unknown name, the host's text already lists the loaded extensions with their descriptions, plus the failed and not-built ones (host.md `ExtOutcome::unknown`). That is the discovery path the description advertises.
   - The common 51,200-byte inline cap and spill (§3.1) apply to `text`.
3. **Safe mode.** There is no `ExtHost` (boot.md §3.14), and every call returns `ext_disabled`.
4. **Abort.** `CallCtx.abort` is the loop's abort flag (`Abort::flag()`, loop.md §3.1). host.md §3.10 waits in 10 ms slices and watches it: on an owner abort it sets the call's `cancel` flag, which traps the guest at the next epoch tick and stops `process.run`/`read` (§3.5.7). The call returns `Aborted` within `ABORT_GRACE` (200 ms), or is abandoned like a timeout if the guest is stuck in a host call. Calls of the batch not yet started are refused by the loop (`aborted: not executed`).

**Discovery without schema churn.** The description never lists extensions: listing them would change the `ext` schema bytes whenever an extension is added, and that would break the cached prefix (spec §4: "This tool's schema never changes"). Instead the static description points at `$INIT_HOME/state/ext/catalog.json`, which host.md rewrites at every reload commit (§4.2 there) with each extension's description and input schema. An unknown name also returns the list. `self.json` has the verified inventory. This replaces the first draft's reserved `_list`/`_describe` names (R1, ext/host.md and ext/wit.md requests), so no extension name is reserved.

### 3.7 `delegate` (DELEGATE, 2026-10-24)

Based on the `delegate` tool INIT wrote for itself (`evo/delegate` 4258c2e). `src/tools/delegate.rs` parses and bounds the call; the loop runs it (`src/loop/delegate.rs`), because it owns the model calls.

- **Fresh context.** A sub-agent gets the same head (persona, rules, tool schemas, so the same head hash and a cache hit) and a message list that starts at an opening text naming its role, budget and mode. The parent's log gains one `tool_result`: the sub-agent's final text plus a cost line, at most 8 KiB in all. A nested `delegate` returns `not_available` (depth 1).
- **Fan-out.** A batch's delegate calls run before its other calls, on worker threads, at most `INIT_DELEGATE_PARALLEL` (default 4, ceiling 8) at once; results keep call order. Writers (`mode:"write"`) form one unit, so at most one runs at a time. Each sub-task has a child `Abort`: an owner abort reaches every sub-task's stream and bash groups.
- **Mode.** `read` (default) refuses `write` and `edit` inside the sub-task. Its `bash` and `ext` keep the body's rights: Seatbelt cannot nest (kernel/sandbox.md §3.11), so the body cannot run them under a read-only profile. The tool description says read mode is advisory for them. Hard read-only scouts need a Root-spawned member (supervise.md §3.14).
- **Bounds** (non-dollar; money is observe-only): `INIT_DELEGATE_MAX_TURNS` (8), `_MAX_CALLS` (24), `_MAX_REQUESTS` (12, retries count), `_MAX_TOKENS` (1,000,000 input + output), `_MAX_SECONDS` (600). A call may lower turns, calls and seconds. The turn before the last allowed one gets a `[init:delegate] … last turn` note, and calls on the last turn do not run. The wall-time cap is hard: a watchdog triggers the sub-task's child abort.
- **Record.** `state/spill/<session>/delegate-<call_id>.json` holds the task, mode, bounds, end reason (`done`, `capped: …`, `aborted`, `error: …`), counts, tokens, wall time, the returned summary and the full transcript, on every exit path.
- **Display.** Each inner call sends `tool_start`/`tool_end` with `parent_call_id` and `call_id = <delegate id>/<inner id>`; the kernel relays the extra field unchanged (supervise.md §4.6) and the TUI nests the calls, collapsed, under the delegate row.
- **Attribution.** Sub-requests carry the parent's `x-init-task` and `x-init-subtask: delegate-<call id>` (kernel/proxy.md §3.5a), so every proxy trace record has `subtask` set. The kernel forwards a well-formed label to the Root as `x-egress-subtask` (kernel/proxy.md §3.6; the Root strips it before the provider). The sentinel's `Turn` counts these requests as `sub_requests`/`sub_cost_musd`, and H10 (requests/turn) and H5b (cost/turn) test the main-loop share (`requests − sub_requests`, `cost − sub_cost`), so adopting `delegate` is not read as a per-turn regression (ADR-011 §3.1; v0.3.0, MERGE).

## 4. Interfaces

### 4.1 The registry (exact; serialized with sorted keys; tools sorted byte-wise by name)

These six objects are the canonical `ToolSpec`s, in the order every request lists them: sorted byte-wise by `name` (`bash`, `delegate`, `edit`, `ext`, `read`, `write`), as log-context.md §3.8.2 fixes for the head. `init-body --manifest` prints them in the same order (boot.md §4.3), provider.md renders them per API, and every byte here reaches the cached prefix. Changing any of them is a `Prefix-change: yes` commit (spec §6.3 G2).

```json
[
 {"description":"Run a command with bash -c in the working directory. Each call is a new process: cd and export do not persist, and stdin is empty. stdout and stderr are merged; if the output exceeds 30 KB, its head and tail are returned and the full output is saved to a file whose path is shown. On timeout or abort the command's whole process group is killed; a background job must redirect its output (cmd > file 2>&1 &) or it is killed when the command returns.",
  "input_schema":{"additionalProperties":false,"properties":{"command":{"description":"The command to run.","type":"string"},"timeout":{"description":"Timeout in whole seconds (default 120, max 3600).","maximum":3600,"minimum":1,"type":"integer"}},"required":["command"],"type":"object"},
  "name":"bash"},
 {"description":"Run a bounded sub-task in a fresh context and return only its final summary. Use it for self-contained work that would flood this context with tool output: reading through a tree, a long search, a review, a comparison of several files. The sub-agent does not see this conversation, so give it a self-contained task and say exactly what to return. It has the same tools except delegate and a budget (by default 8 turns, 24 tool calls, 600 s). Several delegate calls in one reply run in parallel (up to 4) and return in call order. mode \"read\" (default) refuses write and edit, but its bash and ext are not sandboxed read-only, so for them read mode is advisory; mode \"write\" may change files, one writer at a time. Its full record is written under state/spill/.",
  "input_schema":{"additionalProperties":false,"properties":{"context":{"description":"Optional facts it cannot discover from the workspace (paths, commands, constraints), one paragraph.","type":"string"},"max_calls":{"description":"Lower this sub-task's tool-call cap.","minimum":1,"type":"integer"},"max_seconds":{"description":"Lower this sub-task's wall-time cap, in seconds.","minimum":1,"type":"integer"},"max_turns":{"description":"Lower this sub-task's turn cap.","minimum":1,"type":"integer"},"mode":{"description":"\"read\" (default) or \"write\".","enum":["read","write"],"type":"string"},"task":{"description":"The sub-task, self-contained: what to find out or do, and what to return. It cannot ask you questions.","type":"string"}},"required":["task"],"type":"object"},
  "name":"delegate"},
 {"description":"Edit a file by exact text replacement. Each edits[i].old_text must occur exactly once in the file and must not overlap another edit; all edits are matched against the file as it was before this call. If there is no exact match, a match that ignores trailing whitespace, Unicode punctuation variants and a uniform indentation difference is tried. Returns a diff.",
  "input_schema":{"additionalProperties":false,"properties":{"edits":{"description":"Replacements, all matched against the original file.","items":{"additionalProperties":false,"properties":{"new_text":{"description":"Replacement text.","type":"string"},"old_text":{"description":"Text to replace; must match exactly one place.","type":"string"}},"required":["old_text","new_text"],"type":"object"},"minItems":1,"type":"array"},"path":{"description":"File path, absolute or relative to the working directory.","type":"string"}},"required":["path","edits"],"type":"object"},
  "name":"edit"},
 {"description":"Call a WebAssembly extension (a component built from body/ext). name is the extension name and input is its JSON input object (at most 1 MiB). The loaded extensions and their input schemas are listed in $INIT_HOME/state/ext/catalog.json; an unknown name also returns the list.",
  "input_schema":{"additionalProperties":false,"properties":{"input":{"description":"JSON input for the extension.","type":"object"},"name":{"description":"Extension name.","type":"string"}},"required":["name","input"],"type":"object"},
  "name":"ext"},
 {"description":"Read a file. Text is returned with line numbers (cat -n style): at most 2000 lines or 50 KB per call, and lines over 2000 bytes are cut. Use offset and limit to page through longer files. PNG, JPEG, GIF and WebP files are returned as images.",
  "input_schema":{"additionalProperties":false,"properties":{"limit":{"description":"Maximum number of lines to return (default and maximum 2000).","minimum":1,"type":"integer"},"offset":{"description":"Line number to start from, 1-based (default 1).","minimum":1,"type":"integer"},"path":{"description":"File path, absolute or relative to the working directory.","type":"string"}},"required":["path"],"type":"object"},
  "name":"read"},
 {"description":"Create or overwrite a file with the given content. Missing parent directories are created. The file is replaced atomically.",
  "input_schema":{"additionalProperties":false,"properties":{"content":{"description":"The complete new file content.","type":"string"},"path":{"description":"File path, absolute or relative to the working directory.","type":"string"}},"required":["path","content"],"type":"object"},
  "name":"write"}
]
```

Size: 4,986 bytes when minified with sorted keys (measured from this block; 3,405 before `delegate`, DELEGATE 2026-10-24), roughly 850–1,000 tokens depending on the tokenizer. Pi reports "below 1,000 tokens" for prompt plus tools together; INIT's tools alone are at that level, and the genesis prompt adds about 300 tokens (spec App. A). Every limit that changes how the model should act (the 2,000 lines, the 30 KB, the process-group kill, the background-job rule) is in the text, and nothing else is.

### 4.2 Result formats (text content)

| Tool | Success | Notes |
|---|---|---|
| read | `     1\tline` … `\n[Showing lines S-E of T. Use offset=E+1 to continue.]` | image: `[image: image/png, N bytes]` + image block |
| write | `Wrote N bytes (L lines) to <path> (new file).` / `(replaced M bytes).` | |
| edit | `Edited <path>: K replacement(s)[; edits[i] matched ignoring …].` + unified diff | diff ≤ 120 lines |
| bash | cleaned output + trailer `[exit_code=… wall_time=…s]` | `is_error` for a non-zero exit |
| ext | the extension's text | errors: `error: <code>: ext <name> <version>: …` (host.md §4.4) |

Numbers in messages use `,` thousands separators for bytes, and plain digits for line numbers and codes.

### 4.3 Environment exported to bash

See §3.5.5. Variables read by the tools themselves: `HOME` (to expand `~`). The process cwd is set by the kernel (boot.md §3.2).

### 4.4 Spill files

- Location: `$INIT_HOME/state/spill/<INIT_SESSION>/<call_id_sanitized>[-N].txt`, where the sanitized id keeps `[A-Za-z0-9_-]`, maps every other byte to `_`, and is at most 64 bytes.
- Content: raw bytes as the command wrote them (ANSI codes not stripped), capped at 64 MiB.
- Total per session: 1 GiB, tracked in memory in a process-global counter (one body serves one session) and initialized at boot by summing the directory (boot calls `tools::bash::init_spill_total`).
- Lifetime: kept while the session's log references them; log-context.md's masking keeps the `spill_path` pointer. Cleanup is boot.md §3.6: at boot, the spill directories of *other* sessions whose newest file is older than 14 days are deleted by exact path.
- `state/spill/<session_id>/` is in the runtime layout (CONVENTIONS §3, kernel/supervise.md §3.1), agent read-write.

### 4.5 Limits (defaults)

| Limit | Value |
|---|---|
| read: lines per call | 2,000 |
| read: output bytes per call | 51,200 |
| read: bytes per line before cutting | 2,000 |
| read: line-count scan | 256 MiB |
| read: image raw bytes | 3,750,000 |
| write: content | 4 MiB (log line cap 8 MiB, log-context §3.15) |
| edit: file size | 16 MiB |
| edit: edits per call | 64 |
| edit: diff lines | 120 (2 lines of context) |
| bash: inline output | 30,000 bytes |
| bash: head / tail when cut | 4,000 / 26,000 bytes |
| bash: spill per call | 64 MiB |
| bash: default / max timeout | 120 s / 3,600 s, integer seconds (`INIT_TEST_TOOL_TIMEOUT_S` overrides the default only under `INIT_TEST=1`; the description bytes never change) |
| bash: TERM → KILL grace | 2 s |
| bash: drain grace after the leader exits | 2 s |
| any tool: inline result text | 51,200 bytes (head 4,000 + tail 47,200, the rest spilled) |
| spill per session | 1 GiB |
| ext: default deadline | 30 s (host.md §4.1; per extension `timeout_ms`, 100 ms–600 s) |
| ext: owner abort | ≤ 200 ms (`ABORT_GRACE`, host.md §3.10) |
| shared helper, `Separate` capture | last 1,048,576 bytes of stdout and of stderr |

### 4.6 Error format returned to the model

Every error result has `is_error: true`, and its text starts with exactly one line, `error: <code>: <message>`, optionally followed by lines that start with `hint: `. One fixed prefix lets the model, the log and tests recognize errors on every API, including DeepSeek, which has no wire field for `is_error` (provider.md §3.8.1).

| Code | Raised by | Example message |
|---|---|---|
| `invalid_input` | all | `edits[0].old_text: expected a string` · `unknown field "file_path"` + `hint: did you mean "path"?` |
| `invalid_json` | loop | `the arguments for "edit" are not valid JSON (EOF while parsing a string at line 1 column 4096)` |
| `unknown_tool` | loop | `no tool named "grep". Available tools: bash, edit, ext, read, write.` |
| `not_found` | read, edit | `src/lop.rs does not exist` + `hint: parent exists; similar: src/loop.rs` |
| `is_directory` | read, write, edit | `work/demo is a directory` + `hint: use bash: ls -la work/demo` |
| `permission_denied` | read, write, edit | `kernel/config.toml: permission denied` + `hint: writable roots are body/, state/ and work/ (kernel/ is read-only, vault/ is hidden)` |
| `binary_file` | read, edit | `bin/init-body is binary (contains NUL bytes)` |
| `too_large` | read, write, edit | `file is 40,212,991 bytes; edit handles at most 16 MiB` · `content is 5,242,880 bytes; write takes at most 4 MiB` |
| `no_match` | edit | `edits[1].old_text was not found in src/loop.rs` + hint |
| `ambiguous_match` | edit | `edits[0].old_text matches 3 places in src/loop.rs` + hint |
| `overlapping_edits` | edit | `edits[1] and edits[3] overlap` + hint |
| `no_change` | edit | `the edits leave src/loop.rs unchanged` |
| `spawn_failed` | bash | `could not start /opt/homebrew/bin/bash: No such file or directory (os error 2)` |
| `exit_status` | bash | (no `error:` line: the output plus the trailer already say it; `is_error` is true) |
| `timeout` | bash, ext | in the trailer for bash; `extension "pty" exceeded 30 s` for ext |
| `aborted` | all | `not executed (owner abort)` / in the trailer for bash / host kind `Aborted` for ext |
| `ext_unknown` | ext | host text: loaded names with descriptions, failed and not-built ones (host.md `ExtOutcome::unknown`) + `hint: schemas are in $INIT_HOME/state/ext/catalog.json` |
| `ext_trap` | ext | `ext pty 0.2.0: wasm trap: out of bounds memory access` (host kinds Trap, Panic, Exit, StackOverflow, Oom) |
| `ext_failed` | ext | the guest returned an error result (host kind Failed) |
| `ext_busy` | ext | `ext pty 0.2.0: a previous call is still blocked in a host call` (host kind Busy) |
| `ext_host` | ext | host-side failure (host kind Host) |
| `ext_disabled` | ext | `extensions are disabled in safe mode` |
| `io_error` | read, write, edit, bash (spill) | `src/main.rs: No space left on device (os error 28)` + `hint: disk full` |
| (interrupted) | boot repair | not an `error:` line. log-context.md §3.6 owns these two texts, `INTERRUPTED_TEXT` (`[init:interrupted] … (interrupted; state unknown; verify before retrying).`) and `NOT_STARTED_TEXT` (`[init:not-run] …`), with `is_error: true` |
| `length_cutoff` | loop | loop.md §3.6 |
| `refused` | loop | loop.md §3.6 |
| `probe_policy` | boot (probe mode only) | `not executed in probe mode` (boot.md §3.9) |
| `panic` | loop | `the read tool crashed: index out of bounds`; this is a body bug, and the log keeps it for the agent to fix. The result has no source location; the location is in the panic hook's stderr line |

I/O errors map from `io::ErrorKind`: `NotFound` → not_found, `PermissionDenied` and `ReadOnlyFilesystem` → permission_denied, `IsADirectory` → is_directory, `NotADirectory` → `invalid_input` (`a parent of <path> is a file`), `StorageFull` → `io_error` with `hint: disk full`. Anything else is `io_error: <path>: <OS message>`.

## 5. Failure modes and recovery

| Failure | Behavior |
|---|---|
| A command writes GBs of output | head and tail inline; spill capped at 64 MiB and 1 GiB per session; the pipe is always drained, so the command never blocks on a full pipe |
| A command spawns a daemon without redirecting | killed after the 2 s drain grace, with an explanation; the agent learns to redirect, or uses `setsid` / tmux |
| A command ignores TERM | KILL after 2 s |
| A command calls `setsid` or `setpgid` to leave the group | it escapes the group kill (as in every process-group design). The kernel's sandbox-wide sweep is the backstop (kernel/supervise.md §3.8.2) |
| No `/bin/kill` and no `/usr/bin/kill` | bash's builtin kill; if that also fails, `Child::kill()` on the leader, plus a warning in the result |
| Disk full mid-write | the temp file is removed; `io_error` with a disk-full hint; the original is untouched (atomic rename) |
| The file changes between the model's read and its edit | the match fails or reports ambiguity on the current content; the hints tell the model to re-read |
| An edit matched by tier 2 or 3 in an unexpected place | impossible by construction: every tier requires exactly one match; the diff shows the result |
| The model sends `old_string` | `invalid_input` + `did you mean "old_text"?`; one wasted round trip, no silent alias |
| The extension host panics or traps | `ext_trap` result; the host isolates stores (host.md) |

## 6. Performance notes (spec §9)

- **Rank 1 (cache).** The registry is byte-stable (sorted keys, fixed order, no versions or dates in the text). `ext` never changes when extensions change (A2).
- **Rank 3 (tool path).**
  - read, write and edit are in-process. A typical read is a few `read(2)` syscalls with no fork or exec.
  - bash pays one `posix_spawn`/`fork+exec` of bash. The sandbox is already applied to the body, so it is not set up again per call (spec §7.4).
  - There is one capture thread, one waiter thread, and no polling: the waiter blocks in `wait()` and the loop in `recv_timeout`. Only a host `process.run` with a cancel flag wakes every 10 ms (§3.5.7).
  - The process-group kill (1.4 ms) runs only on the cold path.
- **Rank 2 (round-trips).** The hints in edit errors (line numbers, nearest match) and the always-on diff cut the "re-read the file, try again" cycles. The head+tail output keeps both the first error and the summary in one result.
- **Rank 5 (footprint).** Per running bash call: ≤ 30 KB `mem` + a 26 KB ring + a 64 KB read buffer, and 2 threads.
- **Output tokens.** ANSI stripping, `\r` collapsing, `NO_COLOR`/`TERM=dumb` and the 30 KB cap keep tool results, and so the context, small. That is the "spill large outputs to files" layer of spec §4's context design.

## 7. Invariants and security (spec §2.1)

- **Invariant 3 (blast radius).**
  - The tools apply no path policy of their own. The OS sandbox is the only enforcer, so no prompt or tool check can be talked around, and no tool check gives a false sense of safety.
  - `permission_denied` hints name the writable roots, which helps awareness without enforcing anything.
  - The bash environment holds no credentials.
- **No `unsafe`.** Process groups use the safe `CommandExt::process_group`, and kills use an external `kill`. The body keeps `#![forbid(unsafe_code)]` (A6).
- **Invariant 4 (lineage).** Every tool effect is recorded: the call args in the assistant message, the result in `tool_result`, long outputs in spill files referenced by path.
- **Determinism of the prompt.** Tool *results* are volatile data, appended at the tail. They never enter the prefix.
- **`kill_group` never targets pgid ≤ 1.** A negative argument of `-1` would signal every process the user owns.

## 8. Test plan

Unit tests (G0), run in a temp dir:
1. **read:**
   - paging: a 5,000-line file returns lines 1–2000 with the hint `offset=2001`; then 2001–4000; then 4001–5000 with no hint;
   - a 50 KB cap hit before 2,000 lines;
   - a 10,000-byte single line is cut with the hint;
   - CRLF lines displayed without `\r`;
   - invalid UTF-8 gets the note;
   - an empty file;
   - `offset` past the end;
   - a directory; a missing file with a similar-name hint;
   - PNG/JPEG/GIF/WebP magic: an image block, or the text note on a no-image model, or too_large;
   - a NUL byte gives binary_file.
2. **write:**
   - a new nested path creates the parents;
   - replace keeps the mode `0755`;
   - writing through a symlink keeps the link;
   - an existing directory gives is_directory;
   - a temp file is never left behind after an injected rename failure (target in a read-only dir);
   - concurrent readers never see a partial file (a reader thread loops `read` during 1,000 writes).
3. **edit:**
   - tier 1 unique; tier 1 ambiguous (overlapping `aa` in `aaa`);
   - tier 2 with trailing spaces and curly quotes;
   - tier 3 with the model's indent short by 4 spaces (re-indented), long by a tab (stripped), and inconsistent (no match);
   - CRLF file round-trip (stays CRLF); BOM kept;
   - two edits overlapping; two identical `old_text`s (overlap);
   - no_change; empty `old_text`;
   - 64 edits OK, 65 rejected;
   - the diff golden output with line-number shifts;
   - the no_match hint finds the first line elsewhere;
   - a property test: for random files and random unique substrings, edit then reverse-edit restores the original bytes.
4. **bash:**
   - `echo hi` gives `hi` + `[exit_code=0 …]`;
   - `exit 3` gives `is_error` and exit_code=3;
   - stderr/stdout interleave preserved;
   - `yes | head -c 10` terminates;
   - `seq 1 200000` gives head + marker + tail and a spill file with the full bytes;
   - an ANSI-colored output is stripped inline, raw in the spill;
   - a `\r` progress bar is collapsed;
   - `sleep 5` with timeout 1 gives TERM, the timeout trailer, wall < 1.5 s;
   - `trap '' TERM; sleep 30` with timeout 1 gives KILL after 2 s;
   - `sleep 300 &` (no redirect) gives the leader exit, then the group killed after 2 s with the note;
   - `sleep 300 > /dev/null 2>&1 &` returns immediately, and the job is still alive afterwards (killed by the test);
   - abort during `sleep 100` gives the aborted trailer in < 200 ms, with `pgrep -g <pgid>` empty;
   - `timeout: 1.5` gives `invalid_input`; `timeout: 120.0` is accepted as 120;
   - `cd /tmp` does not persist;
   - `echo $INIT_CALL_ID $NO_COLOR $GIT_EDITOR` shows the values;
   - `CARGO_TARGET_DIR` is not overridden if already set;
   - `BASH_ENV` is ignored.
5. **ext** (with host.md's test components): an unknown name gives `ext_unknown` with the host's list; `input` that is not an object, or over 1 MiB, gives `invalid_input` without a host call; safe mode gives `ext_disabled`; a guest exceeding its `timeout_ms` gives `timeout`; an owner abort during a spinning guest gives `aborted` in < 250 ms; each `OutcomeKind` maps to its code (§3.6); the guest sees `/work` = the body cwd.
6. **`run_group`** (shared helper): `Separate` keeps stdout and stderr apart and returns the last 1 MiB of each with exact totals for a 3 MiB writer; `stdin` bytes reach `cat`; a child that never reads stdin does not fail the call (`BrokenPipe` ignored); setting `cancel` during `sleep 100` returns `cancelled` within 50 ms of TERM, with the group empty; `trap '' TERM` under cancel gets KILL after 2 s; `Merged { spill_dir: None, … }` with 100 KB of output gives head + tail and no file.
7. **Registry:** `specs()` serializes to exactly the bytes of §4.1 (golden file). Changing a single character fails this test and must come with `Prefix-change: yes`.

Platform checks (Phase 0):
- T-L1: on Linux (CI image), `/bin/kill -s KILL -- -PGID` works, or the fallback works.
- T-M1: the same on macOS; already verified 2026-10-19.

Scenario IDs from testing.md (roadmap.md Q2):
- L1 suite "seed `tools`" (testing.md §3.2): items 1–4, 6 and 7 above.
- A1 (§3.9): all four core tools in a real task, mock and E2E-4.
- A2 / A2b / A2-real: `ext` reload keeps the §4.1 bytes, so the prefix hash is unchanged; the `ext` call path of item 5.
- A6: `write kernel/x` gives `permission_denied` from the OS; `bash curl https://example.com` fails in the sandbox; `unsafe` fails G0.
- E2E-2 (§3.10): `write` and `bash` against DeepSeek, with streamed arguments.
- M-5 (§3.11): bash output caps and spill keep RSS flat over 1,000 steps (every 100th step reads a 5,000-line file, which exercises the read continuation hint).
- F-hang (a): the process-group kill reaches `sleep 3600` children (the same helper, through supervise's probe sweep).

## 9. Open questions

Resolved in R1/R2 and removed:
- Q-T1 (ext/host.md): host.md §3.13 describes the shared helper as a waiter thread + `recv_timeout`; `CallCtx.abort` exists (§3.6); `run_group` takes a plain cancel flag and a `Separate` capture (§3.5.7).
- Q-T2 (kernel/sandbox.md): `/bin/kill` and same-sandbox signals are allowed.
- Q-T3: `state/spill/<session_id>/` is in CONVENTIONS §3, and the live body's cwd is `$INIT_HOME/work` (supervise §3.4).
- Q-T4 (log-context): the duration field is `wall_ms`; masking keeps `spill_path`.

**Cross-doc:** none.

**Owner decisions** (not in CONVENTIONS §6; the stated default ships unless the owner objects; reconciliation/body-core.md R2 asks the main session to list them)
- **Q-T5.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* The default bash timeout is 120 s (Claude Code's default; Factory found short defaults better). Cold builds of the body with wasmtime may need more; the model passes `timeout` explicitly. OK?
- **Q-T6.** *Settled 2026-10-19, keep as proposed (CONVENTIONS §6).* Background jobs that do not redirect their output are killed after 2 s. The alternative is to leave them running and abandon the reader thread until they exit (Pi's behavior), which leaks a thread and pipe per job. Current choice: kill, and explain.

## 10. Spec deltas

1. **Spec §4 bash: "tail-truncated at about 30k characters".** Here: head (4 KB) + tail (26 KB) when over 30,000 bytes. Why: the first compiler error is at the top. Claude Code (head and tail on failure) and mini-swe-agent (first and last 5,000 characters) do the same.
2. **Spec §4 edit: "falls back to matching that tolerates whitespace and line-ending differences".** Made precise as three tiers: exact; trailing whitespace + punctuation; uniform indent delta with re-indent. Line endings are normalized before all of them. Pi's NFKC step is left out (no std support).
3. **Spec §4 read: "images become image blocks".** Images are not resized: std cannot decode them, and dependencies are fixed. Oversized images get a `too_large` error with a downscale hint instead.
4. **bash, ANSI stripping and `\r` collapsing.** Not in the spec. Added to cut output tokens. The spill file stays raw.
5. **ext discovery through `$INIT_HOME/state/ext/catalog.json`** (written by ext/host.md) and the unknown-name list. The spec says only that the schema never changes; this makes the discovery path concrete.

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.15, and §1.14 for D5–D7, D16 and D17):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP11 D8 | a similar name also qualifies at edit distance ≤ 2, because the doc's own example (`src/lop.rs`) does not share its first 3 characters | §3.2 |
| build: WP11 D4 | `CaptureSpec::Merged` gains `name`, the spill file's name (the call id for bash, `"git"` for boot's git sync) | §3.5.7 |
| build: WP11 D9 | `read` skips the generic 51,200-byte cap, because its trailer can pass it by a few bytes | §3.1 |
| build: WP11 D10 | the timeout trailer says `(TERM)` when TERM was enough, and `(TERM, then KILL after 2s)` otherwise | §3.5.6 |
| build: WP11 D11 | CRLF becomes LF in the bash model text; the spill file stays raw | §3.5.2 |
| build: WP11 D5, D6 | the `Prepared` (with `idx`, `call_id`, `name`), `Ran`, `PathCx` and `ToolCx` shapes; `Tools::prepare(idx, call, &PathCx)` and `Tools::run` returning `Ran` | §3.1 |
| build: WP11 D7 | `ExtHostApi` is a trait with a type-erased `Snapshot` | §3.1, §3.6 |
| build: WP11 D16 | a panic result carries no source location; the location is in the panic hook's stderr line | §4.6 |
| build: WP11 D17 | the session spill total is a process-global counter, set at boot by `init_spill_total` | §4.4 |

## 11. References (checked 2026-10-19)

- Rust std `CommandExt::process_group` (stable 1.64.0): https://doc.rust-lang.org/stable/std/os/unix/process/trait.CommandExt.html
- Rust std `io::pipe` (stable 1.87.0), `PipeWriter: Into<Stdio>`: https://doc.rust-lang.org/stable/std/io/fn.pipe.html
- Rust std `process::Child` (`wait`, `kill` signals only the child), `ExitStatusExt::signal`: https://doc.rust-lang.org/stable/std/process/struct.Child.html
- Rust std `mpsc::Receiver::recv_timeout` (1.12.0): https://doc.rust-lang.org/stable/std/sync/mpsc/struct.Receiver.html
- Rust std `fs::rename`, `OpenOptions::create_new`, `set_permissions`: https://doc.rust-lang.org/stable/std/fs/index.html
- POSIX `kill` utility (negative pid = process group; `--`): https://pubs.opengroup.org/onlinepubs/9799919799/utilities/kill.html
- Pi tools source: `read.ts`, `write.ts`, `edit.ts`, `edit-diff.ts` (normalizeForFuzzyMatch, uniqueness and overlap errors), `bash.ts` (description text, PI_* env, tree kill), `truncate.ts` (DEFAULT_MAX_LINES 2000, DEFAULT_MAX_BYTES 50 KB): https://github.com/earendil-works/pi/tree/main/packages/coding-agent/src/core/tools
- Codex `apply-patch/src/seek_sequence.rs` (exact → trim_end → trim → Unicode punctuation normalization): https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/seek_sequence.rs
- Claude Code tools reference (Read cap and partial view; Bash 2 min default, 30k inline, head and tail on failure; Glob/Grep moved into the shell): https://code.claude.com/docs/en/tools-reference (via research_notes/production_harness_internals.md §2)
- mini-swe-agent SWE-bench config (PAGER=cat, first and last 5,000 characters): https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml (via research_notes/minimal_harnesses.md Q2)
- Aider unified diffs (flexible application: 9× fewer errors): https://aider.chat/docs/unified-diffs.html
- Factory Terminal-Bench post (short default timeouts): https://factory.com/news/terminal-bench
- RFC 4648 (base64): https://www.rfc-editor.org/rfc/rfc4648
- Local verification, macOS 14.6, rustc 1.97.1, 2026-10-19:
  - `process_group(0)` gives pgid = pid;
  - `/bin/kill -s TERM|KILL -- -PGID` and bash's builtin kill empty a 3-member group;
  - `/bin/kill` spawn+wait takes p50 1.44 ms / max 3.18 ms;
  - `std::io::pipe` merges stdout and stderr in order and needs the `Command` dropped for EOF;
  - a background job holds the pipe until killed;
  - the child has default SIGPIPE;
  - `/bin/bash` 3.2.57 and `/opt/homebrew/bin/bash` 5.3.20.
