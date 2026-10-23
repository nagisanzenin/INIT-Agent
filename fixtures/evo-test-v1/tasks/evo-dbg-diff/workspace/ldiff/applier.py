"""Applying the hunks of a patch to a list of lines (SPEC.md section 7) and describing the result (section 8)."""
from .locate import locate
from .unified import render

DEFAULT_FUZZ = 2


class Outcome(object):
    """The result of applying the hunks of one file section: the patched lines, the hunk messages in order, the
    rejected hunks and the number of hunks."""

    messages = []
    rejected = []

    def __init__(self, lines, total):
        self.lines = lines
        self.total = total

    def failed(self, number, expected, hunk):
        self.messages.append(failure_message(number, expected))
        self.rejected.append(hunk)

    def succeeded(self, number, placement, offset):
        if placement.fuzz or offset:
            self.messages.append(success_message(number, placement.position, placement.fuzz, offset))


def success_message(number, position, fuzz, offset):
    text = "Hunk #%d succeeded at %d" % (number, position + 1)
    if fuzz:
        text += " with fuzz %d" % fuzz
    if offset:
        text += " (offset %d line%s)" % (offset, "" if offset in (1, -1) else "s")
    return text + "."


def failure_message(number, expected):
    return "Hunk #%d FAILED at %d." % (number, expected + 1)


def replace(work, hunk, placement):
    """Puts the new side of the hunk in place of its old side (SPEC.md 7.6). Returns the new-side line count."""
    old, new = hunk.old_lines(), hunk.new_lines()
    start, lead, trail = placement.position, placement.lead, placement.trail
    work[start + lead:start + len(old) - trail] = new[lead:len(new) - trail]
    return len(new)


def apply_hunks(lines, hunks, max_fuzz=DEFAULT_FUZZ):
    """Applies the hunks in order to a copy of `lines` (SPEC.md 7.2 to 7.7)."""
    outcome = Outcome(list(lines), len(hunks))
    work = outcome.lines
    shift = offset = floor = 0
    for number, hunk in enumerate(hunks, 1):
        base = hunk.old_start + shift
        expected = base + offset
        placement = locate(work, hunk, expected, floor, max_fuzz)
        if placement is None:
            outcome.failed(number, expected, hunk)
            continue
        offset = placement.position - base
        added = replace(work, hunk, placement)
        shift = added - len(hunk.old_lines())
        floor = placement.position + added
        outcome.succeeded(number, placement, offset)
    return outcome


def summary(outcome, reject_path, dry_run):
    """The closing line of a patch run with rejected hunks (SPEC.md 8.4)."""
    text = "%d out of %d hunk%s FAILED" % (len(outcome.rejected), outcome.total, "" if outcome.total == 1 else "s")
    if dry_run:
        return text
    return text + " -- saving rejects to file " + reject_path


def reject_text(patch, outcome):
    """The content of the reject file (SPEC.md 8.5)."""
    return render(patch.old_label, patch.new_label, outcome.rejected)
