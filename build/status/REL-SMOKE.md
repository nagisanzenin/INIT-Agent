# REL-SMOKE: one live DeepSeek turn through the v0.2.0-preview bundle

Status: **PASS** (2026-10-24 01:45–01:50, lane REL-SMOKE). Owner-authorized real spend, cap $0.05; spent ≈ $0.002.

## What ran
- Bundle `target/dist/v0.2.0-preview/init-v0.2.0-preview-macos-arm64.tar.gz` (sha256 `7d682f63…16c9`), installed by
  `scripts/get.sh --from <tarball> -- --no-service --disk-floor 5` with the isolation of `scripts/bundle-e2e.sh`:
  HOME, INIT_ROOT_HOME, INIT_HOME under `target/test-homes/rels/` (ROOT_HOME 62 bytes), PATH
  `/usr/bin:/bin:/usr/sbin:/sbin` (no Rust), **no mock**: the charter's provider destination `api.deepseek.com:443`,
  kernel `[model] deepseek/deepseek-flash`. get.sh took 10 s.
- `init-root start --foreground` (our child), then the installed `init` in a pty: the TUI first run
  (`Choose a model provider` → `1` → `Paste the DeepSeek API key`, the key typed from the owner's key file into the
  hidden field, which showed `••••` → `key stored for deepseek` → `Choose a model` → `1` →
  `model deepseek/deepseek-flash`). The key file landed 0600, 35 bytes. Setup to ready: 11 s.
- One owner line through `init --raw`: "Reply with the word ok." → reply **`ok`**, idle after 1.7 s, 1 upstream
  request (watchdog: `/stop` above 6 requests or $0.03; it did not fire).
- `init-root egress stats`: 1 request, 6,269 input tokens (cache_read 0, so all cache-miss), 87 output tokens,
  **1,986 µUSD ($0.0020)** estimated, policy `observe`.
- Root stopped (our PID only), the installed `uninstall.sh` (typed `uninstall`) moved the homes into the temp home's
  own `.Trash`, then the temp home was removed by exact path.

## Money
- DeepSeek balance (`GET /user/balance`, key read in-process, never in argv): before 01:43 **$12.19**, after 01:46
  **$12.19**, after 01:49 **$12.19**. The API reports cents, so a $0.002 turn does not show; the delta (< $0.01) is
  consistent with INIT's $0.0020 estimate.

## Key hygiene (`grep -cF -f <keyfile>`, counts only)
- Temp home before deletion (22,525 files, Trash included): 1 file holds the key, the 0600
  `keys/providers/deepseek`. Every log/transcript this lane wrote (get.log, root.out, pty screen bytes, raw attach
  frames, drive.json, egress stats, status, usage ledger, uninstall.log, scripts): 0. `build/status/`: 0.

## Owner state
- Snapshot of ~/.init, ~/.init-root (sealed/ unlisted), ~/.local/bin, ~/.local/share/init, ~/Library/LaunchAgents
  and the shell rc files: identical before, after the stop, and after the cleanup.

## Notes
- The cache-read count was 0 on this first call (a cold prompt), so no cache hit is shown yet.
- Not covered: a multi-turn session, tool use against the live model, the launchd service path.
