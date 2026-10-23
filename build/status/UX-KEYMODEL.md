# UX-KEYMODEL: provider, key and model inside the app (v0.2.0-preview, macOS)

Status: **done** (2026-10-23). $0: no real model call, test keys only, test homes under `*/target/test-homes/`.

## What a user types
1. `init` (Root running, no usable key) → `[setup] Choose a model provider` → `1` (DeepSeek, live-tested; 2–5 are
   marked "experimental (not live-tested)") → for a gateway/local server, the URL → the key in a hidden field
   (the box shows `•`) → `Choose a model` → `1`/`2` → the Root applies it and restarts the agent → chat.
2. Later: `/model` (numbered list, a number switches), `/key [provider]`, `/setup`, `/clear` (= `/new`).
3. Shell: `init-root providers list` · `init-root models [--provider P]` ·
   `init-root providers add <preset> [--base-url URL] [--model ID] [--name ALIAS] [--price IN,CACHED,OUT]` ·
   `init-root model use <provider>/<model>` · `init-root keys set <id>` (unchanged, TTY only).
Existing DeepSeek users (key present, selected model priced) see nothing new: `providers.list` says `ready`.

## Done
- Root `src/providers.rs` (+ `src/providers/tests.rs`): catalog of 5 presets (deepseek live: deepseek-flash,
  deepseek-v4-pro; gateway, anthropic (Messages), openai (Responses), local no-auth: experimental). Prices for
  Anthropic/OpenAI from research_notes …/llm_api_providers.md (verified 2026-10-20).
  - `providers add`: rewrites the charter's `[actions.providers]` class as one explicit block (destinations, paths,
    the provider entry, the µUSD tariff), keeps every other section byte-identical, re-parses and compares with the
    intended table (refuses otherwise), validates with `charter::parse`, writes 0600, applies through
    `charter.apply` (daemon) or `cli::apply_own_charter` (factored out of `charter check --apply`); restores the old
    text if the apply fails; refuses while charter.toml has unapplied edits. Adds the kernel
    `[providers.<id>]` + model table (decimal USD) without selecting it.
  - `model use`: `[model]` (+ `[model.descriptor]` for a custom alias) in `INIT_HOME/kernel/config.toml`, only
    that section replaced; checked against the active charter; daemon: `Ev::Restart(Plan::plain("cold"))` when the
    zone runs (existing supervisor stop/start), else at the next start.
  - Owner-socket requests: `providers.list`, `providers.add`, `model.use`, `keys.set`. `keys.set` is answered in
    `link.rs conn`: the secret is moved out of the request and zeroed, the frame buffer is zeroed, nothing is logged;
    test mode refuses non-`test-` keys (as the egress does).
- TUI `src/setup.rs` (pure state machine like access.rs) + app.rs/line.rs/conn.rs wiring: first run, `/key`,
  `/model`, `/setup`; hidden field in a preallocated 1 KiB buffer, never in the editor/history/transcript, frame
  built by hand and zeroed (`conn::spawn_key`). Line mode on a terminal cannot hide input, so it points to
  `init-root keys set`. `/clear` → `/new` (`proto::canonical`), in completions and the `?` help.
- Docs: provider-configuration.md "In the app" section and status line; tui.md §3.8 rows.

## Tests (all $0)
- `cd root && cargo test --offline -p init-root --features test-fixtures --bin init-root providers` → 9 passed:
  catalog/prices, URLs, spec refusals, the Chat gateway, local, Responses and Messages fragments compared with
  docs/provider-configuration.md's own toml blocks (parsed from the doc), section edits with multi-line strings,
  charter edit keeps the rest of the file, kernel edits keep the owner text, and the daemon request path on an
  installed test home (key 0600, trimmed, test-mode refusal, only the key file holds the key).
- `cd tui && cargo test --offline --bin init-tui --test proto --test tty` → 65 + 13 + 2 passed (setup flow and
  `/clear` unit tests included).
- `cd tui && INIT_ROOT_BIN=… cargo test --offline --test e2e_root uxk` → pass: pty TTY first run against the real
  test-mode Root and fake kernel (provider → hidden key → model v4-pro → kernel restart → chat round trip →
  `/model` back to flash → `/clear` reaches the kernel as `/new`); the key is absent from the screen bytes, the
  kernel's owner lines and every file under both homes except `keys/providers/deepseek` (0600).
- `cd tui && INIT_TUI_E2E=1 … cargo test --offline --test e2e_root` (frozen copies of init-root, its fake kernel and
  init-mockllm) → 3 passed: the existing TUI-R and WIRE-009 sessions still pass with the first-run check.
- `cd root && cargo test --offline -p init-root --features test-fixtures`: the bin's 215 unit tests and the
  integration tests pass, except `wp22` (2 tests: their test-home path is 84 bytes > 80 in this checkout's long
  path, before any Root code runs) and `wp24::sealed_runs_refuse_sealed_reads_and_tiny_cap_is_partial` (the audit
  `partial` flag under a tiny audit cap; its `charter check --apply` step, the path this lane refactored, succeeds).
  Neither touches this lane's code. Rerun alone, wp24 fails the same way: the audit spent 10649 µUSD under a 100 µUSD
  cap and reports `partial:false` (the sealed lane's cap no longer stops it, likely since the observe-only money
  change); for the sealed/egress owner.
- `cargo clippy -D warnings`: tui clean; init-root: no finding in providers.rs/cli.rs/link.rs (pre-existing
  findings in money.rs, zone.rs, sys.rs, sealed/*.rs are other lanes').

## Deviations
- No live key test (`keys.set{test:true}` of ux-v1 §3.4.4): owner rule "no real model calls"; `tested:false` is
  returned. No macOS Keychain backend (file store, as today).
- Providers join the built-in `providers` class (not a class per provider): safe mode only allows that class.
- Official presets must use catalog models; another model goes through `gateway` (or `anthropic`/`openai` with
  `--base-url`, which become `anthropic-gw`/`openai-gw` aliases with a descriptor). OpenAI's >272K tier and
  Anthropic's cache-write tariff are not in the Root tariff (observe-only estimates).
- A gateway without `--price` is $0 ("unknown"), and both the CLI and the TUI say so.

## Needs from others
- UX-BUNDLE: get.sh/installer can leave the provider to `init`'s first run; a Root-written kernel config without
  `[model]` works (model use adds the tables).

## Left
- Live qualification of every non-DeepSeek preset; the `keys.set` live test; Keychain store.
