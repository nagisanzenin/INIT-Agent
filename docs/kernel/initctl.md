# initctl: the agent's syscall interface

```
Status: v0.3 + build sync 2026-10-20 · Area: kernel
Depends on: CONVENTIONS.md (v0.3), spec v0.2 (§2.1, §6, §7.3) and v0.3 (§12–§19), adr/007-root-and-autonomy-ladder.md,
            root/root.md (ledger, Root link), root/egress.md, root/sealed-eval.md, kernel/supervise.md,
            kernel/sandbox.md, kernel/gate.md, kernel/proxy.md, evolution.md, testing.md
Defines:    initctl wire protocol details (framing, envelope, progress frames, lanes), every command with
            args/results/errors (including the v0.3 commands `level`, `task *`, `fleet *`, `kernel promote`,
            `kernel propose`), job model and job records, error-code registry (E_*), peer authentication,
            the level column of the command matrix, the `initctl` thin-client CLI and its exit codes
```

---

## 1. Purpose

`initctl` is how the agent asks the kernel to do the things it cannot do inside its sandbox:
- reload, gate, promote and roll back;
- vendor a dependency;
- read its autonomy level and ceilings;
- from L1, submit tasks it wrote;
- from L2, run a fleet of bodies;
- from L3, promote a kernel it wrote. Before L3 it can only propose a kernel change.

The agent runs `initctl` from its `bash` tool. `initctl` is a thin client: it serializes one request, sends it over a Unix socket, prints the answer and exits. All logic runs in the kernel, outside the body's sandbox instance, so a body never runs kernel code with kernel privileges (spec §3). Since v0.3 the kernel itself runs in the Root's agent zone (spec §13). Every level-gated command is checked against the Root's ledger, and the Root enforces the hard ceilings behind it (kernel/supervise.md §3.16).

## 2. Responsibilities / non-goals

**Responsibilities:**
- the JSON-lines protocol over `run/kernel.sock` and the probe and fleet lane sockets;
- each command's arguments, results, errors and required level;
- long-running jobs and their progress;
- peer authentication;
- the CLI.

**Non-goals:**
- what a gate stage checks, and the task admission rules: kernel/gate.md, with root/sealed-eval.md;
- the handoff, rollback, fleet and kernel-upgrade mechanics: kernel/supervise.md;
- trailer validation rules and their error codes: evolution.md §4.2;
- proxy statistics and soft caps: kernel/proxy.md;
- levels, ceilings, egress classes and the owner endpoint: root/root.md, root/egress.md.

This doc routes to them and fixes the wire shape.

## 3. Design

### 3.1 Transport and framing

- **Socket:** `SOCK_STREAM` Unix domain socket.
  - The live lane (the primary body, `b0`) is `$INIT_HOME/run/kernel.sock`.
  - Each probe gets `probe/<id>/run/kernel.sock`.
  - Each L2 fleet member gets `run/fleet/<bid>/kernel.sock`. It lives under `run/` because the Root's `kernel` profile lets the kernel bind only under `run/` and `probe/` (kernel/sandbox.md §3.3.5).
  - The client takes the path from `INIT_KERNEL_SOCK`.
  - v0.3 removed `run/owner.sock` and the owner lane. The owner reaches the kernel only through the Root: owner lines arrive as `owner.in` and are parsed by kernel/supervise.md §4.6 (`/stop`, `/resume`, `/status`, `/accept`, `/bless`, `/reject`). The owner's status tools read the Root's own state (`init-root status`; root/root.md §4.3).
- **Framing:** one JSON object per line, UTF-8, terminated by `\n`. A request line is at most **64 KiB**; a response or progress line at most **1 MiB**. A longer request line → `E_PROTO`, then close.
- **Version:** every frame has `"v":1`. A request with another `v` → `E_VERSION` (the response still uses `v:1`).
- **Ordering:** one request at a time per connection. The server reads the next request only after it has sent the previous request's final response. Progress frames for a request come before its final response. The CLI uses one connection per invocation.
- **Keys:** sorted on output (serde_json `Value`, no `preserve_order`). Unknown request fields are ignored; unknown response fields must be ignored by clients. A value from a set that a newer kernel may extend (a job `state`, a progress `stage`, a `root.*` code) is read as an open string, so an older `initctl` still parses the frame. Sets the kernel owns and closes (`lane`, `slot`, `mode`) are strict.
- **Connection limits:** at most 8 concurrent connections per lane (the 9th gets `E_RATE` and is closed), at most 10 requests per second per lane (token bucket, burst 20), idle connections closed after 30 s without a request. The 9th connection's `E_RATE` has a null `id` and is sent after the peer check. A request over the rate limit is answered after its line is parsed, so the `E_RATE` echoes its `id`, and the connection stays open.

### 3.2 Envelope

Request (fixed by CONVENTIONS):

```json
{"args":{"ref":"evo/pty-tool"},"cmd":"promote","id":"48121-1","v":1}
```

- `id`: client-chosen string of 1–64 bytes from `[A-Za-z0-9._:-]`. It is echoed on every frame for the request.
- `cmd`: a command name from §4.1. Single-word commands use their name (`ping`, `promote`). Two-word commands use a dotted wire name: `task.submit`, `fleet.spawn`, `kernel.promote`, `job.status`, and so on. The server also accepts the space form (`"task submit"`) and the aliases `wait` (= `job.wait`) and `propose-kernel` (= `kernel.propose`).
- `args`: object, may be `{}`.

Final response, success:

```json
{"id":"48121-1","ok":true,"result":{"job":"j-000043","sha":"4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807","state":"gating"},"v":1}
```

Final response, failure:

```json
{"error":{"code":"E_BUSY","data":{"job":"j-000041"},"details":[],"message":"a promote is already running"},"id":"48121-1","ok":false,"v":1}
```

- `error.code`: an `E_*` code from §4.2, or a dotted code registered by another doc (evolution.md §4.2: `trailer.*`, `predicts.*`, …; gate.md: `gate.*`). Clients treat any unknown code as a failure with exit code 1.
- `error.message`: one line of human-readable text, at most 500 bytes.
- `error.details`: array of `{code, detail}` for multi-problem errors (evolution.md asks for this; all validator problems are reported at once). It is always written, as `[]` when empty.
- `error.data`: optional object with machine-readable context. It is omitted when there is none.

Progress frame (zero or more before the final response; never has an `ok` key):

```json
{"id":"48121-1","progress":{"eta_s":30,"job":"j-000043","msg":"cargo clippy -D warnings","pct":40,"stage":"G0","state":"gating","ts":"2026-10-19T12:31:04.118Z"},"v":1}
```

- Sent on every stage or state change, and otherwise at most once per second while something is happening.
- `pct`: integer 0–100, never decreases within a job. It is an estimate of the whole job, weighted by the expected duration of each stage (gate.md supplies stage weights; default equal).
- `eta_s`: integer seconds or null. Re-projected from the observed rate of the current stage.

### 3.3 Peer authentication

The kernel and the agent run as the **same uid**, so file permissions cannot separate them. Separation comes from which sockets each instance can reach. Per kernel/sandbox.md, the live instance may connect only to `run/kernel.sock` and `run/proxy.sock`; each probe and each fleet member only to its own lane sockets (`path-literal`); and no body to the Root's sockets (default deny plus tail Z6). Peer credentials are a second, independent check: they attribute every connection to a sandbox instance. Since v0.3 the kernel–body boundary is a stability boundary, not the security one (spec §13), but attribution still keeps one body's mistakes out of another's lane.

**On every `accept()`**, before reading any request:

1. Read the peer's credentials.
   - **macOS:** `getpeereid(fd, &uid, &gid)` gives the effective uid/gid the peer had at `connect()` (cannot be faked by the peer). `getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &len)` gives its pid.
   - **Linux:** `getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &ucred, &len)` gives `{pid, uid, gid}` as of `connect()`. The pid is translated into the kernel's pid namespace. Since v0.3 the kernel and the bodies are Root-spawned siblings in different pid namespaces, so the pid is expected to read 0 (UNVERIFIED; test IC-12) and is only logged.
2. Require `uid == geteuid()`. Otherwise close silently.
3. Attribute the peer to the lane's instance:
   - **macOS** (live, member or probe lane): three conditions must all hold. This is the sweep's three-condition rule (D1; kernel/supervise.md §3.8.2), and it also guards attribution:
     - `sandbox_check(pid, NULL, 0) == 1`;
     - `sandbox_check(pid, "file-read-data", SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, <mark>) == 0`, where `<mark>` is `INIT_HOME/run/mark/<instance id>` (Root-written; the id comes from the `spawn` response);
     - the same check on the Root's canary `ROOT_HOME/run/canary` is **non-zero**.

     The canary condition (v0.3) rejects a foreign sandboxed process of the same uid that can read the mark's directory. Calling `sandbox_check` with the path filter on another sandbox's process works from inside the kernel's sandbox: tested 2026-10-20 with a compiled C helper under root/root.md's composed `kernel` profile (kernel/sandbox.md S-Z10). This settles root/root.md §9.1 X3's UNVERIFIED path form.
   - **Linux** (live, member or probe lane): by **socket path**. Each Root-launched instance's mount namespace contains only its own two lane sockets, bound individually (kernel/sandbox.md §3.6.1, §3.13), so only that instance can name the socket. The `SO_PEERCRED` pid is logged, not used.
4. On failure, send `{"v":1,"id":null,"ok":false,"error":{"code":"E_PEER",…}}` and close. Log `{pid, lane, reason}` to `state/kernel/logs/kernel.jsonl`.

The server writes that file as `{"c":"serve","ev",…}` lines, for `E_PEER` and for internal errors. Supervise's own log (`klog`) is one JSON line per message on stderr, which the Root keeps in `ROOT_HOME/logs/kernel-<seq>.stderr`. So the kernel has two log sinks.

```rust
pub struct Peer { pub uid: u32, pub gid: u32, pub pid: i32 }

#[cfg(target_vendor = "apple")]
pub fn peer_of(s: &UnixStream) -> io::Result<Peer> {
    let fd = s.as_raw_fd();
    let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 { return Err(io::Error::last_os_error()); }
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID,
                                       (&mut pid as *mut libc::pid_t).cast(), &mut len) };
    if rc != 0 { return Err(io::Error::last_os_error()); }
    Ok(Peer { uid, gid, pid })
}

#[cfg(target_os = "linux")]
pub fn peer_of(s: &UnixStream) -> io::Result<Peer> {
    let mut c: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED,
                                       (&mut c as *mut libc::ucred).cast(), &mut len) };
    if rc != 0 { return Err(io::Error::last_os_error()); }
    Ok(Peer { uid: c.uid, gid: c.gid, pid: c.pid })
}
```

std's `UnixStream::peer_cred` would do the same, but it is still unstable (`peer_credentials_unix_socket`, tracking issue #42839, open as of 2026-10-19), so the kernel calls `libc` 0.2.191 directly (`getpeereid`, `SOL_LOCAL`, `LOCAL_PEERPID` on Apple; `SO_PEERCRED`, `ucred` on Linux). `nix` 0.31.3 offers the same through `unistd::getpeereid` and `sockopt::{PeerCredentials, LocalPeerPid}` (features `socket`, `user`) if the kernel prefers it.

**Residual race.** A pid read at `connect()` could in theory be reused by another process before step 3 runs (microseconds). Removing it: `LOCAL_PEERTOKEN` (audit token) with `sandbox_check_by_audit_token` on macOS (exported from libsystem_sandbox; signature UNVERIFIED), `SO_PEERPIDFD` on Linux ≥ 6.5 (UNVERIFIED). Phase 0 accepts the race, because the socket path is reachable only from the right sandbox in the first place.

**Socket files.** At start the kernel holds `run/kernel.lock`, so it is the only kernel. It unlinks `run/kernel.sock` and `run/proxy.sock` by exact path and binds them fresh (`UnixListener::bind` never unlinks and fails with `AddrInUse` on a stale file). Lane sockets under `probe/<id>/run/` and `run/fleet/<bid>/` are bound the same way when their instance is created. `run/` is mode `0700`, set by the Root at install. The kernel never chmods `run/` itself, because tail rule Z4 denies every write to it, chmod to the same mode included (root/root.md §3.3.2). For a lane directory below `run/` or under `probe/`, the kernel creates it and chmods it to `0700` only when its mode differs. Path length is checked against 103 bytes (macOS) / 107 bytes (Linux) before binding (supervise.md §3.1). A body instance can connect to a socket the kernel instance bound, across the two Seatbelt instances (kernel/sandbox.md S-Z8b, tested with the composed profiles).

### 3.4 Lanes

| Lane | Socket | Peer must be | Purpose |
|---|---|---|---|
| `live` | `run/kernel.sock` | a process of the primary body's (`b0`) instance | the agent's syscalls |
| `member` (L2) | `run/fleet/<bid>/kernel.sock` | a process of member `<bid>`'s instance | a fleet member's syscalls: promote its own line, quick gates, its own jobs (kernel/supervise.md §3.14) |
| `probe` | `probe/<id>/run/kernel.sock` | a process of that probe instance | what a candidate may do: purpose `probe` (G2, G3) or `run` (a G4 task run) |

v0.2's `owner` lane is gone. The owner's optional signals are owner lines that the Root relays as `owner.in`, parsed by kernel/supervise.md §4.6: `/stop`, `/resume`, `/status`, `/accept`, `/bless`, `/reject`. They never gate anything (D4). v0.2's `approve`, `deny`, `clear-safe` and `panic` no longer exist in the kernel. Panic and leaving system safe mode are the Root's (`init-root panic`, `init-root resume --clear-safe`; root/root.md §4.3).

Command availability (`min level` is checked against the Root's level, kernel/supervise.md §3.16; below it → `E_LEVEL`, and the request never reaches the Root, where it would count as a demoting ceiling hit). The L1–L3 commands are the ones evolution.md §4.1 proposes; this doc owns their wire shape.

| Command | min level | live | member | probe |
|---|---|---|---|---|
| `ping`, `status`, `level` | L0 | yes | yes (member-scoped) | yes (probe-scoped) |
| `reload` | L0 | yes | yes (to itself) | yes (to that probe body; both purposes) |
| `gate` (quick) | L0 | yes | yes (repo inside its root) | purpose `probe`: G0–G2 only, repo inside the probe root; purpose `run`: `E_LANE` (a task run must not start gates) |
| `promote` | L0 | yes (`ref`; from L2 also `--sha` to adopt a qualified SHA) | yes, on its own line: G0–G3, its own handoff, G4 against the system LKG; a pass **qualifies** the SHA (§4.1) | `E_LANE` |
| `rollback` | L0 | yes | `E_LANE` | `E_LANE` |
| `vendor` | L0 | yes | yes | `E_LANE` |
| `task template` | L0 | yes | `E_LANE` | `E_LANE` |
| `task submit` | L1 | yes | `E_LANE` | `E_LANE` |
| `task list`, `task show` | L1 | yes | `E_LANE` | `E_LANE` |
| `budget set`, `model set` | L1 | yes | `E_LANE` | `E_LANE` |
| `fleet list` | L0 | yes | `E_LANE` | `E_LANE` |
| `fleet spawn`, `fleet send` | L2 | yes | `E_LANE` (members cannot create members) | `E_LANE` |
| `fleet stop` | any (stopping is always allowed) | yes | `E_LANE` | `E_LANE` |
| `kernel propose` (alias `propose-kernel`) | L0 | yes | `E_LANE` | `E_LANE` |
| `kernel promote`, `kernel status` | L3 | yes | `E_LANE` | `E_LANE` |
| `job.status`, `job.wait`, `wait`, `job.list`, `job.cancel` | L0 | own lane's jobs | own lane's jobs | own lane's jobs |

The matrix writes two-word commands with a space; on the wire they are dotted (§3.2).

### 3.5 Kernel state gating

Checked before any work, in this order:

| Kernel state (supervise.md) | Refused commands | Code |
|---|---|---|
| control link down, or the cached level/ceilings older than 120 s | every level-gated command, and every command that needs a Root request (`vendor`'s egress, `task submit`'s registration, `fleet spawn`, `kernel promote`) | `E_ROOT` (evolution.md's proposed `E_BUSY{reason:"root_unreachable"}` maps here) |
| outbox backlog (supervise.md §3.13.3) | `promote`, `vendor`, `task submit`, `fleet spawn`, `kernel promote` | `E_ROOT` (`data.reason:"outbox_backlog"`) |
| `Halted` (kernel-level) | everything except `ping`, `status`, `level`, `job.*` | `E_HALTED` |
| `Stopping` (Root `stop` or `prepare_swap`), or shutting down | everything | `E_SHUTTING_DOWN` |
| `Mode=Safe` (body safe mode, or system safe mode from the boot block) | `promote`, `gate`, `reload`, `vendor`, `rollback`, `task submit`, `budget set`, `model set`, `fleet spawn`, `fleet send`, `kernel promote` | `E_SAFE_MODE` |
| `Mode=Provisional` (the primary) | `vendor`, `kernel promote` | `E_PROVISIONAL` |
| provisional depth = `max_provisional_depth` | `promote` | `E_PROVISIONAL_DEPTH` |
| free space < `disk.min_free_build_gib` | `gate`, `promote`, `vendor`, `task submit`, `fleet spawn`, `kernel promote` | `E_DISK` |
| a handoff or rollback in progress, or a promote pipeline running | `promote`, `rollback` | `E_BUSY` |
| an L3 kernel candidate in flight, or a swapped kernel awaiting its sealed audit (evolution.md §3.15.6) | `promote` (live lane), `kernel promote` | `E_BUSY` (`data.reason:"kernel_candidate"`) |
| `fleet spawn` with `fleet_spawns_day` (50) used up | that command | `E_QUOTA` (`data.quota:"fleet_spawns_day"`) |
| level below the command's `min level` (§3.4) | that command | `E_LEVEL` (`data: {need, have}`) |
| a member would exceed `parallel_bodies` (`fleet spawn`; the primary counts) | that command | `E_CEILING` (`data: {ceiling:"parallel_bodies", limit, used}`), and no `spawn` reaches the Root. While `parallel_bodies` is not known yet, the answer is `E_ROOT` with `data.reason:"stale"` |

v0.3 removed `Panicked` and `E_PANICKED`. During a panic the Root has killed every zone process, the kernel included, so no client can connect (CLI exit 69).

### 3.6 Jobs

Every command that can take more than about one second creates a **job** owned by the kernel. A job does not depend on the connection: if `initctl` is killed (for example by the bash tool's timeout), the job continues and can be awaited again.

- **Id:** `j-` + 6-digit zero-padded sequence (`j-000043`), monotonic per `INIT_HOME`. evolution.md's `gate_id` is this job id.
- **Record:** `state/kernel/jobs/<id>.json` (agent-readable), rewritten atomically on every state change (§4.4). The last 200 records are kept; older ones are deleted by exact path.
- **States:** `queued` → `running` → terminal (`succeeded` | `failed` | `cancelled`), with named sub-stages in `stage`. The immediate answer to a job command with `wait:false` says `queued` even if the backend's thread has already started. A milestone or terminal state already reached is reported as it is: `reload` and `rollback` are at `awaiting_boundary` already in their synchronous part.
- **Ownership.** A job belongs to one lane instance (`live:b0`, `member:<bid>`, `probe:<id>`), held in memory, and `job.*` on another lane's job gives `E_LANE`. After a kernel restart only the live lane's records keep an owner; older member and probe records answer `E_LANE`. A job that was unfinished at the restart is `failed` with `error: null` and `reason: "kernel_restart"`, and `wait` on it exits 1.
- **Milestone `awaiting_boundary`.** Some jobs end with something that only happens at the caller's next step boundary: the handoff for `promote`/`rollback`, the reload for `reload`. A body's step boundary comes *after* its current tool call returns, so a tool call that waits for the handoff itself would wait forever. Rule: **`wait` returns at `awaiting_boundary` as if the job were finished** (exit 0). The outcome arrives later as a log event: a `boot` event (reason `promote`/`rollback`) on success, `update_failed` on failure, `reload` for reloads.
- **Milestone `awaiting_swap`** (v0.3). After `system.swap`, a `kernel promote` job is in state `awaiting_root` while the Root runs the gate-of-gates (root/root.md §3.8; progress arrives as `swap` events). Rule (evolution.md §4.1): **`wait` returns at the gate-of-gates verdict.** `gog_pass` moves the job to `awaiting_swap`, which `wait` treats as finished (exit 0). `gog_fail` fails the job with `E_GATE_FAILED{stage:"gate_of_gates"}`. After `awaiting_swap` the swap itself restarts the caller's body: it boots `cold` with `boot.json.kernel_upgrade` set, and the job record shows the outcome.
- **Deadlock guard.** A request with `"wait": true` for `reload` from the live lane gets `E_WOULD_DEADLOCK`, because the reload cannot start before the caller's tool call ends. (`promote` and `rollback` with `wait:true` are fine thanks to the milestone rule.) The guard applies to the live lane only: on a member or probe lane, `reload --wait` is accepted and returns at `awaiting_boundary`.
- **Cancel:** allowed in `queued` and `running` before `awaiting_boundary` or `awaiting_root` (fetch, build, G0–G3 and K0–K3 instances are killed through the Root's `kill`). After that: `E_NOT_CANCELLABLE`. Once the Root holds a kernel candidate, only the Root's verdict ends the job.

### 3.7 Fault-injection point

`serve.promote.after_fetch` (testing.md §3.8.1): placed after the candidate has been fetched into `refs/init/candidates/<job>` and archived, before the kernel checks and the evolution validator run. Recovery after `abort`: the job is marked `failed` with `reason:"kernel_restart"` at cold start, and the candidate ref is deleted.

## 4. Interfaces

### 4.1 Commands

Notation: `args` and `result` objects are shown with types; `?` marks optional fields; examples are complete frames.

#### `ping`

- args: `{}`
- result: `{"kernel_version": string, "lane": "live"|"member"|"probe", "proto": 1, "time": rfc3339}` (v0.3 has no `owner` lane)

```json
{"args":{},"cmd":"ping","id":"1","v":1}
{"id":"1","ok":true,"result":{"kernel_version":"0.1.0","lane":"live","proto":1,"time":"2026-10-19T12:30:00.002Z"},"v":1}
```

#### `status`

- args: `{}`
- result (sections owned by other docs are marked). The example caps ($5.00/day, $3.00/gate) are the recommended values of PENDING OWNER (CONVENTIONS §6 E1). Since v0.3 the daily figure is a charter ceiling held by the Root, and the gate cap stays a kernel soft cap. All numbers are integers, so nothing float-formatted reaches a prompt (body/log-context.md §3.8.1). awareness.md §4.3's names map as: `running_sha` = `current.sha`, `lkg_sha` = `lkg.sha`, `provisional` = `current.provisional`, `restarts_since_lkg` = `counters.restarts_since_lkg`, `safe_mode` = (`mode == "safe"`).

| Field | Type | Source |
|---|---|---|
| `kernel` | `{version, proto, uptime_s}` | supervise |
| `lane` | string | here |
| `slot`, `mode` | string | supervise.md §4.3 |
| `current` | `{sha, provisional, since, depth}` | supervise |
| `lkg` | `{sha, since, seq}` | supervise |
| `handoff` | object \| null | supervise |
| `counters` | `{crashes_total, restarts_since_lkg, rollbacks, swaps_failed, swaps_ok}` | supervise |
| `safe` | object \| null | supervise |
| `disk` | `{free_mib}` (integer) | supervise |
| `sweep` | `"full"` \| `"degraded"` | supervise |
| `jobs` | `[{id, cmd, state, stage}]` (active jobs of this lane) | here |
| `budget` | `{day_cap_musd, day_spent_musd, gate_cap_musd, gate_spent_musd, soft}`: integers, micro-USD (gate fields null when no gate runs). `day_*` are the Root's (`ceilings.get` money block, root/egress.md); `soft` is the proxy's soft budgets as set by `budget set` (`ProxyStatus`, kernel/proxy.md Q-IC1) | kernel/proxy.md `ProxyStatus`, fed by `ceilings.get` |
| `cache_hit_permille` | integer 0–1000 \| null, over the last 50 requests | kernel/proxy.md `ProxyStatus.cache_hit_permille_last50` |
| `constitution_sha256` | `sha256:<64 hex>` of `kernel/CONSTITUTION.md` (the charter's text, placed by the Root), computed at kernel start | here (awareness.md X3) |
| `evolution` | object | evolution.md §4.1 |
| `root` (v0.3) | the Root's published status, relayed verbatim (awareness.md §4.5): `{level, max_level, next, ceilings, classes, audited_good, mode}`, from the boot block, `level`/`ceilings` events and `INIT_HOME/root/` | supervise.md §3.16 |
| `body` (v0.3) | `{id: "b0"\|"<bid>", lane}`; on a member lane `current` is the member's line (`refs/init/lines/<bid>`), and `lkg` is still the system LKG | supervise.md §3.14 |
| `fleet` (v0.3) | `null` when no member runs; else `{campaign, parallel_bodies, members: [{bid, line, slot, busy, share_musd_day, spent_today_musd, submissions, qualified}]}` | supervise.md §3.14, evolution.md §3.15.5 |
| `link` (v0.3) | `{connected: bool, boot_reason, ceilings_age_s, outbox_unanswered}`: the control link's health | supervise.md §3.13 |
| `sandbox` (optional) | the sandbox status object, the same one as `boot.json.env.sandbox` | kernel/sandbox.md §4.2 |

```json
{"id":"2","ok":true,"result":{"body":{"id":"b0","lane":"live"},"budget":{"day_cap_musd":5000000,"day_spent_musd":1420000,"gate_cap_musd":3000000,"gate_spent_musd":310000,"soft":{"day_musd":5000000,"gate_musd":3000000}},"cache_hit_permille":930,"constitution_sha256":"sha256:2c6e…","counters":{"crashes_total":3,"restarts_since_lkg":1,"rollbacks":1,"swaps_failed":2,"swaps_ok":9},"current":{"depth":1,"provisional":true,"sha":"4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807","since":"2026-10-19T12:36:01.004Z"},"disk":{"free_mib":42188},"evolution":{},"fleet":null,"handoff":null,"jobs":[{"cmd":"promote","id":"j-000044","stage":"G4","state":"running"}],"kernel":{"proto":1,"uptime_s":5234,"version":"0.1.0"},"lane":"live","link":{"boot_reason":"cold","ceilings_age_s":12,"connected":true,"outbox_unanswered":0},"lkg":{"seq":7,"sha":"0d3e5a9b1c22f4e6a8b0c2d4e6f8a0b2c4d6e8f0","since":"2026-10-19T09:12:44.000Z"},"mode":"provisional","root":{"audited_good":"t3f09c1e5b7d2","ceilings":{"day_musd":5000000,"parallel_bodies":1,"total_musd":25000000},"classes":["models","crates"],"level":0,"max_level":0,"mode":"normal","next":{"level":1}},"safe":null,"slot":"running","sweep":"full"},"v":1}
```

#### `level` (v0.3)

Read the autonomy ledger and the ceilings: the level, the cap, what the next level needs, and how close each quota is. The result is the Root's `ledger.get` result (the content of `ledger.json`, root/root.md §4.5) plus `ceilings.get` (root/root.md §3.14, with the money block from root/egress.md), passed through unchanged, plus `age_s`. Both are also readable as files in the published view `INIT_HOME/root/` (kernel/sandbox.md `PUB`). The kernel answers from its cache when it is younger than 60 s, otherwise it asks first. No answer → `E_ROOT`. With the control link down the answer is `E_ROOT` even when the cache is fresh (fail closed).

- args: `{}`
- result: `{ledger, ceilings, age_s}`. Every number is an integer.
  - `ledger` = the content of `ledger.json` (root/root.md §4.5): `{level, cap, max_level, supported_max_level, entered_at, entered_seq, evolution_share_max_permille, incident, last_change, next:{level, criteria:[{id, need, have, met}]} | null, seq, v}`. `next` is null at the cap.
  - `ceilings` = `{max_instances, parallel_bodies, cpu_day, disk, init_home, store, money:{…}}`, as the Root defines them.
- errors: `E_ROOT`.

```json
{"args":{},"cmd":"level","id":"3","v":1}
{"id":"3","ok":true,"result":{"age_s":12,"ceilings":{"max_instances":12,"money":{"day_musd":5000000,"total_musd":25000000},"parallel_bodies":1},"ledger":{"cap":0,"entered_at":"2026-10-20T07:55:00.000Z","entered_seq":2,"evolution_share_max_permille":null,"incident":null,"last_change":null,"level":0,"max_level":4,"next":{"criteria":[{"have":7,"id":"min_promotions","met":false,"need":20},{"have":1,"id":"audits_in_window","met":true,"need":1}],"level":1},"seq":311,"supported_max_level":0,"v":1}},"v":1}
```

(The `ceilings` shape is abbreviated: its fields are root/root.md's and root/egress.md's. The money values follow PENDING OWNER CONVENTIONS §6 E1, now set in the charter.)

#### `reload`

T0/T1 reload at the next step boundary. The body does the reload itself (spec §6.1); the kernel relays.

- args: `{"wait"?: bool}` (default false; `true` on the live lane → `E_WOULD_DEADLOCK`)
- result: `{"job": string, "state": "awaiting_boundary"}`
- The kernel sends the body `{"t":"reload","job":…,"kind":"request"}` on stdin (supervise.md §4.5). Reloads of kept components after a data-only promotion (`kind:"kept"`) are kernel-initiated and do not go through initctl. When the body answers `reload_done`, the job becomes `succeeded` (or `failed` if `ok:false`) with `result = {"ok", "prefix_changed", "components", "error"}`. The model sees the outcome as the `reload` event at its next turn (body/log-context.md). No answer within 1800 s → job `failed`, `E_TIMEOUT`.
- If a reload job is already pending, the same job id is returned (idempotent).
- errors: `E_NO_BODY` (no live body), `E_SAFE_MODE`, `E_WOULD_DEADLOCK`.

```json
{"args":{},"cmd":"reload","id":"3","v":1}
{"id":"3","ok":true,"result":{"job":"j-000045","state":"awaiting_boundary"},"v":1}
```

#### `gate` (quick only)

Advisory G0–G3 on a commit, for feedback. Never changes lineage refs other than a transient one.

- args:

| Field | Type | Rule |
|---|---|---|
| `quick` | bool | must be `true`; a full gate runs only through `promote` (`E_ARGS`) |
| `repo` | string | path relative to `INIT_HOME` (or absolute inside it), must resolve (no symlinks) under `body/` or `work/` (probe lane: under the probe root) |
| `rev` | string | a ref name, or a 40-hex commit SHA (e.g. a dirty-tree snapshot made by the CLI, §4.5) |
| `wait` | bool? | default `true` |

- Kernel steps: fetch `rev` from `repo` through `sandboxed-upload-pack` into `refs/init/quick/<job>` (for a bare SHA, the upload-pack wrapper adds `-c uploadpack.allowAnySHA1InWant=true` to its own invocation); run the kernel checks of supervise.md §3.6.1 except "descends from main" (reported as a warning, not an error); build in the slot; run the quick stages (gate.md); delete the transient ref.
- result: `{"job", "sha", "pass": bool, "stages": [{"stage", "pass", "wall_ms", "code"?, "reason"?}], "warnings": [string], "report": "state/kernel/gate/<job>/report.json"}`. Durations are integer ms (ICJ, CONVENTIONS §5); `code` is a `gate.*` code (§4.2)
- `pass:false` is still `ok:true`: the command worked and the gate said no. The CLI exits 1 in that case (§4.5).
- errors: `E_ARGS`, `E_NOT_FOUND` (repo or rev), `E_FETCH`, `E_TREE`, `E_TOO_LARGE`, `E_BUSY` (more than 2 quick gates queued), `E_DISK`, `E_SAFE_MODE`, gate.md codes.

```json
{"args":{"quick":true,"repo":"work/wt-pty","rev":"HEAD","wait":true},"cmd":"gate","id":"4","v":1}
{"id":"4","progress":{"eta_s":null,"job":"j-000046","msg":"fetching","pct":2,"stage":"fetch","state":"running","ts":"2026-10-19T12:40:00.010Z"},"v":1}
{"id":"4","progress":{"eta_s":25,"job":"j-000046","msg":"cargo build (deps cache hit)","pct":20,"stage":"build","state":"running","ts":"2026-10-19T12:40:01.200Z"},"v":1}
{"id":"4","ok":true,"result":{"job":"j-000046","pass":false,"report":"state/kernel/gate/j-000046/report.json","sha":"77aa01…","stages":[{"pass":true,"stage":"G0","wall_ms":31800},{"code":"gate.g1.self_json_drift","pass":false,"reason":"self.json drift: tools[pty].schema","stage":"G1","wall_ms":2100}],"warnings":[]},"v":1}
```

#### `promote`

Gate a branch and, if it passes, hand the conversation to it. On a member lane the same happens on the member's own line (below).

- args: `{"ref": "evo/<id>", "wait"?: bool}` (default `false`; `ref` is `evo/` plus a non-empty id, and the kernel's `git check-ref-format` and id rule, supervise.md §3.6.1, do the rest), or `{"sha": "<40 hex>", "wait"?: bool}` (v0.3, L2, live lane only). `sha` **adopts** a qualified member SHA (`refs/init/qualified/`; kernel/supervise.md §3.14), or any archived SHA (`refs/init/archive/`). The kernel uses the SHA already in lineage, so there is no fetch; gate.md's cached stage results are reused, and G4 re-runs if the system LKG has moved since; and `main` may move non-fast-forward, recorded in lineage. v0.2's owner-lane `allow_foreign` is gone with the owner lane; maintainer commits arrive as seed bundles (kernel/supervise.md §3.11).
- **On a member lane** (L2), the descent check runs against the member's line (`refs/init/lines/<bid>`), not `main`. G0–G3 and the member's handoff run as for the primary, and the member's line moves. G4 runs against the **system LKG**: a pass makes the SHA **qualified** (`refs/init/qualified/<sha>`, no LKG change); a fail or `inconclusive_final` rolls the member back to its previous line. Only the primary's `promote --sha` brings a qualified SHA to the system (evolution.md §3.15.5).
- **Synchronous part** (before the response, < 2 s typical): fetch into lineage, kernel checks (supervise.md §3.6.1), evolution validator (evolution.md §3.9). Any failure is the response.
- **Asynchronous part** (job): build → G0 → G1 → G2 (staging probe) → G3 → [G4, only for synchronous scopes such as `dep`] → `awaiting_boundary` → handoff → `succeeded` (provisional, or LKG after a synchronous G4) | `failed`. An asynchronous G4 is a separate job created at commit (gate.md), and its id appears in `status`. Every LKG commit first stores the binary and components with the Root (`store.put`) and reports `promotion{lkg:true}` (supervise.md §3.6.6).
- **Ordering guarantee:** the kernel sends `pause` to the body only after (1) the gate stages have passed and (2) the final response to every request that started or waits on that job has been written to its socket. That includes the immediate answer of a `promote` with `wait:false`, not only the `wait:true` ones. The caller's tool call therefore always returns before the pause takes effect; the body then pauses at its next step boundary (body/loop.md Q-L1b).
- result (`wait:false`): `{"job", "sha", "state": "queued"}`
- result (`wait:true`): returns at `awaiting_boundary`: `{"job", "sha", "state": "awaiting_boundary", "stages": [...]}`. This answer has no `scope` or `lkg`. The job record's `result` carries them once the job is terminal: `{job, sha, state, scope, stages, handoff, lkg}`. A data-only promotion (supervise.md §3.6.8) has no handoff, so its job goes to `succeeded` without `awaiting_boundary`, with `handoff: false`. A gate failure is `ok:false` with the failing stage's `gate.*` code (or `E_GATE_FAILED` if the gate gave none) and `data: {stage, reason, report}`.
- Idempotency: `promote --sha S` while a job for S is active returns that job. `promote <ref>` while any pipeline runs gets `E_BUSY` with `data.job`, because the SHA is known only after the fetch (the backend may still return the existing job once it knows the SHA). A SHA that passed G0–G3 within the last 3600 s but whose handoff aborted (for example a pause timeout) reuses the cached gate result and goes straight to `awaiting_boundary` (cache owned by gate.md).
- errors: `E_ARGS`, `E_NOT_FOUND`, `E_FETCH`, `E_NOT_DESCENDANT`, `E_FOREIGN_COMMIT`, `E_TREE`, `E_TOO_LARGE`, evolution.md codes (`trailer.*` …, with `details[]`), `E_BUSY`, `E_LANE`, `E_SAFE_MODE`, `E_PROVISIONAL_DEPTH`, `E_DISK`, `E_ROOT`, `E_GATE_FAILED` (wait only).
- Job failure codes after the response: `E_GATE_FAILED`, `E_HANDOFF_FAILED`, `E_PAUSE_TIMEOUT`, `E_CANCELLED`, `E_ROOT` (a synchronous G4 passed but the Root refused `store.put`; `data.root_code`). The calling body also receives them as an `update_failed` notice.
- **`promote --check`** (`{"ref": "evo/<id>", "check": true}`; v0.4, GBAL G7): the synchronous part only, answered at once. It is the same fetch (into a temporary `refs/init/checks/…` ref, deleted again), the same kernel checks, the evolution validator under the same `extend.mode` (the free path's matrix or the gated one) and the scope decision. Nothing is promoted: no job, no quota ticket, no gate, no record, and the pipeline's `E_BUSY` refusals do not apply. Result: `{"check": true, "ok": true, "sha", "parent", "ref", "mode": "free"|"gated", "scope", "stages", "change_type", "summary"}`; a failing commit gets exactly promote's error (`trailer.*` … with `details[]`). `sha` and `wait` are refused with it (`E_ARGS`). The agent runs it before it files a `goal predict` (seed skills/gbal), so an amend forced by the commit rules never makes a filed prediction stale.

```json
{"args":{"ref":"evo/pty-tool"},"cmd":"promote","id":"5","v":1}
{"id":"5","ok":true,"result":{"job":"j-000047","sha":"4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807","state":"queued"},"v":1}
```

```json
{"error":{"code":"trailer.missing:Predicts","data":{"sha":"4b1e0c2d…"},"details":[{"code":"trailer.missing:Predicts","detail":"type feat requires Predicts"},{"code":"trailer.noncanonical_key","detail":"'prefix-change' → 'Prefix-change'"}],"message":"2 problems"},"id":"5","ok":false,"v":1}
```

#### `rollback`

Voluntary return to LKG through the overlapped protocol (supervise.md §3.6.7).

On the free path (`extend.mode = "free"`, ADR-013 G2), when the running version is a change in a change group, `rollback` undoes **that group only**, back to its parent (`to`), not to LKG (which would drop every older open group too). An open group is vetoed (`change.veto`, H12: honoured, never counted); the Root's `extend.rollback` to the parent follows and the kernel performs it (data-only when every change after the parent is t0). A group the Root already closed (for example a guardrail veto still being applied) is rolled back here directly. The result then also carries `group`. A parent that is neither LKG nor reachable data-only falls back to the LKG rollback.

- args: `{"reason": string (≤ 500 bytes), "wait"?: bool}` (default `false`). The reason is required on the wire; the CLI sends `"no reason given"` when `--reason` is absent.
- result: `{"job", "from", "to", "state": "awaiting_boundary"}`
- errors: `E_ALREADY_LKG`, `E_BUSY`, `E_LANE`, `E_SAFE_MODE`.

```json
{"args":{"reason":"G4 will fail: canary pattern in loop.rs"},"cmd":"rollback","id":"6","v":1}
{"id":"6","ok":true,"result":{"from":"4b1e0c2d…","job":"j-000048","state":"awaiting_boundary","to":"0d3e5a9b…"},"v":1}
```

#### `vendor`

The kernel fetches a crate and its new transitive dependencies from crates.io and hands the agent a ready-to-copy `vendor/` delta. Since v0.3 adding crates needs **no approval**. It is an L0 right (spec §15; CONVENTIONS §6 ESC-1, settled 2026-10-20), bounded by the rules below and by the full G4 that the next candidate gets.

- args: `{"crate": string, "version": string, "workspace"?: "body"|"ext", "wait"?: bool}` (defaults `body`, `true`)
  - `crate` matches `^[A-Za-z0-9_-]{1,64}$`; `version` is an exact semver (`^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$`).
- Gating: `E_PROVISIONAL` while the primary is provisional (a provisional body takes no external action); at most 20 vendor jobs per day (`E_QUOTA`). v0.2's `[vendor] require_approval`, `/approve` and `E_DENIED` are removed.
- **Network (D10).** The kernel's only network path is the Root's egress socket (root/root.md §3.3.3), so v0.2's "cargo talks to crates.io" no longer works. The kernel fetches itself, over the egress socket, under egress's built-in **`crates` class**: read-only GETs, no approval, an L0 right (root/egress.md). Cargo's sparse index is served from `index.crates.io` and the `.crate` files from `static.crates.io`; the class allows `GET`/`HEAD` to both on port 443, and the kernel requests them as `/x/crates/<host>/<path>` (root/egress.md §3.5, §4.1). Cargo then runs offline against a **local registry**.
- Kernel procedure (in the kernel's own sandbox; no package code, build script or proc macro runs):
  1. Export lineage `main`'s `Cargo.toml`, `Cargo.lock` and `.cargo/config.toml` to `cache/vendor/<job>/`. Reject (`E_VENDOR_SOURCE`) if `Cargo.toml` has any `git =`, `registry =`, `[patch]`, or a `path =` outside the tree: only crates.io dependencies may be added this way.
  2. Add `<crate> = "=<version>"` under `[dependencies]` (`toml` 1.1.8; a dependency that already exists is replaced). If args say `"workspace": "ext"`, edit `ext/Cargo.toml` instead (its `Cargo.lock` is delivered too).
  3. **Fill a local registry** at `cache/vendor/<job>/registry/` through the egress socket:
     - take the closure of dependency *names* over every index entry whose version matches a requirement (normal, build and target-specific dependencies; dev-dependencies of workspace members only);
     - fetch each name's sparse-index file (`https://index.crates.io/<prefix>/<name>`) into `registry/index/`;
     - stop after `vendor_max_index_files` = 400 files (`E_TOO_LARGE`).
  4. **Resolve offline.** Run `cargo metadata --offline --format-version 1` with `CARGO_HOME=cache/vendor/<job>/cargo-home` (fresh, so no owner config or credentials) and `--config 'source.crates-io.replace-with="init-local"' --config 'source.init-local.local-registry="cache/vendor/<job>/registry"'`. That updates `Cargo.lock` minimally and keeps existing pins. (v0.2's `generate-lockfile` re-resolved everything.)
  5. **Fetch exactly the new `.crate` files** the lock names, from `static.crates.io` over the egress socket. Check each against the index `cksum`, and place it in `registry/`.
  6. `cargo vendor --offline --locked --versioned-dirs [--sync ext/Cargo.toml] cache/vendor/<job>/vendor`, with the same config overrides. Timeout 300 s for steps 3–6. `--sync ext/Cargo.toml` is added when lineage `main` has `ext/Cargo.toml`, so the body and the ext workspace share one `vendor/` (ext/authoring.md, ext/host.md).
  7. Diff against the crates already in lineage `main`'s `vendor/`; copy only new or changed crate directories plus the new `Cargo.lock` to `state/kernel/vendor/<job>/`. Size cap 200 MiB (`E_TOO_LARGE`).
  8. Append `vendor{crate, version, added:[{name, version, checksum}]}` to `vault/lineage.jsonl`. gate.md can verify a candidate's vendored checksums against these records.

  Steps 3–5 are UNVERIFIED as a whole. That a cargo `local-registry` resolves with index files only, before the `.crate` files exist, is to be confirmed at build time (test IC-14b). The fallback is to fetch every `.crate` of the name closure's matching versions before step 4, which costs more bandwidth and is bounded by the same caps.
- result: `{"job", "crate", "version", "added": [{"name", "version", "checksum"}], "lock": "state/kernel/vendor/<job>/Cargo.lock", "vendor_dir": "state/kernel/vendor/<job>/vendor", "note": string}`
- The agent copies the delta into its worktree and commits. The next candidate that changes `Cargo.lock` or `vendor/` gets full G4 (spec §7.3, gate.md).
- errors: `E_ARGS`, `E_PROVISIONAL`, `E_QUOTA`, `E_VENDOR_SOURCE`, `E_VENDOR_FETCH` (crate or version not found, egress refused, network error), `E_TOO_LARGE`, `E_DISK`, `E_LANE`, `E_ROOT`.

```json
{"args":{"crate":"vt100","version":"0.16.2"},"cmd":"vendor","id":"7","v":1}
{"id":"7","ok":true,"result":{"added":[{"checksum":"8a1b…","name":"vt100","version":"0.16.2"},{"checksum":"2c9d…","name":"vte","version":"0.15.0"}],"crate":"vt100","job":"j-000049","lock":"state/kernel/vendor/j-000049/Cargo.lock","note":"copy vendor/* and Cargo.lock into your worktree; the next candidate runs full G4","vendor_dir":"state/kernel/vendor/j-000049/vendor","version":"0.16.2"},"v":1}
```

(The crate names and versions in this example are illustrative, not checked against crates.io.)

#### `kernel propose` (alias `propose-kernel`)

File a change to the kernel, the Root or the charter as a **proposal**. Before L3 this is the only way to change the kernel. At any level it is the only way to propose a Root or charter change (spec §13: Root changes come only from the maintainer or the owner). It is never applied automatically, and nobody is asked to acknowledge it.

- args: `{"title": string (≤ 200 bytes), "patch": string (path under body/ or work/), "rationale": string (≤ 8 KiB)}`
- The kernel reads the patch with `O_NOFOLLOW` (≤ 1 MiB before encoding, regular file, UTF-8). It keeps `state/kernel/proposals/kp-<4 digits>/{proposal.json, patch.diff}` and sends `proposal.file{title, text}` to the Root, where `text` = rationale + patch, capped at 64 KiB (`E_TOO_LARGE` beyond). The Root records it (lineage kind `proposal`), lists it in `init-root status --proposals`, and alerts the owner (root/root.md §3.10.5, §4.3). The local files are written after the Root accepts. If that write then fails, the answer is `E_INTERNAL`, and its message names the Root's id.
- result: `{"proposal": "kp-0007", "root_id": string, "status": "filed", "path": "state/kernel/proposals/kp-0007/"}`
- At most 5 per day, enforced by both the kernel and the Root (`E_QUOTA`, `data.quota:"proposals_day"`).
- errors: `E_ARGS`, `E_NOT_FOUND`, `E_TOO_LARGE`, `E_QUOTA`, `E_LANE`, `E_ROOT`.

v0.2's statuses `acknowledged` and `declined` (owner `/approve`, `/deny`) are removed.

#### `kernel promote`, `kernel status` (v0.3, L3)

Promote a kernel the agent wrote. evolution.md §3.15.6 owns the flow, gate.md the `k` scope (K0–K3), and kernel/supervise.md §3.15 the kernel-side steps. This is the wire shape.

- args: `{"ref": "kevo/<id>", "wait"?: bool}` (default `false`). `ref` names a branch in `work/kernel/` (`^kevo/[a-z0-9][a-z0-9._-]{0,62}$`).
- Level ≥ 3, or `E_LEVEL` without contacting the Root. A `system.swap` below L3 would be a demoting ceiling hit.
- Clean start: refused while the primary is provisional (`E_PROVISIONAL`), while a G4 runs, or while another kernel candidate or a post-swap sealed audit is pending (`E_BUSY`, `data.reason:"kernel_candidate"`).
- **Synchronous part:** fetch into `lineage/kernel-cand.git` (the Root writes `lineage/kernel.git`, D7), the checks of supervise.md §3.6.1, and evolution.md's validator.
- **Asynchronous part:** a gate `kernel` job:
  1. K0 → K1 → K2 → K3 on the running kernel's gate;
  2. `store.put` of two binaries and the source bundle;
  3. `system.swap`, after which the job is `awaiting_root` (the gate-of-gates; `swap` events become job stages);
  4. `awaiting_swap` (on `gog_pass`), then the swap (`prepare`, `installed`, `ready`, `probation`), then `succeeded` or `failed`.
- `wait:true` returns at the gate-of-gates verdict: `awaiting_swap`, or `E_GATE_FAILED{stage:"gate_of_gates"}`.
- result (`wait:false`): `{"job", "sha", "state": "queued"}`; (`wait:true`): `{"job", "sha", "state": "awaiting_swap", "root_job": "s-NNNNNN", "stages": [...]}`.
- errors: `E_ARGS`, `E_NOT_FOUND`, `E_FETCH`, `E_NOT_DESCENDANT`, `E_FOREIGN_COMMIT`, `E_TREE`, `E_TOO_LARGE`, `E_LEVEL`, `E_SAFE_MODE`, `E_PROVISIONAL`, `E_BUSY`, `E_DISK`, `E_ROOT`, `E_LANE`, evolution.md validator codes.
- Job failure codes:
  - K0–K3: `E_GATE_FAILED` with that stage's `gate.k*.*` code (kernel/gate.md);
  - the gate-of-gates: `E_GATE_FAILED{stage:"gate_of_gates"}` (the Root also demotes);
  - after it: `E_KUPGRADE_FAILED{stage}` with `stage` ∈ `store`, `prepare`, `boot`, `probation`. `boot` and `probation` failures are reported by the Root after it has restored the previous tuple, and are not demotions.

`kernel status` — args `{}`; result `{"candidate": {job, sha, state, stages, root_job} | null, "running": {version, built_from, source}, "last_audit": {at, regression} | null}`, from the job records, the boot block and the Root's view.

#### `task template`, `task submit`, `task list`, `task show` (v0.3)

Agent-authored tasks (spec §17). From L1 the agent writes tasks into the gate's pool, so the pool stops depending on the owner. The admission rules are root/sealed-eval.md's, enforced by kernel/gate.md (an `admit` job):
- determinism;
- non-triviality;
- **families** of at least 2 instances, of which a random half is sealed at admission, and the agent sees and develops on the visible half only (D2);
- pre-registration anchored to the Root lineage `seq`;
- exclusion of the author's descendants within K = 5 promotions (D3);
- provenance.

| Command | args | result | Level |
|---|---|---|---|
| `task template` | `{}` | a skeleton task family (`task.json` v1 per instance, kernel/gate.md §3.7.1) for the agent to fill in under `work/` | L0 |
| `task submit` | `{"dir": string (path under work/ or body/), "wait"?: bool}` (default `true`) | a gate `admit` job. The kernel reads the directory per K-F2, and gate.md runs admission (staging copy, rules, sealing of half the family). On pass the kernel registers it with `lineage.append` (`task_admit`, kernel/gate.md §3.7.5); the returned `seq` is the registration anchor. Result (kernel/gate.md Q-IC2): `{"job", "family", "seq", "visible": [instance ids], "sealed": int, "admitted": bool, "checks": [{"rule", "ok", "detail"}]}`; `sealed` is a count, never the ids | **L1** |
| `task list` | `{}` | the agent's families: `[{family, size, sealed, admitted_at, seq, eligible_from_lkg_seq, suspended: reason \| null}]` | **L1** |
| `task show` | `{"family": string}` | one task with its **visible** instances only (prompts, checks, visible outcomes). It never shows sealed instances or another set's tasks (evolution.md §4.1) | **L1** |

- `task submit` quota: `budget.task_submits_day` (kernel/config.md, default 10; `E_QUOTA{quota:"task_submits_day"}`). evolution.md calls it `tasks_day` = 20; this doc uses config.md's key (§9).
- A failed admission is `ok:true` with `admitted:false` and the failing rules (CLI exit 1).
- errors: `E_ARGS`, `E_NOT_FOUND`, `E_LEVEL`, `E_QUOTA`, `E_SAFE_MODE`, `E_DISK`, `E_ROOT`, `E_ADMISSION` (an admission infrastructure error, not a rule failure), `E_LANE`, and the `gate.admit.*` codes (kernel/gate.md).

#### `budget set`, `model set` (v0.3, L1)

| Command | args | result |
|---|---|---|
| `budget set` | `{"set": {<key>: <value>…}}`, keys and bounds per evolution.md §3.14 (soft budgets within the level's freedom). A value is an integer, or a string for the decimal values that config keeps as strings (ICJ has no floats); the CLI sends an integer when the text parses as one | `{"budgets": {...}}` after `ProxyHandle::set_budgets` (kernel/proxy.md). A bound violation, for example a sum above the soft day budget, returns `E_BUDGET` with `data:{key, limit}`. Soft budgets are always clamped below the Root's ceilings (D6) |
| `model set` | `{"model": "<provider>/<id>"}`, among the charter's allowed models | `{"model", "thinning_job"}`: switches the main model and starts thinning (evolution.md §3.12). Errors: `E_ARGS` (not allowed by the charter), `E_BUSY` (a G4 or thinning baseline runs) |

Both need L1 (`E_LEVEL` below) and are primary-lane only.

#### `fleet list`, `fleet spawn`, `fleet send`, `fleet stop` (v0.3, L2)

Fleet control on the primary's lane (evolution.md §3.15.5 owns the policy: HGM parent selection and share growth; kernel/supervise.md §3.14 the mechanics).

| Command | args | result | Level |
|---|---|---|---|
| `fleet list` | `{}` | `{members: [{bid, line, busy, share_musd_day, spent_today_musd, submissions, qualified}], candidates: [{sha, kind: "archive"\|"qualified", cmp_permille, n_s, n_f}], suggested_parent}`: the archive and qualified SHAs with their HGM scores (evolution.md §3.15.5), integers only | L0 |
| `fleet spawn` | `{"parent"?: "main" \| "<40 hex>", "prompt": string, "budget_share_musd_day"?: int}` | `{"bid", "job"}`. The job builds the parent if needed (G0 only), creates the member's root, opens its `member` proxy scope and spawns it (kernel/supervise.md §3.14). The share defaults to `fleet.default_share_musd_day`, is adjusted by evolution.md's halving rule, and is clamped as kernel/proxy.md §3.8 says (`E_BUDGET` beyond it) | **L2** |
| `fleet send` | `{"bid": string, "text": string}` (≤ 64 KiB) | `{"queued": true}`; delivered at the member's next step boundary | **L2** |
| `fleet stop` | `{"bid": string \| "all", "reason"?: string}` | `{"stopping": [bid…]}`; members stop at their next step boundary, their roots are kept 7 days | any |

- Quota: `fleet_spawns_day` = 50 (`E_QUOTA`). The body count is checked before any `spawn` reaches the Root (`E_CEILING`), because a Root refusal would demote (§3.5).
- Members cannot create, message or stop members (`E_LANE`), so the recursion is bounded.
- errors: `E_ARGS`, `E_NOT_FOUND` (unknown `bid` or parent SHA), `E_LEVEL`, `E_CEILING`, `E_QUOTA`, `E_BUDGET`, `E_SAFE_MODE`, `E_DISK`, `E_ROOT`, `E_LANE`.

#### `job.status`, `job.wait` (alias `wait`), `job.list`, `job.cancel`

| Command | args | result |
|---|---|---|
| `job.status` | `{"job"}` | the job record (§4.4) |
| `job.wait` / `wait` | `{"job"? , "last"?: bool, "cmd"?: string, "timeout_s"?: int}` (default 600, max 3600). Exactly one of `job` or `last:true`; `last` picks the newest job of this lane, optionally of command `cmd` | progress frames, then the record when it is terminal, at `awaiting_boundary` or at `awaiting_swap`; on timeout the current record with `"timed_out": true` |
| `job.list` | `{"state"?: "active"\|"all", "limit"?: int ≤ 200}` (default active, 20) | `{"jobs": [record…]}`, newest first |
| `job.cancel` | `{"job"}` | the record with `state: "cancelled"` |

errors: `E_NOT_FOUND`, `E_NOT_CANCELLABLE`, `E_LANE` (a job of another lane).

#### Owner signals (no initctl command)

v0.2's owner-only commands on `run/owner.sock` are gone:
- `approve` and `deny`: no approval flow remains;
- `panic`, `resume` of a panicked or halted system, and `clear-safe`: the Root's (`init-root`, root/root.md §4.3).

What remains are optional owner lines. The Root relays them as `owner.in`, and the kernel parses them (kernel/supervise.md §4.6): `/stop`, `/resume` (the kernel's own `Stopped`/`Halted`), `/status`, `/accept <task_id>`, `/bless <job>` and `/reject <job>`. They are not initctl commands, and no body can send them: the owner endpoint is outside every sandbox.

### 4.2 Error codes

| Code | Meaning | Retry? | CLI exit |
|---|---|---|---|
| `E_PROTO` | malformed frame, line too long, not JSON | no | 76 |
| `E_VERSION` | unsupported `v` | no | 76 |
| `E_UNKNOWN_CMD` | unknown `cmd` | no | 76 |
| `E_ARGS` | invalid or missing args | no | 64 |
| `E_WOULD_DEADLOCK` | `wait:true` on a job that needs the caller's own step boundary | no | 64 |
| `E_PEER` | peer authentication failed | no | 77 |
| `E_LANE` | command not available on this lane, or job of another lane | no | 77 |
| `E_HALTED` | the kernel is in its own `Halted` | after owner `/resume` | 77 |
| `E_SAFE_MODE` | body or system safe mode | after the Root's safe-mode clear (24 h, or `init-root resume --clear-safe`) | 77 |
| `E_LEVEL` | the autonomy level does not grant this command; `data: {need, have}`. Refused locally, never sent to the Root (where it would demote) | after a level-up | 77 |
| `E_CEILING` | a Root ceiling would be exceeded; `data: {ceiling, limit, used}` (e.g. `parallel_bodies`). Refused locally for the same reason | after the ceiling rises or usage falls | 77 |
| `E_PROVISIONAL` | not allowed while provisional | after LKG commit | 77 |
| `E_PROVISIONAL_DEPTH` | provisional chain too deep | after G4 | 77 |
| `E_BUSY` | conflicting pipeline/handoff/queue full. `data.job` names the blocking job; `data.reason` ∈ `pipeline`, `handoff`, `queue_full`, `thinning_baseline` (evolution.md: promotes wait while a thinning baseline G4 runs), `kernel_candidate` (evolution.md §3.15.6) | yes | 75 |
| `E_RATE` | rate or connection limit | yes, after 1 s | 75 |
| `E_QUOTA` | a count quota is used up. `data.quota` ∈ `promote_attempts_task`, `promotes_day`, `quick_gates_day` (evolution.md, `[budget]` count keys), `vendor_day` (20), `proposals_day` (5), `task_submits_day` (`budget.task_submits_day`, kernel/config.md, default 10), `fleet_spawns_day` (50); `data.limit`, `data.resets_at` | after the reset | 75 |
| `E_BUDGET` | `budget set` violates a bound of evolution.md §3.14 (for example the sum exceeds the soft day budget); `data: {key, limit}` | after a fix | 1 |
| `E_ROOT` | the control link is down, the cached level or ceilings are stale, the outbox is backed up (`data.reason`), or the Root refused a request (`data.root_code`, a dotted `root.*` code from root/root.md §4.1.2) | yes | 75 |
| `E_DISK` | free space below threshold | after cleanup | 75 |
| `E_SHUTTING_DOWN` | kernel is stopping | yes | 75 |
| `E_NO_BODY` | no live body to deliver to | yes | 75 |
| `E_NOT_FOUND` | ref, repo, path, job or proposal not found | no | 1 |
| `E_FETCH` | git fetch failed or timed out | maybe | 1 |
| `E_NOT_DESCENDANT` | candidate does not descend from lineage `main` | after rebase | 1 |
| `E_FOREIGN_COMMIT` | a commit in range is not by the agent identity and not upstream | no | 1 |
| `E_TREE` | submodule, escaping symlink, too many entries | no | 1 |
| `E_TOO_LARGE` | size limit (range, tree, patch, vendor) | no | 1 |
| `E_GATE_FAILED` | a gate stage failed and the gate reported no specific `gate.*` code; `data: {stage, reason, report}` | after a fix | 1 |
| `E_HANDOFF_FAILED` | the new body did not take over; old body resumed | after a fix | 1 |
| `E_PAUSE_TIMEOUT` | the body did not reach a step boundary | yes | 1 |
| `E_ALREADY_LKG` | rollback requested while on LKG | no | 1 |
| `E_VENDOR_SOURCE` | non-crates.io source in `Cargo.toml` | no | 1 |
| `E_VENDOR_FETCH` | crate/version not found, egress refused, or network error | maybe | 1 |
| `E_ADMISSION` | task admission could not run (infrastructure); a rule failure is `admitted:false` instead | maybe | 1 |
| `E_KUPGRADE_FAILED` | a kernel swap failed after the gate-of-gates passed; `data: {stage, reason}` with `stage` ∈ `store` `prepare` `boot` `probation`. K0–K3 and gate-of-gates failures are `E_GATE_FAILED` | after a fix | 1 |
| `E_NOT_CANCELLABLE` | job past its cancellable stages | no | 1 |
| `E_CANCELLED` | job was cancelled | no | 1 |
| `E_TIMEOUT` | job deadline passed (e.g. reload 1800 s) | maybe | 1 |
| `E_INTERNAL` | kernel bug; details in `kernel.jsonl` | maybe | 70 |
| `trailer.*`, `predicts.*`, … | evolution.md §4.2 validator codes (dotted) | after a fix | 1 |
| `gate.*` | kernel/gate.md §4.1 error-code table, registered here by reference: `gate.build.*`, `gate.g0.*`…`gate.g4.*`, and since v0.3 `gate.admit.*` (task admission) and `gate.k0.*`…`gate.k3.*` (kernel candidates; kernel/gate.md Q-IC2). G4's verdicts include `inconclusive_final` (D4: after one escalated re-run the incumbent wins); `gate.g4.needs_owner` is removed. When a stage fails, the response or job record carries that stage's `gate.*` code as `error.code` (with `error.data: {stage, reason, report}`), and each failed stage carries it in `stages[].code` | after a fix | 1 |

### 4.3 Progress frame fields

| Field | Type | Meaning |
|---|---|---|
| `job` | string | job id |
| `state` | `queued` `running` `gating` `awaiting_boundary` `handing_off` `awaiting_root` `awaiting_swap` | |
| `stage` | `fetch` `validate` `build` `G0` `G1` `G2` `G3` `G4` `vendor` `registry` `pause` `handoff` `admission` `register` `K0` `K1` `K2` `K3` `store` `gate_of_gates` `prepare` `installed` `ready` `probation` `member` | v0.3 removed `approval` |
| `pct` | int 0–100 | monotone per job |
| `eta_s` | int \| null | |
| `msg` | string ≤ 200 bytes | e.g. the cargo step |
| `ts` | rfc3339 | |

### 4.4 Job record (`state/kernel/jobs/<id>.json`)

```json
{"args":{"ref":"evo/pty-tool"},"cmd":"promote","created_at":"2026-10-19T12:31:00.000Z","error":null,"finished_at":null,"id":"j-000047","lane":"live","progress":{"eta_s":40,"msg":"probe: waiting for ready","pct":55,"stage":"G2","state":"gating","ts":"2026-10-19T12:33:10.000Z"},"result":null,"sha":"4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807","stages":[{"pass":true,"stage":"G0","wall_ms":38000},{"pass":true,"stage":"G1","wall_ms":2000}],"state":"running","v":1}
```

`state` ∈ `queued` `running` `awaiting_boundary` `awaiting_root` `awaiting_swap` `succeeded` `failed` `cancelled`. A `kernel promote` record also carries `root_job` (the Root's `s-NNNNNN`). `error` has the shape of §3.2. `result` is the command's result object once terminal. `lane` is `live`, `member` or `probe`. `progress` has a progress frame's fields without `job`, because the record's own `id` names it. Two fields are optional and absent otherwise: `reason`, why the job ended when no command error says so (for example `kernel_restart`, §3.7), and `timed_out: true`, set only on a `job.wait` answer that timed out and shows the current record.

### 4.5 The `initctl` CLI

Binary `$INIT_HOME/kernel/bin/initctl` (crate `initctl`: std + `init-proto`; no async runtime, no network crates). It is on the sandbox `PATH`.

```
initctl [--sock PATH] [--json] [--quiet] [--timeout SECS] <command> [args]

  status
  ping
  reload [--wait]                      --wait on the live lane gives E_WOULD_DEADLOCK
  gate --quick [<path>|<ref>] [--no-wait]   default: the git toplevel of the current directory
  promote <branch> [--wait]          <branch> = evo/<id>
  rollback [--reason TEXT] [--wait]
  promote --sha <40 hex> [--wait]     L2: adopt a qualified member SHA (primary lane)
  vendor <crate>@<version> [--no-wait] [--workspace body|ext]
  level                                level, cap, next-level checks, ceilings
  task template > work/tasks/<name>/family.json
  task submit <dir> [--no-wait]        L1
  task list | task show <family>       L1
  budget set <key>=<value>…            L1
  model set <provider>/<id>            L1
  fleet list
  fleet spawn [--parent SHA] --prompt-file FILE [--share-musd-day N]   L2
  fleet send <bid> --text TEXT       L2
  fleet stop (<bid> | --all) [--reason TEXT]
  kernel propose <patch-file> --title TEXT [--rationale-file FILE]   (alias: propose-kernel)
  kernel promote <kevo/id> [--wait]    L3; --wait returns at the gate-of-gates verdict
  kernel status                        L3
  wait (<job> | --last [--cmd NAME]) [--timeout SECS]   alias of: job wait
  job status <job> | job wait <job> [--timeout SECS] | job list [--all] | job cancel <job>
  help | --help | --version
```

**Socket selection:** `--sock`, else `$INIT_KERNEL_SOCK`, else `$INIT_HOME/run/kernel.sock`. Connect timeout 2 s. v0.3 removed the `owner.sock` fallback: outside the sandbox the owner uses `init-root`, and a process outside every body instance fails peer attribution (`E_PEER`).

**Arguments the CLI fills in or checks.** Relative paths for `task submit <dir>` and `kernel propose <patch>` are sent absolute (joined to the current directory, symlinks not resolved), so the kernel does not depend on the agent's cwd. `--prompt-file` (the `fleet spawn` prompt, at most 64 KiB) and `--rationale-file` (at most 8 KiB) are read by the CLI. `vendor --workspace` is the wire's `workspace`. `rollback` without `--reason` sends `"no reason given"`. `budget set` sends an integer when the value parses as one, else a string.

**`gate --quick` argument resolution** (all inside the sandbox, as the agent, using the `git` CLI):

1. No argument: `repo = git rev-parse --show-toplevel` of the current directory, `rev = HEAD`.
2. An existing directory: `repo` = that directory's toplevel, `rev = HEAD`.
3. Otherwise: a ref in `$INIT_HOME/body`.
4. If `repo` has uncommitted or untracked (not ignored) changes (`git status --porcelain` non-empty), snapshot them without touching the index, the worktree or any ref:

```sh
export GIT_INDEX_FILE="$TMPDIR/initctl-snap-$$.idx"
git -C "$REPO" read-tree HEAD
git -C "$REPO" add -A
TREE=$(git -C "$REPO" write-tree)
SNAP=$(git -C "$REPO" commit-tree "$TREE" -p HEAD -m "initctl: quick-gate snapshot")
rm -f "$GIT_INDEX_FILE"
```

   and send `rev = $SNAP`. The commit uses the agent identity from the environment.

**Output.**

- stdout gets exactly one pretty-printed JSON document with sorted keys: `result` on success, or `{"error": {…}}` on failure. This is easy for the model to read and for scripts to parse.
- Progress goes to stderr as `[j-000047 G2 55% eta 40s] probe: waiting for ready` (one line per progress frame), unless `--quiet`.
- `--json` prints every raw frame (progress and final) to stdout instead, one per line.
- Local failures (a usage error, no socket configured, a failed `git` call) print one line to stderr and leave stdout empty. With none of `--sock`, `INIT_KERNEL_SOCK` and `INIT_HOME` set, the exit code is 64.

**Exit codes** (sysexits-style, matching testing.md's use of 69):

| Code | Meaning |
|---|---|
| 0 | success (including `wait` returning at `awaiting_boundary` or `awaiting_swap`) |
| 1 | the command ran and failed: `ok:false` with a domain code (including `E_BUDGET`), `gate` with `pass:false`, or `task submit` with `admitted:false` |
| 2 | `wait`/`--timeout` expired; the job is still running |
| 64 | usage error (bad CLI arguments, `E_ARGS`, `E_WOULD_DEADLOCK`) |
| 69 | cannot connect, or the connection dropped before the final response |
| 70 | `E_INTERNAL` |
| 75 | temporary: `E_BUSY`, `E_RATE`, `E_QUOTA`, `E_DISK`, `E_SHUTTING_DOWN`, `E_NO_BODY`, `E_ROOT` |
| 76 | protocol: `E_PROTO`, `E_VERSION`, `E_UNKNOWN_CMD`, unparsable response |
| 77 | not permitted now: `E_PEER`, `E_LANE`, `E_HALTED`, `E_SAFE_MODE`, `E_PROVISIONAL`, `E_PROVISIONAL_DEPTH`, `E_LEVEL`, `E_CEILING` |

`--timeout` on `wait` is the wire's `timeout_s` (default 600), and the client's own deadline is `timeout_s` + 30 s. On any other command `--timeout` is the client's deadline, and expiry exits 2. A `wait` that returns a `failed` or `cancelled` record exits with that job error's code, or 1 when the record has no error code (for example after `kernel_restart`).

**Example session** (from the agent's `bash` tool):

```
$ initctl promote evo/pty-tool
{
  "job": "j-000047",
  "sha": "4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807",
  "state": "queued"
}
$ initctl wait j-000047 --timeout 600
[j-000047 build 20% eta 25s] cargo build (deps cache hit)
[j-000047 G0 35% eta 50s] cargo clippy -D warnings
[j-000047 G2 60% eta 40s] probe: waiting for ready
[j-000047 G3 80% eta 90s] evolvability self-edit
{
  "id": "j-000047",
  "state": "awaiting_boundary",
  ...
}
$ echo $?
0
```

The handoff then happens at the body's next step boundary. The next model turn starts with a `boot` event, reason `promote`.

## 5. Failure modes and recovery

| Failure | Effect | Handling |
|---|---|---|
| kernel down / socket missing | `connect` fails | CLI exit 69; the body's tool result shows it; the Root restarts the kernel (root/root.md §3.6) |
| kernel restarts mid-request | connection drops | CLI exit 69; jobs are marked `failed` with `reason:"kernel_restart"` at cold start (evolution.md F15) and the record says so |
| `initctl` killed by the bash timeout | none for the job | `initctl wait <job>` reattaches |
| request too long / garbage | `E_PROTO`, connection closed | — |
| flood from a hostile body | `E_RATE`, connection cap | other lanes unaffected (per-lane limits) |
| peer cannot be attributed | `E_PEER` | logged; the socket path should have prevented it, so a log entry is a sandbox-bug signal |
| response would exceed 1 MiB | the kernel truncates arrays and adds `"truncated": true`; the full data stays in the job record/report files | — |
| two promotes at once | the second gets `E_BUSY` with `data.job` | the agent waits on that job |
| control link down or level/ceilings stale | level-gated and Root-needing commands get `E_ROOT` | retry after the Root reconnects; nothing is sent to the Root on stale data (a level-gated refusal there demotes) |
| the agent asks above its level | `E_LEVEL` or `E_CEILING`, locally | no demotion, because the request never reached the Root (kernel/supervise.md §3.16) |
| a kernel candidate fails the gate-of-gates | `E_KUPGRADE_FAILED{stage:"gog"}` on the job; the Root demotes (mutation-suite failure, spec §15) | the agent sees `notice{level}` and the job record |
| `vendor`'s egress class refuses | `E_VENDOR_FETCH` (`data.root_code`) | an egress refusal of a policy class may count as a ceiling hit (root/egress.md), so the kernel keeps `vendor` inside crates.io GETs only |

## 6. Performance notes (spec §9)

- **Rank 2 (round trips):** `promote`, `kernel promote` and `fleet spawn` default to non-blocking, so the model can keep working while the gate or the Root runs, and `wait` replaces polling loops (one tool call instead of many).
- **Rank 3 (tool path):** `initctl` is a small std binary: connect, one line out, lines in. Startup is a few milliseconds, the same order as any exec in the sandbox (measured `sandbox-exec` child exec ≈ 3 ms on M2). `status` and `level` are answered from memory without touching disk (`level` asks the Root at most once a minute).
- Progress is rate-limited to 1 frame/s so a long gate does not flood the model's context; `--quiet` drops it entirely.
- Job records are rewritten only on state changes, not on every progress tick.

## 7. Invariants and security (spec §2.1; v0.3 §13–§18)

Since v0.3 the security boundary is the Root's (spec §13). initctl is a stability layer: it keeps each body inside its lane and makes every level and ceiling refusal immediate and explained. The Root re-checks what matters.

- **Recoverability (R1):** no command can write `kernel/`, `root/`, `bin/`, `lineage/` or `state/kernel/` on the agent's behalf except through the supervised pipeline. `kernel propose` only files text. `kernel promote` only reaches `system.swap`, after which the Root alone judges, installs and can revert. No initctl command can panic, resume or clear safe mode: those are the Root's, behind an endpoint no sandbox can reach.
- **Honest stick (R6):** `gate`, `promote` and `kernel promote` take a ref or SHA, never test results. The kernel fetches by SHA into lineage, builds and gates in its own instances, and reports. `task submit` adds tasks only through mechanical admission with a Root-recorded registration anchor, and never touches the sealed set. Nothing a client sends can mark a stage passed.
- **Blast radius (R2, R3):** `vendor` is the only command that reaches the network for the agent: crates.io GETs only, through the Root's egress, exact versions, no package code run, quota-limited and refused while provisional. No approval is needed (an L0 right).
- **Lineage (R4):** every promote leaves `refs/init/archive/<sha>` and a `vault/lineage.jsonl` record, whatever the outcome. Promotions, verdicts, rollbacks and task admissions also reach the Root's signed chain.
- **Humans (R3, R5):** no initctl command waits for a human. The owner's remaining signals (`/accept`, `/bless`, `/reject`, `/stop`) are optional lines from the Root.
- **Level gating:** the kernel refuses level-gated commands locally (`E_LEVEL`, `E_CEILING`) and fails closed on a stale ledger (`E_ROOT`). The Root enforces the same limits again and demotes on a breach (root/root.md §3.13.3).
- Input hardening: all paths in args are resolved by the kernel with `O_NOFOLLOW` and checked to stay under the allowed roots; refs are checked with `git check-ref-format`; every string has a length cap; JSON depth is capped at 32. A relative path is taken from the lane's home: `INIT_HOME`, or the probe root on a probe lane (the backend names a member's roots). A path outside the allowed roots, one with a `..` component, a non-UTF-8 text file, or a directory where a file is wanted (or the reverse) is `E_ARGS`. A symlink in any component, or a missing path, is `E_NOT_FOUND`. A patch over 1 MiB, or a rationale and patch over 64 KiB together, is `E_TOO_LARGE`.

## 8. Test plan

| ID | Case | Expect | Maps to |
|---|---|---|---|
| IC-1 | golden frames: every example in §4.1 round-trips through `init-proto` with sorted keys | byte-equal | — |
| IC-2 | request line of 64 KiB + 1 | `E_PROTO`, connection closed | — |
| IC-3 | `v:2` | `E_VERSION` | — |
| IC-4 | each command from each lane, at levels 0–3 (`init-fakeroot`) | matches the §3.4 matrix; below-level commands get `E_LEVEL` and `init-fakeroot` sees **no** request | A6 |
| IC-5 | connect to `run/kernel.sock` from a process outside the live instance (e.g. a probe instance with a test hole in the profile) | `E_PEER` | A6 |
| IC-6 | connect to the Root's owner or egress socket from the live instance | denied by the sandbox (`EPERM` on macOS; `ENOENT` on Linux) | **A6** |
| IC-7 | `reload` with `wait:true` on live lane | `E_WOULD_DEADLOCK`, exit 64 | — |
| IC-8 | `promote --wait` from the fake body | returns at `awaiting_boundary`, exit 0; handoff follows at the boundary | A3 |
| IC-9 | kill `initctl` mid-gate, then `initctl wait` | job continues; wait returns the final record | — |
| IC-10 | second `promote` during a pipeline | `E_BUSY` with `data.job`, exit 75 | — |
| IC-11 | `gate --quick` on a dirty worktree | snapshot SHA gated; worktree, index and refs unchanged (`git status` identical before/after) | — |
| IC-12 | Linux: `SO_PEERCRED` pid of a process in a sibling bwrap pid namespace | logged as 0 (expected); attribution by socket path still correct | — |
| IC-13 | `vendor` while provisional | `E_PROVISIONAL`, exit 77 | — |
| IC-14 | `vendor` with a `git =` dependency in lineage `Cargo.toml` | `E_VENDOR_SOURCE` | A6 |
| IC-14b | `vendor` against a fake egress serving a recorded sparse index and `.crate` files | `cargo metadata --offline` resolves against the local registry with index files only (else the fallback path); checksums match; no request leaves the egress class | — |
| IC-15 | 9 concurrent connections | 9th gets `E_RATE` | — |
| IC-16 | `init-fakeroot` kills the kernel (panic), then any agent command | CLI exit 69 (no kernel); after `panic_resume`, commands work again | A5 |
| IC-17 | CLI exit-code table: one case per row of §4.5 | exact codes | — |
| IC-18 | `kernel propose` with a symlinked patch path | `E_NOT_FOUND` (O_NOFOLLOW) | A6 |
| IC-19 | `wait --last` and `wait --last --cmd promote` | newest job of the lane (of that command) | — |
| IC-20 | `gate --quick` from a probe lane with purpose `run` | `E_LANE`, exit 77 | — |
| IC-21 | `status` | every number is an integer; `constitution_sha256` matches the installed file; `root` (byte-equal to the Root's status), `link`, `fleet` and `budget.soft` present | — |
| IC-22 | `promote --wait` while the fake body records frame order | final response written before `pause` arrives on stdin | A3 |
| IC-23 | `level` with a fresh cache; with a 61 s old cache; with the link down | from cache; one `ledger.get` + `ceilings.get`; `E_ROOT` | — |
| IC-24 | `task submit` at L0; at L1 with a single-instance task; with a non-deterministic family; with a good family of 4 | `E_LEVEL`; `admitted:false` (families need ≥ 2 instances, D2); `admitted:false` with the failing rule; admitted, half the instances sealed, `visible` lists the other half, `sealed` = 2, `seq` returned | — |
| IC-25 | `fleet spawn` at L2 with `parallel_bodies=2`, twice; the 51st `fleet spawn` of a day; `fleet spawn` and `fleet send` from a member lane; `fleet stop` at L0 | ok; `E_CEILING` with no `spawn` sent; `E_QUOTA{quota:"fleet_spawns_day"}`; `E_LANE`; ok | — |
| IC-26 | `kernel promote` at L2; at L3 (`init-fakeroot`) through the gate-of-gates | `E_LEVEL` with no `system.swap` sent; K0–K3, `store.put` ×3, `system.swap`; `wait` returns at `awaiting_swap` on `gog_pass`, or `E_GATE_FAILED{stage:"gate_of_gates"}` on `gog_fail` | — |
| IC-27 | `vendor` with no owner present | completes without any approval stage; progress never shows `approval`; every fetch goes to the egress `crates` class | — |
| IC-28 | `budget set` with a sum above the soft day budget; `model set` to a model the charter does not allow; both at L0 | `E_BUDGET`; `E_ARGS`; `E_LEVEL` | — |
| IC-29 | `promote` on a member lane, G4 passes; then `promote --sha` of that SHA on the primary lane at L1 and at L2 | the member's line moves and `refs/init/qualified/<sha>` is set with no LKG change; `E_LEVEL`; the ordinary pipeline with cached G0–G3 | — |

## 9. Open questions

Settled:
- rounds 1 and 2: `docs/reconciliation/kernel-supervise.md`; kernel refusals use `E_*` codes, and evolution.md keeps only its validator's dotted codes (CONVENTIONS §4);
- v0.3 decisions D4 (no owner step; `needs_owner` removed; `inconclusive_final`) and D10 (the `crates` egress class, which covers `index.crates.io` and `static.crates.io`), logged in `docs/reconciliation/v03-kernel-supervise.md`;
- the L1–L3 command set adopted from evolution.md §4.1 (`task list/show`, `fleet list/spawn/send/stop`, `promote --sha`, `fleet_spawns_day`), proxy.md Q-IC1 (`budget set`) and gate.md Q-IC2 (`admit` and `kernel` jobs and their codes);
- `status.root` relayed verbatim, as awareness.md asked.

**Own:**

1. Should the probe lane allow `vendor`? Current answer: no, a candidate's G3 self-edit must not need network.
2. Should the CLI's default output be text instead of JSON? Current answer: JSON on stdout (models parse it reliably), human progress on stderr.
3. evolution.md proposed `E_BUSY{reason:"root_unreachable"}`. This doc uses `E_ROOT` for every Root-link problem instead, so one code means "the Root is unreachable or refused", and `E_BUSY` keeps its meaning of "wait for another job".

**Cross-doc follow-ups:**

4. **evolution.md §3.14, §4.1, §4.2 and testing.md TA-14:** the task-submission quota is config.md's `budget.task_submits_day` (default 10; gate.md uses the same), reported as `E_QUOTA{quota:"task_submits_day"}`. evolution.md still writes `tasks_day` = 20.
5. **evolution.md §4.1:** `kernel promote --wait` returns at the gate-of-gates verdict (`awaiting_swap`, or `E_GATE_FAILED{stage:"gate_of_gates"}`), as evolution.md's own stage 1–2 diagram shows; its §4.1 row still says `awaiting_root`.
6. **root/sealed-eval.md and kernel/gate.md:** the family format of `task template` (D2), and the `task_admit` record's data (kernel/gate.md §3.7.5 lists the fields; root/root.md §3.10.5 still names both `task_registered` and `task_admit`).

## 10. Spec deltas

| Spec v0.2 §7.3 / v0.3 | This doc | Why |
|---|---|---|
| `initctl gate --quick <worktree>` | wire takes `{repo, rev}`; the CLI resolves a worktree path and snapshots a dirty tree to a commit | the kernel only gates commits it fetched by SHA |
| `initctl reload` "reports whether the prefix changes" | returns a job at once; the result arrives as the `reload` event (and the job record) | the reload happens at the caller's next step boundary, after the tool call returns |
| `initctl promote` | non-blocking by default; `wait` returns at `awaiting_boundary`; v0.3 adds member lanes (a member promotes its own line, and a G4 pass qualifies the SHA) and `--sha` (the primary adopts a qualified SHA) | a tool call that waits for its own handoff would deadlock; L2 fleet (evolution.md §3.15.5) |
| — | added: `ping`, `wait`, `job.*`, lanes (with purposes `probe` and `run`), progress frames | long operations need progress and reattachment; probes and G4 task runs need restricted lanes |
| §7.3 `initctl status` "budget, cache hit rate" | integers only (micro-USD, permille); v0.3 adds `root` (the Root's status, verbatim), `link`, `fleet` and soft budgets | nothing float-formatted may reach a prompt; v0.3 levels (awareness.md, proxy.md Q-IC1) |
| `initctl vendor` | crates.io exact versions only; output under `state/kernel/vendor/<job>/`; refused while provisional; **no approval** (v0.2's optional approval, `E_DENIED` and the `approval` stage removed); fetched by the kernel through egress's built-in `crates` class into a local registry, then offline cargo | L0 right (spec v0.3 §15, ESC-1); D10 |
| `initctl propose-kernel <patch>` "files a proposal for the owner" | `kernel propose` (alias kept): sent to the Root as `proposal.file`, no acknowledgement statuses; from L3 `kernel promote` is a real self-upgrade through `system.swap` and the gate-of-gates | spec v0.3 §15 L3, §16, §18; root/root.md §3.8 |
| v0.2 owner lane (`run/owner.sock`): `approve`, `deny`, `resume`, `clear-safe`, `stop`, `panic` | removed. Approvals are gone; panic, resume and safe-mode clear are the Root's; `/stop`, `/resume`, `/accept`, `/bless`, `/reject` are optional owner lines relayed as `owner.in` | spec v0.3 §18; root/root.md §3.15 |
| — (v0.3) | new: `level`; `task template/submit/list/show`, `budget set`, `model set` (L1); `fleet list` (L0), `fleet spawn/send/stop` (L2); `kernel promote/status` (L3); codes `E_LEVEL`, `E_CEILING`, `E_ROOT`, `E_BUDGET`, `E_ADMISSION`, `E_KUPGRADE_FAILED`; quotas `task_submits_day`, `fleet_spawns_day`; milestones `awaiting_root`, `awaiting_swap`; registered `gate.admit.*`, `gate.k*.*`; `gate.g4.needs_owner` dropped | spec v0.3 §15, §17; evolution.md §4.1; gate.md Q-IC2; D4 |
| v0.2 `E_PANICKED`, `E_DENIED` | removed | the kernel never observes a panic; no approvals |
| v0.2 peer check for `owner.sock` (peer not sandboxed) | removed; macOS lane attribution adds the Root-canary exclusion; Linux attribution is by socket path | owner lane gone; Root-spawned sibling namespaces (kernel/sandbox.md §3.13) |

**Build sync 2026-10-20** (build → docs; ledger reconciliation/build-deviations.md §1.7):

| Build ref | This doc now says | Section |
|---|---|---|
| build: WP1 D1, D2 | two-word commands have dotted wire names (`task.submit`, `fleet.spawn`, `kernel.promote`); the space form and the aliases `wait` and `propose-kernel` are also accepted; `ping.lane` is `live`, `member` or `probe` (no `owner` lane) | §3.2, §3.4, §4.1 |
| build: WP1 D3 | the `level` result is root/root.md §4.5's `ledger.json` passed through (`next.criteria[{id, need, have, met}]`, `max_level`, `supported_max_level`), and the example follows it | §4.1 |
| build: WP1 D5–D11 | the job record's `progress` has no `job` field, `lane` is `live`, `member` or `probe`, and `reason` and `timed_out` are optional; `rollback.reason` is required on the wire (the CLI sends "no reason given"); CLI additions (`vendor --workspace`, `--wait`/`--no-wait` on `reload` and `gate`, `help`, `--help`, `--version`, absolute paths, the `--prompt-file` and `--rationale-file` caps) and exit details (a failed or cancelled `wait`, `--timeout`, local failures, no socket gives 64); `budget set` sends integers or strings; job states, stages and `root.*` codes are open enums; `error.details` is always written and `error.data` omitted when absent; `promote.ref` is `evo/` plus a non-empty id; `status.sandbox` is optional | §3.1, §3.2, §4.1, §4.4, §4.5 |
| build: WP7 D1–D3, D5–D7, D9, D10 | `wait:false` answers `queued`; every job response counts as a pending final for the pause ordering; `promote --sha` is idempotent per active job and `promote <ref>` during a pipeline gets `E_BUSY`; a job belongs to one lane instance, and after a restart only live-lane records keep an owner; `level` with the link down gives `E_ROOT`, and an unknown `parallel_bodies` gives `E_ROOT{stale}`; path resolution and its error codes, and the local copy of a proposal written after the Root accepts; `reload --wait` is refused on the live lane only; `E_RATE` is decided after the line is parsed | §3.1, §3.5, §3.6, §4.1, §7 |
| build: WP7 D8 | two log sinks: serve writes `state/kernel/logs/kernel.jsonl`, supervise's `klog` writes to stderr | §3.3 |
| build: WP7b N | `promote`'s waited result has no `scope` or `lkg`; the job record's `result` carries them | §4.1 |
| build: FIX1 N | the kernel never chmods `run/` itself (tail rule Z4); it chmods a lane directory only when its mode differs | §3.3 |

## 11. References

Checked 2026-10-19 unless marked; v0.3 additions checked 2026-10-20.

- root/root.md (2026-10-20): §3.5 (spawn broker), §3.13 (ledger, permissions), §3.14 (quotas), §3.15 (owner I/O), §4.1 (control link and requests), §9.1 X3.
- Local experiment (macOS 14.6, M2, 2026-10-20): `sandbox_check` with `SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT` on a process in another sandbox, called from inside root/root.md's composed `kernel` profile through a compiled C helper, returned exact values (kernel/sandbox.md S-Z10). Python ctypes cannot make that variadic call on arm64 (it passes variadic arguments in registers).
- cargo local registries (`local-registry` source replacement) — https://doc.rust-lang.org/cargo/reference/source-replacement.html ; sparse index layout — https://doc.rust-lang.org/cargo/reference/registry-index.html (recalled, not re-fetched on 2026-10-20: UNVERIFIED)

- CONVENTIONS.md §4 (initctl envelope) and spec v0.2 §7.3.
- `getpeereid(3)` (macOS) — https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/getpeereid.3.html ; `<sys/un.h>` `SOL_LOCAL`, `LOCAL_PEERPID`, `LOCAL_PEERTOKEN` (macOS 14.6 SDK, local)
- unix(7), `SO_PEERCRED` — https://man7.org/linux/man-pages/man7/unix.7.html
- Rust `UCred` / `UnixStream::peer_cred`, unstable, tracking issue #42839 — https://doc.rust-lang.org/std/os/unix/net/struct.UCred.html , https://github.com/rust-lang/rust/issues/42839
- `UnixListener::bind` (no unlink, `AddrInUse`) — https://doc.rust-lang.org/std/os/unix/net/struct.UnixListener.html#method.bind
- libc 0.2.191: `LOCAL_PEERPID` — https://docs.rs/libc/0.2.191/aarch64-apple-darwin/libc/constant.LOCAL_PEERPID.html ; `SO_PEERCRED` — https://docs.rs/libc/0.2.191/libc/constant.SO_PEERCRED.html
- nix 0.31.3: `getpeereid` — https://docs.rs/nix/0.31.3/x86_64-apple-darwin/nix/unistd/fn.getpeereid.html ; `PeerCredentials` — https://docs.rs/nix/0.31.3/nix/sys/socket/sockopt/struct.PeerCredentials.html ; `LocalPeerPid` — https://docs.rs/nix/0.31.3/x86_64-apple-darwin/nix/sys/socket/sockopt/struct.LocalPeerPid.html
- crates.io API (versions: libc 0.2.191, nix 0.31.3, serde 1.0.229, serde_json 1.0.154, toml 1.1.8) — https://crates.io/api/v1/crates/<name>
- `sysexits.h` (64 EX_USAGE, 69 EX_UNAVAILABLE, 70 EX_SOFTWARE, 75 EX_TEMPFAIL, 76 EX_PROTOCOL, 77 EX_NOPERM) — macOS 14.6 SDK `usr/include/sysexits.h` (local, checked 2026-10-19)
- git `uploadpack.allowAnySHA1InWant` — https://git-scm.com/docs/git-config#Documentation/git-config.txt-uploadpackallowAnySHA1InWant
- cargo vendor — https://doc.rust-lang.org/cargo/commands/cargo-vendor.html ; `--config` — https://doc.rust-lang.org/cargo/reference/config.html#command-line-overrides
