def format_counts(pairs):
    """One "value: count" line per (value, count) pair returned by value_counts."""
    return "\n".join("%d: %d" % (v, n) for v, n in pairs)
