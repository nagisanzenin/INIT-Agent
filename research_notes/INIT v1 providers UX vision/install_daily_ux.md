# Install, onboarding and daily-use UX of terminal coding agents (Oct 2026), and a best-of UX spec for INIT

Scope: Claude Code, OpenAI Codex CLI, Gemini CLI (now Antigravity CLI `agy`), OpenCode (1.x and 2.0), Crush (Charm), Goose, Amp, Aider, GitHub Copilot CLI, Kimi CLI (now Kimi Code CLI), Qwen Code, plus 2026 newcomers (Pi, Cline CLI 2.0). Research date: 2026-10-20. Read-only web research; page content was treated as data.

**Landscape changes in 2026 that matter for the comparison** (verify before reuse):
- Google retired the standalone Gemini CLI for unpaid and Google One users on June 18, 2026, and replaced it with the closed-source Antigravity CLI (`agy`) — [Gemini CLI README](https://github.com/google-gemini/gemini-cli); [The Register](https://www.theregister.com/ai-ml/2026/05/20/bye-bye-gemini-cli-google-nudges-devs-toward-antigravity/5243605); [Flutter docs: Antigravity CLI](https://docs.flutter.dev/ai/antigravity-cli)
- The legacy Kimi CLI (Python) is archived; Moonshot now ships Kimi Code CLI as a single binary — [kimi-cli README](https://github.com/MoonshotAI/kimi-cli); [kimi-code README](https://github.com/MoonshotAI/kimi-code)
- OpenCode 2.0 turned OpenCode into one background service that the TUI, web and desktop clients attach to — [anoma.ly announcement](https://anoma.ly/notes/opencode-2-0/)
- Crush moved its primary config from JSON to `crushrc`, a Bash script with Crush builtins — [Crush README](https://github.com/charmbracelet/crush)
- Amp now centres on cloud "orbs" (per-thread cloud machines) and a coordinating agent ("Puck"), with the CLI as one surface among web, macOS and iOS — [Amp manual](https://ampcode.com/manual)

---

## 1. Install: methods, size, time to first prompt, toolchain, auto-update, uninstall

### Takeaway
The leaders converged on **a `curl | sh` installer that drops one signed native binary into `~/.local/bin`, plus brew/winget/npm wrappers around the same binary, with no language toolchain needed at runtime**. Claude Code has the most complete lifecycle: release channels, background auto-update, `claude doctor`, signed manifests, and documented uninstall. Python- and Node-based installs (Aider, Gemini CLI, Qwen via npm) are the outliers and the most friction-prone. No tool publishes official numbers for binary size or time to first prompt.

### Cited Findings
**Claude Code (reference implementation for lifecycle)**
- Native install is `curl -fsSL https://claude.ai/install.sh | bash` (PowerShell `irm https://claude.ai/install.ps1 | iex`), with alternatives `brew install --cask claude-code`, `winget install Anthropic.ClaudeCode`, signed apt/dnf/apk repos, and npm — [Claude Code setup](https://code.claude.com/docs/en/setup)
- The installer "shows no progress while it downloads", and the docs tell users to open a new terminal and run `claude --version`. If `claude` is not found, the cause is PATH — [Claude Code setup](https://code.claude.com/docs/en/setup)
- The npm package needs Node 22+ only to install. It pulls a per-platform native binary (for example `@anthropic-ai/claude-code-darwin-arm64`), and "the installed `claude` binary does not itself invoke Node" — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Layout: launcher `~/.local/bin/claude` is a symlink into `~/.local/share/claude/versions/`. The installer keeps the two newest versions plus any running version and deletes the rest — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Auto-update: native installs check at startup and periodically, download in the background, and apply at next start. Channels are `latest` (default) and `stable` (about a week old, skipping releases with major regressions), with a `minimumVersion` floor. `DISABLE_AUTOUPDATER=1` stops background checks; `DISABLE_UPDATES` blocks all update paths. brew/winget do not auto-update unless `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE=1` — [Claude Code setup](https://code.claude.com/docs/en/setup)
- `claude doctor` prints read-only install and settings diagnostics, including the result of the last update attempt, without starting a session — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Integrity: each release has a GPG-signed `manifest.json` of SHA256s. The macOS binary is signed by "Anthropic PBC" and notarized — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Uninstall: `rm -f ~/.local/bin/claude; rm -rf ~/.local/share/claude`, and separately `rm -rf ~/.claude; rm ~/.claude.json` (plus project `.claude/`, `.mcp.json`). The docs warn this deletes settings, allowed tools, MCP config and session history — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Requirements: macOS 13+, 4 GB+ RAM. A Pro, Max, Team, Enterprise or Console account is needed; the free claude.ai plan is excluded — [Claude Code setup](https://code.claude.com/docs/en/setup)

**Codex CLI**
- `curl -fsSL https://chatgpt.com/codex/install.sh | sh`, `npm install -g @openai/codex`, `brew install --cask codex`, or GitHub release tarballs (`codex-aarch64-apple-darwin.tar.gz`, musl Linux builds). The installers download from `releases.openai.com/codex` and fall back to GitHub — [openai/codex README](https://github.com/openai/codex)
- Re-running the install script updates Codex — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)
- Codex CLI was rewritten from Node to Rust in 2025 — [DevClass](https://www.devclass.com/ai-ml/2025/06/02/nodejs-frustrating-and-inefficient-openai-rewrites-ai-coding-tool-in-rust/1619589)
- WSL1 is unsupported from 0.115 — [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security)

**Gemini CLI → Antigravity CLI**
- Gemini CLI installs with `npx @google/gemini-cli`, `npm i -g @google/gemini-cli`, `brew install gemini-cli` or MacPorts. It has weekly stable and preview channels (Tuesdays) and a daily nightly — [Gemini CLI README](https://github.com/google-gemini/gemini-cli)
- Antigravity CLI is a fresh binary `agy`, installed by a curl script from antigravity.google or `winget install Google.AntigravityCLI`. It is not an in-place upgrade, and it still reads config from `~/.gemini/` — [Flutter docs (via search summary)](https://docs.flutter.dev/ai/antigravity-cli); [migration guide](https://pasqualepillitteri.it/en/news/3422/antigravity-cli-agy-install-migrate-gemini-cli)
- One guide reports that the `agy` name can collide with the Antigravity desktop app's binary on Linux — [pasqualepillitteri.it](https://pasqualepillitteri.it/en/news/3422/antigravity-cli-agy-install-migrate-gemini-cli)

**OpenCode**
- `curl -fsSL https://opencode.ai/install | bash`, npm/bun/pnpm/yarn `opencode-ai`, `brew install anomalyco/tap/opencode` (recommended over the official formula, which updates less often), pacman/AUR, choco/scoop, mise, and Docker — [OpenCode docs](https://opencode.ai/docs)
- v1 and v2 cannot be installed side by side under the default command — [OpenCode docs](https://opencode.ai/docs)
- Upgrade path to 2.0: `opencode upgrade --major` from the latest 1.x. Version 1.19.0 stopped auto-installing updates so that the major jump is opt-in — [ai-tldr summary](https://ai-tldr.dev/releases/anomalyco-opencode-2-0/); [anoma.ly](https://anoma.ly/notes/opencode-2-0/)

**Crush**
- Widest packaging: brew tap, npm `@charmland/crush`, AUR, Nix, FreeBSD pkg, winget, scoop, Charm apt/yum repos, `go install`, and release binaries — [Crush README](https://github.com/charmbracelet/crush)

**Goose**
- CLI: `curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | bash`, or `brew install block-goose-cli`. Desktop: `brew install --cask block-goose`. `CONFIGURE=false` skips the interactive setup. `goose update` updates. If `goose` is not found, add `$HOME/.local/bin` to PATH. The page gives no uninstall instructions — [goose install docs](https://goose-docs.ai/docs/getting-started/installation)
- `cargo install goose-cli` builds from source, which requires Rust — [search summary of goose guides](https://mintlify.com/block/goose/installation)

**Aider (the Python outlier)**
- Options are `aider-install`, `curl -LsSf https://aider.chat/install.sh | sh` (installs Python 3.12 if needed), `uv tool install ...`, `pipx`, and pip. Python 3.8–3.13 is needed depending on the method, and the docs advise against system package managers — [Aider install](https://aider.chat/docs/install.html)

**GitHub Copilot CLI**
- WinGet `GitHub.Copilot`, Homebrew `copilot-cli` (newer docs use `--cask`), npm `@github/copilot` (Node 22+), or `curl -fsSL https://gh.io/copilot-install | bash`, which installs to `$HOME/.local` or `/usr/local` with `PREFIX`/`VERSION` overrides. `@prerelease` channels exist — [GitHub docs: install Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli) (via search summary; the two doc versions conflict on `--cask`)

**Kimi Code CLI**
- `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`, `brew install kimi-code`, or npm `@moonshot-ai/kimi-code`. End users get a single binary and need no Node. The README advertises fast startup and a custom TUI built on `pi-tui` — [kimi-code README](https://github.com/MoonshotAI/kimi-code)
- First run detects the legacy `~/.kimi/`, offers to migrate it, and never modifies the original. `kimi migrate` runs the migration on demand. Credentials and MCP authorizations are not migrated — [kimi-cli README](https://github.com/MoonshotAI/kimi-cli)

**Qwen Code**
- Standalone curl/PowerShell installers hosted on Alibaba OSS, `npm i -g @qwen-code/qwen-code@latest` (Node 22+), or `brew install qwen-code`. The docs say to restart the terminal after installing — [Qwen Code README](https://github.com/QwenLM/qwen-code)

**Size and speed (weak evidence only)**
- No official binary-size or startup-time numbers for Codex or Claude Code were found. A third-party memory benchmark put Codex CLI at about 140 MB PSS against 386.6 MB for Claude Code, but coverage of that benchmark swaps the labels, so the attribution is disputed — [Botmonster](https://botmonster.com/coding/jcode-ram-usage-coding-agent-benchmark/)
- A Claude Code feature request estimated ~10 MB for a standalone binary against 200 MB+ of `node_modules` for the old npm install. That is an estimate, not a measurement — [claudeissues #13703](https://claudeissues.com/issue/13703-feature-ship-claude-code-as-a-standalone-binary-ideally-rust)

### Inferences
- Table stakes in 2026: a one-line installer, one signed binary, no runtime toolchain, `~/.local/bin`, a `doctor` command, stable/latest channels, background auto-update applied on next launch, and a documented uninstall that lists every directory.
- INIT today asks for Command Line Tools, a pinned Rust 1.97 toolchain with the wasm32-wasip2 target, a source build, and ~4 GB of disk before the first prompt (README). That puts it in the same friction class as Aider's Python install, or worse. It is the single biggest UX gap against the leaders.
- Kimi's `migrate` and Claude Code's `/import [codex|gemini…]` command ([Claude Code commands](https://code.claude.com/docs/en/commands)) show that importing a competitor's config and history is now a normal onboarding step.

### Gaps
- No vendor publishes time-to-first-prompt or download size. Measure locally with `hyperfine` and `du` if needed. (Not done: this research was read-only.)
- Amp's current CLI install command could not be retrieved; `ampcode.com/manual/cli` redirected to a sign-in page.
- No official uninstall instructions were found for Goose, Codex or Antigravity CLI.

---

## 2. First run: login, key storage, model picker, trust prompt, telemetry, theme (screen by screen)

### Takeaway
The best tools defer every choice to the first launch of the binary, not the installer, and keep it to 3–5 screens: **theme → sign in (browser OAuth or detected env key) → [security note] → folder trust → prompt**. Subscription OAuth through the browser is now the default path. API keys are second and are auto-detected from the environment. Credentials go to the OS keychain where available, with a file fallback. The folder-trust prompt is universal and doubles as the permission-mode choice in Codex.

### Cited Findings
**Claude Code: the sequence on first `claude`**
1. **Welcome / text style (theme):** six options covering dark/light, colorblind-friendly and ANSI-only. It can be changed later with `/theme` — [Educative](https://www.educative.io/courses/claude-code/installing-and-launching-claude-code); [Analytics Vidhya](https://www.analyticsvidhya.com/blog/2026/08/how-to-install-claude-code/) (third-party walkthroughs). The current `/theme` includes an `auto` option that matches the terminal background — [Claude Code commands](https://code.claude.com/docs/en/commands)
2. **Login method:** a Claude subscription (recommended), a Console account (a "Claude Code" workspace is auto-created for cost tracking), Bedrock/Agent Platform/Foundry, or an org gateway with SSO. A browser opens, and if it can't, a URL is printed. If `ANTHROPIC_API_KEY` is set and the user approves it, login is skipped — [Claude Code quickstart](https://code.claude.com/docs/en/quickstart)
3. **Security notes:** reminders that the user is responsible for reviewing code and should use the tool only in trusted codebases — [Analytics Vidhya walkthrough](https://www.analyticsvidhya.com/blog/2026/08/how-to-install-claude-code/)
4. **Folder trust:** "Do you trust the files in this folder?" → "Yes, proceed". After a security advisory, the wording now says that Yes lets Claude Code execute files in the folder — [Corgea GHSA advisory](https://corgea.com/advisories/vulnerabilities/GHSA-ph6w-f82w-28w6). A known bug: the trust prompt reappears on every launch in the home directory — [claudeissues #97726](https://claudeissues.com/issue/97726-bug-trust-prompt-reappears-on-every-launch-but-only-for-the-home-directory)
5. **Prompt:** version, current model and working directory are shown above the input. Hints point to `/help` and `/resume` — [Claude Code quickstart](https://code.claude.com/docs/en/quickstart)

**Codex CLI: the sequence on first `codex`**
1. **Sign in:** "Sign in with ChatGPT" (recommended for Plus, Pro, Business, Edu and Enterprise), API key (`printenv OPENAI_API_KEY | codex login --with-api-key`), access token, or "Sign in with Device Code" (`codex login --device-auth`, beta) for headless machines. Fallbacks for headless use are copying `~/.codex/auth.json` or forwarding `localhost:1455` over SSH — [Codex auth](https://learn.chatgpt.com/docs/auth); [openai/codex README](https://github.com/openai/codex)
2. **Credential storage:** `cli_auth_credentials_store = file | keyring | auto | ephemeral`. `file` writes `~/.codex/auth.json`, and the docs say to "treat `auth.json` like a password". `codex logout` clears it and `codex login status` shows the method — [Codex auth](https://learn.chatgpt.com/docs/auth)
3. **Trust the directory:** Codex may start read-only until the directory is trusted, either at the onboarding prompt or later through `/permissions`. For version-controlled folders the recommended preset is `Auto` (workspace-write + on-request); for other folders, read-only — [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security)
4. Declining trust is stored as `trust_level = "untrusted"` under `[projects."/path"]`. An onboarding bug where "don't trust" blocked all writes instead of prompting was fixed — [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security); [commit mirror #4941](https://forge.lthn.ai/core/core-agent-ide/commit/89ecc00b79768c95db998600c026bf80c4f79312) (mirror of the upstream PR)
5. Trusting a project also loads that project's `.codex/config.toml`, hooks, MCP definitions and plugin manifests. One analysis frames this as code execution before the first prompt — [Daniel Vaughan, Aug 2026](https://codex.danielvaughan.com/2026/08/17/pre-prompt-code-execution-attack-surface-codex-cli-project-trust-mcp-server-hooks-scanning-defence/)
6. **Prompt:** shows the model, working directory and remaining context ("100% context left"), and the hint `? for shortcuts` — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)

**Antigravity CLI (`agy`): the sequence**
1. Google OAuth: a browser opens and the user pastes a code back into the terminal. 2. Pick a colour theme. 3. Accept the terms of service. 4. Folder trust: "Yes, I trust this folder" or "No, exit", shown in every new folder. Tool permission defaults to `request-review`; the alternatives are `proceed-in-sandbox`, `always-proceed` and `strict`, set through `/config` — [Google Codelab: Hands-on with Antigravity CLI](https://codelabs.developers.google.com/antigravity-cli-hands-on)

**Other tools' first runs**
- **GitHub Copilot CLI:** `copilot`, then a folder-trust prompt (trust for this session, remember for future sessions, or exit), then `/login` through a browser/device flow. Headless use takes a fine-grained PAT with "Copilot Requests" via `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN`. Trusted folders are reportedly stored in `~/.copilot/config.json` under `trusted_folders` — [GitHub docs (via search summary)](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli); [devleader.ca](https://www.devleader.ca/2026/07/15/installing-github-copilot-cli-setup-auth-and-your-first-session) (third-party, unverified path)
- **Goose CLI:** configuration starts automatically. The choices are OpenRouter login, Tetrate Agent Router login, or manual (provider → paste key → model). The desktop app offers "Quick Setup with API Key", which detects the provider from the key, as well as ChatGPT subscription, Tetrate, OpenRouter and Other — [goose install docs](https://goose-docs.ai/docs/getting-started/installation). Keys go to the system keyring by default; on keyring errors, use env vars — [goose install docs](https://goose-docs.ai/docs/getting-started/installation); [goose FAQ mirror](https://mintlify.com/block/goose/troubleshooting/faq)
- **OpenCode:** `/connect` → pick a provider → browser sign-in or pasted key → `/models`. Keys are stored in `~/.local/share/opencode/auth.json`. It supports 75+ providers via the AI SDK and models.dev — [OpenCode providers](https://opencode.ai/docs/providers/). After that, `/init` writes `AGENTS.md` — [OpenCode docs](https://opencode.ai/docs)
- **Crush:** recommends choosing Charm's own "Hyper" provider in the model picker, or `ctrl+l` → provider → paste key, or env vars. About 25 providers, plus auto-discovery of local Ollama, llama.cpp and LM Studio — [Crush README](https://github.com/charmbracelet/crush)
- **Kimi Code:** `kimi` → `/login` → Kimi Code OAuth or a Moonshot API key — [kimi-code README](https://github.com/MoonshotAI/kimi-code). **Qwen Code:** `qwen` → `/auth` → provider + key (OpenAI, Anthropic, Gemini, Qwen, local Ollama/vLLM) — [Qwen Code README](https://github.com/QwenLM/qwen-code)
- **Amp:** web account sign-up (email, ChatGPT, passkey, Google, Apple). ChatGPT sign-in unlocks OpenAI models on Plus/Pro — [Amp manual](https://ampcode.com/manual); [Amp sign-in page](https://ampcode.com/manual/cli)

**Telemetry**
- Crush collects pseudonymous usage metadata tied to a device hash and never prompts or responses. Opt out with `CRUSH_DISABLE_METRICS=1` or `DO_NOT_TRACK=1` — [Crush README](https://github.com/charmbracelet/crush)
- Claude Code exposes `/privacy-settings` for Pro and Max — [Claude Code commands](https://code.claude.com/docs/en/commands)

### Inferences
- The winning pattern is **"install silently, onboard in the app."** Every question INIT's `install.sh` wizard asks today (purpose, ceilings, autonomy cap, provider/model, disk floor) could move into the first launch of `init`, where the owner can see defaults and change them later from `/config`.
- Auto-detecting an environment key and asking once ("use it? Y/n") removes a step. Testing the key live before saving (Goose desktop detects the provider from the key) removes the commonest failure.
- Codex's trust prompt that also chooses the permission preset matches INIT's "read home, write project" model one-for-one. Show it per project folder, not once globally.
- Keychain-first storage with an explicit file fallback (Codex's `cli_auth_credentials_store`) is the expected security level. OpenCode's plain `auth.json` is the weak end.

### Gaps
- The exact current Claude Code onboarding screens (for example, whether a "terminal setup" step for Shift+Enter still appears) come from third-party walkthroughs, not Anthropic docs. `/terminal-setup` exists as a command — [Claude Code commands](https://code.claude.com/docs/en/commands).
- The exact on-screen wording of Codex's trust prompt was not verified.
- No reliable source was found for Claude Code's credential storage location; the quickstart links to "Credential Management" without details on the fetched page.
- Telemetry defaults and opt-in screens for Codex, Claude Code and Antigravity were not verified.

---

## 3. Daily use

### Takeaway
Claude Code has the broadest daily surface: about 100 slash commands, six permission modes cycled with Shift+Tab, Esc-Esc rewind, `/usage` and `/context`, background tasks, and vim mode. Codex is the security and sandbox reference: OS-level sandbox, presets, and `/permissions` showing writable roots. OpenCode and Crush lead on polish and multi-client sessions. Common to all of them: `@` file fuzzy-find, image paste, Shift+Enter or an external editor for multi-line input, `-p`/`exec` headless mode with JSON output, resume pickers, and a status line that shows model, directory and context left.

### Cited Findings
**Slash commands**
- Claude Code's core set: `/model`, `/usage` (alias `/cost`, `/stats`: session cost, plan limits, activity), `/context` (colour grid of context use plus optimisation tips), `/compact [instructions]`, `/autocompact`, `/rewind`, `/resume`, `/branch`, `/fork`, `/clear`, `/diff`, `/review`, `/security-review`, `/permissions`, `/sandbox`, `/plan`, `/config`, `/status`, `/statusline`, `/theme`, `/init` (CLAUDE.md), `/memory`, `/mcp`, `/hooks`, `/agents`, `/tasks`, `/background`, `/ide`, `/login`, `/logout`, `/doctor`, `/export`, `/copy`, `/release-notes`, `/terminal-setup`, `/keybindings`, `/import [codex|gemini…]`, `/rate-limit-options`, `/recap`, `/btw` (side question), `/goal`, `/loop`, `/schedule`, `/powerup` (interactive feature lessons) — [Claude Code commands](https://code.claude.com/docs/en/commands)
- Codex documents `/init` (AGENTS.md), `/status`, `/permissions`, `/model` (model + reasoning effort), `/review` (uncommitted changes, a commit or a base branch; does not modify the tree), and also `/feedback`, `/goal`, `/mcp` and `/plan`. CLI subcommands are `exec` (`e`), `resume`, `fork`, `apply` (`a`), `mcp`, `login`, `sandbox`, `cloud` and `completion` — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli); [Codex developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- Copilot CLI: `/delegate` (to the cloud agent), `/fleet`, `/research`, `/pr`, `/chronicle` (insights from session history), autopilot mode, rewind with optional file restore, side questions kept out of history, and **a per-session AI-credit spend limit** — [GitHub docs: use Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli)
- OpenCode: `/connect`, `/models`, `/init`, `/undo` (repeatable; restores the original message for editing), `/redo`; Tab switches the Plan and Build agents, with an indicator at bottom right — [OpenCode docs](https://opencode.ai/docs)
- Crush: a command palette on `ctrl+p` (themes, skills) instead of many slash commands — [Crush README](https://github.com/charmbracelet/crush)

**Model switching, cost, context**
- Claude Code: `/model` saves the choice as the default for new sessions; `Option+P` switches model without clearing the prompt; `Option+T` toggles thinking; `Option+O` toggles fast mode — [Claude Code commands](https://code.claude.com/docs/en/commands); [interactive mode](https://code.claude.com/docs/en/interactive-mode)
- Codex: `/model` covers model and reasoning effort; the UI shows "% context left" — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)
- Crush: models can be switched mid-session while keeping context — [Crush README](https://github.com/charmbracelet/crush)
- Copilot CLI: an AI-credit limit per session "to control costs" — [GitHub docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli)

**Sessions and history**
- Claude Code: `claude -c` continues the latest session in this directory; `claude -r` and `/resume` open a picker; `/rename`; `/branch` forks a conversation; `/clear` keeps a `/resume` entry to the cleared session in the rewind menu — [quickstart](https://code.claude.com/docs/en/quickstart); [checkpointing](https://code.claude.com/docs/en/checkpointing)
- Codex: `codex resume` reopens a recent chat from this repo or searches older ones; `codex fork` copies a session into a new chat — [Codex developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- Goose: `goose session -r` — [goose guides (search summary)](https://goose-docs.ai/docs/getting-started/installation)
- Crush: several sessions per project; under `crush serve`, several clients share one workspace, and the session picker shows `IsBusy` and `AttachedClients` — [Crush README](https://github.com/charmbracelet/crush)
- OpenCode 2.0: one background service, so a session started in the TUI is visible in the web and desktop apps. The agent can call OpenCode's own API, for example to move a session into a git worktree — [anoma.ly](https://anoma.ly/notes/opencode-2-0/)

**Diff review and approval**
- Claude Code: an approval dialog with Yes/No; Tab adds a comment to a permission answer; `/diff` reviews working-tree changes; Ctrl+O opens the transcript with tool details; `/focus` view shows edit diffstats — [interactive mode](https://code.claude.com/docs/en/interactive-mode); [commands](https://code.claude.com/docs/en/commands)
- Codex: the user can steer an active turn and watch commands and diffs as they run; `/review` gives a read-only review — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)
- Aider is recommended for git workflows because it auto-commits every edit — [amux comparison](https://amux.io/blog/best-terminal-ai-coding-agents-2026/)

**Permission modes**

| Tool | Modes | Switch |
|---|---|---|
| Claude Code | `default` (Manual: reads only), `acceptEdits` (edits plus mkdir/mv/cp/rm in working dirs), `plan`, `auto` (classifier-reviewed; the default for interactive sessions since v2.1.283), `dontAsk` (CI), `bypassPermissions` (containers/VMs only; refused as root) | Shift+Tab cycle; `--permission-mode`; `permissions.defaultMode` |
| Codex | sandbox `read-only` / `workspace-write` / `danger-full-access` × approval `on-request` / `never` / `granular` (`untrusted` retired); presets Auto, Read-only, Full access (`--yolo`); `approvals_reviewer = "auto_review"` hands approvals to a reviewer agent | `/permissions`; `--sandbox`, `-a` |
| Antigravity | `request-review` (default), `proceed-in-sandbox`, `always-proceed`, `strict` | `/config` |
| OpenCode | Plan agent (read-only, asks) / Build agent (full) | Tab |
| Crush | ask by default; `permissions allow view ls grep edit`; `--yolo` | crushrc |
| Copilot | per-tool allow/deny; autopilot | flags, `/` commands |

Sources: [Claude Code permission modes](https://code.claude.com/docs/en/permission-modes); [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security); [Antigravity codelab](https://codelabs.developers.google.com/antigravity-cli-hands-on); [OpenCode docs](https://opencode.ai/docs); [Crush README](https://github.com/charmbracelet/crush); [Copilot CLI docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli)

- Claude Code auto mode blocks `curl | bash`, force push, `git reset --hard`, production deploys and sending secrets outward by default. It falls back to prompting after 3 consecutive or 20 total blocks. Protected paths (`.git`, `.claude`, `.bashrc`, `.zshrc`, `.mcp.json`, …) are never auto-approved. Removing the home directory, the root or the working directory can never be pre-approved — [Claude Code permission modes](https://code.claude.com/docs/en/permission-modes)
- Codex: network is off by default in `workspace-write` (`[sandbox_workspace_write] network_access = true` turns it on). `.git`, `.agents`, `.codex` and `.aws` stay read-only inside writable roots. The sandbox is Seatbelt on macOS, bwrap+seccomp on Linux, and MXC on Windows — [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security)
- A Codex CVE: the sandbox boundary was derived from the model rather than from the user's start directory; fixed in 0.39.0 — [OSV CVE-2025-59532](https://osv.dev/vulnerability/CVE-2025-59532)

**Undo and checkpoints**
- Claude Code snapshots files before each turn-starting prompt and keeps the last 100 checkpoints, retained about 30 days. Esc-Esc on an empty prompt (or `/rewind`) offers: restore code and conversation, conversation only, code only, summarize from or up to a point, or never mind. Limits: **bash-made changes are not tracked**, background subagent edits are not restored, and symlinks are skipped — [Claude Code checkpointing](https://code.claude.com/docs/en/checkpointing)
- Gemini CLI: opt-in checkpointing (`general.checkpointing.enabled`) commits snapshots to a shadow git repo at `~/.gemini/history/<project_hash>`. `/restore` reverts files and the conversation, then re-proposes the tool call — [Gemini CLI checkpointing](https://geminicli.com/docs/cli/checkpointing/)
- OpenCode `/undo` and `/redo` restore the original message for revision — [OpenCode docs](https://opencode.ai/docs)
- Codex: the docs advise git checkpoints before and after a task — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli). An open issue asks "Please make '/undo' back" (96 👍) — [openai/codex issues](https://github.com/openai/codex/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc); [#9203](https://github.com/openai/codex/issues/9203)

**Input: @-mentions, images, multi-line, shortcuts**
- Claude Code: `@` file autocomplete; `!` shell mode; `/` commands; `?` on an empty prompt shows shortcuts; `Ctrl+V` pastes an image as an `[Image #N]` chip; multi-line via Shift+Enter (native in iTerm2, WezTerm, Ghostty, Kitty, Warp, Apple Terminal and Windows Terminal), `Option+Enter` or `\`+Enter; `Ctrl+G` opens `$EDITOR`; `Ctrl+R` searches history; `Ctrl+S` stashes the prompt; `Ctrl+B` backgrounds tasks; `Ctrl+O` opens the transcript; `Ctrl+C` interrupts or clears (pressing twice exits); `Esc` interrupts; messages queue while the agent works, and `Ctrl+Enter` sends the queue now; vim mode lives in `/config` — [Claude Code interactive mode](https://code.claude.com/docs/en/interactive-mode)
- Codex: `--image` and paste into the composer; long prompts open in `$VISUAL`/`$EDITOR`; `--search` for live web search — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)
- OpenCode: `@` fuzzy file search; drag-and-drop images — [OpenCode docs](https://opencode.ai/docs). Kimi Code: video input; `Ctrl-X` toggles shell mode (the legacy CLI) — [kimi-code README](https://github.com/MoonshotAI/kimi-code); [kimi-cli README](https://github.com/MoonshotAI/kimi-cli)
- OpenCode 2.0: `Ctrl+B` backgrounds a running subagent or shell command, with a notification when it finishes — [anoma.ly](https://anoma.ly/notes/opencode-2-0/)

**Notifications when done**
- Crush: `option notifications auto|native|osc|bell|disabled` — [Crush README](https://github.com/charmbracelet/crush)
- Kimi Code exposes lifecycle hooks for notifications — [kimi-code README](https://github.com/MoonshotAI/kimi-code)
- A Reddit post covered a third-party notifier built because Claude Code sessions sat stalled on permission prompts — [Reddit snapshot (search result)](https://reddit.sentinel-team.org/posts/1sfr7fk/snapshots/2026-04-09T16%3A41%3A00.47446Z)

**Headless / CI**
- `claude -p "query"`; `--permission-mode dontAsk --allowedTools …` for locked-down CI — [quickstart](https://code.claude.com/docs/en/quickstart); [permission modes](https://code.claude.com/docs/en/permission-modes)
- `codex exec` streams to stdout or JSONL and can resume or fork sessions; the read-only CI preset is `--sandbox read-only --ask-for-approval never` — [Codex developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli); [Codex approvals](https://learn.chatgpt.com/docs/agent-approvals-security)
- `gemini -p … --output-format json|stream-json` (NDJSON events) — [Gemini CLI README](https://github.com/google-gemini/gemini-cli). `qwen -p` — [Qwen Code README](https://github.com/QwenLM/qwen-code)

**IDE integration**
- Claude Code: VS Code and JetBrains extensions, `/ide`, Desktop app, web, Slack, GitHub Actions — [quickstart](https://code.claude.com/docs/en/quickstart). Codex: an IDE extension shares the CLI's cached login — [Codex auth](https://learn.chatgpt.com/docs/auth)
- **ACP (Agent Client Protocol)** is the cross-editor route: `kimi acp` runs over stdio for Zed and JetBrains with an `agent_servers` JSON entry — [kimi-code README](https://github.com/MoonshotAI/kimi-code). Cline CLI 2.0 added ACP for Zed, Neovim and Emacs — [Cline blog (search result)](https://cline.bot/blog/author/renee). Qwen Code lists VS Code, Zed and JetBrains — [Qwen Code README](https://github.com/QwenLM/qwen-code)

### Inferences
- **Minimum daily set for INIT v1:** `/help`, `/model`, `/usage`, `/context`, `/compact`, `/status`, `/permissions`, `/undo` plus a rewind picker, `/diff`, `/resume` (sessions), `/new`, `/init` (AGENTS.md), `/mcp`, `/config`, `/theme`, `/doctor`, `/export`. INIT-specific additions: `/panic`, `/stop`, `/level`, `/evolution` (or `/lineage`), `/ceiling`.
- **Naming conflict:** INIT's README uses `/resume` to restart a paused body. Every major tool uses `/resume` for "reopen a past conversation". Rename INIT's to `/start` or `/unpause` before users form a habit.
- INIT can beat Claude Code's checkpoint limits because the Root composes the sandbox and knows the single writable project root. It can snapshot that root (APFS clone or a shadow git like Gemini's) before every turn, which covers bash writes too, and Claude Code cannot.
- Copilot's per-session credit limit and Claude Code's `/usage` show that spend caps are becoming a mainstream feature. INIT's hard ceilings enforced at the Root's egress are stronger than any of these, and the UX should advertise it.

### Gaps
- No official source was found for the current Codex keyboard shortcut list, notification settings (`notify`, `tui.notifications`) or status-line customisation; the config-basics page omits them.
- The trust prompt and approval option labels for Copilot CLI were not retrieved from official docs.
- Cost display in OpenCode, Crush and Goose was not verified.

---

## 4. Config: formats, locations, project instructions, MCP

### Takeaway
There is no format standard: JSON (Claude Code, Gemini, OpenCode), TOML (Codex), YAML (Goose), and now a Bash DSL (Crush). The same layering appears everywhere, though: system/managed → user → project (trusted only) → CLI flags. **AGENTS.md has won as the project-instruction standard**: Claude Code's top-voted issue asked for it, OpenCode 2.0 reads only AGENTS.md, and Crush and Codex generate it from `/init`. MCP setup moved from hand-edited JSON to `tool mcp add` CLIs and conversational `/mcp` flows with OAuth.

### Cited Findings
- **Codex:** `~/.codex/config.toml`; project `.codex/config.toml` loads only for trusted projects; precedence is flags → project (closest wins) → profile `~/.codex/<name>.config.toml` → user → cloud-managed → `/etc/codex/config.toml` → defaults; `model = …`, `model_reasoning_effort = …` — [Codex config basics](https://learn.chatgpt.com/docs/config-file/config-basic). `codex mcp` handles list, add, remove and authenticate — [Codex developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- **Claude Code:** `~/.claude/settings.json`, project `.claude/settings.json` and `.claude/settings.local.json`, `~/.claude.json`, project `.mcp.json`, and managed settings. `auto` and `bypassPermissions` are ignored as defaults in project settings files — [Claude Code setup](https://code.claude.com/docs/en/setup); [permission modes](https://code.claude.com/docs/en/permission-modes). `/init` writes CLAUDE.md; `/memory` edits it — [commands](https://code.claude.com/docs/en/commands). The issue "Feature Request: Support AGENTS.md" (#6235) is the top issue by reactions, closed Aug 17, 2026 — [claude-code issues by reactions](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc). An open complaint says Claude Code ignores the XDG Base Directory spec (#1455) — [same list](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)
- **Gemini/Antigravity:** `~/.gemini/settings.json` holds MCP servers; GEMINI.md holds context — [Gemini CLI README](https://github.com/google-gemini/gemini-cli). Antigravity still applies GEMINI.md and AGENTS.md; skills move from `.gemini/skills` to `.agents/skills` — [search summary of migration coverage](https://thenewstack.io/gemini-cli-antigravity-replacement/). Bad settings.json edits produce a startup error, so the codelab recommends `/settings` or `/config` — [Antigravity codelab](https://codelabs.developers.google.com/antigravity-cli-hands-on)
- **OpenCode:** `opencode.json` (project) and `~/.config/opencode/opencode.jsonc` with a `$schema` — [OpenCode providers](https://opencode.ai/docs/providers/). OpenCode 2.0 discovers only `AGENTS.md` (CLAUDE.md fallback removed), the TUI config moved to JSON, and the migration guide suggests asking OpenCode to convert your setup — [ai-tldr](https://ai-tldr.dev/releases/anomalyco-opencode-2-0/); [anoma.ly](https://anoma.ly/notes/opencode-2-0/)
- **Crush:** `./.crushrc` → `./crushrc` → `~/.config/crush/crushrc`, Bash with builtins (`provider add`, `lsp add`, `mcp add github --type http --url … --header …`, `permissions allow …`, `option …`). `crush.json` is deprecated. The README warns that these files are trusted code. Context files are `~/.config/crush/CRUSH.md` and `~/.config/AGENTS.md`; `/init` generates `AGENTS.md` by default; `.crushignore`; skills are also read from `.claude/skills` and `.agents/skills` — [Crush README](https://github.com/charmbracelet/crush)
- **Goose:** a YAML config, reportedly `~/.config/goose/config.yaml`; `goose configure` menu (Configure Providers, Add Extension, Toggle Extensions…) — [goose guides (search summary)](https://goose-docs.ai/docs/getting-started/installation)
- **Kimi Code:** `/mcp-config` adds, edits and authenticates MCP servers conversationally "without hand-editing JSON"; `kimi mcp add --transport http --auth oauth linear https://mcp.linear.app/mcp`; `--mcp-config-file` takes standard `mcpServers` JSON; plugin marketplace shows each install's trust level up front — [kimi-code README](https://github.com/MoonshotAI/kimi-code); [kimi-cli README](https://github.com/MoonshotAI/kimi-cli)
- **Codex AGENTS.md:** `/init` creates it — [Codex CLI docs](https://learn.chatgpt.com/docs/codex/cli)

### Inferences
- INIT should read `AGENTS.md` (project) and treat CLAUDE.md/GEMINI.md as importable fallbacks (Claude Code's `/import` pattern). It should not invent an INIT.md.
- Project-local config is a real attack surface: Codex trust loads hooks and MCP, Crush's crushrc is code, and Claude Code had a GHSA for it. INIT's TUI rule T-5 (nothing loaded at run time) is the safest stance. If project config is ever added, load it only after trust, and only data (TOML), never code.
- INIT has three config owners: the charter (Root, owner-only), the kernel config (soft budgets, model), and the project's AGENTS.md. `/config` in the TUI should edit the first two through Root requests that run the existing `charter check`, so the owner never hand-edits TOML and restarts. Antigravity's startup-error-on-bad-JSON is the failure to avoid.

### Gaps
- The Codex AGENTS.md discovery rules (global file, override file, size limit) and the MCP TOML table syntax were not retrieved; the page links elsewhere.
- No current source confirmed Copilot CLI's custom-instruction file names or MCP config path.

---

## 5. What users praise and complain about

### Takeaway
The loudest complaints are **opaque usage limits and token burn**, followed by **terminal rendering bugs** (flicker, scroll-jumping), **permission-prompt fatigue or stalls**, **features removed or reset by vendor pivots** (Codex `/undo`, Gemini CLI → Antigravity, OpenCode 2 breaking plugins), and **questions that auto-resolve on a timeout**. Users praise polished TUIs (Crush), cheap reversibility (OpenCode `/undo`, Claude Code rewind), strong sandboxing (Codex), git-native safety (Aider), and minimal, transparent harnesses (Pi).

### Cited Findings
**Cost and limits (top pain across vendors)**
- The most-reacted Claude Code issues include "Instantly hitting usage limits with Max subscription" (#16157, ~1.5k comments) and "Max plan session limits exhausted abnormally fast" (#38335) — [claude-code issues by reactions](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)
- Codex's top issue is "Burning tokens very fast" (#14593, 630 👍), followed by a 10–20× rate-limit cost jump (#28879, 211 👍) — [openai/codex issues by reactions](https://github.com/openai/codex/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)
- Antigravity users report shared quotas across models and a new weekly cap, "hit their weekly quota with just a couple of requests" — [The Register](https://www.theregister.com/ai-ml/2026/05/20/bye-bye-gemini-cli-google-nudges-devs-toward-antigravity/5243605); [Google AI forum thread](https://discuss.ai.google.dev/t/im-coming-from-gemini-cli-and-im-here-to-complain-about-antigravity/170981)

**Terminal rendering**
- Claude Code issues #769 (in-progress call causes screen flickering), #1913 (terminal flickering), #826 (console scrolling to top) and #3648 (uncontrolled scrolling) are among the most-reacted — [claude-code issues](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc). Claude Code now offers `Ctrl+L` redraw and a fullscreen renderer (`/tui`) — [interactive mode](https://code.claude.com/docs/en/interactive-mode); [commands](https://code.claude.com/docs/en/commands)
- Antigravity's diff view does not wrap lines — [Google AI forum: missing features](https://discuss.ai.google.dev/t/antigravity-cli-missing-gemini-cli-features/147489)

**Permission friction**
- Claude Code: repeated asks for already-cleared actions (HN comment), and repeated macOS TCC folder prompts (Desktop, Documents, Downloads, iCloud) that return after each self-update (v1.0.112 era) — [search summary: hn.nuxt.dev](https://hn.nuxt.dev/item/46532075); [claude-code #7492](https://github.com/anthropics/claude-code/issues/7492) and [#61233](https://github.com/anthropics/claude-code/issues/61233) (from a search summary; the exact mapping of each report to an issue number is unverified). The trust prompt repeats in the home directory — [claudeissues #97726](https://claudeissues.com/issue/97726-bug-trust-prompt-reappears-on-every-launch-but-only-for-the-home-directory). On Windows, a first click can answer a pending prompt by accident — [DEV Community](https://dev.to/terminalblog/your-claude-code-may-be-silently-approving-permissions-heres-how-to-check-4b4o)
- Codex users hit repeated confirmations in read-only mode; workspace-write is the practical fix — [toolnavs explainer](https://toolnavs.com/article/1791-codex-keeps-confirming-commands-adjust-the-approval-and-sandbox-first)
- Antigravity: file edits did not prompt for review under `request-review`, and plan/auto-accept mode selection is missing — [Google AI forum](https://discuss.ai.google.dev/t/antigravity-cli-missing-gemini-cli-features/147489)

**Timeouts on questions**
- Codex: "Add setting to disable the auto-resolve in 60 seconds for questions" (#28969, open) — [openai/codex issues](https://github.com/openai/codex/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc). Claude Code: "AskUserQuestion: 'No response after 60s'" (#73125) — [claude-code issues](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)

**Removed or reset features, vendor pivots**
- Codex "Please make '/undo' back" (#9203) — [openai/codex issues](https://github.com/openai/codex/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)
- The Gemini CLI shutdown drew criticism for building a closed tool on open-source contributions. HN reactions were split: "a disaster and half baked product" against "a great idea" — [The Register](https://www.theregister.com/ai-ml/2026/05/20/bye-bye-gemini-cli-google-nudges-devs-toward-antigravity/5243605); [hn.nuxt.dev thread](https://hn.nuxt.dev/item/48196867)
- OpenCode 2.0 broke v1 plugins and the server API and dropped LSP diagnostics; OpenCode disabled auto-update in 1.19 so the jump is opt-in — [ai-tldr](https://ai-tldr.dev/releases/anomalyco-opencode-2-0/). An analysis recommends upgrading only for plain terminal use — [Capital and Compute](https://capitalandcompute.net/blog/opencode-2-0-what-changed/)
- "Support for OpenCode and Max plan" (Claude Code #17118) was closed — [claude-code issues](https://github.com/anthropics/claude-code/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)

**Disk and resource hygiene**
- Codex "SQLite feedback logs can write ~640 TB/year and consume SSD endurance" (#28224, 154 👍) — [openai/codex issues](https://github.com/openai/codex/issues?q=is%3Aissue%20sort%3Areactions-%2B1-desc)
- Claude Code's docs warn that Homebrew keeps old versions and recommend `brew cleanup` — [Claude Code setup](https://code.claude.com/docs/en/setup)

**Hidden behaviour and bloat (the Pi critique)**
- Pi's author argues that hidden prompt changes, injected reminders and pruned tool output change agent behaviour invisibly. Pi ships four tools (read, write, edit, bash) and a sub-1k-token system prompt, and documents "what we didn't build" (subagents, plan mode, MCP) — [AI Engineer talk](https://www.ai.engineer/talks/RjfbvDXpFls-pi-coding-agent); [glukhov.org review](https://glukhov.org/ai-devtools/pi/pi-coding-agent-review/). Pi has no built-in sandbox or permission system — [glukhov.org review](https://glukhov.org/ai-devtools/pi/pi-coding-agent-review/)

**Delights**
- Crush is the most-praised interface: Charm's "reputation for creating beautiful terminal software" — [claudeskills.info/harness/crush](https://claudeskills.info/harness/crush/) (search summary)
- Codex gets credit for the "strongest security (OS-level sandboxing)"; Claude Code for the deepest feature set (hooks, subagents, SDK) — [kilo.ai comparison](https://kilo.ai/articles/best-cli-coding-agents); [amux comparison](https://amux.io/blog/best-terminal-ai-coding-agents-2026/) (vendor-adjacent roundups)
- Aider: "auto-commit every edit" as the best git workflow — [amux](https://amux.io/blog/best-terminal-ai-coding-agents-2026/)
- Kimi Code: fast startup, plugin trust levels shown up front — [kimi-code README](https://github.com/MoonshotAI/kimi-code)

### Inferences
- INIT's hard money ceilings, a cost shown in the status bar, and `$ per task` are its strongest answer to the industry's #1 complaint. Lead with them in onboarding ("you will never be charged more than $5/day; the Root enforces it").
- Never let a timeout answer an owner question; queue it and notify. Two big vendors are getting complaints for doing the opposite.
- Rendering stability (no flicker, no scroll-jumping) needs to be a tested requirement. INIT's golden-snapshot plus PTY tests (docs/tui.md TUI-1, TUI-12) already point that way.
- INIT's lineage-gated self-evolution answers "hidden behaviour" directly: every change to the agent is a signed, inspectable lineage record. Surface it (`/evolution` showing the diff of what changed in the agent itself).

### Gaps
- Primary Reddit and HN threads could not be opened (Reddit through a mirror; HN through third-party mirrors). Sentiment is partly from secondary summaries.
- The HN front-page numbers for OpenCode (March 2026, a Claude Max legal dispute) appeared only in a secondary summary and are not included as fact.
- Reaction counts for Claude Code issues were not visible; the ranking order is GitHub's sort.

---

## 6. Recommended best-of UX spec for INIT

### Takeaway
Make INIT feel like Claude Code and Codex in the first 60 seconds. One signed download installs a **prebuilt, signed body** with no Rust toolchain needed to chat. All questions move into a first-run flow in `init` itself. The Root, ceilings, autonomy ladder and self-evolution become visible, calm, always-present status rather than setup chores. The cold self-build moves off the critical path into a background "evolution warm-up" that must finish before the first self-change, not before the first prompt.

### Cited Findings (the evidence each spec decision rests on)
- One-line installer, signed binary, `~/.local/bin`, channels, background update applied on restart, `doctor` — [Claude Code setup](https://code.claude.com/docs/en/setup)
- Trust prompt that sets the permission preset per folder; read-only until trusted; protected paths stay read-only inside the writable root; network off by default — [Codex approvals & security](https://learn.chatgpt.com/docs/agent-approvals-security)
- Single background service with many attachable clients — [OpenCode 2.0](https://anoma.ly/notes/opencode-2-0/); `IsBusy` and attached-clients display — [Crush README](https://github.com/charmbracelet/crush)
- Rewind menu, Esc-Esc, restore code or conversation — [Claude Code checkpointing](https://code.claude.com/docs/en/checkpointing); shadow-git snapshots — [Gemini checkpointing](https://geminicli.com/docs/cli/checkpointing/)
- Env-key detection and live provider detection from a pasted key — [Claude Code quickstart](https://code.claude.com/docs/en/quickstart); [goose install](https://goose-docs.ai/docs/getting-started/installation)
- Per-session spend limit — [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli)
- Notifications `auto|native|osc|bell` — [Crush README](https://github.com/charmbracelet/crush)
- Migration that never modifies the old data — [kimi-cli README](https://github.com/MoonshotAI/kimi-cli)

### Inferences — the spec

#### 6.0 Current INIT baseline (from README.md and docs/release.md in this repo)
Requirements are CLT + Rust 1.97 + wasm32-wasip2, ~4 GB release build + ~2 GB first body build, and a 20 GB disk floor. Setup takes three commands: `scripts/install.sh` (source build + 5-question shell wizard + LaunchAgent), `~/.init-root/bin/init-root keys set deepseek`, then `init`. Admin commands live in `~/.init-root/bin`, which is not on PATH. Uninstall moves the homes to the Trash after a typed `uninstall` (already best-in-class). The cold body self-build takes ~4 min.

#### 6.1 Install (target: under 60 s to the first reply on a warm network)
```
$ curl -fsSL https://init.<domain>/install.sh | sh
  INIT 0.2.0 · macOS arm64 · signed + notarized ✓ · manifest sig ✓
  ↓ init-0.2.0-darwin-arm64.tar.zst  38 MB  ████████████ 100%
  ✓ installed  ~/.local/bin/init   (one command; everything else is a subcommand)
  ✓ Root home  ~/.init-root        ✓ agent home ~/.init
  Next: run  init
```
- **One artifact** holds `init` (TUI + launcher), `init-root`, `init-kernel`, and **the prebuilt LKG body tuple** (signed by the release key, and its hash recorded in the release manifest and the first lineage record).
- **Show progress.** Claude Code's docs have to warn that its installer "shows no progress while it downloads". Print the PATH fix inline if `~/.local/bin` is missing, as Goose and Claude Code docs do.
- **No Rust toolchain, no CLT, no network build** to reach the first prompt. Brew cask later (`brew install --cask init`). Same artifact, no auto-update under brew, as Claude Code does.
- **Single command surface:** `init` (chat), `init status|stop|start|panic|update|doctor|uninstall|keys|charter|ledger|audit`. These proxy to `init-root`, so the owner never needs `~/.init-root/bin` on PATH. Keep `init-root` for scripts.
- **Install does not start the LaunchAgent or ask questions.** The first `init` does both, after consent. (Quality guard: nothing runs in the background before the owner has seen and accepted the ceilings.)

#### 6.2 First run (`init`, first time): six screens, each one keypress with a default
```
┌ 1/6  Welcome to INIT ──────────────────────────────────────────┐
│ A self-evolving agent on your Mac. A separate supervisor, the   │
│ Root, holds your keys and money limits and can always roll back. │
│ Theme: ● auto (matches terminal)  ○ dark  ○ light  ○ ANSI-only  │
│ Preflight  macOS 15.1 ✓  sandbox ✓  disk 112 GB free (floor 20) ✓│
│                                            Enter to continue    │
└─────────────────────────────────────────────────────────────────┘
2/6  Model provider
   Found DEEPSEEK_API_KEY in your environment. Use it?  [Y/n]
   ─ or ─  ● DeepSeek   ○ Anthropic   ○ OpenAI   ○ OpenRouter   ○ Local (Ollama)
   Paste key: ••••••••••••••••  → testing… ✓ deepseek-flash replied in 410 ms ($0.0001)
   Stored in: macOS Keychain (item "INIT Root · deepseek"), read only by the Root
3/6  Money ceilings  (enforced by the Root at the network edge, outside the agent's reach)
   ○ Frugal   $2/day · $10 total      ≈ 40 tasks/day at deepseek-flash prices
   ● Default  $5/day · $25 total      ≈ 100 tasks/day
   ○ Custom…
   Raising a ceiling later needs you, at this keyboard. Lowering it is instant.
4/6  Autonomy
   INIT starts at L0 and earns levels by passing its gate. Highest level it may ever earn:
   L0 chat + tools · L1 edits its own skills · L2 … · L3 … · ● L4 (cap)   [details: ?]
   Self-change notices: ● inline + desktop notification  ○ inline only
5/6  Workspace for ~/code/myproj   (asked once per folder, like Codex)
   ● Trust: read your home, write only in this folder, network off   (recommended in a git repo)
   ○ Read-only: read your home, write nothing
   ○ Exit
   Protected even when trusted: .git  .env*  ~/.ssh  ~/.init-root
   Every turn is snapshotted; /undo reverts files, including shell edits.
6/6  Ready
   Root service: installed as LaunchAgent, starts at login  [change in /config]
   Telemetry: none. INIT sends nothing anywhere except your model provider.
   Try:  "what does this project do?"   ·   /help   ·   ? for shortcuts
```
- Defaults are pre-selected, so Enter×6 reaches a working agent. `--yes` skips the screens with the defaults, for scripts.
- **Key handling:** today the TUI must not handle keys (docs/tui.md T-2/T-3). Recommendation: add a Root request `keys.set` on `owner.sock`; the TUI sends the masked input over the socket and keeps nothing. Alternative: drop out to the terminal and run `init keys set`. **This is an owner decision (trust-model change).** Prefer Keychain over a key file, with a file fallback (the Codex `auto` pattern).
- The trust screen reappears only for new folders. Do not repeat it for `~` (Claude Code bug #97726). Treat `~` as read-only and suggest `cd` into a project.

#### 6.3 Cold self-build: take it off the critical path
| Option | Effect on first prompt | Quality guard |
|---|---|---|
| **A. Ship the prebuilt signed body (recommended)** | 0 s build; chat immediately on L0 | Body hash = release manifest hash; Root refuses an unsigned or mismatched tuple; lineage record #1 names it |
| **B. Background "evolution warm-up" after the first reply** | none (nice'd, idle-priority, pauses on battery or under 20% battery) | Before the first self-change, the local toolchain must rebuild the shipped body **bit-identically** (reproducible-build check). On mismatch: notice, evolution disabled, chat unaffected |
| C. Ship a warm build cache (`target/` artifacts, vendored crates) | makes B ~4 min → seconds | Cache is content-addressed, never trusted for promotion. The gate (G0–G4) still builds every candidate from source |
| D. Toolchain on demand | no rustup prompt at install | Pinned Rust 1.97 + wasm32-wasip2 fetched to `~/.init/toolchain` with checksums when the warm-up starts; status shows progress |

Status bar during warm-up: `L0 · deepseek-flash · $0.04 · $1.23/$5.00 today · running · evolve: warming 2:10/4:00`. When done: one dim notice, `▲ evolution ready: local build reproduces body 3f9c0a1b ✓`. If the owner asks for a self-change before warm-up ends: `INIT will start evolving when its build check finishes (~1 min). Chat is unaffected.`

#### 6.4 Daily use
- **Status bar** (already designed in docs/tui.md §3.9). Add `ctx 72% left` (Codex), and a workspace badge `rw ~/code/myproj` / `ro`. Keep Root-sourced fields authoritative and visually distinct from kernel-sourced ones.
- **Commands** (`/` menu, fuzzy). Standard: `/help /model /usage /context /compact /new /resume /rename /diff /undo /rewind /permissions /init /mcp /config /theme /status /doctor /export /copy /release-notes`. INIT-specific: `/panic` (always works, even if the socket is down: existing T-3 path), `/stop`, `/start` (rename from `/resume`), `/level` (ledger and progress to next level), `/evolution` (recent promotions and rollbacks, with the diff of each self-change), `/ceiling` (spend vs limits, and a "raise" flow that requires typing the amount).
- **Keys** (Claude Code parity, so muscle memory transfers): Enter send; Shift+Enter / Option+Enter / `\`+Enter newline; Ctrl+G `$EDITOR`; Esc interrupt; Esc-Esc on an empty prompt opens rewind; Ctrl+C interrupt, then clear, then exit; Ctrl+D detach (INIT keeps running: already the case); Ctrl+O expand tool calls; Ctrl+R history search; Ctrl+V image; `@` file fuzzy-find; `!` shell (in the sandbox); Shift+Tab cycles workspace mode `ro → rw-project → (full, if enabled at launch)`; `?` shortcuts.
- **Approvals:** three options, numbered: `1 Yes` · `2 Yes, and don't ask again for <exact command> in this project` · `3 No, and tell INIT what to do instead` (Tab adds a comment). **No timeout ever auto-answers.** A pending approval raises a desktop notification (OSC 9 / `osascript`, or bell, configurable like Crush) and shows in the status bar `⏸ waiting for you (1)`.
- **`--full`** (Codex `--yolo` equivalent): only as a launch flag, never in the Shift+Tab cycle unless launched with it. It requires typing `full` once per session and shows a red `FULL` badge. Ceilings still apply.
- **/undo snapshots:** before every turn, snapshot the writable project root (APFS `clonefile` or a shadow git outside the project, `~/.init/snapshots/<project-hash>`). `/undo` reverts the last turn's files, **including shell-made changes** (better than Claude Code). `/rewind` shows a list of turns with restore code / conversation / both. Retain 100 turns or 30 days, within the disk floor.
- **Notifications:** turn finished (when the terminal is unfocused), approval needed, ceiling reached, panic, and self-evolution events. Self-evolution notices stay as specified (`▲` promoted, `▼` rollback, red for demotion and panic). On attach, show a **digest** of what happened while the owner was away: `While you were away (6 h): ▲2 promotions · ▼1 rollback (gate G2) · $0.84 spent · level L0→L1`.
- **Headless:** `init -p "task" [--output-format text|json|stream-json] [--workspace ro|rw]`. The exit code is non-zero on a ceiling hit or panic. Ceilings and the Root apply exactly as in interactive mode.
- **Sessions:** `init -c` (continue last in this folder), `init -r` (picker). Since the Root is a service (as in OpenCode 2), several `init` windows can attach. Show `2 clients attached` (Crush pattern).
- **IDE:** `init acp` (stdio ACP), for Zed and JetBrains later. Low priority for v1.

#### 6.5 Config
- `/config` is a TUI form that writes via Root requests: the charter (ceilings, autonomy cap, disk floor; raising a limit requires typing the new value) and kernel config (model, soft budgets). The Root validates with `charter check`; the owner never hand-edits TOML or restarts.
- Project instructions: read `AGENTS.md` (root to cwd). Offer `/import` of CLAUDE.md/GEMINI.md into AGENTS.md. `/init` writes AGENTS.md.
- MCP: `init mcp add <name> --transport stdio|http [--auth oauth]` plus `/mcp` (list, reconnect, auth). MCP servers run inside the Root's sandbox and egress classes; show per-server egress in `/usage`.
- Honour `NO_COLOR` and `DO_NOT_TRACK`. Keep `~/.init` and `~/.init-root` (macOS-native is fine; the XDG complaint is mostly Linux).

#### 6.6 Update
- The Root checks a signed manifest daily and downloads in the background. It applies **only when idle**, never mid-turn, and announces: `↑ INIT 0.2.1 ready (Root + TUI). Applies at next restart · /release-notes`. Channels are `stable` (default for an owner-facing agent with money) and `latest`. `init update` applies now. Brew installs are updated by brew.
- **Keep two kinds of change visually distinct:** maintainer releases (`↑`, blue) and the agent's own self-evolution (`▲`/`▼`). A release never resets the agent's earned level or lineage. Migration never modifies old data in place (the Kimi pattern) and keeps the previous version for one-command rollback (`init update --rollback`).
- Cap logs and state size and show disk use in `/status` (Codex's 640 TB/yr SQLite issue is the cautionary tale).

#### 6.7 Uninstall
```
$ init uninstall
  Stops the Root, unloads the LaunchAgent, removes the Keychain item "INIT Root · deepseek",
  and MOVES to the Trash (nothing is deleted):
    ~/.init-root   ~/.init   ~/.local/bin/init
  Type "uninstall" to continue: _
  ✓ moved to ~/.Trash/INIT-2026-10-20T2104Z/ · empty the Trash yourself
```
`--keep-data` removes only the binaries and the service. This is the current `uninstall.sh` behaviour, wrapped in the single command, plus the Keychain item.

#### 6.8 Quality guards for each shortcut
- Prebuilt body: signature + manifest hash + reproducible rebuild before the first self-change. Promotion always builds from source through G0–G4.
- No toolchain at install: evolution stays disabled (not degraded) until the toolchain and the warm-up pass. Chat quality is unchanged.
- One-keypress onboarding: defaults are the current conservative values ($5/$25, L0 start, read-only outside the project, network off). Raising a limit always takes typed input.
- Background work (warm-up, update download): idle-priority, pauses on battery, respects the disk floor, and never mid-turn.

### Gaps
- No official benchmark for "time to first prompt" exists to set INIT's target; 60 s is a proposal, to be measured.
- Bit-for-bit reproducibility of INIT's Wasm body across machines is unverified. It must be measured on two Macs before relying on option B's check; if builds are not reproducible, fall back to a signed-artifact-only trust model.
- Whether macOS Keychain access from a LaunchAgent-run Root works without user-visible prompts on each upgrade (code-signing identity changes) is untested. Claude Code's reports of repeated TCC prompts after self-updates ([#7492](https://github.com/anthropics/claude-code/issues/7492)) suggest a stable signing identity matters.
- The trust-model change for key entry through the TUI (6.2) needs an owner decision.
