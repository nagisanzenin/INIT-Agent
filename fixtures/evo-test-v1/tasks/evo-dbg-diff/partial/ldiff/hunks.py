"""Hunks (SPEC.md section 3): the Hunk value used by every module, and the grouping of change blocks into hunks.

A hunk's old_start / new_start are positions (SPEC.md 6.6): the index of the first line it covers on that side,
or, when it covers no line on that side, the number of lines before the place where it applies. lines holds
(tag, Line) pairs with tag " " (context), "-" (old side only) or "+" (new side only)."""
from .lcs import diff_blocks

CONTEXT, DELETED, INSERTED = " ", "-", "+"

DEFAULT_CONTEXT = 3


class Hunk(object):
    __slots__ = ("old_start", "old_count", "new_start", "new_count", "lines")

    def __init__(self, old_start, old_count, new_start, new_count, lines):
        self.old_start = old_start
        self.old_count = old_count
        self.new_start = new_start
        self.new_count = new_count
        self.lines = lines

    def old_lines(self):
        return [line for tag, line in self.lines if tag != INSERTED]

    def new_lines(self):
        return [line for tag, line in self.lines if tag != DELETED]

    def leading_context(self):
        count = 0
        for tag, _ in self.lines:
            if tag != CONTEXT:
                break
            count += 1
        return count

    def trailing_context(self):
        count = 0
        for tag, _ in reversed(self.lines):
            if tag != CONTEXT:
                break
            count += 1
        return count

    def changes_something(self):
        return any(tag != CONTEXT for tag, _ in self.lines)

    def reversed(self):
        """The same hunk applied backwards: sides exchanged, line order kept."""
        swap = {CONTEXT: CONTEXT, DELETED: INSERTED, INSERTED: DELETED}
        return Hunk(self.new_start, self.new_count, self.old_start, self.old_count,
                    [(swap[tag], line) for tag, line in self.lines])


def group_blocks(blocks, context):
    """Splits the change blocks into the groups of SPEC.md 3.2."""
    groups = []
    for block in blocks:
        if groups and block.old_start - groups[-1][-1].old_end <= 2 * context:
            groups[-1].append(block)
        else:
            groups.append([block])
    return groups


def make_hunk(old, new, group, context):
    """The hunk of one group of blocks (SPEC.md 3.3)."""
    first, last = group[0], group[-1]
    lead = min(context, first.old_start)
    trail = min(context, len(old) - last.old_end)
    lines = [(CONTEXT, line) for line in old[first.old_start - lead:first.old_start]]
    previous = None
    for block in group:
        if previous is not None:
            lines.extend((CONTEXT, line) for line in old[previous.old_end:block.old_start])
        lines.extend((DELETED, line) for line in old[block.old_start:block.old_end])
        lines.extend((INSERTED, line) for line in new[block.new_start:block.new_end])
        previous = block
    lines.extend((CONTEXT, line) for line in old[last.old_end:last.old_end + trail])
    old_start = first.old_start - lead
    new_start = first.new_start - lead
    return Hunk(old_start, last.old_end + trail - old_start, new_start, last.new_end + trail - new_start, lines)


def make_hunks(old, new, context=DEFAULT_CONTEXT, compared=None):
    """Every hunk of the diff from old to new (lists of lines), in order; empty when the files are equal.
    `compared` (see lcs.comparison) relaxes line equality; context lines are always taken from old."""
    blocks = diff_blocks(old, new, compared)
    return [make_hunk(old, new, group, context) for group in group_blocks(blocks, context)]
