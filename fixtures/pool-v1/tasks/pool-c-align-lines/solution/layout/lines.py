def align_lines(text, width, align="center", fill=" "):
    """Aligns every line of `text` (split on "\\n") in `width` columns."""
    if align not in ("left", "right", "center"):
        raise ValueError("align must be left, right or center")
    if not isinstance(fill, str) or len(fill) != 1:
        raise ValueError("fill must be one character")
    lines = [l.rstrip(" \t") for l in text.split("\n")]
    if width is None:
        width = max(len(l) for l in lines)
    if width < 0:
        raise ValueError("width must be >= 0")
    op = {"left": str.ljust, "right": str.rjust, "center": str.center}[align]
    return "\n".join(op(l, width, fill) for l in lines)


def center_lines(text, width):
    """Centers every line of `text` in `width` columns."""
    result = ""
    lines = text.split('\n')
    for l in lines:
        result += l.center(width)
        result += '\n'

    # Remove the very last empty line
    result = result[:-1]
    return result
