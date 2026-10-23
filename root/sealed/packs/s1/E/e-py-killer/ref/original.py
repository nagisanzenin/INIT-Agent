"""Dev fixture: the unmutated exercise's behaviour adapted to the new interface: distinct digits 1..9,
ascending inside each combination and across the list, `include` ignored.
The validator checks that this scores below 1000 on every instance."""
import itertools


def combinations(target, size, exclude, include=()):
    if size < 1 or target < 0:
        raise ValueError("bad cage")
    allowed = [d for d in range(1, 10) if d not in exclude]
    return [list(c) for c in itertools.combinations(allowed, size) if sum(c) == target]
