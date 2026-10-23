"""Finding where a hunk applies (SPEC.md 7.4 and 7.5): fuzz levels, the order of the candidate positions and
the comparison of a hunk's old side with the working copy."""
import collections

Placement = collections.namedtuple("Placement", "position fuzz lead trail")
"""Where a hunk applies: the position of its first old-side line, the fuzz level of the match and the numbers of
old-side lines that level ignores at the start (lead) and at the end (trail)."""


def fuzz_window(hunk, fuzz):
    """How many old-side lines fuzz level `fuzz` ignores at the start and at the end of the hunk."""
    lead = min(fuzz, hunk.leading_context())
    trail = min(fuzz, hunk.leading_context())
    return lead, trail


def candidates(expected, low, high):
    """Every position in [low, high], in the order SPEC.md 7.5 tries them."""
    return sorted(range(low, high + 1), key=lambda position: abs(position - expected))


def matches_at(work, old, position, lead, trail):
    """Whether the old side, without its first `lead` and last `trail` lines, lies on the working copy when its
    first line is put at `position`."""
    end = len(old) - trail
    return work[position + lead:position + end] == old[lead:end]


def locate(work, hunk, expected, floor, max_fuzz):
    """The Placement of the hunk in the working copy, or None when it applies nowhere."""
    old = hunk.old_lines()
    high = len(work) - len(old)
    for fuzz in range(max_fuzz + 1):
        lead, trail = fuzz_window(hunk, fuzz)
        for position in candidates(expected, floor, high):
            if matches_at(work, old, position, lead, trail):
                return Placement(position, fuzz, lead, trail)
    return None
