"""Kite lexer: turns source text into a list of tokens.

Token kinds: "int", "str", "ident", one kind per keyword (the keyword itself), one kind per operator or
punctuation mark (its text), and "eof".
"""
from .errors import KiteSyntaxError

KEYWORDS = frozenset([
    "let", "fn", "return", "if", "else", "while", "for", "break", "continue",
    "true", "false", "nil", "and", "or", "not",
])
TWO_CHAR = frozenset(["**", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=", "%="])
ONE_CHAR = frozenset("+-*/%<>=(){}[],;:")
ESCAPES = {"n": "\n", "t": "\t", "\\": "\\", '"': '"'}
DIGITS = frozenset("0123456789")
IDENT_START = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_")
IDENT_CHARS = IDENT_START | DIGITS


class Token:
    __slots__ = ("kind", "text", "value", "line")

    def __init__(self, kind, text, value, line):
        self.kind = kind
        self.text = text
        self.value = value
        self.line = line

    def __repr__(self):
        return "Token(%s, %r, line %d)" % (self.kind, self.text, self.line)


class Lexer:
    def __init__(self, source):
        self.src = source
        self.pos = 0
        self.line = 1
        self.tokens = []

    def tokenize(self):
        while True:
            self.skip_blanks()
            if self.pos >= len(self.src):
                self.tokens.append(Token("eof", "", None, self.line))
                return self.tokens
            c = self.src[self.pos]
            if c in DIGITS:
                self.number()
            elif c in IDENT_START:
                self.word()
            elif c == '"':
                self.string()
            else:
                self.operator()

    def skip_blanks(self):
        src = self.src
        while self.pos < len(src):
            c = src[self.pos]
            if c == "\n":
                self.line += 1
                self.pos += 1
            elif c in " \t\r":
                self.pos += 1
            elif c == "#":
                end = src.find("\n", self.pos)
                self.pos = len(src) if end < 0 else end
            elif src.startswith("/*", self.pos):
                end = src.find("*/", self.pos + 2)
                if end < 0:
                    raise KiteSyntaxError("unterminated comment", self.line)
                self.line += src.count("\n", self.pos, end)
                self.pos = end + 2
            else:
                return

    def number(self):
        start = self.pos
        while self.pos < len(self.src) and self.src[self.pos] in DIGITS:
            self.pos += 1
        if self.pos < len(self.src) and self.src[self.pos] in IDENT_START:
            raise KiteSyntaxError("invalid number", self.line)
        text = self.src[start:self.pos]
        self.tokens.append(Token("int", text, int(text), self.line))

    def word(self):
        start = self.pos
        while self.pos < len(self.src) and self.src[self.pos] in IDENT_CHARS:
            self.pos += 1
        text = self.src[start:self.pos]
        kind = text if text in KEYWORDS else "ident"
        self.tokens.append(Token(kind, text, text, self.line))

    def string(self):
        start = self.pos
        self.pos += 1
        out = []
        while True:
            if self.pos >= len(self.src) or self.src[self.pos] == "\n":
                raise KiteSyntaxError("unterminated string", self.line)
            c = self.src[self.pos]
            if c == '"':
                self.pos += 1
                break
            if c == "\\":
                esc = self.src[self.pos + 1:self.pos + 2]
                if esc not in ESCAPES:
                    raise KiteSyntaxError("invalid escape", self.line)
                out.append(ESCAPES[esc])
                self.pos += 2
            else:
                out.append(c)
                self.pos += 1
        self.tokens.append(Token("str", self.src[start:self.pos], "".join(out), self.line))

    def operator(self):
        two = self.src[self.pos:self.pos + 2]
        if two in TWO_CHAR:
            self.tokens.append(Token(two, two, None, self.line))
            self.pos += 2
            return
        c = self.src[self.pos]
        if c in ONE_CHAR:
            self.tokens.append(Token(c, c, None, self.line))
            self.pos += 1
            return
        raise KiteSyntaxError("unexpected character %r" % c, self.line)


def tokenize(source):
    return Lexer(source).tokenize()
