# Agent-harness benchmarks with public per-task data, outside the SWE-bench family (state as of 2026-10-21)

Scope: Terminal-Bench (1.0 to 4.0), Aider Polyglot, Princeton HAL, OpenHands Index (with Commit0), LiveCodeBench, METR (RE-Bench, HCAST), and 2025–2026 harness-comparison and long-horizon benchmarks. The driving question is whether INIT Agent (a Rust harness that runs natively on macOS under Seatbelt, using a DeepSeek "flash" model) can run 5–20 tasks and compare each task with published results from other harnesses at minimal cost.

Method note: most numbers below were pulled directly from primary data endpoints, not from aggregators:
- the tbench.ai / Harbor leaderboard API;
- the Harbor per-trial REST table;
- the HF submission dataset;
- the aider YAML;
- the HAL pages;
- the OpenHands Index results repo;
- METR's runs.jsonl;
- DeepSeek's HF model cards.

Derived numbers (per-task pass counts, spreads, $/trial) were computed from that raw data on 2026-10-21. Machine context: the owner's Mac reports macOS 14.6 (Darwin 23.6), arm64, with a `docker` CLI present at /usr/local/bin/docker (local check).

---

## Q1. Terminal-Bench (1.0 / 2.0 / 2.1 / 3.0 / 4.0): are per-task results public per (agent × model)? Is there a DeepSeek entry? Can the tasks run without Docker?

### Takeaway
Terminal-Bench is now at version 4.0, and 2.0 is two generations stale. Per-trial pass/fail data is public and machine-readable for 2.1, 3.0 and 4.0 (Harbor hub REST) and for most 2026-era 2.0 submissions (the HF submission dataset), including the only official DeepSeek entry: Terminus 2 + DeepSeek-V3.2 on TB 2.0 at 39.6%, with 89 tasks × 5 trials, and per-trial cost ≈ $0.01–0.05. No official TB 2.1, 3.0 or 4.0 row uses a DeepSeek model, and all newer DeepSeek V4/V4.1 Flash TB numbers are vendor self-reports. Every task runs inside a Linux container. There is no native-macOS runtime: Harbor supports Docker, Podman, Apple `container` (macOS 26 only) and cloud sandboxes.

### Cited Findings
**Versions and current state**
- tbench.ai now titles itself "Terminal-Bench 4.0". The old URL /leaderboard/terminal-bench/2.0 returns HTTP 308 to `/?version=2.0`. The site's JS bundle lists the selectable benchmarks: 4.0 (package `terminal-bench/terminal-bench`, leaderboard `4-0-0`), 3.0 (`3-0-0`), 2.1 (`terminal-bench/terminal-bench-2-1`, leaderboard `main`) and 2.0 (`terminal-bench/terminal-bench-2`, leaderboard `2-0`). TB 1.0 is no longer selectable — [tbench.ai](https://www.tbench.ai/)
- Leaderboard JSON for any version can be fetched with `POST https://ofhuhcpkvzjlejydnvyd.supabase.co/functions/v1/leaderboard-read` and body `{"package":"terminal-bench/terminal-bench-2","name":"2-0"}`. This is the call the site's own JS makes — [tbench.ai JS bundle / leaderboard-read endpoint](https://www.tbench.ai/)
- Per-trial rows are exposed at `https://ofhuhcpkvzjlejydnvyd.supabase.co/rest/v1/leaderboard_row_trial?select=trial(task_name,rewards,exception_type)&row_id=eq.<row_id>`, using the publishable key embedded in the public tbench.ai JS bundle. Each trial has:
  - `task_name`, `rewards.reward`, `exception_type` (e.g. AgentTimeoutError);
  - timestamps for env setup, agent setup, agent execution and verifier phases;
  - `trajectory_path` and `archive_path` (trial.tar.gz);
  - the full agent config (agent name, model, reasoning effort, environment type).

  Trials are viewable at `https://hub.harborframework.com/jobs/<job_id>/trials/<trial_id>` — [Harbor hub](https://hub.harborframework.com)

**Per-version leaderboard contents (fetched 2026-10-21)**

| Version | Rows | Trials per row | Inferred tasks × trials | Harnesses (row counts where given) | DeepSeek row? |
|---|---|---|---|---|---|
| TB 4.0 | 35 | 330 | 66 × 5 | Claude Code, Codex, Grok Build, mini-SWE-agent, Muse Code | No |
| TB 3.0 | 12 | 370 | 74 × 5 | Claude Code (5), Codex (3), mini-SWE-agent, Grok Build, Devin, Cursor CLI | No |
| TB 2.1 | 22 | 445 | 89 × 5 (all 22 rows' trials pulled) | Codex, Claude Code, Terminus 2, Cursor CLI, mini-SWE-agent, Gemini CLI | No |
| TB 2.0 | 142 | `n_trials: 0` (hub copy "seeded from tbench.ai") | n/a | see below | Yes: one row |

- **TB 4.0:** best score is Claude Code + Opus 5.5 (max) at 64.85%. Median $/trial is $8.58 (range $0.22 for Codex + GPT-6 Luna to $29.10). Median average trial duration is 3,177 s (range 1,684–9,953 s) — [leaderboard-read 4-0-0](https://www.tbench.ai/)
- **TB 3.0:** top score is 42.7% (mini-SWE-agent + Opus 5). $/trial runs from $2.07 to $19.50 — [leaderboard-read 3-0-0](https://www.tbench.ai/)
- **TB 2.1:** median $/trial is $0.97 (range $0.30–4.63). Median average trial duration is 564 s. Submissions go through the [TB2.1 repo](https://github.com/harbor-framework/terminal-bench-2-1) — [leaderboard-read main](https://www.tbench.ai/)
- **TB 2.0:** dates range 2025-06-17 to 2026-05-14, and 73 of the 142 rows are flagged verified. Harness counts:

  | Harness | Rows | Harness | Rows |
  |---|---|---|---|
  | Terminus 2 | 33 | Mini-SWE-Agent | 13 |
  | OpenHands | 12 | Codex CLI | 7 |
  | Droid | 5 | Crux | 5 |
  | Gemini CLI | 5 | Claude Code | 5 |
  | Mux | 4 | Warp | 3 |
  | Letta Code | 3 | Goose | 3 |
  | Junie CLI | 2 | OpenCode | 1 |
  | Grok CLI | 1 | many one-off harnesses | 1 each |

  The only DeepSeek row is rank 86: Terminus 2 + DeepSeek-V3.2, 39.6%, model date 2025-12-01 — [leaderboard-read 2-0](https://www.tbench.ai/)

**TB 2.0 per-task data on HF**
- The HF dataset `harborframework/terminal-bench-2-leaderboard` holds 76 submission folders under `submissions/terminal-bench/2.0/`. One of them is `Terminus2__DeepSeek-V3.2`, with a job folder dated 2026-02-07 containing 445 trial dirs plus a job-level `result.json` — [HF dataset](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard/tree/main/submissions/terminal-bench/2.0/Terminus2__DeepSeek-V3.2)
- Details of that DeepSeek-V3.2 run:
  - Config: `model_name: deepseek/deepseek-chat` (non-thinking), `n_attempts: 5`, `environment.type: docker`, `n_concurrent_trials: 4`.
  - Timing: the run started 2026-02-07T07:47 and finished 2026-02-08T07:33 (about 24 h wall-clock).
  - Result: mean reward 0.3955. Of 442 graded trials, 176 passed and 266 failed. There were 90 AgentTimeoutError and 1 RewardFileNotFoundError.
  - Per task: 19 of 89 tasks pass 5/5, and 14 tasks pass 3–4/5.
  - Cost: per-trial `agent_result.cost_usd` in a 12-trial sample ranged $0.008–$0.053 with 73k–1.17M input tokens, mostly cache hits (e.g. 666,752 of 685,426 cached).

  [HF job result.json](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard/tree/main/submissions/terminal-bench/2.0/Terminus2__DeepSeek-V3.2)
- The HF task mirror `harborframework/terminal-bench-2.0` is a read-only copy. It says the official leaderboard is on tbench.ai, where "verified trajectories are publicly viewable", and that official submissions use `-k 5` without overriding timeouts or CPU, memory and storage limits — [HF terminal-bench-2.0 README](https://huggingface.co/datasets/harborframework/terminal-bench-2.0/blob/main/README.md)

**Task format**
- The TB paper describes 89 tasks, each with "a unique environment, human-written solution, and comprehensive tests". Frontier systems scored under 65% at publication (arXiv 2601.11868, 2026-01-17) — [arXiv](https://arxiv.org/abs/2601.11868)
- The 89 TB 2.0 task names are identical to the 89 TB 2.1 task names (computed overlap 89/89) — [HF DS run](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard) + [Harbor REST](https://hub.harborframework.com)
- TB 1.0 (repo now `harbor-framework/terminal-bench-1`) used dataset `terminal-bench-core` v0.1.1. Running it requires `uv` and Docker, and the README points new users to Harbor for TB 2.0 — [terminal-bench-1 README](https://github.com/harbor-framework/terminal-bench-1)

**Runtimes (no native macOS)**
- Harbor's `EnvironmentType` enum lists 34 backends. Local ones are `docker`, `podman`, `apple-container` and `singularity`. Cloud ones include `daytona`, `modal`, `e2b`, `runloop`, `ec2`, `gke`, `hf-sandbox` and others. There is no host or native-process backend — [harbor environment_type.py](https://github.com/harbor-framework/harbor/tree/main/src/harbor/environments)
- `AppleContainerEnvironment` is documented as "lightweight Linux VMs on Apple silicon". It requires arm64 and the `container` CLI — [apple_container.py](https://github.com/harbor-framework/harbor/tree/main/src/harbor/environments)
- Apple's `container` "is supported on macOS 26 … We do not support older versions of macOS" — [apple/container README](https://github.com/apple/container)
- The Harbor CLI supports:
  - a custom agent via import path (`--agent-import-path`, deprecated in favour of `--agent`);
  - task selection with `-i/--include-task-name`, `-x/--exclude-task-name` and `-l/--n-tasks`;
  - repeated attempts with `-k/--n-attempts`;
  - backend choice with `-e/--env`.

  [harbor cli/jobs.py](https://github.com/harbor-framework/harbor/blob/main/src/harbor/cli/jobs.py)
- Harbor ships many prebuilt ("installed") agents: claude_code, codex, opencode, goose, gemini_cli, cursor_cli, openhands, openhands_sdk, mini_swe_agent, swe_agent, aider, qwen_code, kimi_cli, copilot_cli, junie, cline, trae_agent, pi, hermes, devin, grok_build, muse_code and others. Terminus 2 runs as a Harbor-native agent — [harbor agents/installed](https://github.com/harbor-framework/harbor/tree/main/src/harbor/agents/installed)

**DeepSeek self-reported TB numbers (none of these is an official leaderboard row)**

| Model / card | Benchmark | Score | Harness and settings |
|---|---|---|---|
| DeepSeek-V3.1 | TB 1.0 | 31.3 | "Terminus 1" — [card](https://huggingface.co/deepseek-ai/DeepSeek-V3.1) |
| V3.1-Terminus | TB | 36.7 | [card](https://huggingface.co/deepseek-ai/DeepSeek-V3.1-Terminus) |
| V3.2-Exp | TB | 37.7 | [card](https://huggingface.co/deepseek-ai/DeepSeek-V3.2-Exp) |
| DeepSeek-V3.2 (paper) | TB 2.0 | 46.4 thinking / 37.1 non-thinking | Claude Code. The paper says its thinking-mode context management "is currently incompatible with Terminus" — [arXiv 2512.02556](https://arxiv.org/abs/2512.02556) |
| DeepSeek-V3.2 (paper) | TB 2.0 | 39.3 non-thinking | Terminus — [arXiv 2512.02556](https://arxiv.org/abs/2512.02556) |
| V4 preview card (created 2026-04-22) | TB 2.0 | V4-Flash Non-Think / High / Max = 49.1 / 56.6 / 56.9; V4-Pro = 59.1 / 63.3 / 67.9 | harness not stated — [card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash) |
| V4-Flash-0731 | TB 2.1 | 82.7 (preview Flash 61.8, V4-Pro preview 72.1) | "minimal mode of DeepSeek Harness (to be released)", max effort, T=1.0, top_p=0.95 — [card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash-0731) |
| V4-Pro-0813 | TB 2.1 | 87.9 | DeepSeek Harness minimal — [card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Pro-0813) |
| V4.1-Flash (card created 2026-09-10) | TB 2.1 / 3.0 / 4.0 | 90.6 / 30.0 / 31.2 | DeepSeek Harness Minimal, 1M context, reasoning_effort=100 — [card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash) |
| V4.1-Flash-1017 (2026-10-17) | TB 4.0 | 42.3 | Claude Code harness — [card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash-1017) |

- **Aggregators conflate versions.**
  - BenchLM lists "DeepSeek V4 Flash 0731" at 56.9% and "V4 Pro 0813" at 67.9% on TB 2.0. Those values match the April V4 *preview* card (Flash-Max 56.9, Pro-Max 67.9); the 0731 and 0813 cards report only TB 2.1 — [BenchLM TB2](https://benchlm.ai/benchmarks/terminal-bench-2) vs [V4 card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash)
  - BenchLM's TB 2.1 entry "DeepSeek V4.1 Flash 1017 (90.6%)" uses the V4.1-Flash (not 1017) self-reported value — [BenchLM TB2.1](https://benchlm.ai/benchmarks/terminalbench21) vs [V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)

**Per-task harness sensitivity (computed from TB 2.1 per-trial data)**
- The per-trial pulls reproduce reported accuracies within ±0.9 pp for 21 of 22 rows. The exception is Cursor CLI + Grok 4.5: 88.3% computed vs 79.3% reported, so treat that row's per-trial data with care — [Harbor REST](https://hub.harborframework.com)
- Averaged over the 22 rows, 25 of the 89 tasks have ≥95% mean pass rate. Only 2 tasks have ≤20%: `filter-js-from-html` and `make-doom-for-mips` — computed from [Harbor REST](https://hub.harborframework.com)

### Inferences
- **Recommended per-task comparison: TB 2.0 against the DeepSeek-V3.2/Terminus 2 baseline.** That baseline is Terminus 2 + deepseek-chat, with 5 trials per task and costs. TB 2.1 rows add 22 (harness × frontier model) references on the same 89 task names. However, TB 2.1 revised tasks, so 2.0↔2.1 per-task comparisons are approximate.
- **For a cheap 5–20 task subset, prefer tasks where DeepSeek-V3.2 was neither 0/5 nor 5/5 and where frontier rows are mid-range.** Computed candidates (DS-V3.2 passes on TB 2.0, then mean pass rate over the 22 TB 2.1 rows):

  | Task | DS-V3.2 passes | TB 2.1 mean |
  |---|---|---|
  | adaptive-rejection-sampler | 1/5 | 0.79 |
  | build-cython-ext | 1/5 | 0.88 |
  | build-pov-ray | 4/5 | 0.70 |
  | cancel-async-tasks | 1/5 | 0.68 |
  | compile-compcert | 3/5 | 0.67 |
  | configure-git-webserver | 2/5 | 0.36 |
  | custom-memory-heap-crash | 4/5 | 0.72 |
  | feal-linear-cryptanalysis | 2/5 | 0.86 |
  | hf-model-inference | 4/5 | 0.85 |
  | mailman | 4/5 | 0.84 |
  | path-tracing | 1/5 | 0.79 |
  | pypi-server | 4/5 | 0.75 |
  | qemu-startup | 4/5 | 0.84 |
  | query-optimize | 2/5 | 0.66 |
  | train-fasttext | 1/5 | 0.32 |
  | winning-avg-corewars | 2/5 | 0.75 |

  Add 2–3 anchor tasks that DS-V3.2 solved 5/5 and nearly everyone solves (e.g. `nginx-request-logging`, `modernize-scientific-stack`, `multi-source-data-merger`) as sanity checks.
- **Estimated cost:** 20 tasks × 5 trials ≈ 100 trials. At DeepSeek-V3.2-like token use (≈$0.01–0.05 per trial), the model cost is ≈ $1–5. A flash model at similar prices should land in the same order of magnitude. This is unverified for INIT's exact model and prices.
- **Quality guard:** run ≥3, ideally 5, trials per task. DS-V3.2 had 14 of 89 tasks at 3–4/5, and harness swaps flip many tasks (see Q5), so single-trial per-task comparisons would mostly measure noise.
- **Docker is unavoidable for Terminal-Bench on this Mac.**
  - The owner's macOS 14.6 cannot use Apple `container`, which needs macOS 26.
  - Practical options are local Docker Desktop (a `docker` CLI is present) or a paid cloud sandbox (Daytona, Modal, E2B).
  - INIT's Seatbelt sandbox would not be exercised: Harbor runs the agent's commands inside the Linux container. Either the harness runs on the host and calls `environment.exec`, as Terminus does, or a Linux build of INIT is installed into the container.
  - Many TB images are likely x86_64. On Apple silicon these would run under emulation, which may slow or break some tasks (e.g. qemu or compile-heavy ones). This is not verified per task.
- **TB 3.0 and 4.0 are long-horizon** (≈1 h median per trial, $2–29 per trial for frontier models) and DeepSeek V4.x Flash scores only 7–42% there by its own reports. They are poor targets for a minimal-cost INIT smoke test.

### Gaps
- Whether *older* TB 2.0 rows (2025, e.g. Terminus 2 / Claude Code / OpenHands with GPT-5, Opus 4.5, Sonnet 4.5) have per-trial data downloadable anywhere. They are not in the hub DB (`n_trials: 0`) and not among the 76 HF submission folders, and I did not find a bulk archive.
- Exact TB task counts for 3.0 and 4.0 were inferred from trials/5. They were not read from a task list.
- No official TB 1.0 leaderboard data was retrievable from the current site, so I could not check for a DeepSeek row on TB 1.0.
- DeepSeek's "DeepSeek Harness" (DSH) used for its V4/V4.1 TB numbers was described as "to be released" in the 0731 card. I did not verify whether it has since been released.
- I did not verify which API model name corresponds to "deepseek-flash", or its current $/token.

---

## Q2. Aider Polyglot: which per-model results are published, are per-exercise results public, which DeepSeek entries exist, and can it run natively on macOS (Python and Rust subsets)?

### Takeaway
The Aider Polyglot leaderboard is stale: its last entries are DeepSeek-V3.2-Exp on 2025-10-03, and the leaderboard file has had no commits since 2025-10-04. It publishes only aggregate results per model, all with the aider harness. Per-exercise outcomes are written locally (`.aider.results.json`) but are not published. Exercises can run natively on macOS for Python (34) and Rust (30) with pytest and cargo, but the README insists on Docker for safety. Per-task cross-harness comparison is not possible from public data.

### Cited Findings
- **Size and format:** 225 exercises — cpp 26, go 39, java 47, javascript 49, python 34, rust 30, counted from directories. The exercise repo was last pushed 2024-12-22 — [Aider-AI/polyglot-benchmark](https://github.com/Aider-AI/polyglot-benchmark)
- **Leaderboard file:** `polyglot_leaderboard.yml` has 69 entries. Its last commits are 2025-10-03 ("add deepseek model test results") and 2025-10-04. The top entry is gpt-5 (high) at 88.0% for $29.08 — [polyglot_leaderboard.yml](https://github.com/Aider-AI/aider/blob/main/aider/website/_data/polyglot_leaderboard.yml); rendered at [aider.chat leaderboards](https://aider.chat/docs/leaderboards/)
- **Fields per entry:** pass_rate_1 and pass_rate_2, pass_num, percent_cases_well_formed, malformed responses, prompt and completion tokens, test_timeouts, `command`, versions, seconds_per_case and total_cost. There are no per-exercise fields — [YAML](https://github.com/Aider-AI/aider/blob/main/aider/website/_data/polyglot_leaderboard.yml)
- **DeepSeek entries.** All use aider. `total_cost` covers all 225 exercises; $/exercise is derived as total/225.

  | Date | Model | Edit format | pass_rate_2 | pass_rate_1 | Total cost | ≈ $/exercise | s/case |
  |---|---|---|---|---|---|---|---|
  | 2024-12-21 | DeepSeek Chat V2.5 | diff | 17.8 | | $0.51 | | |
  | 2024-12-25 | DeepSeek Chat V3 (prev) | diff | 48.4 | | $0.34 | | |
  | 2025-01-20 | DeepSeek R1 | diff | 56.9 | | $5.42 | | |
  | 2025-01-23 | DeepSeek R1 + claude-3-5-sonnet | architect | 64.0 | | $13.29 | | |
  | 2025-03-24 | DeepSeek V3 (0324) | diff | 55.1 | | $1.12 | | |
  | 2025-06-06 | DeepSeek R1 (0528) | diff | 71.4 (224 cases) | | $4.80 | | |
  | 2025-10-03 | DeepSeek-V3.2-Exp (Reasoner) | diff | 74.2 | 39.6 | $1.30 | ≈ $0.0058 | 291 |
  | 2025-10-03 | DeepSeek-V3.2-Exp (Chat) | diff | 70.2 | 38.7 | $0.88 | ≈ $0.0039 | 104 |

  [YAML](https://github.com/Aider-AI/aider/blob/main/aider/website/_data/polyglot_leaderboard.yml)
- **DeepSeek self-reported Aider-Polyglot:** V3.1 76.3 (thinking), V3.1-Terminus 76.1, V3.2-Exp 74.5. The V4/V4.1 cards do not report Aider Polyglot — [V3.1 card](https://huggingface.co/deepseek-ai/DeepSeek-V3.1), [V3.1-Terminus card](https://huggingface.co/deepseek-ai/DeepSeek-V3.1-Terminus), [V3.2-Exp card](https://huggingface.co/deepseek-ai/DeepSeek-V3.2-Exp)
- **Protocol:** the default is `--tries 2`: the model gets a second attempt after seeing test output. Results go to `tmp.benchmarks/<run>/…/<exercise>/.aider.results.json`, and `--stats` aggregates them — [benchmark.py](https://github.com/Aider-AI/aider/blob/main/benchmark/benchmark.py), [benchmark README](https://github.com/Aider-AI/aider/blob/main/benchmark/README.md)
- **Test commands:** `.py` → `pytest`; `.rs` → `cargo test -- --include-ignored`; `.go` → `go test ./...`; `.js` and `.cpp` → wrapper scripts under `/aider/benchmark/`; `.java` → `./gradlew test` — [benchmark.py](https://github.com/Aider-AI/aider/blob/main/benchmark/benchmark.py)
- **Docker guidance:** the README says the benchmark "is intended to be run *inside a docker container*" because LLM code may be dangerous (e.g. `os.system("sudo rm -rf /")`). The Dockerfile is `buildpack-deps:jammy` with cmake, openjdk-21, Go 1.21.5, rustup, Node 20 and aider — [README](https://github.com/Aider-AI/aider/blob/main/benchmark/README.md), [Dockerfile](https://github.com/Aider-AI/aider/blob/main/benchmark/Dockerfile)
- **Harbor adapter:** Harbor has an `aider_polyglot` adapter covering all 225 tasks as Docker tasks with an encrypted oracle, so any Harbor agent can run them. Parity results (aggregate only):
  - Claude Code v1.0.92 + claude-3-7-sonnet: 34.2% ± 2.2 on the original fork vs 35.3% ± 0.7 on the TB adapter (2 runs).
  - Claude Code v2.0.32 + claude-3-haiku: 2.67% vs 2.7%.

  [harbor adapters/aider_polyglot](https://github.com/harbor-framework/harbor/tree/main/adapters/aider_polyglot), [parity_summary.csv](https://github.com/harbor-framework/harbor/blob/main/adapters/parity_summary.csv)
- **Comparison point:** aider's own leaderboard lists claude-3-7-sonnet (no thinking) at pass_rate_2 = 60.4% — [YAML](https://github.com/Aider-AI/aider/blob/main/aider/website/_data/polyglot_leaderboard.yml)

### Inferences
- **Native macOS run is feasible for the Python and Rust subsets (64 exercises)** with Homebrew Python, pytest and rustup. That is exactly the Seatbelt-native scenario Terminal-Bench cannot offer. The Seatbelt sandbox would substitute for the Docker safety advice; that is a guard INIT already has.
- **The only cross-harness reference is aggregate**: aider vs DeepSeek models, plus Claude Code ≈35% vs aider ≈60% on Sonnet 3.7. That comparison is not protocol-matched (aider gives a second try with test errors; the Claude Code harness differs in prompts), so it is weak evidence of a harness effect.
- **Minimal-cost option:** run INIT + deepseek-flash on, say, 10 Python + 10 Rust exercises. Compare only against the aggregate DeepSeek-V3.2-Exp aider numbers (70–74% pass@2, ≈$0.004–0.006 per exercise), or rerun aider itself locally on the same 20 exercises to get a per-task baseline (cost ≈ cents).

### Gaps
- I found no published per-exercise results for any model or harness: no repo, HF dataset or blog dump.
- I did not verify whether aider has published anything since 2025-10 (the repo was last pushed 2026-05-22, but the leaderboard YAML was not updated).

---

## Q3. Princeton HAL: benchmarks and harnesses covered, downloadable traces and costs, DeepSeek runs

### Takeaway
HAL covers 9 benchmarks with 26,597 rollouts. It reports accuracy and total $ cost per (agent scaffold × model) and publishes encrypted per-run trace bundles (381 zips) on HF; the password is in a public decrypt script. DeepSeek V3, V3.1 and R1 appear. HAL has paused adding new models, so data is mostly from early 2025 to November 2025. All tasks are Linux, web or Docker-oriented, and there are no TB, polyglot or macOS-native tasks.

### Cited Findings
- **Coverage:** 9 benchmarks — AssistantBench, CORE-Bench Hard, GAIA, Online Mind2Web, SciCode, ScienceAgentBench, SWE-bench Verified Mini, TAU-bench Airline and USACO — with "26 597 Rollouts".
- **Status:** "We have paused updating HAL leaderboard with new models and are currently focusing on measuring reliability". CORE-Bench is declared "solved" after Claude Code + Opus 4.5 reached 77.8% (95.5% with manual grading) at $87.16 — [hal.cs.princeton.edu](https://hal.cs.princeton.edu/)
- **Harnesses** (from the leaderboard pages): Browser-Use, SeeAct, CORE-Agent, HAL Generalist Agent, HF Open Deep Research, Claude Code (submitted by N. Carlini), SWE-Agent, SciCode Tool Calling / Zero Shot, SAB Self-Debug, TAU-bench Tool Calling, and USACO Episodic + Semantic — [HAL benchmark pages](https://hal.cs.princeton.edu/corebench_hard)
- **DeepSeek rows** (accuracy, total $ per benchmark run):
  - CORE-Bench Hard ([corebench_hard](https://hal.cs.princeton.edu/corebench_hard)):
    - CORE-Agent: DeepSeek V3.1 (Aug 2025) 20.00% at $12.55; DeepSeek V3 (Mar 2025) 17.78% at $25.26; R1 (Jan 2025) 6.67% at $81.11.
    - HAL Generalist: V3 8.89% at $4.69; R1-0528 8.89% at $7.77.
  - SWE-bench Verified Mini: SWE-Agent + V3 24% at $11.77; HAL Generalist + V3 10% at $30.17 — [page](https://hal.cs.princeton.edu/swebench_verified_mini)
  - TAU-bench Airline: TAU Tool Calling + V3 44% at $5.43 (Pareto-optimal); HAL Generalist + V3 18% at $10.73 — [page](https://hal.cs.princeton.edu/taubench_airline)
  - USACO: Episodic + Semantic + V3 39.09% at $12.08; R1 38.11% at $80.04 — [page](https://hal.cs.princeton.edu/usaco)
  - GAIA: HAL Generalist + R1 30.30% at $73.19; HAL Generalist + V3 29.39% at $17.40 — [page](https://hal.cs.princeton.edu/gaia)
  - DeepSeek V3 and R1 also appear on AssistantBench, Online Mind2Web, SciCode and ScienceAgentBench — [HAL pages](https://hal.cs.princeton.edu/assistantbench)
- **Traces:** HF `agent-evals/hal_traces` has 381 files, one zip per (benchmark, agent, model, run), e.g. `assistantbench_…_deepseekaideepseekv3_…_UPLOAD.zip` at 61 MB; some exceed 1.5 GB. The zips are Fernet-encrypted, with the PBKDF2 password embedded in `hal-decrypt.sh` (anti-contamination, not access control) — [HF hal_traces](https://huggingface.co/datasets/agent-evals/hal_traces), [hal-decrypt.sh](https://hal.cs.princeton.edu/static/downloads/hal-decrypt.sh)
- **Paper and code:** the paper (arXiv 2510.11977) was accepted to ICLR 2026, and the harness code is at [princeton-pli/hal-harness](https://github.com/princeton-pli/hal-harness) — [arXiv](https://arxiv.org/abs/2510.11977)

### Inferences
- HAL is the best public source for cost-aware, same-model, different-scaffold comparisons with full traces (see Q5).
- Its tasks are web, scientific-reproduction, SWE and customer-service tasks, and most need Linux or Docker plus browsers. Model coverage stops around Claude Opus 4.5 and DeepSeek V3.1, so it is a poor fit for a cheap macOS-native INIT run with a 2026 DeepSeek flash model.
- USACO (programming problems with stdin/stdout judging) and SciCode could plausibly be run natively, but this is unverified.

### Gaps
- I did not decrypt a trace bundle to confirm that per-task success and per-task cost fields exist inside. The site's "Heatmap" suggests per-task outcomes are rendered, but I did not extract them.
- HAL has no DeepSeek V3.2 or V4 entries.

---

## Q4. OpenHands Index (with Commit0) and LiveCodeBench: per-task data, harnesses, DeepSeek, macOS feasibility

### Takeaway
The OpenHands Index publishes per-instance resolved/cost JSON for 5 benchmarks — SWE-bench Verified (500), SWE-bench Multimodal, SWT-bench, Commit0 (16 libraries) and GAIA (165) — for about 35 models with the OpenHands harness. It also covers Claude Code, Codex and Gemini CLI via ACP on a few models, and includes DeepSeek-V3.2-Reasoner and DeepSeek-V4-Pro. Commit0 itself needs Docker or Modal. LiveCodeBench has no agentic or harness leaderboard; its only "agentic" use found is Harbor's adapter parity runs.

### Cited Findings
- **Repo layout:** `results/<model>/{metadata.json, scores.json, instance_results/<benchmark>.json}`, where instance results map instance ID → `{resolved, cost}`. `scores.json` gives score, `cost_per_instance`, `average_runtime`, a `full_archive` URL (results.tar.gz) and a Laminar visualization link — [OpenHands/openhands-index-results](https://github.com/OpenHands/openhands-index-results)
- **Coverage:** `results/` covers 35 model directories, including DeepSeek-V3.2-Reasoner, DeepSeek-V4-Pro, GLM-4.7/5/5.1, Kimi-K2.x, MiniMax, Qwen3-Coder, Claude Opus 4.5–4.8, Fable 5, GPT-5.2–5.5 and Gemini 3.x. `alternative_agents/` contains acp-claude (Claude Code: opus-4-6, opus-4-7, sonnet-4-5), acp-codex (GPT-5.4, GPT-5.5), acp-gemini and openhands_subagents — [repo](https://github.com/OpenHands/openhands-index-results)
- **DeepSeek results** (score, $/instance):

  | Model | OpenHands version | SWE-bench | SWT | SWE-MM | Commit0 | GAIA |
  |---|---|---|---|---|---|---|
  | DeepSeek-V4-Pro (submitted 2026-05-16..20) | v1.22.1 | 73.2 ($0.040) | 68.1 ($0.035) | 36.8 ($0.375) | 12.5 ($0.158; 2 of 16, avg 1,805 s) | 12.7 ($0.159) |
  | DeepSeek-V3.2-Reasoner | v1.8.3, Commit0 v1.17.0 | 71.6 ($0.16) | 53.6 | 27.9 | 25.0 ($0.57) | 50.3 |

  Per-instance files are confirmed: swe-bench n=500 (366 resolved) and gaia n=165 for V4-Pro — [results/DeepSeek-V4-Pro](https://github.com/OpenHands/openhands-index-results/tree/main/results/DeepSeek-V4-Pro), [results/DeepSeek-V3.2-Reasoner](https://github.com/OpenHands/openhands-index-results/tree/main/results/DeepSeek-V3.2-Reasoner)
- **Methodology caveat:** an OpenHands blog post reportedly says a methodology update dropped several models by 10–30 pp. I saw this only in a search summary — [OpenHands Index: 3 Months Out](https://www.openhands.dev/blog/openhands-index-3-months-out); intro post: [Introducing the OpenHands Index](https://openhands.dev/blog/introducing-the-openhands-index)
- **Commit0:**
  - Format: "57 core Python libraries" to rebuild from scratch so that their unit tests pass.
  - Backends: `--backend` defaults to `modal`, with `docker` as the alternative.
  - Status: the repo was last pushed 2026-02-24.
  - In the OpenHands Index, commit0 has 16 instances.

  [commit-0/commit0](https://github.com/commit-0/commit0); OpenHands data [repo](https://github.com/OpenHands/openhands-index-results)
- **LiveCodeBench:** Harbor has a `livecodebench` adapter, with parity on 100 tasks:
  - terminus-2 + gpt-5-mini: 77.25 vs 76.50;
  - claude-code 2.0.32 + claude-haiku-4.5: 54.50 vs 53.25.

  [parity_summary.csv](https://github.com/harbor-framework/harbor/blob/main/adapters/parity_summary.csv)
- **DeepSeek LiveCodeBench (non-agentic, self-reported):** the V4 preview card reports pass@1 of 55.2 / 88.4 / 91.6 for V4-Flash Non-Think / High / Max — [V4 card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash)

### Inferences
- The OpenHands Index is the best public per-instance, cost-annotated, multi-harness dataset with DeepSeek. Its tasks are SWE-family (excluded here) plus Commit0 and GAIA, which are Docker/Modal and web-heavy. That limits macOS-native reuse.
- Commit0 instances are long (≈30 min per library for DeepSeek-V4-Pro) and Python-only. A native-macOS run with a local venv is conceivable but unvalidated, and comparability with the OpenHands Docker runs is uncertain.

### Gaps
- No agentic LiveCodeBench leaderboard with per-harness entries was found.
- I did not open `full_archive` tarballs to check trajectory completeness.
- I did not verify the date and details of the "10–30 pp drop" claim.

---

## Q5. Public evidence that the same model with a different harness gets a different score, and the size of the gap

### Takeaway
The evidence is abundant. Same-model gaps are typically 3–20 pp on Terminal-Bench (TB 2.0 up to 18–21 pp; TB 2.1 3–8 pp). They are up to 34–38 pp on HAL (scaffold choice), 12–19 pp on OpenHands Index Commit0 and GAIA (OpenHands vs Claude Code), and 6.5 pp on TB 2.1 / 8.7 pp on DeepSWE in DeepSeek's own 8-scaffold sweep of V4.1-Flash. Per-task disagreement is much larger than aggregate gaps: 7–13 of 89 tasks flip by ≥3 of 5 trials between harnesses on the same model.

### Cited Findings
**TB 2.0 (official rows; harness and version mixes, not controlled)** — computed from [leaderboard-read 2-0](https://www.tbench.ai/)

| Model | Harnesses (score) | Spread |
|---|---|---|
| Claude Opus 4.6 | Meta-Harness 76.4 … Claude Code 58.0 | 18.4 pp |
| Claude Opus 4.5 | Droid 63.1, Letta 59.1, Mux 58.4, Terminus 2 57.8, Goose 54.3, Claude Code 52.1, OpenHands 51.9, OpenCode 51.7 | 11.4 pp |
| GPT-5 | Codex CLI 49.6, OpenHands 43.8, Terminus 2 35.2, Mini-SWE-Agent 33.9 | 15.7 pp |
| GPT-5.3-Codex | 9 harnesses, 64.7–78.4 | 13.7 pp |
| Gemini 3.1 Pro | TongAgents 80.2 vs Gemini CLI 59.4 | 20.8 pp |
| Claude Haiku 4.5 | Goose 35.5 vs OpenHands 13.9 | 21.6 pp |
| Claude Sonnet 4.5 | 6 harnesses, 40.1–46.5 | 6.4 pp (low sensitivity) |

**TB 2.1 (same 445-trial protocol)** — [leaderboard-read main](https://www.tbench.ai/) + [Harbor REST](https://hub.harborframework.com)

| Model | Harness A | Harness B | Gap |
|---|---|---|---|
| Fable 5 | Claude Code 83.8 | Terminus 2 80.5 | 3.3 pp |
| GPT-5.5 | Codex 83.2 | Terminus 2 78.0 | 5.2 pp |
| Opus 4.7 | Claude Code 68.9 | Terminus 2 66.1 | 2.8 pp |
| Gemini 3 Pro | Terminus 2 73.9 | Gemini CLI 65.8 | 8.1 pp |
| Gemini 3.1 Pro | Terminus 2 65.6 | Gemini CLI 65.8 | ≈0 |

- Per task (computed), the number of tasks where the two harnesses differ by ≥3 of 5 passes on the same model:
  - Fable 5, Claude Code vs Terminus 2: 12 tasks (e.g. `gcode-to-text` 5/5 vs 0/5; `path-tracing-reverse` 5/5 vs 0/5; `compile-compcert` 2/5 vs 5/5).
  - Opus 4.7: 13 tasks.
  - Gemini 3 Pro: 10 tasks.
  - Gemini 3.1 Pro: 7 tasks.
  - GPT-5.5: 4 tasks.

**DeepSeek's own controlled scaffold sweep (V4.1-Flash, max effort)**
- Setup: N=3 on TB 2.1 and N=8 on DeepSWE v1.1, with Linux containers, T=1.0, top_p=0.95, 1M context and max_steps=500. TB 2.1 ran without network.

  | Benchmark | Claude Code | Codex | OpenCode | Pi | mini-SWE | DSH Minimal | DSH Standard | DSH PTC | Spread |
  |---|---|---|---|---|---|---|---|---|---|
  | TB 2.1 | 88.0 | 84.1 | 85.0 | 86.1 | 90.3 | 90.6 | 85.8 | 85.8 | 6.5 pp |
  | DeepSWE v1.1 | 69.8 | 65.6 | 65.5 | 66.2 | 74.2 | 72.6 | 70.5 | 67.6 | 8.7 pp |

  [DeepSeek-V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)
- DeepSeek-V3.2 on TB 2.0 (non-thinking): Claude Code 37.1 vs Terminus 39.3. Thinking mode with Claude Code reached 46.4, and thinking mode was "incompatible with Terminus" — [arXiv 2512.02556](https://arxiv.org/abs/2512.02556). The official Terminus 2 row is 39.6 — [tbench.ai](https://www.tbench.ai/)

**HAL (scaffold choice, same model)**
- SWE-bench Verified Mini, Claude Sonnet 4.5: SWE-Agent 68% vs HAL Generalist 34% (34 pp) — [HAL SWE-bench Mini](https://hal.cs.princeton.edu/swebench_verified_mini)
- CORE-Bench Hard, Opus 4.5: Claude Code 77.8% vs CORE-Agent 42.2% vs HAL Generalist 33.3% (35.6 pp, Claude Code vs CORE-Agent) — [HAL CORE-Bench](https://hal.cs.princeton.edu/corebench_hard)
- TAU-bench Airline, o4-mini High: TAU Tool Calling 56% vs HAL Generalist 18% (38 pp). DeepSeek V3: 44% vs 18% (26 pp) — [HAL TAU-bench](https://hal.cs.princeton.edu/taubench_airline)

**OpenHands Index (OpenHands vs Claude Code via ACP, same model)** — [openhands-index-results](https://github.com/OpenHands/openhands-index-results)

| Model | Benchmark | OpenHands | Other harness | Gap |
|---|---|---|---|---|
| Opus 4.7 | Commit0 | 56.2 | Claude Code 43.8 | 12.4 pp |
| Opus 4.7 | SWE-MM | 48.5 | Claude Code 32.4 | 16.1 pp |
| Opus 4.7 | GAIA | 81.2 | Claude Code 74.5 | 6.7 pp |
| Opus 4.7 | SWE-bench | 81.6 | Claude Code 75.8 | 5.8 pp |
| Opus 4.6 | Commit0 | 56.2 | Claude Code 37.5 | 18.7 pp |
| Opus 4.6 | GAIA | 80.0 | Claude Code 66.1 | 13.9 pp |
| GPT-5.5 | Commit0 | 43.8 | Codex 37.5 | 6.3 pp |
| GPT-5.5 | GAIA | 86.1 | Codex 84.8 | 1.3 pp |

Caveat: OpenHands runs its own index, so a home-harness bias is possible.

**Papers**
- Harness-Bench (arXiv 2605.27922; v1 2026-05-27, v2 2026-10-15): 106 sandboxed offline tasks and 5,194 trajectories across "representative harness configurations" and multiple model backends. It argues that capability should be reported per model-harness configuration — [arXiv](https://arxiv.org/abs/2605.27922)
- "Beyond Final Scores" (arXiv 2608.13417, 2026-08-13): 36 long-horizon AI R&D tasks and 7 frontier models. The abstract mentions harness design affecting stability — [arXiv](https://arxiv.org/abs/2608.13417)

### Inferences
- Harness effects are model-specific. Some models (Sonnet 4.5, GLM 4.7, Gemini 2.5 Flash) are harness-robust (≤2–6 pp), while others swing 15–20 pp.
- For INIT this means a 5–20 task comparison will be dominated by per-task variance unless it uses ≥3–5 trials. INIT should be compared against a same-model baseline (Terminus 2 + DeepSeek on TB 2.0, or rerunning Terminus 2 / mini-SWE-agent with the same flash model on the same subset), not against frontier-model rows.
- DeepSeek's own sweep suggests that, with a 2026 DeepSeek flash model, minimal scaffolds (DSH Minimal, mini-SWE) beat heavier CLIs (Codex, OpenCode) by 4–9 pp. That is a useful prior for INIT's design.

### Gaps
- I could not verify these secondary claims against primary sources:
  - Harness-Bench's per-harness table and the "23.8 pp spread (NanoBot 76.2 vs OpenClaw 52.4)" figure, which came from secondary write-ups;
  - the claim that arXiv 2608.13417 compares Claude Code, native CLIs and OpenCode with best@3 gaps ≤0.035;
  - a "10-harness SWE-bench Pro sweep (GLM-5.2: 23.2→52.4%)" figure seen only in a newsletter summary.
- I did not check whether Harness-Bench releases per-task data.

---

## Q6. Long-horizon benchmarks (many turns, hours) with public per-task results

### Takeaway
Three sources give long-horizon tasks with public per-task outcomes:
- **TB 3.0 and 4.0:** per-trial public via Harbor REST. TB 4.0 trials average 28 min to 2.8 h; frontier models reach 4.6–64.9%.
- **METR's time-horizon runs data:** 24k–42k per-run rows across 228 tasks up to 30 human-hours, including DeepSeek R1/V3 in TH 1.0. Most task code is private, and the public task repos are deprecated.
- **OpenHands Index Commit0:** about 30 min per instance.

None of these is runnable natively on macOS.

### Cited Findings
- **TB 4.0:** average trial duration 1,684–9,953 s (median 3,177 s), cost $0.22–29.10 per trial, scores 11.2–64.9%, and per-trial rewards and trajectories public. TB 3.0 has 12 rows at $2.07–19.50 per trial. Both are computed from [leaderboard-read](https://www.tbench.ai/) + [Harbor REST](https://hub.harborframework.com)
- **DeepSeek on TB 3.0 / 4.0 (self-reported only):** V4.1-Flash 30.0 / 31.2 and V4-Flash 7.6 / 7.0 (DSH Minimal); V4.1-Flash-1017 TB 4.0 42.3 (Claude Code) — [V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash), [1017 card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash-1017)
- **METR time-horizon data:**
  - Repo: `METR/eval-analysis-public` ships `reports/time-horizon-1-1/data/raw/runs.jsonl` and `time-horizon-1-0/...`. The repo was last pushed 2026-03-06.
  - Fields: task_id, task_family, task_source (HCAST / SWAA / RE-Bench), model/alias, scaffold, score_cont, score_binarized, human_minutes, generation_cost, tokens_count, time_limit, started and completed timestamps.
  - TH 1.1: 24,008 runs over 228 tasks, with 20 model aliases plus human baselines up to GPT-5.3-Codex and Claude Opus 4.6. Scaffolds include metr_agents/react, triframe and duet. Max human_minutes is 1,800, and 55 tasks are ≥4 h.
  - TH 1.0: 41,629 runs, including DeepSeek-R1, R1-0528, V3 and V3-0324 (≈1,345 runs each), with scaffolds flock-public and modular-public.

  [METR/eval-analysis-public](https://github.com/METR/eval-analysis-public); method paper [arXiv 2503.14499](https://arxiv.org/abs/2503.14499)
- **METR public task code:**
  - `hcast-public` is marked "DEPRECATED … The HCAST tasks are now deprecated" and contains only a subset of HCAST tasks.
  - `public-tasks` is deprecated (31 example tasks plus summaries of 186).
  - `RE-Bench` is a separate repo.

  [METR/hcast-public](https://github.com/METR/hcast-public), [METR/public-tasks](https://github.com/METR/public-tasks), [METR/RE-Bench](https://github.com/METR/RE-Bench)
- **OpenHands Index Commit0:** average runtime per instance of 636 s (Opus 4.7) to 1,805 s (DeepSeek-V4-Pro), with per-instance resolved/cost — [openhands-index-results](https://github.com/OpenHands/openhands-index-results)
- **Harbor Index:** an "80-task benchmark … distilled from more than 6,000 candidate tasks", with hub-hosted public trial uploads required for submission. The `harbor-index-1-3` leaderboard returned 0 rows via the API on 2026-10-21 — [harbor-framework/harbor-index](https://github.com/harbor-framework/harbor-index)

### Inferences
- **METR runs.jsonl is the richest long-horizon per-task dataset** and includes DeepSeek (2025 models), but its tasks mostly cannot be obtained or run, so INIT could not reproduce comparable runs.
- **TB 3.0/4.0 are reproducible, with public per-trial data**, but each trial costs dollars for frontier models, takes about an hour, and needs Docker. For a deepseek-flash INIT run, a 5-task TB 4.0 subset × 3 trials is plausible on cost: at DeepSeek V3.2-like token prices, likely well under $10 total. This is unverified.

### Gaps
- I did not find whether DeepSWE v1.1, NL2Repo-Bench, ProgramBench or AutomationBench (used in DeepSeek's cards) publish per-task results for several harnesses.
- METR has not published newer time-horizon run data after 2026-03 in this repo.

---

## Q7. Bottom line for INIT Agent: can it run 5–20 tasks and compare per task with published harness results at minimal cost?

### Takeaway
Yes, with one condition: it needs a Linux container. The cleanest path is **Terminal-Bench 2.0/2.1 via Harbor on local Docker**:
1. Pick 5–20 of the 89 tasks.
2. Run INIT + deepseek-flash with `-k 3..5`.
3. Compare per task against the official **Terminus 2 + DeepSeek-V3.2** run (per-trial on HF) and the 22 TB 2.1 per-trial rows (Claude Code, Codex, Terminus 2, Gemini CLI, Cursor CLI, mini-SWE-agent).

Expected model cost is about $1–5 at DeepSeek-V3.2-like usage. For a Seatbelt-native, Docker-free check, **Aider Polyglot Python + Rust (64 exercises)** runs on macOS, but public comparisons there are aggregate-only.

### Cited Findings
- Per-trial TB references:
  - TB 2.0 DeepSeek-V3.2 / Terminus 2: 445 trials with reward and cost — [HF](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard/tree/main/submissions/terminal-bench/2.0/Terminus2__DeepSeek-V3.2)
  - TB 2.1: 22 rows × 445 trials — [Harbor REST](https://hub.harborframework.com)
- Harbor supports custom agents, task include lists, `-k` attempts and a Docker backend — [harbor cli/jobs.py](https://github.com/harbor-framework/harbor/blob/main/src/harbor/cli/jobs.py)
- There is no native-macOS environment. Apple container needs macOS 26, and the owner's machine runs 14.6 — [harbor environments](https://github.com/harbor-framework/harbor/tree/main/src/harbor/environments), [apple/container](https://github.com/apple/container)
- The polyglot Python and Rust test commands are pytest and cargo, so they are runnable natively — [benchmark.py](https://github.com/Aider-AI/aider/blob/main/benchmark/benchmark.py)

### Inferences
- **Recommended plan (2 stages, minimal spend).**
  1. **Docker-free smoke test (≈ $0.10–0.50):** run 10 polyglot Python + 10 Rust exercises natively under Seatbelt with INIT + deepseek-flash. Also run stock aider + the same model on the same 20 exercises to get a same-model per-task baseline, because no public per-exercise data exists.
  2. **Per-task harness comparison (≈ $1–5 model cost plus local Docker time):**
     - Pick about 16 discriminative TB tasks (list in Q1) plus 3 anchor tasks, and run them with `harbor run -d terminal-bench/terminal-bench-2 -i <task>… -k 3..5 -e docker --agent <INIT adapter>`.
     - Compare against DS-V3.2/Terminus 2 per-task pass counts and the TB 2.1 rows.
     - Optionally also run Terminus 2 with deepseek-flash on the same subset for a same-model control.
- **Quality guards:**
  - Use the same task versions (2.0 vs 2.1 matters).
  - Use the default timeouts and limits, without overrides.
  - Run ≥3 trials per task.
  - Report a 95% CI and a per-task pass/k table.
  - Treat vendor DeepSeek TB numbers as non-comparable, because they use a different harness (DSH) and sometimes no network.
- **Risks:**
  - ARM-host emulation of x86 task images may cause slowdowns or timeouts that are not INIT's fault. Check by running the Harbor `oracle` agent on the subset first.
  - The DS-V3.2 reference run had 90 of 445 AgentTimeoutErrors, so timeouts are a major failure mode for cheap models.

### Gaps
- I did not verify that the chosen TB tasks build and pass the oracle on arm64 Docker Desktop on macOS 14.
- I did not verify deepseek-flash pricing or its cache-hit discount, which determines whether the $1–5 estimate holds.
