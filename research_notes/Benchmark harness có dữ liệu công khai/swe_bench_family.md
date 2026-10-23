# SWE-bench family (Verified, Lite, Multilingual, Multimodal, Pro, Multi-SWE-bench, SWE-bench-Live, SWE-rebench, DeepSWE): public per-instance data and how a tiny DeepSeek-flash run on a Mac can be compared against it

Research date: 2026-10-21. Primary data was pulled directly from the GitHub repos, the S3 bucket and the HF APIs during this session (raw files downloaded and analysed; the scripts and derived numbers are described inline). Anything older than ~6 months is marked **[stale?]**.

Abbreviations: "mini" = mini-SWE-agent (bash-only harness from the SWE-agent team); "entry" = one leaderboard submission folder.

---

## Q1. Where exactly are per-instance results published, and what metadata comes with them?

### Takeaway
SWE-bench (Verified/Lite/Test/Multilingual/Multimodal) has the richest public per-instance data: every entry in `SWE-bench/experiments` has a `results/results.json` list of resolved instance_ids and/or a `per_instance_details.json` (resolved + cost + api_calls per instance for mini runs), with patches/logs/trajectories in a public S3 bucket or the submitter's own GitHub repo. Multi-SWE-bench and SWE-bench-Live also publish per-instance results in GitHub repos; Scale's SWE-bench Pro publishes trajectories (Docent) but per-instance resolve lists were not found; SWE-rebench publishes aggregates only.

### Cited Findings

**SWE-bench (Princeton/Stanford team) — `SWE-bench/experiments`**
- Repo layout: `evaluation/{lite,verified,multimodal,multilingual,test}/<date>_<name>/` with `metadata.yaml`, `README.md`, logo and `results/` ("resolved instance ids + breakdowns"). Since mid-2026 the repo "holds entries, not artifacts": predictions, logs and traces live in the submitter's own public repo named by `metadata.yaml`'s `assets` block; entries made before mid-2026 point at `s3://swe-bench-submissions/` and "those keep working". `python -m analysis.download_logs evaluation/<split>/<entry>` fetches them, "Neither path needs credentials." — [experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md)
- New (Sept 2026): `swebench submit verify evaluation/verified/<entry> -s verified` "re-grades every instance from its recorded test output … No Docker, no re-execution." Per-instance artifacts in the submitter repo: `all_preds.jsonl`, `logs/<instance_id>/{patch.diff, report.json, test_output.txt.gz}`, `trajs/<instance_id>.*`. — [experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md)
- Counts measured from a sparse clone of `SWE-bench/experiments` (main, last push 2026-09-03): Verified 182 entries (135 with `results/results.json`, 41 with `per_instance_details.json`; 175 usable for a per-instance matrix, 7 old mini runs have neither); Lite 84 entries (all with results.json); Test (full 2,294) 24; Multimodal 22; Multilingual 14 (all with per_instance_details.json). — [SWE-bench/experiments evaluation/](https://github.com/SWE-bench/experiments/tree/main/evaluation)
- `results.json` keys: always `resolved` (list of ids), `no_generation`, `no_logs`; ~13 Verified/25 Lite entries also have `generated`, `applied`, `no_apply`, `install_fail`, `test_errored`, `test_timeout`, `reset_failed`. — measured from [experiments repo](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified)
- `per_instance_details.json` (mini runs) gives `{instance_id: {api_calls, cost, resolved}}`, e.g. DeepSeek V3.2 Reasoner on astropy__astropy-12907: 24 calls, $0.00976, resolved=true. The newest entry (Gemini 3.5 Flash, 2026-09-01) has only `api_calls` + `resolved` (no cost) and only 441 of 500 ids listed. — [deepseek-v3.2-reasoner entry](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified/20251201_mini-v1.17.1_deepseek-v3.2-reasoner); [gemini-3-5-flash entry](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified/20260901_mini-v2.4.2_gemini-3-5-flash)
- The S3 bucket is publicly listable without credentials; per-instance folders contain e.g. `patch.diff`, `report.json`, `job-output.json` (mini) or `eval.sh`, `patch.diff`, `report.json`, `test_output.txt` (OpenHands+GPT-5). — [S3 listing, DeepSeek V3.2 reasoner](https://swe-bench-submissions.s3.amazonaws.com/?list-type=2&prefix=bash-only/20251201_mini-v1.17.1_deepseek-v3.2-reasoner/&max-keys=5); [S3 listing, OpenHands GPT-5](https://swe-bench-submissions.s3.amazonaws.com/?list-type=2&prefix=verified/20250807_openhands_gpt5/logs/astropy__astropy-12907/)
- `metadata.yaml` fields: `tags.agent`, `agent_org`, `model` (exact API id, e.g. `openai/gpt-5-2025-08-07`, `deepseek-chat`, `DeepSeek-V3-0324`, `deepseek-v3.2`), `model_display`, `reasoning_effort`, `system.attempts` (1 or 2+), `os_model`, `os_system`, `checked` (SWE-bench team reproduced it); mini entries add `mini-swe-agent_version`, `cost`, `instance_cost`, `instance_calls`, `commit`, and `trajs_docent` (Transluce Docent dashboard). No explicit turn-limit field. — [deepseek-3-2-high metadata](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20260217_mini-v2.0.0_deepseek-3-2-high/metadata.yaml); [openhands_gpt5 metadata](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20250807_openhands_gpt5/metadata.yaml)
- Turn/cost limits for mini come from its config: current `swebench.yaml` has `step_limit: 250`, `cost_limit: 3.` ($3/instance), command `timeout: 60`, `environment_class: docker`. — [mini-swe-agent swebench.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml)
- Leaderboard JSON: `data/leaderboards.json` (4.1 MB) in the website repo holds boards Multilingual (13), Test (24), Verified (180), Lite (84), Multimodal (22), each row with agent, model, cost, instance_cost, instance_calls, logs/trajs URIs, `trajs_docent`, tags, and for mini rows the full `per_instance_details`. Last commit 2026-09-01 "Drop bash-only board"; its Verified rows end at 2026-02-26, i.e. it does **not** yet include the 2026-09-01 Gemini 3.5 Flash entry that exists in the experiments repo (JSON lags the repo). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json); [site commits](https://github.com/SWE-bench/swe-bench.github.io/commits/master)
- Submission policy: since 2025-11-18, Verified and Multilingual "only accept submissions from academic teams and research institutions with open source methods and peer-reviewed publications" (arXiv/tech report + academic/lab affiliation); "Anybody is still welcome to submit to the SWE-bench Multimodal leaderboard." — [experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md)

**Multi-SWE-bench (ByteDance Seed) — `multi-swe-bench/experiments`**
- Each entry must include `all_preds.jsonl`, `results/results.json`, `logs/[org]/[repo]/evals/pr-[id]/{fix.patch, fix-patch-run.log, report.json}`, `metadata.yaml` and `trajs/`. — [Multi-SWE-bench experiments README](https://github.com/multi-swe-bench/experiments/blob/main/README.md)
- `results.json` contains totals plus per-instance id lists (`completed_ids`, `unresolved_ids`, `submitted_ids`, `empty_error_patch_ids`, …), e.g. MopenHands + DeepSeek-R1-0528 on flash: 46 resolved / 300. No cost or turn fields. — [flash entry results.json](https://github.com/multi-swe-bench/experiments/tree/main/evaluation/flash/20250712_MopenHands_DeepSeek-R1-0528)
- Repo last pushed 2026-07-21 (CodeArts-Agent + GLM-5.1 C++ results); most per-language folders still dominated by the 2025-03/04 paper runs. **[mostly stale]** — [experiments commits](https://github.com/multi-swe-bench/experiments/commits/main)

**SWE-bench-Live (Microsoft) — `SWE-bench-Live/submission` + site JSONL**
- Each submission: `submissions/{subset}/{agent}/{model}/{README.md, preds.json, results.json, logs|trajs/}`; raw trajectories are mandatory (at least ten samples if an org forbids full release). Agent forks used for official runs: SWE-agent, OpenHands, ClaudeCode, Win-Agent. — [SWE-bench-Live submission README](https://github.com/SWE-bench-Live/submission/blob/main/README.md)
- Leaderboard data `reports-0605.jsonl` (162 rows; updated 2026-10-20) has `name, set, total, resolved, date, url, verified`; 2025 rows store `resolved` as a full instance-id list (plus `applied`, `located` lists); 2026 rows store only counts (per-instance then lives in each submission's `results.json`, e.g. 18.9 KB for TianxiCode). Sets: lite, full, verified, ccpp, csharp, java, tsjs, go, rust, windows. — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl); [leaderboard.js](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/leaderboard.js)

**SWE-bench Pro (Scale AI)**
- Public leaderboard: 731-instance public set; harness mini-swe-agent for the newer (asterisked) rows; "250 turns" for most entries, grayed-out entries used a 50-turn limit and capped cost; trajectories published via a linked Docent dashboard; per-instance resolve lists not stated. — [Scale SWE-Bench Pro (Public)](https://labs.scale.com/leaderboard/swe_bench_pro_public)
- Repo notes: "(10/3) We have updated results without cap limit here: https://scaleapi.github.io/SWE-bench_Pro-os/"; "(05/18) We have identified some issues with the leaderboard and are currently working on addressing them." — [SWE-bench_Pro-os README](https://github.com/scaleapi/SWE-bench_Pro-os)

**SWE-rebench (Nebius)**
- Reports resolved rate ± margin, pass@5, $/problem, tokens/problem; current window 15/05/2026–01/07/2026 with 111 problems from 65 repos; links an HF dataset with the problems and Docker images; per-instance downloads/trajectories not stated. — [swe-rebench.com](https://swe-rebench.com/)
- HF `nebius/SWE-rebench-leaderboard` contains the monthly task sets (`data/2025_01` … `data/2026_03` parquet), not per-model results. — [HF dataset](https://huggingface.co/datasets/nebius/SWE-rebench-leaderboard)

### Inferences
- For a per-task comparison, SWE-bench Verified is still the best-instrumented target: 175 entries × 500 ids can be turned into a resolved/not-resolved matrix with no credentials (I built it in this session from the sparse clone in ~2 s, 11 MB).
- Cost and step counts per instance are only available for mini runs (41 Verified + 14 Multilingual entries); for other harnesses you only get resolved/unresolved per id plus raw logs.

### Gaps
- Could not confirm whether Scale publishes per-instance pass/fail for Pro (the Docent trajectories may imply it, not verified).
- SWE-rebench per-instance results: not found.

---

## Q2. Which harness × model combinations are listed (incl. Claude Code / Codex CLI / OpenCode)?

### Takeaway
SWE-bench Verified lists 77 distinct agents (47 mini-SWE-agent runs, 9 OpenHands, 8 SWE-agent, many closed vendor agents), but no Claude Code / Codex CLI / OpenCode entries. CLI-agent products appear on SWE-bench-Live (Claude Code fork), SWE-rebench (Claude Code, Codex, Junie, Cursor), SWE-bench Pro V2's locked adapters (Claude Code, Codex, mini) and DeepSeek's own cross-harness table on DeepSWE.

### Cited Findings
- From `leaderboards.json`: Verified has 77 distinct agents; most frequent mini-SWE-agent 47, OpenHands 9, SWE-agent 8, EPAM AI/Run 6, RAG baseline 6, Tools 5. Lite: 58 agents (SWE-agent 6, Moatless 5, KGCompass 3, Agentless-1.5 2, OpenHands 2, AutoCodeRover 2, Aider 1). Multimodal: 12 agents (GUIRepair 4, OpenHands-Versa 2, SWE-agent variants, Agentless, Zencoder…). Test: 13 agents. — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Date ranges in that JSON: Verified 2023-10-10 → 2026-02-26; Lite → 2025-09-11 **[stale]**; Test → 2025-12-19; Multimodal → 2025-11-17; Multilingual 2026-02-13 → 2026-02-20 (all mini v2.0.0/2.0.0a0). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Open harness entries on Verified (selection): OpenHands + GPT-5 71.8% (2025-08-07, checked), OpenHands + Claude 4 Sonnet 70.4%, OpenHands + Kimi K2 65.4%, OpenHands + Qwen3-Coder-480B 69.6%, Moatless + Claude 4 Sonnet 70.8%, SWE-agent + Claude 4 Sonnet 66.6%, Agentless-1.5 + Claude 3.5 Sonnet 50.8%, Agentless Lite + o3-mini 42.4%, live-SWE-agent + Claude 4.5 Opus 79.2% (2025-12-15), AutoCodeRover-v2.1 51.6%. — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Searching names/agents for "claude code", "codex", "opencode" across all SWE-bench boards returned no entries (Aider appears once on Lite, 2024-05-23). — derived from [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- SWE-bench-Live official agents: SWE-agent, OpenHands, ClaudeCode, Win-Agent forks; vendor submissions include TianxiCode (Lenovo), Slingshot, AMI Agent, agav, AiWork.Code, MIT-IBM BOAD. — [submission README](https://github.com/SWE-bench-Live/submission/blob/main/README.md); [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)
- SWE-rebench splits entries into "Model" and "Agent" types; Agent entries include Junie, Claude Code, Codex, Cursor. — [swe-rebench.com](https://swe-rebench.com/)
- SWE-bench Pro V2 ships locked adapters `locked_claude_code:LockedClaudeCode`, `locked_mini_swe:LockedMiniSwe`, `locked_codex:LockedCodex`, usable with any OpenAI-compatible endpoint (`--ae OPENAI_API_BASE=...`). — [SWE-bench Pro V2 README](https://github.com/scaleapi/SWE-bench_Pro-os/blob/main/v2/README.md)
- Multi-SWE-bench: MSWE-agent, MagentLess, MopenHands (ByteDance multi-language ports of SWE-agent, Agentless, OpenHands) × {Claude 3.5/3.7 Sonnet, DeepSeek-V3, DeepSeek-R1, Doubao-1.5-pro/thinking, GPT-4o, o1, o3-mini-high, Qwen2.5-72B, Gemini-2.5-Pro, Llama-4-Maverick}; plus later RepoRepair, InfCode, CodeArts-Agent entries. — [Multi-SWE-bench experiments/evaluation](https://github.com/multi-swe-bench/experiments/tree/main/evaluation)

### Inferences
- If the new agent is a "Claude-Code-like CLI", the closest public per-instance comparators are SWE-bench-Live (Claude Code fork + OpenHands + SWE-agent with the same DeepSeek-V4-Pro model) rather than SWE-bench Verified.

### Gaps
- SWE-rebench agent-entry details (turn limits, per-task outcomes) not retrieved.

---

## Q3. DeepSeek entries (V3, V3.1, V3.2, V4, V4.1, "flash") — exact scores, dates, harnesses

### Takeaway
Independent DeepSeek-with-public-per-instance-data entries exist on SWE-bench Verified (mini + DeepSeek V3.2: 60.0% at $0.028/task, 70.0% at $0.448/task), Multilingual (mini + V3.2: 59.0%), Lite (Moatless/KGCompass + V3), Multi-SWE-bench (V3/R1, 2025), and — most relevant to "deepseek-flash" — SWE-bench-Live Lite: **TianxiCode + DeepSeek-v4.1-Flash 213/300 = 71.0% (2026-10-08, verified)**. No SWE-bench Verified leaderboard entry yet for V4/V4.1 Flash; DeepSeek's self-reported V4-Flash Verified scores are 73.7–79.0%.

### Cited Findings

**SWE-bench Verified (500)**
- mini-SWE-agent v1.17.1 + DeepSeek V3.2 Reasoner — 60.0%, total $14.04, $0.0281/instance, 46.4 API calls avg, 2025-12-01, checked=true. — [entry](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified/20251201_mini-v1.17.1_deepseek-v3.2-reasoner)
- mini-SWE-agent v2.0.0 + DeepSeek V3.2 (reasoning_effort high, model id `deepseek-v3.2`) — 70.0%, total $223.92, $0.448/instance, 88.5 calls avg, 2026-02-17. — [metadata.yaml](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20260217_mini-v2.0.0_deepseek-3-2-high/metadata.yaml)
- SWE-Exp + DeepSeek-V3-0324 — 42.0% (210/500), attempts: 2, not checked, 2025-08-06; arXiv 2507.23361. — [SWE-Exp metadata.yml](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20250806_SWE-Exp_DeepSeek-V3/metadata.yml)
- ByteDance's runs on SWE-bench Verified (stored as Multi-SWE-bench `python/verified`, 2025-03-29): Agentless + DeepSeek-R1 211/500 (42.2%), Agentless + DeepSeek-V3 205/500 (41.0%), OpenHands + V3 139/500 (27.8%), OpenHands + R1 130/500 (26.0%), SWE-agent + V3 21/500 (4.2%, only 115 submitted), SWE-agent + R1 10/500 (2.0%, 80 submitted). **[stale]** — [Multi-SWE-bench python/verified](https://github.com/multi-swe-bench/experiments/tree/main/evaluation/python/verified)

**SWE-bench Lite (300)** **[stale]**
- Moatless Tools + DeepSeek V3 (`deepseek-chat`) 30.67% (2025-01-11, checked); KGCompass + DeepSeek V3 36.67% (2025-06-09); DARS Agent (Claude 3.5 Sonnet + DeepSeek R1) 47.0% (2025-02-05). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json); [moatless metadata](https://github.com/SWE-bench/experiments/blob/main/evaluation/lite/20250111_moatless_deepseek_v3/metadata.yaml)

**SWE-bench Multilingual (300, 9 languages)**
- mini v2.0.0a0 + DeepSeek V3.2 — 59.0% (177/300), $0.384/instance, 82.7 calls, 2026-02-13 (other models in the same batch: Gemini 3 Flash 72.7%, Claude 4.6 Opus 72.0%, MiniMax M2.5 68.3% at $0.101/inst, GPT-5 mini 39.7% at $0.052/inst). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)

**SWE-bench-Live**
- TianxiCode + DeepSeek-v4.1-Flash: Lite 213/300 = 71.0% (2026-10-08, verified=True — top of Lite board); Java 109/160 (2026-10-16); Go 88/138 (2026-10-16); Rust 87/171 (2026-10-20). Submission README: TianxiCode CLI v0.1.486 (Lenovo), model `tianxi-agent-plan/deepseek-flash` with `--variant max --thinking`, single rollout, no retries/best-of-N, official task container. — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl); [TianxiCode README](https://github.com/SWE-bench-Live/submission/tree/main/submissions/lite/tianxicode/deepseek-flash)
- DeepSeek-V4-Pro (2026-06-05, verified) across three harnesses on MultiLang (resolved/total): TS/JS OpenHands 75, Claude Code 63, SWE-agent 75 /204; Java 40/39/40 /102; C# 30/29/35 /78; Go 27/29/29 /98; Rust 18/19/22 /64; C/C++ 15/15/16 /66. Windows: Win-agent + V4-Pro 13/60. — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)
- DeepSeek-V3.1-Terminus (2026-01-08) across Claude Code / OpenHands / SWE-agent: Java 14/15/17 of 62; Rust 8/12/13 of 45; Go 20/15/14 of 68; TS/JS 25/18/17 of 162; C# 4/5/6 of 28; C/C++ 5/12/13 of 48; Windows Win-agent 10/50. — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)
- DeepSeek V3 on Lite (2025-05-01, per-instance id lists in the JSONL): SWE-agent 46/300 (15.3%), Agentless 40/300 (13.3%), OpenHands 39/300 (13.0%). **[stale]** — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)

**Multi-SWE-bench** **[stale, 2025]**
- flash (300): MopenHands + DeepSeek-R1-0528 46/300 = 15.3% (vs Claude 4 Sonnet 25.0%, Qwen3-Coder-480B 25.3%, o3 23.7%, Kimi-K2 18.3%). mini (400): MagentLess + R1 40 (10.0%), MagentLess + V3 32 (8.0%), MopenHands + V3 30 (7.5%), MopenHands + R1 28 (7.0%), MSWE-agent + V3 20 (5.0%), MSWE-agent + R1 15 (3.8%). Full per-language: e.g. Java MagentLess + R1 29/128 (22.7%); Go MagentLess + V3 23/428 (5.4%). — computed from [experiments results.json files](https://github.com/multi-swe-bench/experiments/tree/main/evaluation)

**SWE-bench Pro**
- Scale public board: `deepseek-v3p2` 15.56 ± 2.63 (rank 20 of 25); harness not stated for that row. — [Scale SWE-Bench Pro (Public)](https://labs.scale.com/leaderboard/swe_bench_pro_public)

**SWE-rebench**
- DeepSeek-V4 Pro [high]: 40.2% ± 1.29, pass@5 64.0%, $0.15/problem (window 2026-05-15 → 2026-07-01); DeepSeek-V4 Flash [high], V3.2, V3.1, V3-0324, R1-0528 listed but no values captured for that window by my fetch. — [swe-rebench.com](https://swe-rebench.com/)

**DeepSeek self-reported (model cards; harness for SWE-bench numbers not stated in the lines retrieved)**
- DeepSeek-V4-Flash (preview, 2026-04-22; 284B total/13B active, 1M ctx): SWE Verified 73.7 / 78.6 / 79.0 (Non-think / High / Max); SWE Pro 49.1 / 52.3 / 52.6; SWE Multilingual 69.7 / 70.2 / 73.3. V4-Pro Max: 80.6 / 55.4 / 76.2. — [DeepSeek-V4-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash)
- DeepSeek-V4-Flash-0731 (official release, 2026-07-31): no SWE-bench numbers; DeepSWE 54.4 (vs 7.3 for the preview), evaluated with "the minimal mode of DeepSeek Harness (to be released)". — [V4-Flash-0731 card](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash-0731)
- DeepSeek-V4.1-Flash (2026-09-10; 552B backbone, 8B active prefill/16B decode) and V4.1-Flash-1017 (2026-10-17): cards drop SWE-bench Verified/Pro entirely and report DeepSWE v1.1 = 74.2 and 77.0 respectively (1017 card: "Mini-SWE-V2 harness for DeepSWE v1.1"). — [V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash); [V4.1-Flash-1017 card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash-1017)
- Third-party: FlowHunt lists V4-Flash Verified 79.0% as self-reported and claims CAISI measured V4-Pro at 74.0% vs 80.6% reported (unverified aggregator). — [FlowHunt V4 Flash](https://www.flowhunt.io/ai-leaderboard/deepseek-v4-flash/)

### Inferences
- The model the owner calls "deepseek-flash" most likely maps to DeepSeek-V4-Flash(-0731) or V4.1-Flash(-1017); the only independent, per-instance-backed result for it found is the SWE-bench-Live Lite TianxiCode entry (71.0%), which uses a vendor CLI (`tianxi-agent-plan/deepseek-flash`) — a good per-task comparator for a new CLI-style agent with the same model family.
- The Scale Pro score for DeepSeek V3.2 (15.6%) vs DeepSeek's own Pro claims for V4 (49–55%) are not the same model or harness; treat cross-source comparisons with caution.

### Gaps
- DeepSeek V3.1 / V3.2 self-reported SWE-bench numbers were not re-verified in this session (not fetched).
- Which harness DeepSeek used for its V4 SWE Verified/Pro numbers: not found in the retrieved card sections.
- Nemotron-CORTEXA Verified entries (58.2% 2025-04-10; 68.2% 2025-05-16, "Multiple" models) matched "deepseek" in their metadata text; whether DeepSeek is a component was not confirmed.
- SWE-rebench V4 Flash numbers not captured.

---

## Q4. How evaluation runs: Docker, sb-cli, Modal, macOS Apple Silicon, costs and limits

### Takeaway
You can generate patches anywhere (including a Mac) and grade them without local Docker via (a) sb-cli (free cloud grading on AWS, `--instance_ids` subset support, quota-limited, Verified/Lite/Multimodal only), (b) the SWE-bench harness on Modal (`--modal true`, needs a Modal account), or (c) for Pro V2, Harbor on Modal. Local Docker on Apple Silicon is "experimental" (images are x86_64; the v5 CLI can build arm images locally from a task repo). The catch is the *agent* phase: official environments are x86 Linux containers, so a Mac-native agent run without Docker lacks the exact repo environment.

### Cited Findings
- sb-cli: `pip install sb-cli`; API key via `sb-cli gen-api-key <email>` + email verification code; subsets `swe-bench-m`, `swe-bench_lite`, `swe-bench_verified`; splits `dev`/`test`. — [sb-cli README](https://github.com/SWE-bench/sb-cli/blob/main/README.md); [config.py](https://github.com/SWE-bench/sb-cli/blob/main/sb_cli/config.py)
- `sb-cli submit <subset> <split> --predictions_path preds.json [--instance_ids id1,id2,id3] [--run_id] [--wait_for_evaluation 1]`; predictions are `{instance_id: {model_patch, model_name_or_path}}` or a list. Client uploads with up to 24 threads; polls with 5- and 30-minute timeouts. — [submit docs](https://github.com/SWE-bench/sb-cli/blob/main/docs/user-guide/submit.md); [submit.py](https://github.com/SWE-bench/sb-cli/blob/main/sb_cli/submit.py)
- Quotas: `sb-cli get-quotas` shows "Remaining Submission Quotas" per subset/split; the doc's example shows `swe-bench_verified test 1`, `swe-bench_lite test 1`, `swe-bench_lite dev 976`, `swe-bench-m dev 997`; "Quotas are refreshed periodically according to your subscription level". — [get-quotas docs](https://github.com/SWE-bench/sb-cli/blob/main/docs/user-guide/get-quotas.md)
- mini-swe-agent docs: sb-cli is "free"; "Typically you will have results within 20 minutes (this is not limited by how many instances you run, but by the slowest-to-evaluate instance)". — [mini-swe-agent SWE-bench usage](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/usage/swebench.md)
- sb-cli repo last commit 2025-08-14 **[possibly stale]**; client has no `multilingual` subset. `https://api.swebench.com/` answered HTTP 200 on 2026-10-21 (liveness only; no authenticated test was made). — [sb-cli commits](https://github.com/SWE-bench/sb-cli/commits/main)
- SWE-bench harness v5 CLI (Sept 2026): `swebench eval verified -p <preds> --run-id <id> -j <workers>`; `-i <instance_id>` for single instances; `swebench report <run_id>` re-grades saved logs without containers; `swebench infer verified -m gpt-5` runs mini-SWE-agent; "On an M-series Mac or another ARM-based system, use `--task-repo` so the images are built locally with Docker Buildx." Recommended: "x86_64 machine with at least 120GB of free storage, 16GB of RAM, and 8 CPU cores … Support for arm64 machines is experimental." Result caching is keyed by `run_id` + `instance_id` only (re-using a run_id with a new patch returns the old result). — [SWE-bench README](https://github.com/SWE-bench/SWE-bench/blob/main/README.md)
- Modal: `pip install modal swebench[modal]`, `modal setup`, then `python -m swebench.harness.run_evaluation --dataset_name ... --predictions_path ... --parallelism 10 --modal true`. — [SWE-bench evaluation guide](https://github.com/SWE-bench/SWE-bench/blob/main/docs/guides/evaluation.md)
- mini-swe-agent environments: docker, local, singularity, bubblewrap, swerex_docker, **swerex_modal** (Modal sandboxes; `runtime_timeout` 1800 s/instance in `swebench_modal.yaml`), **contree** (Nebius ConTree cloud sandboxes, token required). `--slice 0:5` and `--filter <regex>` select instances. Warning: "The docker containers for Linux assume an x86 Linux architecture; you might not be able to run them on other architectures." — [mini-swe-agent SWE-bench usage](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/usage/swebench.md); [swerex_modal docs](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/reference/environments/swerex_modal.md); [contree docs](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/reference/environments/contree.md); [swebench_modal.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench_modal.yaml)
- SWE-bench Multimodal: previously gradable "*only*" via sb-cli; "[Sep. 1, 2026]: SWE-bench Multimodal v2 is now fully open source, with 480 tasks available for local evaluation". — [experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md); [SWE-bench README](https://github.com/SWE-bench/SWE-bench/blob/main/README.md)
- SWE-bench Pro V2: images `ghcr.io/scaleapi/swe-bench_pro-v2:<instance_id>` are linux/amd64, anonymous pull; run with `harbor[modal]>=0.22` + `modal setup`; locked protocol: offline agent phase (only model endpoint allowlisted), diff re-graded on a pristine image (`patch_replay`), "50-minute budget per task". v1 used Docker Hub `jefzda/sweap-images` with Modal (recommended) or `--use_local_docker` (beta). — [Pro V2 README](https://github.com/scaleapi/SWE-bench_Pro-os/blob/main/v2/README.md); [Pro README](https://github.com/scaleapi/SWE-bench_Pro-os)
- SWE-bench Verified mini (50 tasks) "requires 5GB instead of 130GB of storage". — [HF card](https://huggingface.co/datasets/MariusHobbhahn/swe-bench-verified-mini)

### Inferences
- Minimal-cost Mac path for 5–20 Verified tasks: run the agent in a cloud sandbox with the official x86 image (mini's `swerex_modal`/`contree`, or own harness on Modal), collect `git diff` → `preds.json`, grade with `sb-cli submit swe-bench_verified test --instance_ids …` (free) or with `swebench … --modal true`. Quality guard: same image + same grader as the published entries, so per-instance outcomes are comparable.
- A Docker-less, Mac-native agent run (repo checked out locally, no test environment) is feasible for patch generation but not apples-to-apples: published agents could run tests inside the image. Mark such results as "no-env" if used.
- The Verified `test` quota (example: 1) may make sb-cli unsuitable for many small iterative runs; Modal or local Docker (Buildx, `--task-repo`) on a small subset (mini-50 ≈ 5 GB) are the fallbacks. Within the owner's 20 GB free-disk rule, the 50-task subset is the only local-Docker option that fits comfortably.

### Gaps
- Actual current sb-cli quotas per account, whether a 10-instance submission consumes a whole "test" run, and whether a paid "subscription level" exists: not documented beyond the example.
- Modal pricing / free credits for grading: not researched in this session.
- Whether arm64 locally-built images reproduce x86 grading outcomes for all 500 instances: not found.

---

## Q5. Official or accepted tiny subsets with per-harness results; statistical caveats

### Takeaway
There is no official "Verified mini" from the SWE-bench team, but the community `MariusHobbhahn/swe-bench-verified-mini` (50 tasks, django + sphinx only) is widely used and — because every Verified entry publishes resolved ids — per-harness scores on it can be derived for 175 entries (r = 0.967 with full-set scores). Multi-SWE-bench has official `mini` (400) and `flash` (300, easy/medium/hard tags) subsets with entries; SWE-bench Pro V2 has HARD-51; SWE-bench-Live has a 300-task Lite.

### Cited Findings
- verified-mini: 50 of 500, "approximately the same distribution of performance, test pass rates and difficulty", 5 GB vs 130 GB; drop-in for the Inspect implementation. — [HF card](https://huggingface.co/datasets/MariusHobbhahn/swe-bench-verified-mini)
- Measured in this session: the 50 ids are 25 django + 25 sphinx-doc. Across 175 Verified entries, Pearson r(full score, mini-50 score) = 0.967; mini-50 averages 4.6 pp lower than full (sd 5.05 pp, max |diff| 17.8 pp). Examples (full → mini50): DeepSeek V3.2 high 70.0 → 60.0; DeepSeek V3.2 Reasoner 60.0 → 56.0; SWE-Exp + DeepSeek-V3 42.0 → 44.0; OpenHands + GPT-5 71.8 → 62.0; OpenHands + Claude 4 Sonnet 70.4 → 70.0; GPT-5 mini (mini v2.0.0) 56.2 → 66.0; Gemini 3.5 Flash 71.8 → 70.0. — computed from [SWE-bench/experiments](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified) + [verified-mini ids](https://huggingface.co/datasets/MariusHobbhahn/swe-bench-verified-mini)
- Instance difficulty spread across 175 Verified entries: 26 instances solved by none; 85 instances solved by <10% of entries; 74 instances solved by 35–65% of the 75 entries dated ≥ 2025-07-01 (django 29, sympy 16, psf 6, pytest 6, sphinx 5, matplotlib 4, scikit-learn 4, …). Example random 15 from that mid band with DeepSeek V3.2 reasoner/high outcomes: django-11848 (1/1), django-12965 (0/1), django-14404 (1/1), django-16938 (0/1), matplotlib-25775 (1/0), seaborn-3069 (0/1), requests-2931 (1/0), pytest-6197 (0/0), sklearn-13124 (0/1), sklearn-14983 (1/1), sphinx-7454 (1/1), sympy-13877 (0/0), sympy-13878 (0/1), sympy-15875 (0/0), sympy-19040 (0/1). — computed from [SWE-bench/experiments](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified)
- Run-to-run noise, same model family + same harness family: DeepSeek V3.2 reasoner (mini v1.17.1) vs DeepSeek V3.2 high (mini v2.0.0) — both solved 275, only reasoner 25, only high 75 → they disagree on 100/500 = 20% of instances. — computed from the two [per_instance_details.json](https://github.com/SWE-bench/experiments/tree/main/evaluation/verified/20260217_mini-v2.0.0_deepseek-3-2-high) files
- Multi-SWE-bench `flash/index.json`: 300 ids with `easy_ids`/`medium_ids`/`hard_ids`; 9 MopenHands entries (2025-07); `mini/index.json`: 400 ids, 37 entries (MSWE-agent/MagentLess/MopenHands, 2025-03/04/07). — [flash](https://github.com/multi-swe-bench/experiments/tree/main/evaluation/flash); [mini](https://github.com/multi-swe-bench/experiments/tree/main/evaluation/mini)
- SWE-bench Pro HARD-51: "The 51 tasks failed by at least two of five model families (Claude Opus 5, GLM-5.3, Kimi-K3, Inkling, Gemini 3.8 Flash) … Scores on it separate frontier models far better than the full set does." — [Pro V2 README](https://github.com/scaleapi/SWE-bench_Pro-os/blob/main/v2/README.md)

### Inferences
- Binomial noise on n = 10 is about ±30 pp (95% CI) and on n = 20 about ±22 pp, so a 5–20-task run cannot rank harnesses by aggregate score. What it can do is per-task paired comparison: for each instance, compare the new agent's outcome against the solve-rate of that instance across the 175 entries (and specifically against the two DeepSeek V3.2 mini runs and TianxiCode+DeepSeek-flash on Live). Choosing tasks from the 35–65% band maximises information per task; avoid the 26 never-solved instances (likely broken tests per the OpenAI audit, Q6).
- The 20% instance-level disagreement between two DeepSeek V3.2 mini runs means even a perfect replica harness will differ on ~2 of 10 tasks; a single run on 10 tasks should be read with that noise floor.

### Gaps
- No official SWE-bench-team "Verified mini" exists; the 50-task community subset covers only two repos.
- No accepted Pro V2 or Live "mini" subset with published per-harness results was found besides HARD-51 (results for HARD-51 per harness not retrieved).

---

## Q6. Contamination / saturation caveats in 2026

### Takeaway
SWE-bench Verified is widely treated as saturated and contaminated in 2026: OpenAI stopped reporting it (Feb 23, 2026), audits found ~59% of the hardest-failing tasks had flawed tests, and the SWE-bench team now restricts Verified submissions to academic work. Attention moved to SWE-bench Pro (itself revised into V2 after grading problems), SWE-bench-Live, SWE-rebench and new hand-written benchmarks such as Datacurve's DeepSWE.

### Cited Findings
- OpenAI (Feb 23, 2026) stopped reporting SWE-bench Verified and recommended others do the same, recommending SWE-bench Pro meanwhile; audited 138 hard problems (~27.6%) and found 59.4% had material test/problem-statement issues (~35.5% enforce unrelated implementation details, 18.8% check unrequested functionality); GPT-5.2, Claude Opus 4.5 and Gemini 3 Flash Preview could reproduce some original fixes. (Primary page returned HTTP 403 to my fetcher; numbers from secondary reports.) — [OpenAI post](https://openai.com/index/why-we-no-longer-evaluate-swe-bench-verified/); [AI Weekly summary](https://aiweekly.co/alerts/openai-drops-swe-bench-verified-backs-pro-after-flaw-audit); [GIGAZINE, 2026-04-29](https://gigazine.net/gsc_news/en/20260429-swe-bench-verified)
- Saturation: scores rose only ~6 pp over Aug 2025 → Feb 2026. — [GIGAZINE](https://gigazine.net/gsc_news/en/20260429-swe-bench-verified). (A web-search summary also cited a top-score move of 74.9% → 80.9% over six months; the originating page was not fetched, so treat as unverified — candidate source: [Pebblous blog](https://blog.pebblous.ai/blog/swe-bench-verified-retired/en/).)
- Epoch AI's benchmark review gives SWE-bench Verified a "Flawed" verdict. — [Epoch AI review](https://epoch.ai/benchmarks/swe-bench-verified/review)
- SWE-bench team: Verified/Multilingual submissions restricted to academic/research teams with publications since 2025-11-18; 2026 Verified entries are only mini-SWE-agent runs (Feb 2026 batch, and Gemini 3.5 Flash on 2026-09-01); the "Bash Only" board was folded into Verified as a filter on 2026-09-01. — [experiments README](https://github.com/SWE-bench/experiments/blob/main/README.md); [site commits](https://github.com/SWE-bench/swe-bench.github.io/commits/master)
- SWE-bench Pro problems: Scale flagged "issues with the leaderboard" (05/18/2026); removed outdated tests (2/9/2026); released V2 (2026-09-22): 731 → 642 tasks (89 dropped), 529 problem statements rewritten, 214 test patches and 38 gold patches revised, 211 images fixed; HF default config is now V2 (v1 kept as config `v1`). — [SWE-bench_Pro-os README](https://github.com/scaleapi/SWE-bench_Pro-os); [V2 README](https://github.com/scaleapi/SWE-bench_Pro-os/blob/main/v2/README.md)
- Datacurve released DeepSWE v1 (May 26, 2026) with a critique that SWE-Bench Pro's inherited test suites misgraded about a third of audited agent runs; v1.1 (mid-June 2026) isolates grading; 113 hand-written tasks (TS, Go, Python, JS, Rust); primary leaderboard uses mini-swe-agent; Pier runs Claude Code/Codex CLI/Gemini CLI as pilots. (Secondary source.) — [i-programmer](https://www.i-programmer.info/news/105-artificial-intelligence/19016-deepswe-best-benchmark-for-evaluating-ai-coding-agents.html); [Harbor hub deep-swe-1-1](https://hub.harborframework.com/datasets/datacurve/deep-swe-1-1/latest)
- A GIGAZINE headline reports OpenAI saying "approximately 30% of SWE-Bench Pro is broken"; a blog claims OpenAI retracted its Pro recommendation in July 2026 — not confirmed in any primary source. — [GIGAZINE](https://gigazine.net/gsc_news/en/20260429-swe-bench-verified); [search summary of buildmvpfast/codex blog](https://codex.danielvaughan.com/2026/07/31/swe-bench-reliability-crisis-paichecker-change2task-codex-cli-evaluation-benchmark-vacuum/)
- SWE-bench-Live Lite: 2025 entries scored 7–36%, while 2026 entries score 60–71% on the same 300-task Lite set (e.g. SWE-agent + Claude 4.5 Sonnet 36.0% on 2025-12-01 vs TianxiCode + DeepSeek-v4.1-Flash 71.0% on 2026-10-08). — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)
- Model vendors are moving off Verified: DeepSeek's V4.1-Flash cards (Sept/Oct 2026) report DeepSWE v1.1 and Terminal-Bench, not SWE-bench Verified/Pro. — [V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)

### Inferences
- For the INIT Agent use case (compare a small agent's per-task behaviour vs published harnesses), Verified's flaws matter less than for frontier ranking — per-instance comparisons on the same instances and grader remain valid relative measures — but absolute scores for 2026 models (DeepSeek-V4/V4.1 flash) likely benefit from contamination; prefer mid-difficulty, previously-solvable instances and cross-check a few tasks on SWE-bench-Live Lite or Pro V2.
- The Live Lite jump from ≤36% to 60–71% within a year suggests either strong harness/model gains or that a fixed "live" subset ages into training data; the reason was not established.

### Gaps
- Could not read OpenAI's primary post directly (403); exact wording and figures should be re-checked.
- No primary confirmation of the "30% of Pro broken"/retraction claims.

---

## Q7. What mini-SWE-agent's bash-only results show about the same model across harnesses

### Takeaway
On SWE-bench Verified, the same model in mini-SWE-agent (bash only, 250 steps) usually scores below the best specialised scaffold — by ~2–5 pp for 2025-H2/2026 frontier models and by 10–20+ pp for older/weaker models. On DeepSWE v1.1, DeepSeek reports the reverse for its own model: mini-SWE scored highest (74.2) vs Claude Code 69.8, Codex 65.6, OpenCode 65.5.

### Cited Findings
- Same model, different harness, SWE-bench Verified (from `leaderboards.json`):
  - Claude 4.5 Opus: mini 74.4 (medium, v1.16.0) / 76.8 (high, v2.0.0) vs live-SWE-agent 79.2, Sonar Foundation Agent 79.2.
  - Gemini 3 Pro Preview: mini 74.2 vs live-SWE-agent 77.4.
  - Claude 4.5 Sonnet: mini 70.6 / 71.4 (high) vs Sonar 74.8.
  - Claude 4 Sonnet: mini 64.93 vs SWE-agent 66.6, OpenHands 70.4, Moatless 70.8, Bloop 71.2, Tools 72.4, Lingxi 74.6, EPAM 76.8.
  - Claude 4 Opus: mini 67.6 vs Tools 73.2. Claude 3.7 Sonnet: mini 52.8 vs SWE-agent (review heavy) 62.4, Tools 63.2, Aime-coder 66.4.
  - GPT-5: mini 65.0 (medium) vs OpenHands 71.8, Prometheus-v1.2.1 74.4.
  - Kimi K2 Instruct: mini 43.8 vs CodeSweep SWE-agent 53.4; Kimi K2: OpenHands 65.4, Lingxi 71.2.
  - o4-mini: mini 45.0 vs PatchPilot-v1.1 64.6. Qwen3-Coder-30B-A3B: OpenHands 51.6 vs EntroPO+R2E 52.2 / 60.4 (TTS).
  — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Data anomaly: the mini + Gemini 2.0 Flash row (13.52%) has instance_cost 0.0 and calls 0.0, so its large gap to Google Jules (52.2%) is not a clean harness comparison. — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Same mini harness, version/effort change: DeepSeek V3.2 60.0% (v1.17.1, "reasoner") → 70.0% (v2.0.0, high); Claude 4.5 Opus 74.4 → 76.8; GPT-5 mini 59.8 (v1.7.0) → 56.2 (v2.0.0). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- DeepSeek-V4.1-Flash, max effort, N=8 samples/task, max_steps=500, 1M ctx: DeepSWE v1.1 — Claude Code 69.8, Codex 65.6, OpenCode 65.5, Pi 66.2, mini-SWE 74.2, DSH Minimal 72.6, DSH Standard 70.5, DSH PTC 67.6; Terminal-Bench 2.1 — 88.0 / 84.1 / 85.0 / 86.1 / 90.3 / 90.6 / 85.8 / 85.8. — [V4.1-Flash card](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)
- SWE-bench-Live (DeepSeek-V4-Pro, June 2026): OpenHands, Claude Code and SWE-agent land within a few tasks of each other per language (e.g. Java 40/39/40 of 102), except TS/JS where Claude Code is 12 tasks lower (63 vs 75 of 204). — [reports-0605.jsonl](https://github.com/SWE-bench-Live/swe-bench-live.github.io/blob/main/reports-0605.jsonl)
- Scale's Pro leaderboard's newest entries (Muse Spark 1.1 61.5, gpt-5.4 xHigh 59.1, claude-opus-4-6 51.9, gemini-3.1-pro 46.1) are all mini-swe-agent runs, i.e. Scale standardises on bash-only mini. — [Scale SWE-Bench Pro (Public)](https://labs.scale.com/leaderboard/swe_bench_pro_public)

### Inferences
- Harness effects are model-dependent: for strong 2026 models a minimal bash loop is near the frontier scaffolds (and for DeepSeek-V4.1-Flash it was the best of 8 on DeepSWE), so a small new agent driving deepseek-flash should be compared first against mini-SWE-agent + DeepSeek runs (same model family, published per-instance + cost), then against CLI agents on Live.

### Gaps
- No public SWE-bench Verified run of DeepSeek-V4/V4.1 Flash in mini or any other harness was found (only DeepSeek's self-reports).

---

## Q8. Typical cost per task for cheap models

### Takeaway
With mini-SWE-agent on Verified, cheap models cost about $0.03–0.15 per task (DeepSeek V3.2 reasoner $0.028 mean, $0.024 median, $0.050 p90; GPT-5 mini $0.035–0.047; MiniMax M2.5 $0.073; GLM 4.6 $0.097; Kimi K2.5 $0.147); higher-effort runs cost more ($0.45 for DeepSeek V3.2 high). A 10–20-task DeepSeek-flash pilot should cost cents to a few dollars in tokens; grading via sb-cli is free.

### Cited Findings
- Verified, mini-SWE-agent, $/instance (avg API calls): DeepSeek V3.2 Reasoner $0.0281 (46.4); GPT-5 mini $0.0355 (14.5, v1.7.0) / $0.0472 (20.3, v2.0.0); GPT-5 nano $0.0381 (39.8); gpt-oss-120b $0.0571 (27.6); MiniMax M2.5 high $0.0733 (60.5) at 75.8%; GLM 4.6 $0.0966 (49.3); Kimi K2.5 high $0.147 (51.2); Gemini 3 Flash high $0.356 (56.1) at 75.8%; DeepSeek V3.2 high $0.448 (88.5); Claude 4.6 Opus $0.552 (28.9); Claude 4.5 Opus high $0.754 (32.9). — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Per-instance distribution, DeepSeek V3.2 Reasoner: mean $0.0281, median $0.0241, p90 $0.050, max $0.150; API calls median 41, p90 71, max 250 (step limit hit). DeepSeek V3.2 high: mean $0.448, median $0.365, max $2.52; calls median 83, max 250. — computed from [per_instance_details.json](https://github.com/SWE-bench/experiments/blob/main/evaluation/verified/20251201_mini-v1.17.1_deepseek-v3.2-reasoner/per_instance_details.json)
- Multilingual, mini v2.0.0a0: DeepSeek V3.2 $0.384/inst; MiniMax M2.5 $0.101; GPT-5 mini $0.052. — [leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- SWE-rebench: DeepSeek-V4 Pro [high] $0.15/problem; top entries $0.85–$4.40/problem. — [swe-rebench.com](https://swe-rebench.com/)
- DeepSWE (secondary): gpt-5.6-sol [max] 73% at $8.39/task average. — [search summary citing i-programmer/Datacurve](https://www.i-programmer.info/news/105-artificial-intelligence/19016-deepswe-best-benchmark-for-evaluating-ai-coding-agents.html)
- mini-swe-agent supports global caps via `MSWEA_GLOBAL_CALL_LIMIT` and `MSWEA_GLOBAL_COST_LIMIT`; default per-instance `cost_limit: 3.`. — [mini-swe-agent SWE-bench usage](https://github.com/SWE-agent/mini-swe-agent/blob/main/docs/usage/swebench.md); [swebench.yaml](https://github.com/SWE-agent/mini-swe-agent/blob/main/src/minisweagent/config/benchmarks/swebench.yaml)

### Inferences
- Budget estimate for the pilot: 10–20 Verified tasks × $0.03–0.45 ≈ $0.30–$9 in DeepSeek tokens (low end for non-max effort with cache hits), plus $0 grading on sb-cli (quota permitting) or a small Modal bill. Quality guard: keep `step_limit` 250 and the official images so step/cost numbers stay comparable to the mini entries.
- The 16× cost gap between the two DeepSeek V3.2 mini runs (same model id family) shows cost depends heavily on reasoning effort and possibly on pricing/caching accounting; record tokens, not only dollars.

### Gaps
- Why the DeepSeek V3.2 high run cost 16× the reasoner run (pricing source, cache accounting, effort) is not documented in the entry.
- No published per-task cost for DeepSeek-V4/V4.1 Flash on any SWE-bench variant.
