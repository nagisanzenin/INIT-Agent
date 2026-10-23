"""Dev fixture: a half-right solution with the instance's replies but the classic rules (yelling = some
letters, all upper case; no trailing-off, number or whisper rules; the classic order). It must score
below 1000."""
P = None  # @@PARAMS@@


def response(hey_bob):
    s = hey_bob.strip()
    r = P["replies"]
    if not s:
        return r["silence"]
    letters = [c for c in s if c.isalpha()]
    yell = bool(letters) and s.upper() == s
    if yell and s.endswith("?"):
        return r["yelled_question"]
    if yell:
        return r["yell"]
    if s.endswith("?"):
        return r["question"]
    return r["other"]
