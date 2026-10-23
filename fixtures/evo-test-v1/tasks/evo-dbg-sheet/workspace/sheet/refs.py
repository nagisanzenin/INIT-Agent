"""A1-style cell references: column letters, cell names, anchored references and rectangles.

A position is a (row, col) tuple of 1-based integers: A1 is (1, 1), C2 is (2, 3), AA10 is (10, 27).
"""
import re

MAX_COL = 18278  # ZZZ
MAX_ROW = 9999999

REF_RE = re.compile(r"^(\$?)([A-Za-z]{1,3})(\$?)([1-9][0-9]{0,6})$")


def col_index(letters):
    """Column number of a column name (letters in bijective base 26, SPEC.md section 5.1)."""
    n = 0
    for ch in letters.upper():
        n = n * 26 + (ord(ch) - ord("A") + 1)
    return n


def col_letters(n):
    """Column name of a column number (the inverse of col_index)."""
    s = ""
    while n > 0:
        n, r = divmod(n, 26)
        if r == 0:
            r = 26
        s = chr(ord("A") + r - 1) + s
    return s


def cell_name(pos):
    return "%s%d" % (col_letters(pos[1]), pos[0])


def in_sheet(row, col):
    return 1 <= row <= MAX_ROW and 1 <= col <= MAX_COL


class Ref:
    """A cell reference as written in a formula: a position plus its $ anchors."""

    __slots__ = ("row", "col", "abs_row", "abs_col")

    def __init__(self, row, col, abs_row=False, abs_col=False):
        self.row = row
        self.col = col
        self.abs_row = abs_row
        self.abs_col = abs_col

    @property
    def pos(self):
        return (self.row, self.col)

    def text(self):
        return "%s%s%s%d" % ("$" if self.abs_col else "", col_letters(self.col), "$" if self.abs_row else "",
                             self.row)

    def shifted(self, drow, dcol):
        """The reference moved by (drow, dcol) as a copy moves it, or None when it leaves the sheet."""
        row = self.row if self.abs_row else self.row + drow
        col = self.col if self.abs_col else self.col + dcol
        if not in_sheet(row, col):
            return None
        return Ref(row, col, self.abs_row, self.abs_col)

    def coord(self, axis):
        """The row (axis 0) or the column (axis 1)."""
        return self.col if axis else self.row

    def with_coord(self, axis, i):
        """The reference with its row (axis 0) or column (axis 1) set to i, or None when that leaves the sheet."""
        row, col = (self.row, i) if axis else (i, self.col)
        if not in_sheet(row, col):
            return None
        return Ref(row, col, self.abs_row, self.abs_col)

    def __repr__(self):
        return "Ref(%s)" % self.text()


def parse_ref(text):
    """A Ref for text such as B7, $B7, B$7 or $B$7 (letters in any case), or None."""
    m = REF_RE.match(text)
    if m is None:
        return None
    return Ref(int(m.group(4)), col_index(m.group(2)), m.group(3) == "$", m.group(1) == "$")


class Rect:
    """A rectangle of cells given by two corners in any order."""

    __slots__ = ("top", "left", "bottom", "right")

    def __init__(self, a, b):
        self.top, self.bottom = min(a[0], b[0]), max(a[0], b[0])
        self.left, self.right = min(a[1], b[1]), max(a[1], b[1])

    def contains(self, pos):
        return self.top <= pos[0] <= self.bottom and self.left <= pos[1] <= self.right

    def rows(self):
        """Positions row by row, each row left to right."""
        return [[(r, c) for c in range(self.left, self.right + 1)] for r in range(self.top, self.bottom + 1)]

    def cells(self):
        return [p for row in self.rows() for p in row]

    @property
    def shape(self):
        return (self.bottom - self.top + 1, self.right - self.left + 1)

    @property
    def origin(self):
        return (self.top, self.left)

    def __repr__(self):
        return "Rect(%s:%s)" % (cell_name((self.top, self.left)), cell_name((self.bottom, self.right)))


def parse_range(text):
    """A Rect for A1:C3 (corners in any order) or for a single cell such as B2, or None."""
    parts = text.split(":")
    if len(parts) > 2:
        return None
    refs = [parse_ref(p) for p in parts]
    if any(r is None for r in refs):
        return None
    return Rect(refs[0].pos, refs[-1].pos)
