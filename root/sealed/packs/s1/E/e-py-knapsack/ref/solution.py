"""Reference solution for e-py-knapsack. gen/run replaces the P line with the instance's parameters:
tie ("fewest" | "lightest" | "most": the first tie-break between sets of equal value), cap (bool:
max_items is honoured) and groups (bool: at most one item per group)."""
P = None  # @@PARAMS@@


def best_items(max_weight, items, max_items=None):
    if max_weight < 0:
        raise ValueError("max_weight must not be negative")
    if P["cap"] and max_items is not None and max_items < 0:
        raise ValueError("max_items must not be negative")
    n = len(items)
    best, best_key = [], None
    for mask in range(1 << n):
        idx = [i for i in range(n) if mask >> i & 1]
        weight = sum(items[i]["weight"] for i in idx)
        if weight > max_weight:
            continue
        if P["cap"] and max_items is not None and len(idx) > max_items:
            continue
        if P["groups"]:
            seen = [items[i]["group"] for i in idx if "group" in items[i]]
            if len(seen) != len(set(seen)):
                continue
        value = sum(items[i]["value"] for i in idx)
        if P["tie"] == "fewest":
            tie = len(idx)
        elif P["tie"] == "lightest":
            tie = weight
        else:
            tie = -len(idx)
        key = (-value, tie, idx)
        if best_key is None or key < best_key:
            best, best_key = idx, key
    return best
