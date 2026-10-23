# S1-T: 14 family-T templates (macOS terminal mini-tasks, written fresh, INIT licence)

Lane S1-T (docs/m7-design.md §3.1.1 family T, §3.1.2-§3.1.4; scope build/specs/PROOF-LEAN.md: T = 14).
Templates live in `root/sealed/packs/s1/T/t-*/` (template.json v1 per FORMAT.md, NOTICE, gen/run, check.py).
Shared helpers are in `T/_shared/` (skipped by the lint and validator):
- `tgen.py`: wires the hidden checker into each instance as one `sh` step of the shared runner. It adds
  hidden/check.sh, which finds python3 >= 3.9; hidden/tcheck.py, which is the prelude plus the template's
  check.py; and hidden/spec.json. It also holds `Git`, a deterministic git writer (loose objects, refs,
  index v2; checked with `git fsck --strict` and a clean `git status`).
- `tcheck_common.py`: the checker prelude. Workspace code runs only in private copies under `$S1_SCRATCH`,
  in its own process group with timeouts. It also provides the `git()` wrapper (no global config,
  safe.directory=*) and prints `PASS`/`FAIL` per name.
- `dev_validate.py`: the lane's own dev-seed validator. It was used until S1-CORE's s1validate landed.

All data is seeded from `ctx.rng()` only. Checks are deterministic and use POSIX sh plus python3 >= 3.9
(stdlib). Seven templates also use git, make or BSD tar from the Command Line Tools. Everything runs
offline and natively, with no Docker. Each template has the knob 1-5 (default 3), which adds or removes
traps. Scoring is `fraction` over the check names, and a pass is 1000.

## Templates

"Guess" is the expected deepseek-flash full-pass rate (score 1000 within 10 turns) at the default difficulty.
The target is 30-70%.

| id | skill tested | main traps at default | guess |
|---|---|---|---|
| t-merge-rules | git merge of a feature branch, conflicts resolved by stated per-file rules, a two-parent merge commit | git silently merges a downgrade the rules reject; 1.10 vs 1.9; a key deleted on one side and changed on the other | 38% |
| t-make-repair | GNU make repair: tabs, `$<`/`$^`, `$$`, order-only dir, prerequisites, .PHONY, clean | latent bugs that only show in incremental builds (edit, new page, tool change), up-to-date runs, `make test` with a test/ dir, `make clean` | 40% |
| t-log-csv | `./log2csv`: mixed JSON, key=value and bracketed log lines to RFC 4180 CSV | UTC normalisation from mixed offsets, round-half-up ms, health-check filter by exact path, quoting, stable ties | 40% |
| t-shell-quoting | make a seeded /bin/sh file script safe for any file name, behaviour unchanged | `$(ls)`, unquoted expansions, leading `-`, glob characters, `echo` backslashes, `read` without IFS=/-r, no final newline | 45% |
| t-rename-pattern | file media by date patterns into library/YYYY/MM with collision suffixes and a rename log | spaces, day-first scan dates, ms names colliding, `.jpeg` sorting before `.jpg`, near-miss decoys | 45% |
| t-find-commit | history search: the commit that first introduced S, the commit that removed R, the root author | S in an earlier message, introduced on a merged side branch, file renamed, removed and re-added, a near-duplicate decoy | 48% |
| t-config-migrate | `./migrate`: INI v1 to JSON v2 by a seeded mapping (renames, durations, bools, lists, host:port, defaults, env sections) | `%` breaks configparser interpolation, bare-number durations, a missing section, type-strict JSON compare | 40% |
| t-cron-next | `./cronnext`: next run per crontab line | DOM/DOW OR rule, steps from range start, due exactly at --now, 7 = Sunday, names, spacing kept | 45% |
| t-perms-tar | fix package.sh: tar.gz with one top dir, required modes, excludes, sorted entries | modes copied from disk, bsdtar walk order, empty dirs, `._*` AppleDouble entries (COPYFILE_DISABLE), deleting junk instead of excluding it | 40% |
| t-env-expand | fix a layered env-file launcher (`${VAR:-def}`, `$VAR`, layers, overrides) | 3-8 seeded bugs: subshell `while read`, single quotes, `cut` at `=`, `${X-def}` vs `${X:-def}`, comments, last line, layer order | 35% |
| t-table-align | `./align`: TSV to aligned table | code-point widths with non-ASCII (byte-based awk misaligns), decimal padding, a numeric-looking column with one text cell | 45% |
| t-checksum-manifest | sha256 manifest in shasum format, plus a verify report of a stale SHA256SUMS | paths relative to release/, byte order vs locale order, spaces, exclusions, `*` binary marks, comments, MISSING | 45% |
| t-bisect-skip (own design) | first bad commit with a 0/1/125 probe, then restore the repo | untestable commits near the boundary, a transient failure mid-history (plain bisect lands wrong at knob >= 3), bisect state left behind | 40% |
| t-mirror-sync (own design) | one-way sync of dst/ to src/ with exclude/protect globs, deletions, pruning, action log | same-size content changes (rsync quick-check misses them), dir patterns, protected files, anchored vs basename patterns | 40% |

Mean guess about 42%. The knob moves a template about 15-25 points per step. The calibration in m7
§3.1.6 then sets the real per-template knob from Root aggregates.

m7's list has 12 categories; the two own-design extras are t-bisect-skip and t-mirror-sync. I first planned
sqlite-report and patch-series, but POOL-LITE's visible pool has both, so I swapped them to keep sealed T
disjoint from the pool. POOL-LITE has S1-T's list.

## Tests

- `python3 root/sealed/tools/s1validate.py <id>`, the full run (P01-P05, P09, knobs 1-5, checker digest):
  all 14 ok, 19 passed each. Every template had one full run after its last change.
- Final regression, run sequentially through the light pool:
  `INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 python3 root/sealed/tools/s1validate.py --quick <14 ids>`
  gave `14 template(s), 0 failed, 0 skipped`. Checks took 4-10 s per template under load.
- Python 3.9: `PATH=/usr/bin:/bin /usr/bin/python3 T/_shared/dev_validate.py --seeds 1 <id>` is OK for
  all 14. No-op scores are 0 everywhere except t-shell-quoting (333); empty-workspace scores are 0.
- `python3 -B root/sealed/tools/s1lint.py` reports 0 failed. One item is pending, the index, which
  S1-CORE writes at release.
- Spot checks of realistic wrong solutions all score partial and less than 1000. Examples: byte-width align
  200; make with only the hard bugs fixed 714; rsync -a without checksums and without a log 600; bisect
  wrong but restored 750; merge taking main's pins 833; shell `echo` instead of `printf` 833.

## Deviations
- D1: check.json uses a single `sh` step per template whose script runs hidden/tcheck.py. FORMAT.md allows
  this, and it keeps T checks to one code path per template.
- D2: none of m7's 12 named T categories is dropped; the 2 extras are own design (see above).
- D3: the check names share one weight, so partial credit is uniform across names. A pass still requires
  every name. In t-bisect-skip a wrong answer with the repo restored scores 750.
- D4: t-perms-tar depends on `COPYFILE_DISABLE=1`. On machines where files carry com.apple.* xattrs, bsdtar
  adds `._*` members unless that variable is set. The prompt states the rule, so the task is fair but
  machine-sensitive.

## Needs from others
- ROOT-S1a: the draw needs the `toolchains` probes for git, make and tar (template.json). The check env
  needs `/usr/bin` on PATH; the CLT shims for git and make need xcode-select set up for the check user.

## Left
- Nothing for the 14 templates. Real-model per-template pass rates come later from the Root's
  `sealed_calibrate` aggregates; the knob defaults then move per m7 §3.1.6.
