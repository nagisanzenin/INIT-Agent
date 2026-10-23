def format_counts(tbl):
    """One "value: count" line per entry of the table returned by value_counts."""
    return "\n".join("%s: %s" % (v, n) for v, n in tbl.items())
