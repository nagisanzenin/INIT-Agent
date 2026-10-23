# Root security review for v0.1.0 — trust boundary (read-only, adversarial)

```
Reviewer lane: REVIEW · Date: 2026-10-20 · Scope: the Root (root/crates/init-root, root/sandbox-profiles)
Method: read every src file of init-root and the SBPL profiles + the zone-side control link (docs); ran
        the existing crate tests mentally against the code; wrote no probes that signal foreign processes.
Target: the single trust boundary between the Root (owner uid, unsandboxed) and the agent zone (kernel +
        bodies, Seatbelt-sandboxed, agent-rewritable). "zone" below = any agent-zone process, worst case a
        fully mis-evolved kernel speaking the control link (§4.1) and the egress wire (root/egress.md §4.1).
Build note: this review assumes the production Root is a cargo **release** build. There is no
        `[profile.release]` and no `overflow-checks` override anywhere (root/Cargo.toml, ci/env.sh, .cargo),
        so release integer overflow **wraps** (dev/test builds panic instead — relevant to F1).
```

## Counts by severity

| Severity | Count |
|---|---|
| Critical | 0 |
| High | 1 |
| Medium | 3 |
| Low | 4 |

No finding lets the zone read keys, forge lineage, escalate its autonomy level, or impersonate the owner —
those guarantees held (see "Guarantees checked and holding"). The findings are in the money metering path
(the ceiling backstop) and in denial-of-service surface.

---

## Findings

### F1 — HIGH — money admission uses unchecked u64 addition on a zone-controlled reservation

- **Where:** `root/crates/init-root/src/egress/money.rs:109` and `:112` (`Money::admit`), reservation built
  at `root/crates/init-root/src/egress.rs:426` from `ins.max_out` (zone's `max_tokens`); the same shape in
  the sealed lane at `root/crates/init-root/src/egress/lane.rs:145`.
- **Broken guarantee:** "the agent zone can NEVER exceed the money ceilings" (spec invariant #3; the hard
  ceiling admission gate).
- **Exploit (from the zone):** the reservation is
  `reserved = tap::musd([(body_len/3, in_price), (max_out, out_price)])`, where `max_out` is the request's
  `max_tokens`/`max_completion_tokens`/`max_output_tokens` — a raw `u64` the zone controls, with no cap
  (classes.rs `inspect`, egress.rs:426). `tap::musd` saturates to `u64::MAX` (tap.rs:38). `admit` then
  computes `total_spent + total_reserved + r` and `day_spent + day_reserved + r` with plain `+`
  (money.rs:109,112). In a release build this wraps: once any spend exists (so `spent+reserved ≥ 1`, i.e.
  after one ordinary request), a request with an enormous `max_tokens` makes `reserved ≈ u64::MAX`, the sum
  wraps below the ceiling, and the request is **admitted even though the day/total ceiling is already
  reached** — the gate that is the whole point of R2 is bypassed deterministically. The sealed lane
  (lane.rs:145, `Pool::Agent` family evals at L3+) has the identical unchecked pattern against the sealed
  reserve.
- **Caveat on real-dollar impact:** to overflow, the reservation (hence `max_tokens`) must be astronomically
  large, which most providers reject with a 400 (billed 0). So converting the bypass into real overspend
  depends on the upstream clamping `max_tokens` instead of rejecting it. The *admission guarantee itself* is
  broken unconditionally; the dollar loss is provider-dependent. In dev/test builds (overflow-checks on) the
  same input panics the per-request thread instead of wrapping — a per-request failure, not a wedge (the
  money mutex is poison-tolerant via `link::lock`), but still a correctness defect.
- **Fix:** use `checked_add`/`saturating_add` in `admit` (both the total and day checks) and in the sealed
  lane's budget check, and cap `ins.max_out` to a sane bound (e.g. the model's `reserve_out` or a fixed
  ceiling) in `classes::inspect`. With saturating arithmetic, `reserved == u64::MAX` yields a sum `> limit`
  and is correctly refused.
- **Confidence:** High that the arithmetic is unchecked and the gate is bypassable in release; Medium that it
  yields real overspend (provider-dependent).

### F2 — MEDIUM — metering and model/level gating hang off a path-suffix heuristic; billable non-completion endpoints are forwarded unmetered and ungated

- **Where:** `root/crates/init-root/src/egress.rs:418` (`let metered = c.model && family != Family::None &&
  h.method == "POST"`), with `family` from `tap::Family::of` (tap.rs:21) matching only paths ending in
  `chat/completions`, `messages`, or `responses`.
- **Broken guarantee:** money ceiling + per-model level gate, for any model-class path that is billable but
  not one of those three suffixes.
- **Exploit (from the zone):** a model-class `POST` whose allowlisted path does not end in a recognized
  completion suffix gets `metered = false`, so the Root skips `inspect` entirely — **no reservation, no
  money accounting, no `class.model`/`class.level` check** — yet still forwards the body with the injected
  provider key and the provider bills it. With the built-in `providers` class (paths `chat/completions`,
  `beta/chat/completions`, `models`, `user/balance`) the only non-completion paths are free/cheap, so the
  default charter is safe. But any owner-added billable endpoint — DeepSeek legacy `completions`/FIM, OpenAI
  `embeddings`, `images/generations`, `audio/*`, `moderations` — would be spent against the provider while
  invisible to the ceiling and reachable with any (even above-level) model string.
- **Fix:** gate on the *class* and *method*, not the path family: meter (reserve + account) every `POST` to a
  `model` class, and run the model/level check for any model-class request that carries a `model` field,
  regardless of `Family`. Treat an unrecognized metered path as metered-at-reserve rather than free. The tap
  may still return `None` usage (then charge the reservation, as it already does for truncated streams).
- **Confidence:** High on the code behavior; Medium on impact (depends on the owner's `[actions]` paths).

### F3 — MEDIUM — unbounded per-connection thread spawn on egress.sock (DoS of the Root)

- **Where:** `root/crates/init-root/src/egress.rs:223-227` (`for c in l.incoming().flatten() { thread::spawn(... x.conn(c)) }`),
  no cap on concurrent connections; `MAX_INFLIGHT` (egress.rs:38) bounds only in-flight *forwards*, not
  open connections. The owner socket has the same shape (`link.rs` `serve`) but is owner-only and so not a
  zone vector.
- **Broken guarantee:** "the zone can NEVER crash or wedge the Root (DoS)."
- **Exploit (from the zone):** the kernel (the only process that may reach `egress.sock`; agent-authored from
  L3) opens a large number of connections and holds each with a partial request head. Each spawns a Root
  thread blocked in `read_request`; `inflight` is never incremented (it rises only after a full request is
  read), so the `MAX_INFLIGHT` cap never trips. Thread/stack and memory growth are unbounded → Root
  exhaustion. Local `GET /_root/health` / `/_root/ceilings` are also answered *before* the inflight guard
  (egress.rs:393-400), so a flood of those spins Root CPU uncapped too (see F7).
- **Fix:** bound concurrent accepted connections (a semaphore or a small fixed worker pool), apply a read
  timeout to the request-head read, and move the local `/_root/*` answers behind the same connection budget.
- **Confidence:** Medium. Clear unbounded spawn; exploitability is from L3 (maintainer kernel at L0).

### F4 — LOW — settle does not re-gate, and the input reservation is a bytes/3 estimate: bounded ceiling overshoot

- **Where:** `root/crates/init-root/src/egress/money.rs:130` (`settle` adds actual cost without re-checking
  the ceiling); reservation input term `body_len/3` at egress.rs:426; output term is the zone's `max_out`.
- **Broken guarantee:** the ceiling, softly — admitted spend can transiently exceed it.
- **Exploit (from the zone):** `settle` books the *actual* cost over the released reservation and never
  re-checks the limit, so when a request's actual cost exceeds its reservation the spend crosses the ceiling
  and only *subsequent* admissions are refused. Output cost cannot exceed the reservation (provider honors
  `max_tokens`), but input tokens can exceed `ceil(body_len/3)` for dense/non-Latin text, so each in-flight
  request can overshoot a little; total overshoot is bounded by `MAX_INFLIGHT × per-request input
  under-count`. Not an unbounded bypass (that is F1), but the ceiling is not a hard wall under concurrency.
- **Fix:** reserve input at a conservative tokens-per-byte (e.g. 1 token/byte) or clamp the per-request
  overshoot; optionally refuse new admissions while `spent > limit` even before the day roll (already the
  effect, but make the invariant explicit).
- **Confidence:** Medium.

### F5 — LOW — on macOS without `live-sweep`, a `setsid` child escapes panic/sweep

- **Where:** `root/crates/init-root/src/zone.rs:122-134` (`kill`/`kill_all` signal only the leader pid and its
  process group) and `:297-308` (`scan` is a no-op unless the `live-sweep` feature is built, which the
  owner's machine never enables — `status.sweep = "held_only"`, §3.5.4).
- **Broken guarantee:** panic "kills everything" / instance sweep completeness (partial).
- **Exploit (from the zone):** a body (or kernel) that calls `setsid()` moves to a new process group; on
  panic/crash/sweep the Root's `killpg(leader_pgid)` and per-instance `kill` miss it, and the D1 process-table
  scan that would catch it is compiled out in production. The escapee survives a panic. It remains confined:
  it still carries the inherited Seatbelt policy (reaches only `egress.sock`, which panic closes, and
  INIT_HOME sockets; cannot read ROOT_HOME or keys), so damage is bounded, but it is not reaped and could
  keep a body-local socket alive across a panic the owner believes is total. This is documented as the
  `held_only` limitation; flagged because the panic guarantee is stated absolutely.
- **Fix:** none cheap without the live scan on the owner machine; at minimum surface `sweep=held_only` in the
  panic result/alert so the owner knows an escapee is possible, and consider a bounded, sandbox-filtered scan
  (the D1 membership predicate already exists in `sweep.rs`) gated to run only on panic, not routine sweeps.
- **Confidence:** Medium (behavior is as designed; it is the stated guarantee that is absolute).

### F6 — LOW — fd-channel stash can hold up to ~512 descriptors from a hostile kernel

- **Where:** `root/crates/init-root/src/sup.rs:190-205` (`Kctl::take_fds`: stash of up to 32 datagrams, each
  up to 16 `SCM_RIGHTS` fds, retained 60 s), fed by `sys::recv_fds` (sys.rs:136).
- **Broken guarantee:** DoS resilience (minor).
- **Exploit (from the zone):** a kernel sends many fd-bearing datagrams tagged with ids that are never
  claimed; the Root holds ~32×16 descriptors for up to 60 s. Bounded (oldest entries are dropped and their
  `OwnedFd`s closed), so this is not a true exhaustion, but it is attacker-influenced fd pressure on the Root.
- **Fix:** cap total stashed fds (not just datagram count) and shorten the retention, or refuse datagrams
  whose tag does not match a pending request id.
- **Confidence:** Medium.

### F7 — LOW — local `/_root/health` and `/_root/ceilings` answered before the inflight guard

- **Where:** `root/crates/init-root/src/egress.rs:393-400` (returns before `inflight.fetch_add`).
- **Broken guarantee:** DoS resilience (minor; subsumed by F3).
- **Exploit:** the kernel floods these local GETs; each rebuilds the ceilings view (`ceilings()` takes the
  money mutex and iterates classes/models) with no rate or concurrency cap → CPU/lock pressure.
- **Fix:** cheap-cache the ceilings object and/or put these behind the connection budget of F3.
- **Confidence:** Low.

---

## Guarantees checked and found holding (one line each)

- **Keys never reach the zone** — keys live under `ROOT_HOME/keys/` which Z1 hides from every zone profile;
  `util::read_secret` opens `O_NOFOLLOW|O_CLOEXEC` and enforces regular-file + `uid==euid` + mode `& 077==0`;
  `Secret` has no Debug/Display/Serialize and is zeroed on drop; auth is injected below the kernel, every
  inbound header (incl. `authorization`) is dropped (KERNEL_HEADERS allowlist), and upstream 401/403 bodies
  are withheld (`upstream.auth`).
- **Lineage cannot be forged or altered** — the Ed25519 seed is in-process and in `ROOT_HOME` (zone cannot
  read it); kernel appends are restricted to the `KERNEL`/`x_*` kind set and signed as `kernel:<build>`
  (kinds.rs), never the Root-only kinds; the chain is hash-linked with `prev`/`seq`/monotonic `at`, verified
  with `verify_strict` over a domain-separated hash at every open (tail) and fully daily, and `head.json` is
  reconciled (verify.rs rules 1–8); torn tails are copied aside and truncated.
- **Autonomy level cannot be escalated by the zone** — `Level::fold` is driven only by Root-written kinds
  (`ladder`/`audit`/`mutation`/`charter`/`genesis`) plus legitimate kernel signals; the kernel cannot emit
  `ladder`/`audit`/`mutation` (not in `KERNEL`); and `SUPPORTED_MAX_LEVEL == 0` in release caps `cap()` at L0,
  while `system.swap`/`sealed.family_eval` require L3 (sup.rs:356).
- **Owner cannot be impersonated on owner.sock** — `link::Root::conn` requires the peer `uid==euid` **and**
  `sandboxed(pid) != Some(true)`; the zone is sandboxed and is additionally OS-denied the socket (it lives in
  the Z1-hidden `ROOT_HOME`, and Z6 permits only `egress.sock` + INIT_HOME sockets).
- **Zone cannot stop the Root stopping/panicking/rolling back** — panic runs in the unsandboxed Root, sets
  `Panicked` before killing, closes egress, kills held instances, and demotes under one lineage lock; the
  latch file survives restarts and re-enters `Panicked` at cold start; the zone cannot signal the Root
  (Seatbelt `signal`/`process-info` only `same-sandbox`). (Residual: F5.)
- **Network destinations are confined to the allowlist** — `classes::route` enforces exact `host:port`
  against `destinations` (no IP literal for model classes, no wildcards), rejects CONNECT/absolute-form and
  path traversal, never forwards 3xx (`max_redirects(0)` + class rule), does not forward a `host` header, and
  forbids non-loopback in test mode / loopback in production.
- **Sandbox-profile composition is sound** — profile text is `include_str!`-compiled, chosen by id; values
  travel only as `-D` params (no text splicing); the deny-only tail is always appended last; `Z_*` param
  names and `MARK` are refused (V1); the `kernel` profile and `sealed` kind are unreachable to the kernel
  (V5 + `check_class`); V2 rejects non-canonical/`..`/NUL paths and V3 confines inner read rules to
  INIT_HOME/toolchain/devdir/extra_read; writes, network and Root paths are held by the tail regardless of
  params.
- **Blob store is traversal-safe** — `blob()`/`put()` validate `want` and the stored name as 64-lowercase-hex
  only, write with `O_EXCL|O_NOFOLLOW` as `0444` by computed hash, dedup by content, and bound total size;
  `store.get` is hex-validated and rooted in `ROOT_HOME/store/blobs`.
- **No fd leaks into the zone** — `exec-clean` empties the signal mask, resets SIGPIPE, and closes every fd
  `≥ 3` except `--keep`ed ones before `execv`; `dup_high` renumbers control fds `≥ 10`; `recv_fds` sets
  `FD_CLOEXEC` on received descriptors.
- **No env injection into spawned zone processes** — the broker `env_clear()`s and V4 forbids any
  `INIT_ROOT_*` key and bounds env size/count; the kernel's env is Root-built; `INIT_TEST*` passthrough is
  gated on `root.test_mode`.
- **No JSON/ICJ differential at the boundary** — lineage lines must re-serialize to byte-identical ICJ
  (`icj(r) == line`), serde structs error on duplicate keys, and request bodies are forwarded upstream
  byte-for-byte (`b.body(body)`), so the Root's parsed view and the provider's cannot diverge on `model`.
- **Charter / money limits are owner-only and bounded** — the charter is read and applied only from
  `ROOT_HOME/charter.toml` by the owner path; the zone's view is read-only; `musd` caps limits at ≤ 7 integer
  digits (≈1e13 µUSD), well inside `u64`.
- **Seatbelt gaps** — every profile ends with `mach-lookup (xpc-service-name-prefix "")` denied, the prelude
  enumerates only the measured mach globals, `~/.config/init-agent` is not a read root (it is the key path and
  lives outside all read roots / inside Z1), and network is Unix-socket-only; the test matrix (§3.3.5) covers
  deny-after-allow ordering and the redefinition attack.

---

## Notes / residual risk

- F1 is the one finding that breaks a core invariant as written; the fix is a two-line change to saturating
  arithmetic plus a `max_out` cap, and should land before v0.1.0 tagging. F2 should land with it since both
  are in the ceiling/metering path and share a fix location (`classes::inspect` / `egress::check`).
- The Linux zone (bwrap/seccomp, root.md §3.4) is UNVERIFIED and not built; sealed runs are unconfined on
  Linux (run.rs:144). Not in scope for a macOS v0.1.0 but it is a boundary hole on Linux hosts.
- All of the above is static review plus the crate's own unit tests; no live sweep, no signals to foreign
  processes, and `~/.init`, `~/.init-root`, `~/.config/init-agent` were not touched.
