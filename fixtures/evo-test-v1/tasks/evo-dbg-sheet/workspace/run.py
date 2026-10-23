"""Command line: python3 run.py BOOK.csv [EDITS]  (SPEC.md section 1)."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from sheet.csvio import CsvError, format_row, parse_csv  # noqa: E402
from sheet.refs import col_index, parse_range, parse_ref  # noqa: E402
from sheet.values import to_text  # noqa: E402
from sheet.workbook import Workbook  # noqa: E402


ROW_RE = re.compile(r"^[1-9][0-9]{0,6}$")
COL_RE = re.compile(r"^[A-Za-z]{1,3}$")


class UsageError(Exception):
    pass


def read_text(path):
    try:
        with open(path, "rb") as f:
            return f.read().decode("utf-8")
    except (OSError, UnicodeDecodeError):
        raise UsageError("cannot read %s" % path)


def rows_of(book, rect):
    return [format_row([to_text(book.value(p)) for p in row]) for row in rect.rows()]


def sheet_rows(book):
    nrows, ncols = book.used_size()
    return [format_row([to_text(book.value((r, c))) for c in range(1, ncols + 1)]) for r in range(1, nrows + 1)]


def run_command(book, line):
    """Output lines of one EDITS command, or None when the line is not a valid command."""
    parts = line.split(" ", 2)
    cmd = parts[0]
    if cmd == "set" and len(parts) >= 2:
        ref = parse_ref(parts[1])
        if ref is None:
            return None
        book.set_cells([(ref.pos, parts[2] if len(parts) == 3 else "")])
        return []
    if cmd == "copy" and len(parts) == 3:
        src, dest = parse_ref(parts[1]), parse_range(parts[2])
        if src is None or dest is None:
            return None
        book.copy(src.pos, dest)
        return []
    if cmd == "print" and len(parts) == 1:
        return sheet_rows(book)
    if cmd == "print" and len(parts) == 2:
        rect = parse_range(parts[1])
        return None if rect is None else rows_of(book, rect)
    if cmd in ("insert", "delete") and len(parts) == 3:
        if parts[1] == "row" and ROW_RE.match(parts[2]):
            book.restructure(0, int(parts[2]), cmd == "delete")
            return []
        if parts[1] == "col" and COL_RE.match(parts[2]):
            book.restructure(1, col_index(parts[2]), cmd == "delete")
            return []
        return None
    if line == "recalc":
        return ["recalc:" + "".join(" " + n for n in book.recalc_names()) + "\n"]
    return None


def main(argv):
    out = sys.stdout.buffer
    if len(argv) not in (2, 3):
        raise UsageError("usage: run.py BOOK.csv [EDITS]")
    try:
        rows = parse_csv(read_text(argv[1]))
    except CsvError as e:
        raise UsageError("%s: %s" % (argv[1], e))
    book = Workbook()
    book.load_rows(rows)
    if len(argv) == 2:
        out.write("".join(sheet_rows(book)).encode("utf-8"))
        return 0
    lines = read_text(argv[2]).split("\n")
    if lines[-1] == "":
        lines.pop()
    for n, line in enumerate(lines, 1):
        if line.endswith("\r"):
            line = line[:-1]
        if line.strip() == "" or line.startswith("#"):
            continue
        result = run_command(book, line)
        if result is None:
            out.flush()
            raise UsageError("line %d: bad command" % n)
        out.write("".join(result).encode("utf-8"))
    return 0


if __name__ == "__main__":
    try:
        code = main(sys.argv)
    except UsageError as e:
        sys.stdout.flush()
        sys.stderr.write("run.py: %s\n" % e)
        code = 2
    sys.stdout.flush()
    sys.exit(code)
