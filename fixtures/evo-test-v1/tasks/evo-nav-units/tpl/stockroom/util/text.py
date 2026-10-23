"""Plain-text helpers: whitespace cleanup and the aligned tables every text output uses."""


def clean(value) -> str:
    """str(value) with surrounding whitespace removed and inner runs collapsed to one space; None -> ""."""
    if value is None:
        return ""
    return " ".join(str(value).split())


def parse_bool(value) -> bool:
    text = str(value).strip().lower()
    if text in ("1", "true", "yes", "y", "on"):
        return True
    if text in ("0", "false", "no", "n", "off", ""):
        return False
    raise ValueError("bad boolean %r" % value)


def table(header, rows) -> str:
    """Columns left-aligned, separated by two spaces, trailing spaces removed; one line per row plus the
    header line. Cells are converted with str(); None prints as "-"."""
    cells = [[("-" if c is None else str(c)) for c in row] for row in rows]
    widths = [len(h) for h in header]
    for row in cells:
        for i, c in enumerate(row):
            widths[i] = max(widths[i], len(c))
    lines = ["  ".join(h.ljust(widths[i]) for i, h in enumerate(header)).rstrip()]
    for row in cells:
        lines.append("  ".join(c.ljust(widths[i]) for i, c in enumerate(row)).rstrip())
    return "\n".join(lines) + "\n"


def plural(n, word) -> str:
    return "%d %s%s" % (n, word, "" if n == 1 else "s")
