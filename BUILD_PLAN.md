# Overnight build plan, 2026-10-20

*Orchestrated by the main Claude session while the owner sleeps. Progress is logged in `BUILD_LOG.md`.*

## Goal for the morning

Full Phase 0 (M0–M6) is about 189 agent-hours, roughly 5 calendar days (docs/roadmap.md). Tonight aims at:

- **M0 done:** repo skeleton, all three Cargo workspaces building offline, `wasm32-wasip2` installed.
- **M1 done:** `init-proto` and `initctl`, plus the $0 test kit (mock LLM, fake body, fake kernel, fake Root, scenario runner).
- **M2/M3 well underway:**
  - `init-root` R5, R4 and R1
  - kernel supervise and the gate core
  - body log, context, loop and tools
  - the macOS sandbox profiles and self-test
  - proxy features

Every WP is committed only when its own tests are green. Nothing is claimed done without a passing test.

## Decisions for tonight (owner, 2026-10-20)

- Disk floor lowered to **5 GB free** for tonight; the normal rule is 20 GB. Lanes pause if free space drops below 5 GB.
- **No quota limit** tonight.
- **DeepSeek key:** stored at `~/.config/init-agent/deepseek.key` (mode 0600), outside the repo. Agents read it only from that file. They never print it, log it, commit it, or pass it in a prompt. The account holds **$10 in total**, so it is used sparingly, for flow tests only. Tonight's real-model spend is capped at **$1.00**, using `deepseek-flash` off-peak. Benchmarks wait until the build is complete.
- Every other test uses the $0 mock model.
- The constitution text is built as written; it is still PENDING OWNER (awareness.md §3.3).
- Charter template: `max_level = 4`; `/panic` demotes; mirror goes to `nagisanzenin/INIT-Lineage`, created at M0, not tonight.

## Priority tonight: one thin end-to-end slice

The owner wants to see the whole path work end to end ("thông luồng"). After the unit-level WPs, the lanes converge on **Slice-1**:

```
owner message → init-root (charter, keys, egress forwarder, money ceiling)
             → kernel (spawn the body through the Root; proxy forwards to the Root egress)
             → body (log, context, loop, tools, DeepSeek adapter)
             → deepseek-flash → a tool call (write a file) → the result back to the owner
```

Slice-1 runs against the mock first ($0). Then it runs once against the real DeepSeek, costing at most a few cents. The full ladder, T2 handoff, gate G2/G3, sealed eval, ext host and A1–A6 come after Slice-1.

## Shape

- **One checkout, many crates.** The kernel workspace is split into library crates, one per lane area (`kernel-supervise`, `kernel-proxy`, `kernel-gate`, `kernel-selfmap`, `kernel-serve`, `kernel-sandbox`), linked by a thin `init-kernel` binary. A broken crate in one lane therefore never blocks another lane's tests.
- **Target dirs:** one per workspace (`kernel/`, `root/`, `seed/`), shared by every lane in that workspace, to save disk.
- **Vendoring:** a single repo-root `vendor/`, not committed, regenerable with `cargo vendor --sync`.
- **One agent per work package**, so each starts with a fresh context. When a WP finishes, the orchestrator starts the next WP in that lane.

| Lane | Sequence tonight (WP ids from roadmap.md) |
|---|---|
| 0. setup | WP0 skeleton + vendoring + `wasm32-wasip2` |
| R (root) | WP20 skeleton/charter/CLI → WP23 lineage → WP21 R1 → WP22 egress → WP25 ledger → WP24 sealed-eval min |
| K1 (critical path) | WP1 init-proto/initctl → WP4 supervise → WP7 serve |
| K2 | WP8 gate (G0, G1, G4, statistics, evo validator) → WP9 selfmap |
| T (testkit) | WP2 init-mockllm → WP3 fakes + scenario runner |
| B (body) | WP12 log + context → WP11 loop + tools → WP10 provider → WP13 boot + awareness files |
| X | WP6a macOS sandbox → WP5 proxy → WP14 WIT + ext host |

## Safety rules for every build agent

- Signal or kill only processes the agent spawned itself, identified by PID. No sweep or pattern kill on the live system. Sweep logic is tested against fake process tables only.
- Tests use temporary homes inside the repo (`target/test-homes/…`). They never touch `~/.init`, `~/.init-root` or any other path outside the repo.
- Network access is limited to crates.io (cargo fetch and vendor) and GitHub (push, by the orchestrator only). The owner's email and git config are never sent anywhere.
- The only allowed install is `rustup target add wasm32-wasip2`.
- Delete files only by exact path.
- No real model API calls.

## Morning checklist for the owner

1. Read `BUILD_LOG.md`: progress %, what is green, and what is blocked.
2. Restart Docker Desktop. A sandbox test killed it at 01:20.
3. Free disk back above 20 GB, or 35 GB for the full Phase 0.
4. Read the CONSTITUTION v0.3 text (docs/awareness.md §3.3).
5. Provide the DeepSeek key for the end-to-end flow test.
