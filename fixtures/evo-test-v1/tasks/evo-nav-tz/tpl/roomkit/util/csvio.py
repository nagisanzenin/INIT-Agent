"""CSV text with an optional schema line.

A store file starts with `#roomkit-schema: N`; a file without that line is version 1. Rows come back as
(line number, dict) pairs; line numbers count every physical line of the file, the schema line included.
"""
import csv
import io
import os

from roomkit.errors import StoreError

SCHEMA_PREFIX = "#roomkit-schema:"


def parse_rows(text):
    """[(line, [cells])] for every non-empty line of a CSV text (cells stripped)."""
    out = []
    lines = text.split("\n")
    for n, line in enumerate(lines, 1):
        if line.endswith("\r"):
            line = line[:-1]
        if not line.strip():
            continue
        cells = next(csv.reader([line]))
        out.append((n, [c.strip() for c in cells]))
    return out


def to_text(header, rows):
    """CSV text with '\\n' line ends: the header row, then the rows (lists of strings)."""
    buf = io.StringIO()
    w = csv.writer(buf, lineterminator="\n")
    w.writerow(header)
    for row in rows:
        w.writerow(row)
    return buf.getvalue()


def split_schema(text, name):
    """(version, rest of the text, line offset). The schema line is line 1 when present."""
    first, _, rest = text.partition("\n")
    if first.startswith(SCHEMA_PREFIX):
        raw = first[len(SCHEMA_PREFIX):].strip()
        try:
            version = int(raw)
        except ValueError:
            raise StoreError("%s: bad schema line '%s'" % (name, first.strip()))
        return version, rest, 1
    return 1, text, 0


def read_versioned(path):
    """(version, header, [(line, dict)]) of a store CSV file; None when the file does not exist."""
    name = os.path.basename(path)
    if not os.path.exists(path):
        return None
    try:
        with open(path, "r", encoding="utf-8", newline="") as f:
            text = f.read()
    except OSError as e:
        raise StoreError("%s: %s" % (name, e.strerror or e))
    version, rest, shift = split_schema(text, name)
    rows = parse_rows(rest)
    if not rows:
        return version, [], []
    (_, header), body = rows[0], rows[1:]
    out = []
    for n, cells in body:
        rec = {}
        for i, h in enumerate(header):
            rec[h] = cells[i] if i < len(cells) else ""
        out.append((n + shift, rec))
    return version, header, out


def write_versioned(path, version, header, rows):
    name = os.path.basename(path)
    text = "%s %d\n" % (SCHEMA_PREFIX, version) + to_text(header, rows)
    try:
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text)
    except OSError as e:
        raise StoreError("%s: %s" % (name, e.strerror or e))
