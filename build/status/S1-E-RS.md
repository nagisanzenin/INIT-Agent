# S1-E-RS: 12 Rust family-E templates for pack s1

Lane S1-E-RS (docs/m7-design.md §3.1.1 family E; scope build/specs/PROOF-LEAN.md D-L1: E = Py 12 + Rust 12).
Format: root/sealed/packs/s1/FORMAT.md (S1-CORE, e81e4d5).

## Done
12 templates under `root/sealed/packs/s1/E/e-rs-*/` (template.json, gen/run, ref/lib.rs, ref/naive.rs,
NOTICE, LICENSE). They share `E/_shared_rs/ersgen.py`, the Rust instance scaffold (one crate; stub; protected
`tests/visible.rs` + `Cargo.toml`; README with the task text; NOTICE; hidden `cargo_test` step over
`hidden/hidden_s1.rs` = the visible examples + 24-30 seeded hidden cases; dev fixture `dev/naive.tar`).

- Source: exercism/rust at `cc099ae090a6acbe6f53f1e36325263046ba333e` (MIT, `Copyright (c) 2021 Exercism`).
  Each template's LICENSE is a byte copy of the upstream LICENSE, which is byte-identical to
  `LICENSES/Exercism-MIT.txt`. Each NOTICE names the exercise path and the commit.
- Only a-m slugs (M7-P08), with no overlap with S1-E-PY's slugs.
- Every template states its rules in full, with the seeded values, in the prompt and the README. The expected
  values come from a Python oracle in gen/run. The Rust reference (`ref/lib.rs`, rendered with the seeded
  constants) is the instance solution, so validation cross-checks the two implementations.
  `ref/naive.rs` is the track's `.meta/example.rs`, the unmutated public solution. Where the API changed it gets
  a marked INIT adapter. It must score below 1000 on every validated seed and difficulty, which is the check
  that "a memorised public solution fails".
- Every difficulty of every template differs from the original exercise. Generators raise GenError if fewer
  than 4 graded cases separate the mutation from the original.

| id | source exercise | seeded mutation | difficulty (default, range) | guess: deepseek-flash pass rate |
|---|---|---|---|---|
| e-rs-luhn | luhn | separator set (strict placement at d3), doubling period 2/3, multiplier 2/3, modulus 7/9/10/11, minimum digits | 2 (1-3) | ~65% |
| e-rs-isbn-verifier | isbn-verifier | symbol count, weight scheme, modulus 11/13 with seeded check letters, separator char (strict placement + no lowercase at d3) | 2 (1-3) | ~60% |
| e-rs-armstrong-numbers | armstrong-numbers | base b and exponent offset k; at d2 the whole u64 range without overflow | 1 (1-2) | ~70% |
| e-rs-allergies | allergies | seeded allergen-to-bit permutation, bit offset, list order; XOR mask at d3 | 2 (1-3) | ~65% |
| e-rs-binary-search | binary-search | sort order (ascending / descending / by absolute value), leftmost or rightmost duplicate, Err(insertion index) | 2 (1-3) | ~60% |
| e-rs-crypto-square | crypto-square | digits kept or dropped, case, wide or tall rectangle, padding char, chunk separator, reversed chunks at d4 | 3 (2-4) | ~50% |
| e-rs-flower-field | flower-field | flower/empty/zero symbols, neighbourhood (orthogonal, diagonal, Moore, knight, radius-2 cross), torus wrap at d4 | 3 (2-4) | ~50% |
| e-rs-matching-brackets | matching-brackets | active bracket pairs (others are text), skipped quoted sections (d3+), maximum depth (d4) | 3 (2-4) | ~50% |
| e-rs-circular-buffer | circular-buffer | read end and overwrite-eviction end, capacity 0 (d3+), peek/drain with String elements (d4) | 3 (2-4) | ~55% |
| e-rs-clock | clock | hours per day, minutes per hour, separator, hour 0 shown as H (d3+), full i32 range with chained adds (d4) | 3 (2-4) | ~45% |
| e-rs-book-store | book-store | number of titles 4-6, unit price, discount table (rerolled until greedy grouping is wrong; non-monotone at d4+), larger baskets at d5 | 4 (3-5) | ~35% |
| e-rs-bowling | bowling | frames 5-9, pins per frame, strike/spare bonus lengths 1-3 with matching fill rolls (d4+), errors and incomplete games | 4 (3-5) | ~30% |

Difficulty guesses assume a pass needs all graded cases (score 1000), within the E caps (8 turns, 24k output
tokens). They are guesses until the Root's `sealed_calibrate` aggregates arrive.

## Commits
- `c7608b3`: 8 templates (luhn, crypto-square, flower-field, binary-search, clock, circular-buffer, book-store,
  bowling) and `E/_shared_rs/ersgen.py`.
- `7a29cef`: isbn-verifier, armstrong-numbers, allergies (written by a helper of this lane
  and reviewed here), matching-brackets (whose selection fix makes the discriminating cases always present), and
  this status file.

## Tests (dev seeds only, mock-free: no model calls)
- S1-CORE validator, full run (M7-P01..P05, P09, knob sweep, checker digest), one template at a time:
  `INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 python3 root/sealed/tools/s1validate.py <id>`.
  Result: **12/12 ok**. luhn 90.8 s, crypto-square 57.9 s, flower-field 54.7 s, binary-search 26.1 s,
  clock 28.0 s, circular-buffer 33.5 s, book-store 38.8 s, bowling 39.9 s, matching-brackets 27.8 s,
  isbn-verifier 21.2 s, armstrong-numbers 20.2 s, allergies 23.2 s.
- Lane naive check (scratch `validate_rs.py`): determinism; solution 1000 (x3 on the first seed);
  no-op 0 right after the solution; empty < 1000; `dev/naive.tar` < 1000; tamper exit 3. It runs 3 seeds at
  the default difficulty plus the min and max difficulty. **12/12 pass.** The naive (public) solution scores
  0-750 permille, so it never passes. The slowest warm check took 7.9 s, within the 10 s limit.
- Generator stress, Python only: 60 seeds × every difficulty × 8 templates, plus 450 seeds for
  matching-brackets, all with 0 failures. The slowest gen/run took 1.7 s (book-store d5).
- `python3 root/sealed/tools/s1lint.py`: P07 licences ok, P08 disjointness ok.

## Deviations
- Ex-D1: `ref/naive.rs` and the `dev/naive` fixture are extra (FORMAT.md does not require them). The S1-CORE
  validator ignores `dev/`; the lane's own check (below) runs it.
- Ex-D2: e-rs-bowling at d5 can have a strike in frame F-1 followed by an open frame F (3 strike-bonus rolls,
  only 2 left). The rule text states that the bonus then counts the rolls that exist.

## Needs from others
- None blocking. Root (ROOT-S1a): the sealed check env needs the Rust toolchain on PATH (S1-CORE noted this).

## Left
- Calibration by real-model aggregates (REAL-M7 line C) may move the difficulty knobs.
