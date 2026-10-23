# Release evolution pilot — DeepSeek, maximum $10

2026-10-23 implementation draft. Owner authorized implementation and a DeepSeek-only pilot costing at most $10. No API requests have been made by this harness lane. This document supersedes the earlier estimate's requirement to obtain spend permission again. Execution still requires the readiness gates below. Multi-OS and multi-provider qualification remain release gates outside this macOS pilot.

## Registered design

Question: does persisted self-modification improve performance on new tasks at equal total model budget?

Before the paid manifest is frozen, the same learning-only instruction is appended
to each family's prompt for both arms. It states that later tasks use changed data
and fresh conversations, with ordinary task workspaces and conversation history
removed. Native evolution is optional: use it when available and worthwhile,
complete normal admission before finishing, and continue the current task if it
is unavailable or refused. It specifies no implementation and reveals no scorer,
reward, transfer input, or expected answer. Transfer prompts stay unchanged. The
trusted runner must enforce that isolation before learning snapshots and transfers;
a prompt alone is not evidence that it happened. Admitted artifacts, Root lineage,
usage accounting, and required kernel state remain intact.

This is a small descriptive pilot with uncalibrated candidate fixtures. A strong
baseline may already reach the quality ceiling; equal perfect scores cannot show
that evolution improves quality. Report learning cost, transfer cost and latency,
actual admission/retention/use evidence, and the raw paired outcomes. No additional
paid warmup, calibration, task growth, or outcome-based task replacement is added
by the learning instruction.

A is genesis with evolution disabled and the same ordinary coding tools. B starts from the same snapshot and may evolve its body/skills/extensions in learning. Ordinary workspace helpers and conversation history are cleared equally before the learning snapshots used for transfer; B may retain admitted native capabilities. Freeze both arms after learning. Each transfer starts from an independent copy of that arm's post-learning snapshot; do not chain transfers. A must remain unable to persist body changes, and freezing B must retain its admitted capability. Do not equate freezing with restoring genesis.

`tests/real/release_ab/harness.py plan` emits 48 task slots: four families (structured validation, navigation, transformation, resumable work), two fresh learning repeats, two arms, one learning plus two transfer tasks each. AB/BA order is deterministic and balanced within each family. The committed seed is 20261023. Each arm/block has $0.21 learning and $0.12 per transfer, with wall limits 1200/600/600 seconds including compilation. No adaptive budget transfer between arms or outcomes.

`fixtures.py` now generates deterministic candidate fixtures and external scorers for all four families. They are not yet calibrated or frozen as the benchmark. The resumable family scores event replay; actual interruption/restart behavior belongs to runtime qualification, not the fixture score. Before execution, build disjoint learning/transfer/calibration instances with deterministic external scorers. Freeze fixture, prompt and scorer SHA-256 values via `freeze()`, plus exact binary SHA and full repository commit SHA. Store this manifest with the protocol in a reviewed commit before observing main outcomes. Existing exposed evolution tasks are development-only. Never read sealed draw/pack sources. Use hidden scorers outside the agent sandbox.

Record failed learning and missing self-changes as assigned B, without replacement. Capped/failed completed attempts count as primary failures; unexecuted tasks remain missing, never invented failures. Reserve-funded infrastructure retries require a distinct receipt plus link to the original; keep original outcomes, and preregister any retry selection rule before running. Current analyzer deliberately rejects duplicate task IDs and does not implement retry aggregation.

## Budget and trusted admission integration

All accounting is integer micro-USD (1 USD = 1,000,000 units).

| Category | Maximum |
|---|---:|
| Endpoint smoke | $0.40 |
| Separate calibration | $0.80 |
| Main (8 blocks, both arms) | $7.20 |
| Reliability/rollback checks | $0.80 |
| Explicit infrastructure reserve | $0.80 |
| Total | $10.00 |

`Ledger` is a durable SQLite coordinator with `BEGIN IMMEDIATE` and synchronous FULL. Initialization takes a freshly queried real balance and explicit account reserve; the effective ceiling is min($10, available minus reserve). Each main run is registered with its fixed cap. Non-main runs must be registered in their category. Every test home and every request must share one owner-controlled database inaccessible to the agent. Never initialize a second ledger for retries.

### Selected integration: reserve whole task before launch

`Lifecycle.admit_task(ledger, task)` durably reserves the FULL task cap before any Root process or model-capable setup starts. Its permanently unique `task:<task_id>` request ID prevents replay even after a zero-cost settlement. `finish_task()` requires the driver to confirm process exit; its default conservatively retains the whole cap. An outcome never earns a second admission. The $7.20 main envelope therefore covers all main tasks even with missing usage. Smoke/calibration/reliability must likewise use registered, uniquely admitted envelopes; no unguarded setup calls.

The trusted Root then enforces at most this task's additional allowance using its existing per-request Money admission. Its cumulative spend ledger is retained across restores; configure cumulative limits from verified current spent/reserved values plus the task cap, account for all sealed/twin/other paid lanes, and freeze policy while the task runs. No delayed balance polling is a substitute. Root output-token bounds and model pricing must be verified before this is safe. Parent integration owns that production enforcement. If all underlying request reservations are conservative and Root remains within its task allowance, this approach does not require a second global per-request hook.

The lifecycle excludes `rh/egress` from snapshots and preserves the current directory across every restore. An unknown/reserved request survives Root restart through its durable start record. Never copy an older spend ledger over the current one, increase a task allowance on restart, or rerun a task ID under a different request ID. The outer SQLite ledger lives outside the snapshot tree.

### Alternative finer-grained integration

The same ledger can instead reserve every upstream request. Do not mix the two accounting models within one experiment. If this alternative is chosen, trusted egress integration must do the following in order:

1. Validate provider/model and pricing against the frozen current price snapshot. Reject substitutions and non-DeepSeek egress. Obtain a conservative billed-input token bound and enforce output limits including reasoning tokens. Use peak rates, no speculative cache/off-peak discounts. `worst_case_uusd()` rounds up the supplied bounds; it does NOT tokenize or prove that the supplied bound is valid.
2. `ledger.reserve(unique_request_id, task_id, worst_case_uusd)` BEFORE opening any upstream request. A budget refusal means no network send. One outstanding request across all homes is allowed; requests are immutable unique IDs.
3. Only after the reservation commits may Root send the request. No automatic HTTP retry under the same reservation; a retry gets its own reservation after settling the prior request.
4. `ledger.settle(id, actual_uusd)` only from validated usage and frozen prices. On timeout, missing/inconsistent usage or uncertain cancellation, settle with `None`: the whole bound remains charged. A process crash leaves an unresolved reservation that blocks new requests. Recover it conservatively as unknown, never discard it. Actual usage above the bound records the real charge and persistently halts the ledger; this is a failed safety precondition, not a successful hard cap.
5. Reconcile priced usage and ending balance, considering billing lag. Unexplained balance discrepancy pauses the program. No other consumers may spend against the same benchmark accounting assumptions during the run.

This module is NOT wired into the production launcher/Root limits yet and is NOT a network proxy. Running the old `evo_checks.py run` with this ledger in a side process does not enforce the ceiling: the old driver polls usage after send and must not be used as the $10 guard. `Ledger` provides tested coordination primitives, not proof of current production spending safety.

## Runtime driver contract and current lifecycle implementation

`lifecycle.py` supplies stable-path isolated homes under `target/ab-homes/r/{rh,ih}`, preserving installed absolute paths. This shorter repository test-home location meets the production 64-byte home bound; nested test instances can deliberately exercise path validation. It creates immutable hash-checked snapshots, archives prior task state, prevents snapshots while marked running, checks dead Root PID/stale sockets after driver-confirmed exit, rejects symlinks/special files and injected provider credentials, and preserves cumulative Root egress accounting. It never launches or kills processes. Standard-library copies may consume substantial disk with real binaries; check free space before invoking on a real installation.

Linux copies first attempt filesystem reflinks and fall back to private inode
copies. After the trusted completed checkpoint is acknowledged, the driver may
call `prune_previous(ledger, output)`: only an exact registered stopped runtime
image with a completed journal entry, matching immutable receipt, and verified
owner copies of workspace/trace/ceilings/lineage can be removed. Unknown or
unreceipted images remain. Monotonic archive names prevent reuse after pruning.
`evict_completed_snapshots(ledger, output)` releases a learned snapshot only after
its learning and both transfer outcomes have the same verified evidence. Genesis
remains; evicted snapshot fingerprints stay in metadata and cannot be restored or
replaced under the same name. These operations never delete the external ledger
or task evidence and do not bypass the runtime disk floor.

`arm_config()` returns the identical model/tool-budget contract with evolution allowed only for B learning. Both transfer arms preserve existing capabilities and require the proper post-learning snapshot. This configuration is declarative until the runtime enforces it. **`extend.mode = gated` is not a freeze switch.** Actual Root/body controls must deny new persisted changes without discarding B's learned capabilities.

The remaining driver must use fresh HEAD-matched binaries and implement:

- `start(snapshot, evolution_allowed, owner_controlled_ledger, task_id)` with Root-enforced identical model/tools and the A/B evolution permission difference;
- `execute(prompt, fixture, wall_limit)`; stop only owned PIDs and keep all evidence;
- `snapshot()` after learning, then branch each transfer from it without cross-arm access;
- `freeze_evolution_preserving_capabilities()` and verified restart/reuse;
- trusted scoring outside the agent-readable filesystem;
- immutable task receipt creation and request/lineage linkage.

Do not advertise this directory as a completed agent benchmark runner until this contract is integrated. Do not run the paid main protocol until all readiness gates pass: adequate disk; fresh binaries; $0 evolution creation/admission/restart/use/rollback; A persistence denial; frozen B persistence/reuse; whole-task admission plus verified Root per-request ceilings (or shared egress reservations); verified input/output bounds; fixture/scorer isolation; frozen manifest commit; live balance/pricing preflight. User already authorized the $10 limit; these are technical prerequisites, not requests to authorize again.

## Receipts and analysis

Each immutable task receipt should contain the original protocol schema: experiment ID; protocol/repo/binary/fixture/scorer hashes; family/block/repeat/arm/phase/order; OS/provider/endpoint/requested and returned model/effort; start and wall time; status/cap stop; uncached/cached/output tokens; charged and reserved money; score/pass; source/result snapshot hashes; capability/promotion/rollback IDs. Additionally use `id`, `charged_uusd`, and `protocol_sha` matching the emitted manifest.

`analyze(manifest, receipts)` currently validates unique slot IDs, protocol, arm/phase, charge type, boolean pass and completion consistency. It summarizes arm costs including learning, transfer passes, per-family paired differences and mean transfer difference only when all 48 receipts exist. It is an offline preliminary analyzer, NOT a full receipt-schema or trusted-attribution validator. Failed/capped tasks cannot claim a pass. Real accounting should be reconciled against the trusted ledger, never rely on receipt self-reported charges.

Attribution must be checked separately against Root-owned evidence linking capability ID, artifact hash, accepted promotion, restart availability and actual transfer use. Zone-authored notes/claims are not proof. A B task with no retained self-change is still in intention-to-treat analysis; report counts with/without verified evolution separately. Compare ordinary helpers in A. No causal gain claim from mocked outcomes.

Only four independent families exist, so report paired family effects with the raw outcomes; avoid false precision from treating all 16 transfers as independent. Any uncertainty method must be frozen before results, and causal interpretation remains limited to this pilot. If both arms solve everything or fail everything, report insufficient resolution.

## Current validation

Run `$ python3 -m unittest discover -s tests/real/release_ab -v` from the repository root. Tests are offline, use short-lived homes under `target/test-homes/release-ab`, and exercise balanced schedule/equal caps, durable missing-usage charges, overrun halt, concurrent reservation serialization, category/account caps, integer rounding and incomplete/invalid receipt handling. Test outcomes are harness correctness evidence, not INIT performance evidence.

## Offline fixture tooling

```
python3 tests/real/release_ab/fixtures.py generate <owner-only-new-registry.json>
python3 tests/real/release_ab/fixtures.py materialize <registry.json> validation learn <new-workspace>
python3 tests/real/release_ab/fixtures.py score <registry.json> validation learn <workspace>
```

Only workspace files enter the agent sandbox; the registry contains gold answers and must remain inaccessible. Scorers inspect JSON or parse Python AST without executing agent code, enforce regular files/size limits, and reject symlink outputs. `hashes(registry)` supplies fixture/prompt/scorer hashes to `freeze()`. Keep main seeds frozen before outcomes, and use different seeds for calibration. These small candidate tasks may saturate at 100% for both arms; calibrate difficulty without selecting only A failures. No model performance scores have been generated.

Validation now includes 19 offline tests across accounting, lifecycle and fixture scorers. Synthetic known-answer scorer checks are strictly unit tests, not A/B observations.

## Subsequent owner steering — product monetary caps removed

The owner subsequently removed product Root maximum-dollar ceilings, retaining cost display and manual stop. The benchmark's separate authorized $10 maximum remains binding. Consequently, the whole-task admission design above **is no longer sufficient for paid execution**, because its safety argument depended on trusted Root per-request ceilings. Keep it as preparation/history, not proof of a hard limit. Paid launch now requires a trusted external DeepSeek gateway enforcing conservative per-request reservations outside agent control, or equivalent independently verified enforcement. The parent integration lane owns that gateway. No paid calls have been made by this lane.

## External DeepSeek benchmark gateway implementation

`tests/real/release_ab/gateway.py` implements the selected external per-request approach. It is separate from product Root policy. On 2026-10-23, the [official Chat API](https://api-docs.deepseek.com/api/create-chat-completion/) documents `/chat/completions`, explicit `max_tokens`, streamed usage and completion-token reasoning breakdown; the [official price page](https://api-docs.deepseek.com/quick_start/pricing/) lists Flash peak uncached/cache/output rates of $0.30/$0.006/$1.20 per million tokens. The implementation fixes those rates and the `deepseek-flash` model. Reasoning tokens are counted within completion usage, not added twice. Price changes require a new verified snapshot before more requests.

The listener binds only 127.0.0.1 and requires an independent runtime bearer token. The owner selects one already-registered task ID when launching the gateway; client headers cannot choose the task, provider, key or ledger. Upstream is fixed HTTPS `api.deepseek.com:443/chat/completions` with TLS certificate checks, no environment proxy handling, redirects or retries. Requests are limited to 512 KiB, explicit output of 1–4096 tokens, at most 512 text-only messages and 64 function tools. Unknown fields, image/file parts, server-side tools, multiple completions, schema references and unsupported endpoints are refused before network access. Do not silently strip fields from the real agent request to bypass validation; configure its supported benchmark request shape deliberately and equally for both arms.

Streaming preserves validated data events. The gateway verifies the returned model and single-choice shape, requires a complete stream before crediting usage, checks token totals/cache consistency, and uses peak prices even off-peak. Both a 20-second socket timeout and 180-second post-connect transport deadline are configured; DNS/connect behavior also depends on OS networking. Responses are capped at 8 MiB and individual SSE events at 1 MiB. Client disconnects, upstream errors, malformed/truncated output or bound violations retain the full reservation and halt subsequent requests. This avoids assuming disconnected generation was canceled. A complete response with absent usage keeps its entire reservation; it is never booked as zero. A process crash leaves a durable pending reservation that blocks subsequent calls. There is no automatic recovery/unhalt or retry command.

Reservation is committed to the shared SQLite ledger before invoking transport. Returned usage beyond the request's assumed bound halts the program; if its peak-priced amount exceeds the reservation, the actual larger amount is recorded. Usage-based accounting is a conservative estimate, not a claim that the provider charged peak rates or that its account balance has reconciled.

**Residual input-bound assumption:** for this narrow text/function-tool dialect, reserved input tokens equal UTF-8 request bytes plus 4,096 framing tokens plus 64 per message/tool. This is deliberately conservative, but the provider does not publish a formal maximum for internal framing or tokenization expansion. Therefore this implementation is not a mathematical proof against arbitrary provider-side billing behavior. A whole-context reservation would remove the smaller framing assumption but would exceed the current per-task caps before useful calls. The remaining readiness decision must explicitly assess this assumption; do not describe a successful unit test as proof of a hard provider billing ceiling. Keep the account reserve and reconcile actual usage/balance.

Example owner-side launch (credentials must already be supplied privately through the process environment; never paste them into shell history or arguments):

```
python3 tests/real/release_ab/gateway.py --ledger <owner-ledger.sqlite> \
  --task-id <registered-task-id> --port <port> --pricing-checked 2026-10-23
```

Required environment variable names are `DEEPSEEK_API_KEY` and `INIT_AB_GATEWAY_TOKEN` (at least 32 characters). Keys are never written by the gateway to files or logs. Parent integration must keep the process, ledger and real key outside the agent sandbox, route Root through this endpoint, prevent any alternative direct-provider route, and qualify this with a mock end-to-end run before paid calls. Root monetary caps must not be reinstated merely to run this benchmark.

**Current validation:** 31 offline tests pass, including 12 gateway tests using a synthetic in-process upstream. No real DeepSeek request has been made by this lane, no model benchmark score exists, and Root-to-gateway integration is not yet proven.

### Output-cap mismatch and fail-closed readiness

Current seed `context/model.rs` uses 65,536 default output tokens for Flash, a 16,000-token summary path, and `loop/mod.rs` doubles the next output allowance after a length cutoff. These exceed the gateway's explicit 4,096-token limit. Paid requests are therefore **not admissible yet**. The runtime must expose and enforce a deliberately frozen benchmark cap across normal requests, length retries, compaction and probes, equally for both arms. The gateway refuses larger requests; it does not silently rewrite them.

`python3 tests/real/release_ab/readiness.py` emits the current blockers and exits 2. The optional `--evidence <owner-controlled.json>` accepts a frozen manifest and hash-bound qualification receipts for runtime output, evolution freeze/restart, native platform/sandbox/rollback/disk, external gateway routing without direct bypass, and pricing/balance/input-bound review. Runtime qualification additionally requires captured request samples from all four paths that pass gateway validation. Receipt booleans alone do not count; receipts must be files bound to the manifest's repository/binary/protocol hashes. This validates artifact consistency, not the truth of an untrusted self-authored receipt. Parent must produce those receipts from actual qualification tests.

Lifecycle snapshot hashing now uses a versioned domain, framed path lengths, type tags, permissions, file lengths and fixed-size content digests. It rejects a file changing size/mtime during hashing. This removes ambiguous concatenation and empty-file/empty-directory collisions. Existing pre-v2 hashes are not reusable qualification evidence.

### Executable native runner (2026-10-23 resumed work)

`runner.py --config OWNER_CONFIG.json` runs preflight only. Add `--execute --limit 1`
for the first frozen schedule task after qualification; use `--limit 48` for the
whole schedule. Limit selects a prefix, never individual successful-looking tasks.
A later launch skips only completed tasks whose immutable receipts still match
SQLite. An interrupted execution permanently blocks automatic replay. The driver
itself has not been run against a real native runtime yet; 41 stdlib offline tests
pass. No model scores or native qualification receipts have been generated here.

Owner config fields:

- `evidence`: readiness evidence JSON containing manifest and actual receipt pointers.
- `fixtures`: frozen fixture registry JSON; prompts/scorer/data hashes must match.
- `ledger`: existing initialized external SQLite ledger, bound to the same manifest.
- `output`: owner-only evidence directory, outside live homes and snapshots.
- `base`: optional stable lifecycle base, normally repository `target/ab-homes`.
- `gateway_port`: fixed unprivileged localhost port, configured in installed charter.
- `binaries`: absolute runtime binary path → SHA256 mapping, including installed
  `base/r/rh/bin/init-root`; its hash must equal manifest `binary_sha`.
- `source_archive`: `{path, sha256}` for exact source archive used to build binaries;
  a Git commit alone cannot identify current uncommitted changes.
- `frozen_artifacts`: INIT_HOME-relative file/directory paths covered by actual
  write-denial qualification. Hashes are checked before/after frozen tasks.

Platform, evolution-freeze and gateway-route receipts must additionally contain
matching `runtime_binaries` and `source_archive_sha256`; evolution-freeze must
contain the identical `frozen_artifacts` list. Populate them from actual native
probes: normal kernel `change.open`/swap denial, frozen filesystem write denial,
network/direct-provider bypass denial, restart capability reuse, and recorded
request bodies for all output-limit paths. Descriptive policy fields alone are
insufficient. Preflight checks evidence integrity, not the truth of invented
receipts. No stub or synthetic proof is supplied.

Install genesis **before** adding benchmark policy, at the stable `base/r/rh` and
`base/r/ih` paths (production path length <=64 bytes). Remove all provider secrets
before adopting genesis. Configure the installed runtime's explicit model
`request_max_tokens=4096` descriptor and localhost DeepSeek route; the runner does
not silently rewrite charter/model behavior. The upstream credential is supplied
only through owner `DEEPSEEK_API_KEY`. Root receives a freshly generated gateway
bearer in its provider key file, never the upstream key or inherited owner HOME.
The synthetic token is removed before snapshotting.

For each task the runner restores its arm snapshot, claims execution durably,
materializes only workspace inputs, writes owner-only benchmark policy, launches
the external gateway and Root, verifies exact Root-reported raw policy hash,
sets workspace, observes real owner busy→idle, stops Root and waits for no runtime
instances, terminates only its own processes, externally scores files, and writes
receipt/owner trace/ceilings/lineage. Frozen artifact changes fail the run. Each
transfer restores its arm's post-learning snapshot independently. Root egress
accounting remains cumulative outside restored snapshots; the external ledger is
never snapshotted. Unknown/crashed upstream requests retain their full reservation.

Receipts include original binary/source bindings, source-snapshot hash, before/after
artifact hashes, score, observed duration, accounted microUSD, output workspace,
lineage start offset/hash, and final lineage hash. Attribution explicitly remains
`requires_independent_lineage_verification`: a score increase is not proof that an
admitted capability caused it. Parent must check accepted evolution plus actual
capability use/reuse in the task's new signed lineage segment.

### Global hard cap revision (protocol v2; supersedes byte-bound global reservations)

Before **every** upstream POST, reserve **$0.319488** globally: the full
1,048,576-token context at $0.30/M input plus 4,096 output tokens at $1.20/M.
`requests.global_held` records this independently of the smaller byte-based task
estimate in `held`. Task/category envelopes are planning estimates until actual
usage arrives; they are not claimed to bound provider tokenizer/framing costs.
Known validated usage settles both columns. Unknown usage keeps the full global
reservation. Thus 31 unknown calls hold $9.904128 and the 32nd is refused before
transport. The $10 global cap no longer depends on the byte-token approximation.
A usage or output contract violation still halts the run. Parent must freeze the
current provider context/prices before qualification. New manifests use version 2;
the gateway refuses ledgers missing its v2 full-context policy rather than silently
continuing an old experiment. No paid requests preceded this protocol revision.

For ephemeral workers, `Gateway(..., checkpoint=callback)` invokes
`callback(event, request_id, sqlite_serialized_bytes)` synchronously **after** the
local reservation transaction and **before** any POST, then after settlement.
A reserve-sync failure makes no upstream call and leaves a pending full hold.
A settle-sync failure retains the full hold locally and halts; the remote last
acknowledged hold also stays full. CLI `--checkpoint-adapter OWNER.py` loads an
owner-only `build_checkpoint(ledger_path)` factory. The adapter must acquire a
permanent single-worker run lock, durably append sequence/predecessor/SHA-bound
immutable checkpoints, and acknowledge head advancement before returning. A
crashed worker must never silently initialize/reset/unlock the run. Runner config
`ephemeral_worker: true` requires `checkpoint_adapter` and its SHA256. Actual Modal
storage/locking implementation is parent-owned; this hook is not a claim it exists.

### Offline bootstrap and actual native qualification commands

After parent compiles exact native binaries, supply a JSON map with `init-root`,
`init-kernel`, `initctl`, `init-mockllm` pointing to those binaries. On a disposable
cloud checkout with >=27 GiB free (not this disk-constrained laptop):

```
python3 tests/real/release_ab/bootstrap.py --binaries /owner/binaries.json \
  --output "$PWD/target/ab-homes/bootstrap" --toolchain /trusted/rust-toolchain --install
python3 tests/real/release_ab/qualify_native.py \
  --test-binary /owner/compiled-init-root-tests --root-binary /owner/init-root \
  --output "$PWD/target/ab-homes/qualification"
```

Bootstrap packages the extracted current source tree, including full selected
vendor crates on Linux, into temporary Git repositories/bundles; it does not commit
to the project. It avoids existing release packaging's HEAD/dirty mismatch and
macOS-only clone/stub behavior. The supported native install always builds the Body
from seed, so copying a prebuilt Body is not a supported shortcut. `--no-service`
prevents service installation; HOME must equal passwd HOME for broker rules, while
explicit INIT homes remain disposable. Packaging/install logs and source-tree,
bundle and binary hashes are preserved. No keys or provider calls are involved.

Qualification executes the actual compiled `linux_zone_real_isolation` test and
requires exactly one passing test (zero matching tests fails). Then it starts real
Root/Kernel/Body against `mock_gateway.py`, whose upstream is in-memory scripted
responses and contains no external transport. Captures go through the production
Gateway validation/ledger. It induces a length cutoff, then asks the normal Body
bash tool to attempt writes to existing frozen artifact directories and a normal
`initctl gate`. It checks tool-result-channel denial markers, unchanged artifact
hashes, and the explicit `root.benchmark_frozen` code rather than assuming every
failure proves freeze. Both genesis and transfer policies are exercised. Scenario
labels are intentions, not proof of the runtime path; raw captures are retained.
Compaction, probe, learned-capability restart/reuse, rollback and direct Root route
bypass qualification remain explicitly missing until separately observed. The
script never emits an overall passed qualification receipt from partial evidence.

Linux runner config `workspace_mode: "internal_tasks"` uses the already-writable
INIT_HOME/work/tasks/<task-id>, with no workspace/home grant. Both arms receive the
same deterministic workspace instruction; its effective prompt hash is recorded.
Outputs are copied to owner evidence after stopping. Frozen artifact paths are
`body`, `bin`, `kernel`, `work/body`, `work/kernel`; these exact paths must also be
bound in the actual freeze receipt. A qualified broad workspace grant can continue
using the default external-workspace mode. Current stdlib test count: **52 passed**;
no native qualification or paid A/B was executed by this lane.

### Trusted cloud checkpoint IPC integration

The cloud gateway must load the **thin** `work/modal/checkpoint_client.py` adapter,
not a Modal client. The trusted worker owns `Broker(store, worker, random_token)`
and serves it with `checkpoint_broker.serve(broker, socket_path)` on an owner-only
Unix socket. Only `INIT_AB_CHECKPOINT_SOCKET` and `INIT_AB_CHECKPOINT_TOKEN` are
allowlisted from runner into gateway; neither reaches Root. Modal credentials stay
in the trusted worker. The old direct Modal gateway factory is explicitly disabled.

The first IPC attach claims the session permanently and checkpoints initial SQLite.
Subsequent per-task gateway attaches must present bytes whose digest equals the
latest acknowledged checkpoint; they do not reacquire the permanent claim. Every
checkpoint carries its predecessor digest. Stale writers, divergent attaches and
uncertain durable writes halt the broker. A new worker cannot reclaim/reset even
with the same worker string. Unauthorized probes do not poison the owner session.

Runner task-journal mutations are also durable: attach current state, mutate task
admission/completion, then send an `owner` checkpoint. This prevents task metadata
from drifting between the last gateway settlement and the next task's attach.
Checkpoint failures halt before further launch. The permanent worker claim remains
for explicit recovery inspection; no automatic unlock or ledger reset exists.
Eight offline IPC/checkpoint tests cover repeated attaches, sequence races, crashed
worker reclaim denial, unauthorized probes and uncertain head persistence. No
Modal API was invoked by these tests.

### Minimum safe L0 body pilot gate (separate from full release coverage)

Missing normal-loop compaction or probe captures alone need not prevent a small
pilot when the immutable Root benchmark policy and external gateway reject every
upstream request with a wrong model or output bound. Such unqualified runtime paths
can fail the task and must be reported as failures; they cannot bypass the global
reservation/output guard. Full release coverage still requires those paths.

`aggregate_pilot.py --config INPUT.json --output NEW_DIRECTORY` produces
`evidence.json` and `report.json` from actual owner-controlled qualification output.
Its config requires `manifest`, `native`, `evolution`, `runtime_binaries`,
`source_archive`, `execution_context`, `billing`, and `durable_ledger`. The first
three are paths to the frozen manifest and native/evolution qualification folders;
the next two are the exact binary hash mapping and source archive `{path,sha256}`.
Execution context must bind the same repository/binary/protocol/runtime/source
identities and record free disk bytes plus credential isolation. Billing and cloud
durability receipts come from their trusted owner lanes, not from this aggregator.

Mandatory evidence remains: actual native isolation test; genesis and transfer
filesystem/change denials and unchanged artifacts; actual Root wrong-model and
4,097-output-token denials observed through normal Body tools; gateway-captured
requests; verified body admission; stopped/fresh-boot capability reuse in Root's
usage records; actual owner rollback and parent restoration; exact source/binary
bindings; >=27 GiB pre-run free space; credential separation; checked pricing/live
balance; full-context reservations; durable pre-POST single-writer checkpoints and
fail-closed recovery. Missing logs cannot be replaced by model narrative. Any input
integrity failure yields a blocked aggregation receipt that runner preflight also
requires, so ignoring `report.json` cannot turn a failed aggregation into readiness.

Pilot `readiness.py` scope is `body-pilot-v2`; it still requires separate
`universal_request_guard` and `durable_ledger` receipts. Existing full-coverage
readiness remains strict. Explicit remaining release gaps include compaction/probe
behavior, kernel evolution at legitimately earned L1, and other OS/provider native
qualification. No native or paid success has been inferred from the 55 passing
offline unit tests.
