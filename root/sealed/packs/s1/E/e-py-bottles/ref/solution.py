"""Reference solution for e-py-bottles. gen/run replaces the P line with the instance's parameters:
one / many (the object's singular and plural), colour, place (the phrase after the object), verb, max (the
highest start), k (items falling per verse) and refrain (an extra line for even starting counts, or "")."""
P = None  # @@PARAMS@@

_WORDS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
          "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty"]


def _things(n):
    return "%s %s %s" % (_WORDS[n], P["colour"], P["one"] if n == 1 else P["many"])


def _verse(n):
    m = min(P["k"], n)
    head = _things(n)
    head = head[0].upper() + head[1:] + " " + P["place"] + ","
    lines = [head, head]
    if P["refrain"] and n % 2 == 0:
        lines.append(P["refrain"])
    lines.append("And if %s should accidentally %s," % (_things(m), P["verb"]))
    lines.append("There'll be %s %s." % (_things(n - m), P["place"]))
    return lines


def recite(start, take=1):
    if not 1 <= start <= P["max"]:
        raise ValueError("start must be between 1 and %d" % P["max"])
    starts = list(range(start, 0, -P["k"]))
    if take < 1 or take > len(starts):
        raise ValueError("take must be between 1 and %d" % len(starts))
    out = []
    for i, n in enumerate(starts[:take]):
        if i:
            out.append("")
        out.extend(_verse(n))
    return out
