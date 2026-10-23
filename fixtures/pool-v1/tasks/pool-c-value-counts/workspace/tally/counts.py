def value_counts(bag):
    """How often each value 0..99 occurs among the values of the dict `bag`."""
    values = list(bag.values())
    tbl = {}
    for v in range(100):
        if v in values:
            tbl[v] = values.count(v)
    return tbl
