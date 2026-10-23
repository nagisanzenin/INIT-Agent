"""Reference solution for e-py-change. gen/run replaces the P line with the instance's parameters:
order ("asc" | "desc", the output order), tie ("larger" | "smaller", the last tie-break), distinct
(bool: fewer different coin values breaks ties first) and forbid (a pair of coin values that may not both
be used, or None)."""
P = None  # @@PARAMS@@


def _combos(denoms, avail, target):
    """Every multiset of coins (count vectors) summing to target within the available counts."""
    out = []
    counts = [0] * len(denoms)

    def rec(i, rem):
        if rem == 0:
            out.append(list(counts))
            return
        if i == len(denoms):
            return
        d = denoms[i]
        for k in range(min(avail[i], rem // d), -1, -1):
            counts[i] = k
            rec(i + 1, rem - k * d)
        counts[i] = 0

    rec(0, target)
    return out


def find_fewest_coins(coins, target):
    if target < 0:
        raise ValueError("target can't be negative")
    if target == 0:
        return []
    denoms = sorted(d for d in coins if coins[d] > 0)
    avail = [coins[d] for d in denoms]
    best, best_key = None, None
    for counts in _combos(denoms, avail, target):
        used = [d for d, k in zip(denoms, counts) if k]
        if P["forbid"] is not None and P["forbid"][0] in used and P["forbid"][1] in used:
            continue
        flat = []
        for d, k in zip(denoms, counts):
            flat += [d] * k
        key = [len(flat)]
        if P["distinct"]:
            key.append(len(used))
        if P["tie"] == "larger":
            key.append([-x for x in sorted(flat, reverse=True)])
        else:
            key.append(sorted(flat))
        if best_key is None or key < best_key:
            best, best_key = flat, key
    if best is None:
        raise ValueError("can't make target with given coins")
    return sorted(best, reverse=(P["order"] == "desc"))
