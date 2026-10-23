"""An independent model of README.md's word rules (used by check.py)."""


def words(text):
    out = []
    cur = ""
    i = 0
    while i < len(text):
        ch = text[i]
        if ("a" <= ch <= "z") or ("A" <= ch <= "Z"):
            cur += ch.lower()
        elif ch == "'" and cur and i + 1 < len(text) and (("a" <= text[i + 1] <= "z") or ("A" <= text[i + 1] <= "Z")):
            cur += "'"
        else:
            if cur:
                out.append(cur)
            cur = ""
        i += 1
    if cur:
        out.append(cur)
    return out


def top(texts, n):
    counts = {}
    for t in texts:
        for w in words(t):
            counts[w] = counts.get(w, 0) + 1
    items = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0].encode()))[:n]
    return "".join("%d\t%s\n" % (c, w) for w, c in items)
