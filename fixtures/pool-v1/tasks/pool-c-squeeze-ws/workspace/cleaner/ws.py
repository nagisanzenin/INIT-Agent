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
