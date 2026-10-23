#!/usr/bin/env python3
"""RF-10 vectors (ADR-011 §3, lane X3): the sentinel's exact tails, computed independently with Python integers
and fractions. Writes tails.json next to this file. Numerators and denominators are the unreduced integers the
Rust code forms (src/sentinel/stats.rs), as decimal strings, so the comparison is digit for digit.

    python3 -I root/crates/init-root/tests/fixtures/sentinel/tails.py
"""
import json
import os
from fractions import Fraction
from math import comb, gcd

HARD = Fraction(2, 1000)
SOFT = Fraction(5, 100)


def exceed(m, j, n, k):
    num = sum(comb(j - 1 + i, i) * comb(m - j + n - i, n - i) for i in range(k, n + 1))
    return num, comb(m + n, n)


def exceed_k(m, j, n, alpha):
    for k in range(n + 1):
        num, den = exceed(m, j, n, k)
        if Fraction(num, den) <= alpha:
            return k
    return None


def reduce(a, d):
    g = gcd(a, d)
    return a // g, d // g


def binom(n, k, a, d):
    b = d - a
    num = sum(comb(n, i) * a**i * b**(n - i) for i in range(k, n + 1))
    return num, d**n


def binom_k(n, a, d, alpha):
    best = None
    for k in range(n, -1, -1):
        num, den = binom(n, k, a, d)
        if Fraction(num, den) <= alpha:
            best = k
        else:
            break
    return best


def fisher(N, K, n, a):
    num = sum(comb(K, x) * comb(N - K, n - x) for x in range(a, min(n, K) + 1) if n - x <= N - K)
    return num, comb(N, n)


def rate(r1, r2):
    """max of two rates, reduced (ADR-011: r0 = max(floor, (b+1)/(m+2)))."""
    return reduce(*max(r1, r2, key=lambda r: Fraction(*r)))


def main():
    out = {"exceed": [], "exceed_k": [], "binom": [], "binom_k": [], "h9": [], "fisher": []}
    for n in (5, 10, 20):
        for k in range(n + 1):
            num, den = exceed(40, 10, n, k)
            out["exceed"].append({"m": 40, "j": 10, "n": n, "k": k, "num": str(num), "den": str(den)})
    for n in range(1, 21):
        out["exceed_k"].append({"m": 40, "j": 10, "n": n, "hard": exceed_k(40, 10, n, HARD), "soft": exceed_k(40, 10, n, SOFT)})
    rates = [(1, 20), (1, 10)] + [rate((1, 20), (b + 1, 42)) for b in range(41)]
    for a, d in sorted(set(rates)):
        for n in (5, 10, 20):
            out["binom_k"].append({"n": n, "a": a, "d": d, "hard": binom_k(n, a, d, HARD), "soft": binom_k(n, a, d, SOFT)})
    for n in (5, 10, 20):
        for k in range(n + 1):
            num, den = binom(n, k, 1, 10)
            out["binom"].append({"n": n, "k": k, "a": 1, "d": 10, "num": str(num), "den": str(den)})
    for b in (0, 4, 8, 20):
        a, d = rate((1, 50), (b + 1, 402))
        for n, k in ((40, 3), (120, 5), (400, 12), (1000, 30)):
            num, den = binom(n, k, a, d)
            out["h9"].append({"n": n, "k": k, "a": a, "d": d, "num": str(num), "den": str(den),
                              "hard": Fraction(num, den) <= HARD})
    for K, n, a in ((10, 10, 4), (30, 20, 8), (60, 60, 35), (8, 12, 4), (20, 5, 4)):
        num, den = fisher(120, K, n, a)
        out["fisher"].append({"N": 120, "K": K, "n": n, "a": a, "num": str(num), "den": str(den)})
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tails.json")
    with open(path, "w") as f:
        json.dump(out, f, indent=0, sort_keys=True)
        f.write("\n")


if __name__ == "__main__":
    main()
