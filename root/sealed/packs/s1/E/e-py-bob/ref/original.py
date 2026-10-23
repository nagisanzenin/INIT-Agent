"""Dev fixture: the unmutated exercise's behaviour (the classic five replies). It must score below 1000."""


def response(hey_bob):
    s = hey_bob.strip()
    if not s:
        return "Fine. Be that way!"
    yell = any(c.isalpha() for c in s) and s.upper() == s
    if yell and s.endswith("?"):
        return "Calm down, I know what I'm doing!"
    if yell:
        return "Whoa, chill out!"
    if s.endswith("?"):
        return "Sure."
    return "Whatever."
