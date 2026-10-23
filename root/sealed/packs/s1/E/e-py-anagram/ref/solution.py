"""Reference solution for e-py-anagram. gen/run replaces the P line with the instance's parameters:
ignore (characters removed before comparing), case ("insensitive" | "sensitive"), order ("input" |
"alpha" | "length"), dedupe (bool) and plus_one (bool: one extra character is allowed)."""
P = None  # @@PARAMS@@


def _norm(s):
    s = "".join(ch for ch in s if ch not in P["ignore"])
    return s.lower() if P["case"] == "insensitive" else s


def _counts(s):
    out = {}
    for ch in s:
        out[ch] = out.get(ch, 0) + 1
    return out


def _qualifies(word_n, cand_n):
    if cand_n == word_n:
        return False
    w, c = _counts(word_n), _counts(cand_n)
    if c == w:
        return True
    if not P["plus_one"] or len(cand_n) != len(word_n) + 1:
        return False
    extra = 0
    for ch, k in c.items():
        if k < w.get(ch, 0):
            return False
        extra += k - w.get(ch, 0)
    return extra == 1 and all(ch in c for ch in w)


def find_anagrams(word, candidates):
    word_n = _norm(word)
    out = []
    for cand in candidates:
        if P["dedupe"] and cand in out:
            continue
        if _qualifies(word_n, _norm(cand)):
            out.append(cand)
    if P["order"] == "alpha":
        out = sorted(out)
    elif P["order"] == "length":
        out = sorted(out, key=len)
    return out
