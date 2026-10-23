"""Plain-text tables for the command line.

    render(["id", "name"], [["atlas", "Atlas"], ["b2", "Basement"]])

    id     name
    -----  --------
    atlas  Atlas
    b2     Basement

Columns are separated by two spaces; numbers (ints) are right-aligned, everything else left-aligned. Trailing
spaces are removed from every line.
"""


def _cell(v):
    if v is None:
        return ""
    if isinstance(v, (list, tuple)):
        return ";".join(str(x) for x in v)
    return str(v)


def widths(headers, rows):
    w = [len(h) for h in headers]
    for row in rows:
        for i, v in enumerate(row):
            if i < len(w):
                w[i] = max(w[i], len(_cell(v)))
    return w


def render(headers, rows, empty="(none)"):
    """The table as one string with a final newline; `empty` alone when there are no rows."""
    if not rows:
        return empty + "\n"
    w = widths(headers, rows)
    numeric = [all(isinstance(r[i], int) and not isinstance(r[i], bool) for r in rows if i < len(r))
               for i in range(len(headers))]
    lines = ["  ".join(h.ljust(w[i]) for i, h in enumerate(headers)).rstrip(),
             "  ".join("-" * w[i] for i in range(len(headers)))]
    for row in rows:
        cells = []
        for i in range(len(headers)):
            text = _cell(row[i]) if i < len(row) else ""
            cells.append(text.rjust(w[i]) if numeric[i] else text.ljust(w[i]))
        lines.append("  ".join(cells).rstrip())
    return "\n".join(lines) + "\n"


def render_pairs(pairs):
    """Two-column key/value text: 'key: value' lines, keys padded to the longest."""
    if not pairs:
        return ""
    k = max(len(a) for a, _ in pairs)
    return "".join("%s: %s\n" % (a.ljust(k), _cell(b)) for a, b in pairs)
