"""Reference solution for e-py-bob. gen/run replaces the P line with the instance's parameters:
k (upper-case letters needed to count as yelling), order (the categories in the order they are checked)
and replies (category -> reply)."""
P = None  # @@PARAMS@@

_UP = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
_LO = "abcdefghijklmnopqrstuvwxyz"
_DIGITS = "0123456789"


def _is(cat, s):
    ups = sum(1 for c in s if c in _UP)
    los = sum(1 for c in s if c in _LO)
    yell = ups >= P["k"] and los == 0
    question = s.endswith("?")
    if cat == "silence":
        return s == ""
    if cat == "yelled_question":
        return yell and question
    if cat == "yell":
        return yell
    if cat == "question":
        return question
    if cat == "trailing":
        return s.endswith("...")
    if cat == "number":
        return ups + los == 0 and any(c in _DIGITS for c in s)
    if cat == "whisper":
        return los >= 1 and ups == 0 and s.endswith(".")
    return cat == "other"


def response(hey_bob):
    s = hey_bob.strip()
    for cat in P["order"]:
        if _is(cat, s):
            return P["replies"][cat]
    return P["replies"]["other"]
