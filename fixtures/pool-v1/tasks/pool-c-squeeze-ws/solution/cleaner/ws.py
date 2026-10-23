WHITESPACE = {'\t', '\r', '\v', ' ', '\f', '\n'}


def replace_ws(text, insert):
    """Replaces every whitespace character of `text` with `insert`."""
    whitespaces = {'\t', '\r', '\v', ' ', '\f', '\n'}
    clean = ''
    for char in text:
        if char in whitespaces:
            clean += insert
        else:
            clean += char
    return clean


def squeeze_ws(text, insert=" ", keep_newlines=False):
    """Trims `text` and turns every inner run of whitespace into one `insert` (or one newline)."""
    out = []
    run = None
    for ch in text:
        if ch in WHITESPACE:
            run = (run or "") + ch
            continue
        if run is not None and out:
            out.append("\n" if keep_newlines and "\n" in run else insert)
        run = None
        out.append(ch)
    return "".join(out)
