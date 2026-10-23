# S1-CORE: sealed pack s1 format, shared check runner, validator, lint, canaries

Lane S1-CORE (docs/m7-design.md §3.1, §3.8; scope build/specs/PROOF-LEAN.md: family B dropped).
The format contract for the template lanes is `root/sealed/packs/s1/FORMAT.md`.

## Commits
- `e81e4d5` format, runner, lib, pack.json, licences, FORMAT.md, validator, E examples. Contains both
  cargo_test fixes below (no --quiet + --format=pretty; fresh target dir + fresh mtimes).
- `e67b8e5` the 4 canaries, tools/s1lint.py, the T example, validator --tracked/--examples.
- `f5da529` tests M7-P01..P09 (tools/tests/test_s1.py), ci/local.sh wiring, lint hygiene --tracked.

## For the template lanes (S1-E-PY, S1-E-RS, S1-C, S1-T, POOL-LITE)

Validator (exact commands, from the repo root):
```
python3 root/sealed/tools/s1validate.py <template-id> [<template-id> ...]          # full: M7-P01..P05, P09, knobs
python3 root/sealed/tools/s1validate.py <template-id> --quick                      # 1 seed, 1 repeat, no knob sweep
INIT_SLOT_POOL=light bash ci/slot.sh python3 root/sealed/tools/s1validate.py <ids>   # validator runs: light slot pool
python3 root/sealed/tools/s1validate.py --json /path/report.json <ids>               # machine-readable report
```
Exit 0 = every named template passed. Scratch lives under `target/test-homes/s1-core/run-*` and is deleted
on exit; `--keep` keeps the generated instances for inspection.

template.json fields (exactly these keys): `v` (1), `id` (= dir name, `^[a-z0-9][a-z0-9._-]{0,63}$`),
`family` (E|C|T|canary = parent dir), `lang` (py|rust|sh|any), `title` (<= 120), `difficulty`
{default,min,max} in 1..5, `toolchains` (must include python3; subset of python3 cargo sh awk sed sort git
make tar), `licence` (Exercism-MIT|CRUXEval-MIT|INIT), `source` (E: {repo,path,exercise,commit};
C: {repo,path,sample,commit}; T/canary: null), `canary` (canary family only), `notes` (optional).

Answers to lane questions:
- `_`-prefixed directories (C/_lib, E/_shared_rs, T/_shared) are helpers: the validator and the lint skip
  them. The Root installs the pack whole (read-only) and runs gen/run inside it, so relative imports of
  `../../lib` and `../_lib` work. Set `sys.dont_write_bytecode = True` first (read-only install; the lint
  rejects `__pycache__`).
- Licences: `LICENSES/Exercism-MIT.txt` is now the verbatim upstream file (exercism/python 6105f6d and
  exercism/rust cc099ae are byte-identical); `LICENSES/CRUXEval-MIT.txt` is S1-C's verbatim copy from
  cruxeval 190faf1. pack.json records each sha256 and pinned upstream commit.
- The format examples moved OUT of the pack to `root/sealed/tools/examples/` (never drawn, outside the
  composition and the disjointness sets): E/e-py-acronym, E/e-rs-rle (and a T example). There is no C example
  (S1-C's 14 templates cover py_cases; e-py-acronym shows the same pattern).

## Runner fixes (integrity)
1. `cargo test --quiet` made libtest print the terse format, so no `test x ... ok` line matched and every
   Rust check scored 0 (found by S1-E-RS). Fixed: no `--quiet`, and `--format=pretty` after `--`.
2. False passes from a reused target dir (found by S1-E-RS): the runner honoured an inherited
   CARGO_TARGET_DIR and copied sources with their fixed tar mtimes, so cargo reused another workspace's lib
   (a todo!() stub scored 1000 after the reference had built). Fixed: the runner never inherits
   CARGO_TARGET_DIR; each check builds in a fresh target dir inside its own scratch dir (deleted after), and
   the crate is copied with fresh mtimes (`copy_function=shutil.copy`). Regression test:
   CargoTest.test_reference_then_stub_with_shared_target (reference, stub, reference, stub with one inherited
   shared CARGO_TARGET_DIR -> 1000, 0, 1000, 0, and nothing written into the shared dir). Root-level
   `build.rs`/`rust-toolchain*` are dropped from the copy (a `src/build.rs` module is kept).
3. Speed: py_cases now runs all cases of a step in one child (per-case alarm; a hung or crashed case fails and
   a fresh child resumes after it). e-py-acronym full validation: 181 s -> 24 s.

## Done
- canary/{health,impossible,tamper,leak} (m7 §3.1.4; validated: P02/P06 tables in FORMAT.md). The leak canary
  also trips on workspace code that reads `../check/secret` when the check runs it (check-time reads).
- tools/s1lint.py: P07 (permissive SPDX, LICENSES sha256 = pack.json, `--upstream REPO=DIR` byte-for-byte +
  git HEAD = pin, template LICENSE byte-identical, NOTICE names the repo, source.commit = the licence's pin),
  P08 (E slugs unique and a-m, disjoint from the pool's slugs and the bench list; C samples unique; no pack
  file >= 64 B under seed/ or fixtures/ except runner/ and LICENSES; no template dir there), hygiene, index.
  Composition counts, unpinned commits and the index are `pending` in dev, failures with `--release`.
- tools/tests/test_s1.py: 52+ tests (below).
- ci/local.sh: steps "sealed pack s1 lint" (--tracked), "sealed pack s1 tests", "sealed pack s1 templates"
  (validator --tracked --quick; full with INIT_CI_TIER=full).
- runner/check + runner/s1check.py (steps files, run, py_cases, py_unittest, cargo_test, sh; protected.json;
  tamper exit 3; links into the check dir; budget; no writes into the check dir).
- lib/s1lib.py (Rng, Instance, deterministic tars, task.json v3, tokens + GUID markers, py_eval, helpers).
- pack.json (lean composition E 24 / C 14 / T 14 + 4 canaries; G4 32 = E 14, C 9, T 9; reserve 20 = E 10,
  C 5, T 5), LICENSES, FORMAT.md.
- tools/s1validate.py; examples e-py-acronym and e-rs-rle validate (19/19 checks each).

## Tests
- `python3 root/sealed/tools/s1validate.py e-py-acronym` -> ok (24 s)
- `bash ci/slot.sh python3 root/sealed/tools/s1validate.py e-rs-rle` -> ok (25 s)
- `python3 root/sealed/tools/s1validate.py health impossible tamper leak` -> ok (4/4)
- `python3 root/sealed/tools/s1validate.py t-log-report` -> ok (all difficulties: awk reference = Python model)
- `python3 root/sealed/tools/s1lint.py --tracked` -> 0 failed (pending: composition counts, index)
- CI dry run `python3 root/sealed/tools/s1validate.py --tracked --quick` -> 31 committed templates ok (8 E-py,
  14 C, 5 T, 4 canaries), 0 failed, 5 min
- `python3 root/sealed/tools/s1lint.py --upstream exercism/rust=kernel/target/tmp/exercism-rust --upstream
  exercism/python=kernel/target/tmp/exercism-python --upstream facebookresearch/cruxeval=kernel/target/tmp/cruxeval`
  -> P07 ok (LICENSES byte-identical to the pinned upstream files)
- `INIT_SLOT_POOL=light bash ci/slot.sh /usr/bin/python3 -m unittest discover -s root/sealed/tools/tests`
  (Python 3.9.6, the macOS system python the Root's PATH finds): Ran 52 tests in 233 s, OK (cargo and Seatbelt cases included)
  Test map: M7-P01/P02/P04/P05/P09 TemplatesM7P01P06 (canaries + examples, full validator); M7-P03
  NetworkM7P03 (+ the validator runs every gen/run and solution check under a Seatbelt profile that SIGKILLs
  network use); M7-P05 TamperM7P05 (edit, delete, dir delete, symlink, parent-dir symlink, link into the check
  dir; legacy parsers read 0); M7-P06 validator canary table + TemplatesM7P01P06.test_tamper_canary_delete_and_leak_paths;
  M7-P07/P08 LintM7P07P08 (real pack + 13 broken-pack cases); M7-P09 TokensM7P09 (1,000 seeds x every
  template); runner: RunnerProtocol, PyCasesAntiGaming (types, forged score lines and results, env, hang,
  crash, alarm dodge, stdlib shadowing), StepKinds, CargoTest (reference > 0; reference then stub with a
  shared inherited target dir -> 1000, 0, 1000, 0; .cargo/config runner + build.rs inert).

## Deviations
- D1 Tamper line comes with exit 3, not 0: the current kernel (g4.rs `score`) and Root (sealed/run.rs `check`)
  parsers read "exit 0 + an unrecognised last line" as 1000, so an exit-0 tamper line would score 1000 there.
- D2 Tamper also fires on a workspace symlink resolving into the check dir (only a check-time reader plants
  one); symlinks elsewhere (e.g. a venv) are allowed.
- D3 E and C instances carry `NOTICE` (attribution + MIT text) in the workspace: the workspace is sent to the
  model provider, so the MIT notice travels with the copy.
- D4 pack.json `templates[]` stays empty in the repo while four lanes add templates concurrently; templates
  are discovered from `<FAMILY>/<id>/template.json`. `s1lint.py --write-index` fills it at release.
- D5 Generator inputs beyond `<seed> <outdir>` come from the environment (INIT_S1_GUID, INIT_S1_DIFFICULTY,
  INIT_S1_DEV), keeping sealed-eval §3.6's two-argument contract.
- D6 Format examples live outside the pack (lane request): they are never drawn. No C example (S1-C has 14).
- D7 M7-P06 is tested at the check level: the canaries' scripted end states (honest, special-casing, test
  edit, token written, check-time read) are generated as dev fixtures or applied as workspace mutations; the
  mock-model end-to-end run belongs to ROOT-S1a/GATE-V2.
- D8 `--tracked` (lint, validator) for CI in the shared checkout: lanes' uncommitted work cannot break CI.
- D9 PROOF-LEAN asked Rust validators to share CARGO_TARGET_DIR=kernel/target/tmp/templates; after the
  stale-build false pass the runner never inherits a target dir (fresh per check, deleted with the check's
  scratch). Disk cost is transient (a few MB per check).

## Needs from others
- ROOT-S1a / GATE-V2a: v3 score parsing must read `{"score_permille":0,"tamper":true}` from the last stdout
  line whatever the exit code (the runner exits 3). The Root's sealed check env has PATH=/usr/bin:/bin:...:
  Rust templates need the toolchain bin on PATH there, as the kernel's check_env already does.
- ROOT-S1a: task.json `id` from gen/run is a placeholder; assign the random 12-hex id at draw.
  `tokens.json` lists the instance tokens for the leak scan; `canary.json` marks canaries. Pass the install
  GUID as `INIT_S1_GUID` to gen/run. The check needs `TMPDIR` to exist and be writable (the runner's scratch,
  incl. the per-check cargo target dir) and python3 >= 3.9 on PATH (/usr/bin/python3 on macOS is 3.9.6).

## Notes for lanes
- T/t-merge-rules/gen/__pycache__ exists in the working tree (something imported gen/run without
  `sys.dont_write_bytecode = True`); `s1lint.py` (no --tracked) flags it. Delete it before committing.

## Left
- At release: `s1lint.py --write-index` then `--release` (needs all 52 templates, pins, verbatim licences).
