"""Formula parser (SPEC.md section 5.2): tokens -> expression tree, plus the reference helpers built on it."""
from .refs import Rect
from .tokenizer import (BOOL, COLON, COMMA, EOF, ERR, FUNC, LPAREN, NAME, NUM, OP, REF, RPAREN, STR, ParseError,
                        tokenize)

# operator -> (precedence, right associative); a higher precedence binds tighter
BINARY = {
    "^": (6, True),
    "*": (4, False), "/": (4, False),
    "+": (3, False), "-": (3, False),
    "&": (2, False),
    "=": (1, False), "<>": (1, False), "<": (1, False), "<=": (1, False), ">": (1, False), ">=": (1, False),
}
UNARY_PREC = 7


class Node:
    __slots__ = ()


class Const(Node):
    """A literal: number, string, logical value or error value."""
    __slots__ = ("value",)

    def __init__(self, value):
        self.value = value


class CellRef(Node):
    __slots__ = ("ref",)

    def __init__(self, ref):
        self.ref = ref


class RangeRef(Node):
    __slots__ = ("first", "last", "rect")

    def __init__(self, first, last):
        self.first = first
        self.last = last
        self.rect = Rect(first.pos, last.pos)


class Name(Node):
    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name


class Unary(Node):
    __slots__ = ("op", "operand")

    def __init__(self, op, operand):
        self.op = op
        self.operand = operand


class Binary(Node):
    __slots__ = ("op", "left", "right")

    def __init__(self, op, left, right):
        self.op = op
        self.left = left
        self.right = right


class Call(Node):
    __slots__ = ("name", "args")

    def __init__(self, name, args):
        self.name = name
        self.args = args


class Parser:
    def __init__(self, tokens):
        self.toks = tokens
        self.i = 0

    def peek(self):
        return self.toks[self.i]

    def take(self):
        tok = self.toks[self.i]
        if tok.kind != EOF:
            self.i += 1
        return tok

    def expect(self, kind):
        tok = self.take()
        if tok.kind != kind:
            raise ParseError("expected %r, found %s" % (kind, tok.describe()))
        return tok

    def formula(self):
        node = self.expr(0)
        tok = self.peek()
        if tok.kind != EOF:
            raise ParseError("unexpected %s" % tok.describe())
        return node

    def expr(self, min_prec):
        left = self.unary()
        while True:
            tok = self.peek()
            if tok.kind != OP:
                return left
            prec, right_assoc = BINARY[tok.value]
            if prec < min_prec:
                return left
            self.take()
            right = self.expr(prec if right_assoc else prec + 1)
            left = Binary(tok.value, left, right)

    def unary(self):
        tok = self.peek()
        if tok.kind == OP and tok.value in ("+", "-"):
            self.take()
            return Unary(tok.value, self.expr(UNARY_PREC))
        return self.primary()

    def primary(self):
        tok = self.take()
        if tok.kind in (NUM, STR, BOOL, ERR):
            return Const(tok.value)
        if tok.kind == REF:
            if self.peek().kind == COLON:
                self.take()
                last = self.expect(REF)
                return RangeRef(tok.value, last.value)
            return CellRef(tok.value)
        if tok.kind == NAME:
            return Name(tok.value)
        if tok.kind == FUNC:
            return self.call(tok.value)
        if tok.kind == LPAREN:
            node = self.expr(0)
            self.expect(RPAREN)
            return node
        raise ParseError("unexpected %s" % tok.describe())

    def call(self, name):
        self.expect(LPAREN)
        args = []
        if self.peek().kind == RPAREN:
            self.take()
            return Call(name, args)
        while True:
            args.append(self.expr(0))
            tok = self.take()
            if tok.kind == RPAREN:
                return Call(name, args)
            if tok.kind != COMMA:
                raise ParseError("expected ',' or ')', found %s" % tok.describe())


def parse(src):
    """The expression tree of a formula (the text after '='). Raises ParseError."""
    return Parser(tokenize(src)).formula()


def references(node):
    """(cells, rects): the positions of the single-cell references in reading order, and the range rectangles."""
    cells, rects = [], []
    stack = [node]
    while stack:
        n = stack.pop()
        if isinstance(n, CellRef):
            cells.append(n.ref.pos)
        elif isinstance(n, RangeRef):
            rects.append(n.rect)
        elif isinstance(n, Unary):
            stack.append(n.operand)
        elif isinstance(n, Binary):
            stack.extend((n.right, n.left))
        elif isinstance(n, Call):
            stack.extend(reversed(n.args))
    return cells, rects


def remap_formula(src, cell_fn, range_fn):
    """The formula text src with every reference rewritten: cell_fn(ref) gives the new Ref of a cell reference and
    range_fn(first, last) the new pair of corners of a range; None stands for #REF!. Other text is kept."""
    try:
        toks = tokenize(src)
    except ParseError:
        return src
    out, last, i = [], 0, 0
    while i < len(toks):
        tok = toks[i]
        if tok.kind != REF:
            i += 1
            continue
        if toks[i + 1].kind == COLON and toks[i + 2].kind == REF:
            end = toks[i + 2].end
            moved = range_fn(tok.value, toks[i + 2].value)
            i += 3
        else:
            end = tok.end
            one = cell_fn(tok.value)
            moved = None if one is None else (one,)
            i += 1
        out.append(src[last:tok.start])
        out.append("#REF!" if moved is None else ":".join(r.text() for r in moved))
        last = end
    out.append(src[last:])
    return "".join(out)


def _pair(a, b):
    return None if a is None or b is None else (a, b)


def shift_formula(src, drow, dcol):
    """The formula text src with its references moved by (drow, dcol), as copying moves them (SPEC.md 9.2)."""
    def cell(ref):
        return ref.shifted(drow, dcol)

    def span(first, last):
        return _pair(first.shifted(drow, dcol), last.shifted(drow, dcol))
    return remap_formula(src, cell, span)


def insert_formula(src, axis, at):
    """The formula text src after a row (axis 0) or column (axis 1) is inserted before number `at` (SPEC.md 9.5)."""
    def cell(ref):
        i = ref.coord(axis)
        return ref.with_coord(axis, i + 1 if i >= at else i)

    def span(first, last):
        return _pair(cell(first), cell(last))
    return remap_formula(src, cell, span)


def delete_formula(src, axis, at):
    """The formula text src after row (axis 0) or column (axis 1) number `at` is deleted (SPEC.md 9.5)."""
    def cell(ref):
        i = ref.coord(axis)
        if i == at:
            return None
        return ref.with_coord(axis, i - 1 if i > at else i)

    def span(first, last):
        lo, hi = sorted((first.coord(axis), last.coord(axis)))
        if lo == hi == at:
            return None
        new_lo = lo - 1 if lo >= at else lo
        new_hi = hi - 1 if hi >= at else hi
        if first.coord(axis) > last.coord(axis):
            new_lo, new_hi = new_hi, new_lo
        return _pair(first.with_coord(axis, new_lo), last.with_coord(axis, new_hi))
    return remap_formula(src, cell, span)
