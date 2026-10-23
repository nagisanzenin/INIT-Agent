# S1-C: family C, CRUXEval-seeded change templates (14)

Lane S1-C of the lean proof sprint (build/specs/PROOF-LEAN.md; docs/m7-design.md §3.1.1 C, §3.1.2–3.1.4;
docs/root/sealed-eval.md §3.6). Format: root/sealed/packs/s1/FORMAT.md (S1-CORE e81e4d5).

## Done
14 templates under `root/sealed/packs/s1/C/c-*/` (template.json, NOTICE, LICENSE, gen/run) plus the family
helper `C/_lib/s1c.py`. Source: facebookresearch/cruxeval at commit `190faf16d175b5847b0af05d937872b1fb395942`
(MIT, `data/cruxeval.jsonl`). `LICENSES/CRUXEval-MIT.txt` and every template's `LICENSE` are byte copies of that
commit's LICENSE. Each NOTICE records the commit and the sample id. Every gen/run keeps the verbatim CRUXEval
function as `ORIGINAL`; all 14 were checked against the jsonl at that commit.

| id | CRUXEval | operator | seeded change (one draw per seed) | default d | pass guess at default d |
|---|---|---|---|---|---|
| c-word-stride | sample_183 | out_map | result list: reverse each string's words / prefix the word count / join into one string with a seeded separator | 4 | 60% (75% at d3) |
| c-triple-counts | sample_780 | in_map | values mapped before counting: `v % m`, `v // d` or the digit sum | 3 | 70% |
| c-replace-rules | sample_399 | const | 2 of 3: the pattern length limit, the blocking character, the expansion factor | 3 | 65% |
| c-zero-gap | sample_76 | const | 2 of 3: the filter floor, the short-list limit, the gap (value and width) | 3 | 65% |
| c-vowel-stretch | sample_601 | param | new required parameters: `times`, `times + joiner`, or `vowels + times` | 4 | 60% (75% at d3) |
| c-chunk-levels | sample_667 | param | new required `width`, plus `first` (level start) or `label` (replaces `level`) | 3 | 60% |
| c-mark-count | sample_5 | cond | the "is decimal" test becomes one of 5 character classes, possibly with lower/upper swapped | 3 | 60% |
| c-pair-count | sample_793 | cond | `!=` becomes `<`, `>`, sum divisible by k, absolute difference <= w, or same parity (self-pairs i == j included) | 3 | 55% |
| c-median-weave | sample_363 | order | the ascending sort becomes descending, by absolute value, by last digit, evens first, or negatives last | 3 | 55% |
| c-min-pairs | sample_771 | order | each round picks the max, the last occurrence of the min or max, or the smallest last digit | 3 | 45% |
| c-rfind-ends | sample_729 | ret_extra | returns `(ends, extra)`: uncovered character count, leftmost start, or s1 with the found occurrences removed | 3 | 45% |
| c-rise-runs | sample_223 | ret_extra | returns `(count, extra)`: largest single addition, number of adding steps, or last adding index | 3 | 45% |
| c-classify | sample_369 | case_add | 2 of 5 new classes (hex, neg-int/neg-float, blank, word, pair) at stated positions in the check chain | 3 | 40% |
| c-rule-apply | sample_623 | case_add | 2 new rule symbols (of 7) with 2 actions (of 6), placed before the fallback rule | 3 | 45% |

The mean guess at the default difficulty is about 55% (deepseek-flash, the C default of 8 turns). These are
guesses only. The Root's per-template aggregates (calibration, template probes) move each knob by one step.

### The eight operators (documented in `C/_lib/s1c.py`)
`out_map` post-processes the return value. `in_map` pre-processes an argument. `const` changes literal
constants of the logic. `param` lifts hard-coded values into new required parameters, which the hidden calls
always pass. `cond` changes a predicate. `order` changes a traversal, selection or sort order. `ret_extra`
also returns a seeded extra value computed by the same logic. `case_add` adds new cases to existing dispatch
logic at stated positions. Every task asks for new behaviour relative to today's code ("today: …"), never
"restore the original", so looking up the public function solves nothing.

### Instance shape
- Workspace: `<pkg>/__init__.py`, `<pkg>/<core>.py`, `<pkg>/<helper>.py` (a 3-file package; the logic or its
  constants are split between core and helper), `README.md`, `NOTICE` (attribution + MIT), and
  `tests/test_<pkg>.py` (the visible examples, protected).
- Seeded names: package `<stem>_<5 letters>` (registered as an extra token), module names, and the function
  name.
- Hidden check: one `py_cases` step over `check/hidden/cases.json`, with the visible examples plus 27 seeded
  cases. Expected values come from the modified reference through `s1lib.py_eval`. Inputs are
  type-preserving perturbations of the CRUXEval input plus fresh seeded inputs of the same types, ASCII
  only, never dependent on set or hash order. Inputs on which the reference raises are dropped. At most 20%
  of hidden cases are regression guards (same value under the original), so the no-op scores <= 500 by
  construction. Scoring is `fraction`; a pass is 1000.
- Difficulty knob: d1 gives 5 examples plus a hint; d2 4 examples; d3 3 examples; d4 adds a seeded secondary
  out_map twist on the final result plus 4 examples; d5 the twist plus 3 examples.

## Tests
- `python3 root/sealed/tools/s1validate.py <all 14 c-* ids>` (full: schema, P01–P05, P09, knob sweep d1–d5,
  checker digest) in 3 parallel batches: **14/14 ok, 19 checks each** (44–89 s per template while
  the machine's load was about 90). `s1lint.py`: C has no findings (14/14 composition).
- Robustness sweep: 40 dev seeds per template at mixed difficulties, gen/run only. 560/560 generated.
- `xchk.py` (S1-C's extra checks, dev seeds; maintainer scratch, not committed): X1 gen/run is byte-identical
  under python 3.9 (/usr/bin) and 3.14 (Homebrew), and in dev mode the packaged original equals the verbatim
  CRUXEval function on every candidate input; X2 a 3.9-generated solution scores 1000 under the 3.14 runner,
  and the no-op scores the same on both; X3 the prompt's `python3 -m unittest tests/test_<pkg>.py` passes on
  the solution and fails on the no-op. **28/28 pass** (14 templates x 2 dev seeds, d3 and d5). No-op scores
  0–166 permille.

## Deviations
- D1 The operator and the pass guess live in template.json `notes` and in the task labels (`change`,
  `<operator>`), because FORMAT.md allows exactly the listed template.json keys.
- D2 `c-word-stride` and `c-vowel-stretch` default to difficulty 4: at d3 I guess them above the 30–70% target.
- D3 Turns follow m7 §3.1.3 (C = 8, from s1lib), not the 12 mentioned in the lane brief.
- D4 The modified reference may raise on some inputs (c-min-pairs: the picked item is the last one).
  Those inputs are never used as cases, and no case expects an exception.

## Needs from others
- None blocking. POOL-LITE has the sample-id list, so its visible CRUX tasks stay disjoint: 5, 76, 183, 223,
  363, 369, 399, 601, 623, 667, 729, 771, 780, 793.

## Left
- Real-model calibration (Root aggregates only) to replace the guesses above.
