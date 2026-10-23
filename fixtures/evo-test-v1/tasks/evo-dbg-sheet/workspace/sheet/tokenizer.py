"""Formula tokenizer (SPEC.md section 5.1). A formula is the text after the leading '='."""
import re

from .refs import parse_ref
from .values import ERRORS

NUM, STR, BOOL, ERR, REF, FUNC, NAME, OP, LPAREN, RPAREN, COMMA, COLON, EOF = (
    "number", "string", "logical", "error", "reference", "function", "name", "operator", "(", ")", ",", ":",
    "end of formula")

NUM_RE = re.compile(r"([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?")
WORD_RE = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
DIGITS = "0123456789"
OPERATORS = ("<=", ">=", "<>", "+", "-", "*", "/", "^", "&", "=", "<", ">")
PUNCT = {"(": LPAREN, ")": RPAREN, ",": COMMA, ":": COLON}


class ParseError(Exception):
    pass


class Token:
    __slots__ = ("kind", "value", "start", "end")

    def __init__(self, kind, value, start, end):
        self.kind = kind
        self.value = value
        self.start = start
        self.end = end

    def describe(self):
        if self.kind in (EOF, LPAREN, RPAREN, COMMA, COLON):
            return repr(self.kind) if self.kind != EOF else self.kind
        return "%s %r" % (self.kind, self.value)

    def __repr__(self):
        return "Token(%s, %r, %d:%d)" % (self.kind, self.value, self.start, self.end)


def _string(src, i):
    """A double-quoted string starting at src[i]; returns (value, end)."""
    out = []
    j = i + 1
    while j < len(src):
        c = src[j]
        if c == '"':
            if src.startswith('"', j + 1):
                out.append('"')
                j += 2
                continue
            return "".join(out), j + 1
        out.append(c)
        j += 1
    raise ParseError("unterminated string at position %d" % i)


def _error_literal(src, i):
    for code in ERRORS:
        if src[i:i + len(code)].upper() == code:
            return ERRORS[code], i + len(code)
    raise ParseError("unknown error literal at position %d" % i)


def tokenize(src):
    """The tokens of a formula, ending with an EOF token. Raises ParseError."""
    toks = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in " \t":
            i += 1
            continue
        if c in DIGITS or (c == "." and i + 1 < n and src[i + 1] in DIGITS):
            m = NUM_RE.match(src, i)
            x = float(m.group(0))
            if x == float("inf"):
                raise ParseError("number too large at position %d" % i)
            toks.append(Token(NUM, x, i, m.end()))
            i = m.end()
        elif c == '"':
            value, j = _string(src, i)
            toks.append(Token(STR, value, i, j))
            i = j
        elif c == "#":
            value, j = _error_literal(src, i)
            toks.append(Token(ERR, value, i, j))
            i = j
        elif WORD_RE.match(c):
            m = WORD_RE.match(src, i)
            word, j = m.group(0), m.end()
            toks.append(_word(word, src, i, j))
            i = j
        elif c in PUNCT:
            toks.append(Token(PUNCT[c], c, i, i + 1))
            i += 1
        else:
            for op in OPERATORS:
                if src.startswith(op, i):
                    toks.append(Token(OP, op, i, i + len(op)))
                    i += len(op)
                    break
            else:
                raise ParseError("unexpected character %r at position %d" % (c, i))
    toks.append(Token(EOF, None, n, n))
    return toks


def _word(word, src, i, j):
    if j < len(src) and src[j] == "(" and "$" not in word:
        return Token(FUNC, word.upper(), i, j)
    ref = parse_ref(word)
    if ref is not None:
        return Token(REF, ref, i, j)
    if "$" in word:
        raise ParseError("bad reference %r at position %d" % (word, i))
    if word.upper() in ("TRUE", "FALSE"):
        return Token(BOOL, word.upper() == "TRUE", i, j)
    return Token(NAME, word, i, j)
