"""The unified diff format (SPEC.md section 4): rendering hunks as text. The diff command prints this text and the
patch command writes rejected hunks with it."""

NO_NEWLINE = "\\ No newline at end of file"


def format_range(start, count):
    """The range text of one side of a hunk header (SPEC.md 4.2) from its position and count."""
    if count == 1:
        return "%d" % (start + 1)
    if count == 0:
        return "%d,0" % start
    return "%d,%d" % (start + 1, count)


def hunk_header(hunk):
    return "@@ -%s +%s @@" % (format_range(hunk.old_start, hunk.old_count),
                              format_range(hunk.new_start, hunk.new_count))


def hunk_text_lines(hunk):
    """The lines of one hunk, header first, without line terminators."""
    out = [hunk_header(hunk)]
    for tag, line in hunk.lines:
        out.append(tag + line.text)
        if not line.eol:
            out.append(NO_NEWLINE)
    return out


def file_header(old_label, new_label):
    return ["--- " + old_label, "+++ " + new_label]


def render(old_label, new_label, hunks):
    """A complete unified diff: the two header lines, then every hunk; each line ends with a newline."""
    lines = file_header(old_label, new_label)
    for hunk in hunks:
        lines.extend(hunk_text_lines(hunk))
    return "".join(line + "\n" for line in lines)
