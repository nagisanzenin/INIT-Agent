"""Expression evaluation (SPEC.md section 6): operators, references and function calls."""
import math

from . import parser as P
from .functions import FUNCTIONS
from .refs import Rect
from .values import (BLANK, DIV0, NAME, VALUE, compare, finite, is_error, to_number, to_text)

COMPARISONS = {
    "=": lambda c: c == 0, "<>": lambda c: c != 0, "<": lambda c: c < 0, "<=": lambda c: c <= 0,
    ">": lambda c: c > 0, ">=": lambda c: c >= 0,
}


def arith(op, x, y):
    if op == "+":
        return finite(x + y)
    if op == "-":
        return finite(x - y)
    if op == "*":
        return finite(x * y)
    if op == "/":
        if y == 0:
            return DIV0
        return finite(x / y)
    if x == 0 and y < 0:
        return DIV0
    try:
        return finite(math.pow(x, y))
    except (ValueError, OverflowError):
        return VALUE


class Arg:
    """One function argument, evaluated only when the function asks for it."""

    __slots__ = ("ev", "node")

    def __init__(self, ev, node):
        self.ev = ev
        self.node = node

    @property
    def is_ref(self):
        return isinstance(self.node, (P.CellRef, P.RangeRef))

    @property
    def rect(self):
        """The rectangle a reference argument covers (a single cell for a cell reference)."""
        if isinstance(self.node, P.RangeRef):
            return self.node.rect
        return Rect(self.node.ref.pos, self.node.ref.pos)

    def value(self):
        return self.ev.eval(self.node)

    def cells(self):
        """Values of the cells a reference argument covers, row by row."""
        return [self.ev.lookup(p) for p in self.rect.cells()]


class Evaluator:
    def __init__(self, lookup):
        self.lookup = lookup

    def eval(self, node):
        if isinstance(node, P.Const):
            return node.value
        if isinstance(node, P.CellRef):
            return self.lookup(node.ref.pos)
        if isinstance(node, P.Binary):
            return self.binary(node)
        if isinstance(node, P.Unary):
            x = to_number(self.eval(node.operand))
            if is_error(x):
                return x
            return -x if node.op == "-" else x
        if isinstance(node, P.Call):
            return self.call(node)
        if isinstance(node, P.Name):
            return NAME
        return VALUE

    def binary(self, node):
        a = self.eval(node.left)
        b = self.eval(node.right)
        if is_error(a):
            return a
        if is_error(b):
            return b
        op = node.op
        if op == "&":
            return to_text(a) + to_text(b)
        if op in COMPARISONS:
            return COMPARISONS[op](compare(a, b))
        x = to_number(a)
        if is_error(x):
            return x
        y = to_number(b)
        if is_error(y):
            return y
        return arith(op, x, y)

    def call(self, node):
        fn = FUNCTIONS.get(node.name)
        if fn is None:
            return NAME
        lo, hi, impl = fn
        if not lo <= len(node.args) <= hi:
            return VALUE
        return impl([Arg(self, a) for a in node.args])


def evaluate(node, lookup):
    """The value of a formula cell: its expression's value, with a blank result shown as 0."""
    v = Evaluator(lookup).eval(node)
    return 0.0 if v is BLANK else v
