"""Line alignment (SPEC.md section 2): the edit script between two lists of lines, and its change blocks.

The script is a list of Op values in file order. op.old and op.new are the positions on each side where the
operation happens: a kept line is old[op.old] == new[op.new], a deletion removes old[op.old] at new position
op.new, an insertion adds new[op.new] at old position op.old."""
import collections
import re

Op = collections.namedtuple("Op", "tag old new")
Block = collections.namedtuple("Block", "old_start old_end new_start new_end")

KEEP, DELETE, INSERT = "=", "-", "+"


SPACE_RUN = re.compile(r"[ \t]+")


def comparison(ignore_case=False, space_change=False, all_space=False):
    """The function that maps a line's text to the text the diff compares (SPEC.md 5.2)."""
    def compared(text):
        if all_space:
            text = text.replace(" ", "").replace("\t", "")
        elif space_change:
            text = SPACE_RUN.sub(" ", text).rstrip(" ")
        if ignore_case:
            text = text.lower()
        return text
    return compared


def line_keys(old, new, compared=None):
    """Replaces every line by a small integer, equal lines (section 1.3) by the same one, so that the table below
    compares integers instead of tuples. `compared` maps a text to the text that is compared."""
    keys = {}
    if compared is None:
        a = [keys.setdefault(line, len(keys)) for line in old]
        b = [keys.setdefault(line, len(keys)) for line in new]
    else:
        a = [keys.setdefault((compared(line.text), line.eol), len(keys)) for line in old]
        b = [keys.setdefault((compared(line.text), line.eol), len(keys)) for line in new]
    return a, b


def common_prefix(a, b):
    limit = min(len(a), len(b))
    p = 0
    while p < limit and a[p] == b[p]:
        p += 1
    return p


def common_suffix(a, b, prefix):
    limit = min(len(a), len(b)) - prefix
    q = 0
    while q < limit and a[len(a) - 1 - q] == b[len(b) - 1 - q]:
        q += 1
    return q


def lcs_table(a, b):
    """table[i][j] is the length of a longest common subsequence of a[i:] and b[j:]."""
    n, m = len(a), len(b)
    table = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n - 1, -1, -1):
        row, below = table[i], table[i + 1]
        ai = a[i]
        for j in range(m - 1, -1, -1):
            if ai == b[j]:
                row[j] = below[j + 1] + 1
            else:
                down, right = below[j], row[j + 1]
                row[j] = down if down >= right else right
    return table


def align(a, b, old_base, new_base):
    """The edit script of the middle parts a and b, which start at old_base / new_base in the whole files."""
    table = lcs_table(a, b)
    ops = []
    i = j = 0
    while i < len(a) and j < len(b):
        if a[i] == b[j]:
            ops.append(Op(KEEP, old_base + i, new_base + j))
            i += 1
            j += 1
        elif table[i + 1][j] > table[i][j + 1]:
            ops.append(Op(DELETE, old_base + i, new_base + j))
            i += 1
        else:
            ops.append(Op(INSERT, old_base + i, new_base + j))
            j += 1
    while i < len(a):
        ops.append(Op(DELETE, old_base + i, new_base + j))
        i += 1
    while j < len(b):
        ops.append(Op(INSERT, old_base + i, new_base + j))
        j += 1
    return ops


def edit_script(old, new, compared=None):
    """The full edit script of section 2: common prefix, aligned middle, common suffix."""
    a, b = line_keys(old, new, compared)
    p = common_prefix(a, b)
    q = common_suffix(a, b, p)
    ops = [Op(KEEP, i, i) for i in range(p)]
    ops.extend(align(a[p:len(a) - q], b[p:len(b) - q], p, p))
    ops.extend(Op(KEEP, len(a) - q + k, len(b) - q + k) for k in range(q))
    return ops


def change_blocks(ops, old_count, new_count):
    """The maximal runs of deletions and insertions of a script, as Block ranges [start, end) on both sides."""
    blocks = []
    start = None
    for op in ops:
        if op.tag == KEEP:
            if start is not None:
                blocks.append(Block(start[0], op.old, start[1], op.new))
                start = None
        elif start is None:
            start = (op.old, op.new)
    if start is not None:
        blocks.append(Block(start[0], old_count, start[1], new_count))
    return blocks


def diff_blocks(old, new, compared=None):
    """The change blocks between two lists of lines, in file order."""
    return change_blocks(edit_script(old, new, compared), len(old), len(new))
