# dev fixture (partial): str.split() whitespace (too wide) and no keep_newlines
def replace_ws(text, insert):
    whitespaces = {'\t', '\r', '\v', ' ', '\f', '\n'}
    return ''.join(insert if c in whitespaces else c for c in text)


def squeeze_ws(text, insert=" ", keep_newlines=False):
    return insert.join(text.split())
