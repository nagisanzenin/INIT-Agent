def value_counts(bag, min_count=1):
    """(value, count) pairs for the int values of the dict `bag`, most frequent first."""
    if min_count < 1:
        raise ValueError("min_count must be >= 1")
    tbl = {}
    for v in bag.values():
        if isinstance(v, int) and not isinstance(v, bool):
            tbl[v] = tbl.get(v, 0) + 1
    pairs = [(v, n) for v, n in tbl.items() if n >= min_count]
    pairs.sort(key=lambda p: (-p[1], p[0]))
    return pairs
