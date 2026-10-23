# S1-E-PY: 12 Python family-E templates (Exercism, seeded semantic change)

Lane S1-E-PY (docs/m7-design.md §3.1.1-§3.1.3; scope build/specs/PROOF-LEAN.md D-L1: E = Py 12 + Rust 12).
Templates live in `root/sealed/packs/s1/E/e-py-*/` (template.json, gen/run, ref/, NOTICE, LICENSE) and follow
S1-CORE's `root/sealed/packs/s1/FORMAT.md`. The shared generator helpers are in `E/_shared_py/epy.py`.

Source: exercism/python at commit `6105f6d9d3ef66ce3d5c8f529d2882af3d76b706` (2026-09-30), MIT,
"Copyright (c) 2021 Exercism". Each template's LICENSE is a byte copy of the upstream LICENSE, which is
byte-identical to `LICENSES/Exercism-MIT.txt`, and its NOTICE names the exercise path and the commit. Only the
exercise idea is used: the prompts, tests, reference solutions and dev fixtures were written fresh.

## Templates

Difficulty is the m7 1-5 knob (`difficulty.default`, range in brackets). The "guess" is the expected
deepseek-flash outcome at the default (target 30-70% solved within 8 turns): easy = upper end, hard = lower end.

| id | exercise | mutation (deltas added per difficulty step) | default [range] | guess |
|---|---|---|---|---|
| `e-py-anagram` | anagram | d1 seeded ignored characters (space, hyphen, apostrophe, dot) and output order (input, code-point sort, length); d2 + seeded case rule and de-duplication; d3 + near anagrams (exactly one extra character) | 2 [1-3] | easy |
| `e-py-atbash` | atbash-cipher | d1 seeded group size and separator; d2 + shifted (still self-inverse) table and digit rule (keep, complement, drop); d3 + alternating group sizes; d4 + strict decode (`ValueError`) | 2 [1-4] | easy |
| `e-py-etl` | etl | d1 seeded output case, repeated-letter rule (sum, max, min), multi-letter entries with ignored characters; d2 + digit-string scores and bonus letters times m; d3 + sorted (letter, score) tuples instead of a dict | 2 [1-3] | easy |
| `e-py-bob` | bob | d2 seeded replies, yelling = at least k capitals and no lower case, a trailing-off rule, a seeded rule order; d3 + numbers-only rule; d4 + whisper rule (collides with trailing-off) | 3 [2-4] | medium |
| `e-py-bottles` | bottle-song | d2 seeded object, colour, place, verb, number words up to a seeded maximum 12-20, exact line templates, errors; d3 + 2 or 3 items fall per verse (fewer at the end); d4 + seeded refrain on even counts | 3 [2-4] | medium |
| `e-py-bst` | binary-search-tree | d2 seeded ordering key (numeric, mirrored numeric, reverse text, digit sum) and duplicate rule (left, right, ignore), `sorted_data`, `height`; d3 + `levels()`; d4 + `path()` with seeded step letters | 3 [2-4] | medium |
| `e-py-killer` | killer-sudoku-helper | d2 seeded digit range (never 1-9), inner and outer order; d3 + `include`; d4 + a digit may appear twice | 3 [2-4] | medium |
| `e-py-knapsack` | knapsack | d2 return the chosen indices with a seeded tie-break (fewest items, lowest weight, most items; then lexicographic); d3 + `max_items`; d4 + item groups | 3 [2-4] | medium |
| `e-py-life` | game-of-life | d2 seeded live/dead characters and Birth/Survival rule (never B3/S23), bounded board, input validation; d3 + torus; d4 + von Neumann neighbourhood | 3 [2-4] | medium |
| `e-py-change` | change | d3 limited supply (dict of counts), seeded tie-break (prefer larger or smaller coins), seeded output order; d4 + fewest different values first; d5 + a seeded forbidden pair | 4 [3-5] | hard |
| `e-py-grep` | grep | d3 files as a dict, list output, seeded field separator, new `-c` with stated precedence; d4 + `-w` and zero-padded line numbers; d5 + `-mN` | 4 [3-5] | hard |
| `e-py-meetup` | meetup | d3 seeded string date format and a seeded 7-day window descriptor replacing `teenth` (which now raises); d4 + seeded name for second-to-last; d5 + integer weeks and a seeded skip day | 4 [3-5] | hard |

Every template: 20-40 hidden `py_cases` (the visible examples included), 3-5 visible examples in
`tests/test_<module>.py` (protected), a stub that raises `NotImplementedError` (no-op scores 0), and the
dev fixture `dev/original.tar`, the unmutated exercise's behaviour adapted to the new interface. The lane
validator requires that fixture to score < 1000 on every dev seed and difficulty, so a solution memorised from
the public exercise fails the hidden tests.

## Validation

S1-CORE's validator, full mode, run once per template, one at a time, through the light slot. All 12 are ok,
0 failed, 0 warnings (atbash 18 checks passed, every other template 17):
```
INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 python3 root/sealed/tools/s1validate.py <id>
e-py-anagram 27.9s ok · e-py-atbash 30.9s ok · e-py-bob 21.6s ok · e-py-bottles 15.0s ok · e-py-bst 18.5s ok
e-py-change 64.5s ok · e-py-etl 26.3s ok · e-py-grep 22.6s ok · e-py-killer 18.0s ok · e-py-knapsack 22.2s ok
e-py-life 14.6s ok · e-py-meetup 33.0s ok
```
s1validate covers P01 (determinism, seeds differ), P02 (solution x3 = 1000, no-op <= 500 and equal, empty
< 1000), P03 (no network), P04 (<= 60 s), P05 (tamper probes), P09 (tokens, GUID), the difficulty sweep
and checker integrity.

The lane checker is a scratch script, not committed. It ran with 2 dev seeds at every difficulty in each range
and also checks that gen is deterministic, that there are 20-40 hidden cases, and that each check takes
<= 10 s. Its key result is that every dev fixture scored < 1000 on every seed and difficulty; the highest
scores seen were:

| template | original | naive |
|---|---|---|
| anagram | 428 | - |
| atbash | 516 | - |
| bob | 0 | 785 |
| bottles | 321 | 821 |
| bst | 441 | 823 |
| change | 269 | - |
| etl | 148 | - |
| grep | 400 | - |
| killer | 290 | 870 |
| knapsack | 863 | - |
| life | 321 | 357 |
| meetup | 666 | - |

Generation was also run under /usr/bin/python3 3.9.6, where the check runs, and is byte-identical to 3.14.

Blind solvability: a fresh Sonnet agent saw only the prompt and the workspace of one default-difficulty
instance per template (dev seed `s1-e-py/blind/<id>`). It scored 1000 on all 12 hidden checks, so no spec
is ambiguous enough to make a template unsolvable. The one wording it flagged as loose (bob: "no letters")
now says `A`-`Z`, `a`-`z`.

The template ids above are all in pack lint's P08 a-m set. `s1lint.py` reports P07 and P08 ok; its two
remaining FAILs are another lane's `__pycache__` under T/.

## Disjointness
E-Python slugs (all a-m, M7-P08): anagram, atbash-cipher, binary-search-tree, bob, bottle-song, change, etl,
game-of-life, grep, killer-sudoku-helper, knapsack, meetup. Agreed with S1-E-RS so that no slug is used by both
tracks (Rust: allergies, armstrong-numbers, binary-search, book-store, bowling, circular-buffer, clock,
crypto-square, flower-field, isbn-verifier, luhn, matching-brackets). acronym is S1-CORE's format example.

## Deviations
- D1 (helper lane, recorded): `e-py-bst` does not use a `length_then_text` key, which orders digit strings
  without leading zeros exactly like `numeric`; its keys are numeric, mirrored numeric, reverse text and digit
  sum. `e-py-life`'s random Birth/Survival rules are limited to birth counts in 2..4 and survival counts in 1..4,
  and a rule that leaves fewer than 60% of evolving cases changed and non-empty is redrawn. Its `ref/original.py`
  carries the P line, so it uses the instance's cell characters with classic B3/S23 and fails on the rule alone.
- D2: the shared generator code is `E/_shared_py/epy.py`: `ref_with_params`, `load_ref` and `assemble`.
  Helper dirs prefixed with `_` are allowed by FORMAT.md, and the pack installs whole. Each `gen/run` imports it
  after `s1lib`, with `sys.dont_write_bytecode` set first.
- D3: every template writes extra dev fixtures (`dev/original.tar`, and `dev/naive.tar` for bob, bottles, bst,
  killer and life). s1validate ignores them. The lane's own checker (below) requires each to score < 1000.
  bottles, bst, killer and life also run a `cover()` guard at generation time. It requires the original fixture,
  and the reference with each single delta removed, to differ from the reference on at least 3 hidden cases;
  otherwise generation fails. The other eight build their discriminating cases explicitly, for example ties,
  binding supplies, `teenth` and threshold-length capitals.
- D4: I did not write `LICENSES/Exercism-MIT.txt`, because S1-CORE owns it. S1-CORE has made it the verbatim
  upstream file (python 6105f6d and rust cc099ae are byte-identical), and every `e-py-*/LICENSE` is a byte copy
  of it.

## Needs from others
- S1-CORE (suggestion): add an s1validate check that every `dev/*.tar` of an E or C template scores < 1000.
  Today only the lane checker enforces that a solution memorised from the public exercise fails.
- ROOT-S1a / REAL-M7 calibration: a blind Sonnet solver, given only the prompt and workspace, scored 1000 on
  all 12 default-difficulty instances. So the specs are complete and solvable, and the defaults may sit on the
  easy side for a strong model. Calibration should push templates above 80% one knob step harder (m7 §3.1.6).
  Every template has at least one harder level.

## Left
- Nothing for this lane's scope. The real-model pass rates (deepseek-flash) come from the Root's calibration
  (REAL-M7). Templates outside 20-80% move one knob step (m7 §3.1.6).
- Scratch cleaned: the exercism/python clone at kernel/target/tmp/exercism-python was deleted by exact path.
  The validators' scratch under target/test-homes/s1-core is removed by s1validate on exit.
