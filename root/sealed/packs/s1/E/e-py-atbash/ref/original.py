"""Dev fixture: the unmutated exercise's behaviour (classic Atbash, groups of 5 joined by spaces, digits kept).
The validator checks that this scores below 1000 on every instance."""

_LETTERS = "abcdefghijklmnopqrstuvwxyz"


def _translate(text):
    out = []
    for ch in text.lower():
        if ch in _LETTERS:
            out.append(_LETTERS[25 - _LETTERS.index(ch)])
        elif ch.isdigit():
            out.append(ch)
    return "".join(out)


def encode(plain_text):
    s = _translate(plain_text)
    return " ".join(s[i:i + 5] for i in range(0, len(s), 5))


def decode(ciphered_text):
    return _translate(ciphered_text)
