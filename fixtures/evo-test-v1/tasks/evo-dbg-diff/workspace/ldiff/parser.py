"""Reading a unified diff (SPEC.md section 6) into a Patch: two labels and a list of hunks."""
import re

from .hunks import CONTEXT, DELETED, INSERTED, Hunk
from .text import Line

HUNK_HEADER = re.compile(r"@@ -([0-9]+)(?:,([0-9]+))? \+([0-9]+)(?:,([0-9]+))? @@")
MARKER = "\\"


class PatchError(Exception):
    """A malformed patch; lineno is the 1-based line of the patch text where the problem shows."""

    def __init__(self, lineno, message):
        Exception.__init__(self, "line %d: %s" % (lineno, message))
        self.lineno = lineno


class Patch(object):
    __slots__ = ("old_label", "new_label", "hunks")

    def __init__(self, old_label, new_label, hunks):
        self.old_label = old_label
        self.new_label = new_label
        self.hunks = hunks

    def reversed(self):
        return Patch(self.new_label, self.old_label, [hunk.reversed() for hunk in self.hunks])


class Reader(object):
    """The lines of the patch text with a cursor; lineno is the number of the line returned last."""

    def __init__(self, text):
        self.lines = text.split("\n")
        if self.lines[-1] == "":
            self.lines.pop()
        self.lineno = 0

    def peek(self):
        return self.lines[self.lineno] if self.lineno < len(self.lines) else None

    def next(self):
        line = self.peek()
        if line is not None:
            self.lineno += 1
        return line


def label_of(line):
    """The label of a `--- ` or `+++ ` line (SPEC.md 6.2)."""
    fields = line[4:].split(None, 1)
    return fields[0] if fields else ""


def parse_range(start, count, lineno):
    """The position and count of one side of a hunk header (SPEC.md 6.3 and 6.6)."""
    start = int(start)
    count = 1 if count is None else int(count)
    if start == 0 and count:
        raise PatchError(lineno, "a range with lines must start at line 1 or later")
    return start - 1, count


def mark_no_newline(lines, lineno):
    """A marker line: the line read last lacks its newline."""
    if not lines:
        raise PatchError(lineno, "marker without a line before it")
    tag, line = lines[-1]
    if not line.eol:
        raise PatchError(lineno, "second marker for the same line")
    lines[-1] = (tag, Line(line.text, False))


def parse_hunk_header(text, lineno):
    match = HUNK_HEADER.match(text)
    if match is None:
        raise PatchError(lineno, "malformed hunk header")
    old_start, old_count = parse_range(match.group(1), match.group(2), lineno)
    new_start, new_count = parse_range(match.group(3), match.group(4), lineno)
    return old_start, old_count, new_start, new_count


def parse_hunk(reader):
    """Reads one hunk; the reader stands on its header line."""
    header_lineno = reader.lineno + 1
    old_start, old_count, new_start, new_count = parse_hunk_header(reader.next(), header_lineno)
    old_left, new_left = old_count, new_count
    lines = []
    while old_left or new_left:
        text = reader.next()
        if text is None:
            raise PatchError(reader.lineno + 1, "the patch ends inside a hunk")
        if text.startswith(MARKER):
            mark_no_newline(lines, reader.lineno)
            continue
        tag = text[:1] or CONTEXT
        if tag == CONTEXT:
            if not old_left or not new_left:
                raise PatchError(reader.lineno, "more lines than the hunk header says")
            old_left -= 1
            new_left -= 1
        elif tag == DELETED:
            if not old_left:
                raise PatchError(reader.lineno, "more old lines than the hunk header says")
            old_left -= 1
        elif tag == INSERTED:
            if not new_left:
                raise PatchError(reader.lineno, "more new lines than the hunk header says")
            new_left -= 1
        else:
            raise PatchError(reader.lineno, "unexpected line in a hunk")
        lines.append((tag, Line(text[1:], True)))
    following = reader.peek()
    if following is not None and following.startswith(MARKER):
        reader.next()
        mark_no_newline(lines, reader.lineno)
    hunk = Hunk(old_start, old_count, new_start, new_count, lines)
    if not hunk.changes_something():
        raise PatchError(header_lineno, "the hunk changes nothing")
    return hunk


def find_file_header(reader):
    """Skips the lines before the file header and returns the two labels."""
    while True:
        line = reader.next()
        if line is None:
            raise PatchError(max(reader.lineno, 1), "no file header (--- line) found")
        if line.startswith("--- "):
            break
    old_label = label_of(line)
    line = reader.next()
    if line is None or not line.startswith("+++ "):
        raise PatchError(reader.lineno + (line is None), "the --- line is not followed by a +++ line")
    return old_label, label_of(line)


def parse_section(reader):
    """Reads one file section: its header and its hunks."""
    old_label, new_label = find_file_header(reader)
    following = reader.peek()
    if following is None or not following.startswith("@@"):
        raise PatchError(reader.lineno + 1, "the file header is not followed by a hunk")
    hunks = []
    while following is not None and following.startswith("@@"):
        start = reader.lineno + 1
        hunk = parse_hunk(reader)
        if hunks and hunk.old_start < hunks[-1].old_start + hunks[-1].old_count:
            raise PatchError(start, "hunks out of order or overlapping")
        hunks.append(hunk)
        following = reader.peek()
    return Patch(old_label, new_label, hunks)


def skip_to_next_section(reader):
    """Skips the lines after a section up to the next `--- ` line; returns False at the end of the text."""
    while True:
        following = reader.peek()
        if following is None:
            return False
        if following.startswith("--- "):
            return True
        reader.next()


def parse_patch(text):
    """Parses a whole patch (SPEC.md section 6) into its file sections, in order."""
    reader = Reader(text)
    sections = [parse_section(reader)]
    while skip_to_next_section(reader):
        sections.append(parse_section(reader))
    return sections
