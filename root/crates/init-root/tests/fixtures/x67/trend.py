#!/usr/bin/env python3
"""Owner-corrections trend vectors (X10 item 5, lane X67): r0, the magnitude guard, the exact binomial tail and the
verdict, computed independently with Python integers and fractions. Writes trend.json next to this file; the Root's
`ledger::tests::trend_vectors` compares `ledger::corrections_trend` with it field by field.

    python3 -I root/crates/init-root/tests/fixtures/x67/trend.py

Rule: reference (n_ref, c_ref) = the first 14 free-path owner days with >= 40 owner turns; window (n, c) = the last
14 owner days after it, >= 40 owner turns. r0 = max(50/1000, (c_ref + 1)/(n_ref + 2)). Fire when c/n >= 1.5 * r0 and
the one-sided Fisher tail P(X >= c), X ~ Hypergeometric(N = n_ref + n, K = c_ref + c, n drawn), is <= 1/10000. The
tail is computed only when the magnitude guard holds.
"""
import json
import os
from fractions import Fraction
from math import comb

MIN = 40
ALPHA = Fraction(1, 10000)
FLOOR = Fraction(50, 1000)


def trend(nr, cr, n, c):
    r0 = max(FLOOR, Fraction(min(cr, nr) + 1, nr + 2))
    out = {"ref": [nr, cr], "win": [n, c], "r0": [r0.numerator, r0.denominator], "r0_permille": r0.numerator * 1000 // r0.denominator,
           "rate_permille": c * 1000 // n if n else 0, "ready": nr >= MIN and n >= MIN, "fire": False, "p_ppm": None}
    if not out["ready"] or c > n or Fraction(c, n) < Fraction(3, 2) * r0:
        return out
    big_n, big_k = nr + n, min(cr, nr) + c
    tail = Fraction(sum(comb(big_k, x) * comb(big_n - big_k, n - x) for x in range(c, min(big_k, n) + 1)), comb(big_n, n))
    out["p_ppm"] = (tail * 1_000_000).numerator // (tail * 1_000_000).denominator
    out["fire"] = tail <= ALPHA
    return out


CASES = [(448, 45, 448, 45), (448, 45, 448, 68), (448, 45, 448, 70), (448, 45, 448, 80), (448, 45, 448, 90),
         (39, 4, 448, 200), (448, 45, 39, 39), (448, 0, 448, 40), (448, 448, 448, 448), (100, 10, 60, 15),
         (100, 10, 60, 20), (2000, 200, 700, 120), (2000, 200, 700, 140), (60, 2, 40, 9), (60, 30, 300, 230)]

if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "trend.json"), "w") as f:
        json.dump([trend(*c) for c in CASES], f, indent=1, sort_keys=True)
        f.write("\n")
