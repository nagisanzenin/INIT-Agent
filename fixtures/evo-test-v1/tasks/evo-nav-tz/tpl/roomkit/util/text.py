"""Small text helpers: ids, tags, quoting, plurals."""
import re

_SLUG_RE = re.compile(r"^[a-z0-9-]{1,30}$")


def is_slug(text):
    return isinstance(text, str) and bool(_SLUG_RE.match(text))


def split_tags(text):
    """'video; whiteboard;' -> ['video', 'whiteboard'] (empty parts dropped, order kept, duplicates dropped)."""
    out = []
    for part in (text or "").split(";"):
        part = part.strip()
        if part and part not in out:
            out.append(part)
    return out


def join_tags(tags):
    return ";".join(tags)


def plural(n, word, many=None):
    if n == 1:
        return "%d %s" % (n, word)
    return "%d %s" % (n, many or word + "s")


def truncate(text, n):
    if len(text) <= n:
        return text
    return text[: max(0, n - 3)] + "..."


def strip_all(values):
    return [v.strip() if isinstance(v, str) else v for v in values]


def parse_key_value(text):
    """'tz=JST' -> ('tz', 'JST'); used for --opt KEY=VALUE."""
    if "=" not in text:
        return None
    k, v = text.split("=", 1)
    k = k.strip()
    if not k:
        return None
    return k, v.strip()
