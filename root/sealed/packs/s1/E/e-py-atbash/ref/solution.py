"""Reference solution for e-py-atbash. gen/run replaces the P line with the instance's parameters:
shift (0..25), digits ("keep" | "complement" | "drop"), groups (list of group sizes, used cyclically),
sep (the group separator) and strict (decode rejects unexpected characters)."""
P = None  # @@PARAMS@@

_LETTERS = "abcdefghijklmnopqrstuvwxyz"


def _letter(ch):
    return _LETTERS[(25 - _LETTERS.index(ch) + P["shift"]) % 26]


def _digit(ch):
    if P["digits"] == "keep":
        return ch
    if P["digits"] == "complement":
        return str(9 - int(ch))
    return ""


def _translate(text):
    out = []
    for ch in text:
        low = ch.lower() if "A" <= ch <= "Z" else ch
        if "a" <= low <= "z":
            out.append(_letter(low))
        elif "0" <= ch <= "9":
            out.append(_digit(ch))
    return "".join(out)


def encode(plain_text):
    s = _translate(plain_text)
    sizes = P["groups"]
    groups, i, k = [], 0, 0
    while i < len(s):
        n = sizes[k % len(sizes)]
        groups.append(s[i:i + n])
        i += n
        k += 1
    return P["sep"].join(groups)


def decode(ciphered_text):
    if P["strict"]:
        for ch in ciphered_text:
            if not ("a" <= ch <= "z" or "0" <= ch <= "9" or ch == P["sep"]):
                raise ValueError("unexpected character %r" % ch)
    return _translate(ciphered_text)
