"""Cell values and the conversions between them (SPEC.md sections 3 and 6).

A value is one of: a number (int or float), a str, a bool, BLANK, or an ErrorValue.
"""
import math
import re


class Blank:
    """The value of a cell without content."""

    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self):
        return "BLANK"


BLANK = Blank()


class ErrorValue:
    __slots__ = ("code",)

    def __init__(self, code):
        self.code = code

    def __eq__(self, other):
        return isinstance(other, ErrorValue) and other.code == self.code

    def __hash__(self):
        return hash(self.code)

    def __repr__(self):
        return self.code


DIV0 = ErrorValue("#DIV/0!")
VALUE = ErrorValue("#VALUE!")
REF = ErrorValue("#REF!")
NAME = ErrorValue("#NAME?")
CYCLE = ErrorValue("#CYCLE!")
PARSE = ErrorValue("#PARSE!")
ERRORS = {e.code: e for e in (DIV0, VALUE, REF, NAME, CYCLE, PARSE)}

NUMBER_RE = re.compile(r"^[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$")


def is_error(v):
    return isinstance(v, ErrorValue)


def is_number(v):
    return isinstance(v, (int, float))


def is_text(v):
    return isinstance(v, str)


def is_bool(v):
    return isinstance(v, bool)


def parse_number(text):
    """The number written as text (number syntax of SPEC.md section 2), or None."""
    if not NUMBER_RE.match(text):
        return None
    x = float(text)
    if not math.isfinite(x):
        return None
    return x


def parse_content(text):
    """The constant value of a cell's content text that is not a formula (SPEC.md section 2)."""
    if text == "":
        return BLANK
    if text.startswith("'"):
        return text[1:]
    x = parse_number(text)
    if x:
        return x
    up = text.upper()
    if up == "TRUE":
        return True
    if up == "FALSE":
        return False
    return text


def format_number(x):
    if float(x).is_integer() and abs(x) < 1e15:
        return "%d" % x
    return "%.10g" % x


def to_text(v):
    """Text form of a non-error value; this is also how values are displayed."""
    if v is True:
        return "TRUE"
    if v is False:
        return "FALSE"
    if is_number(v):
        return format_number(v)
    if v is BLANK:
        return ""
    if is_error(v):
        return v.code
    return v


def to_number(v):
    """Number for arithmetic, or an ErrorValue."""
    if is_bool(v):
        return 1.0 if v else 0.0
    if is_number(v):
        return v
    if v is BLANK:
        return 0.0
    if is_text(v):
        x = parse_number(v.strip(" "))
        return VALUE if x is None else x
    return v


def to_bool(v):
    """Logical value, or an ErrorValue."""
    if is_bool(v):
        return v
    if is_number(v):
        return v != 0
    if v is BLANK:
        return False
    if is_text(v):
        up = v.upper()
        if up == "TRUE":
            return True
        if up == "FALSE":
            return False
        return VALUE
    return v


def finite(x):
    """x itself, or #VALUE! when x is not a finite number."""
    if isinstance(x, float) and not math.isfinite(x):
        return VALUE
    return x


def type_rank(v):
    """Ordering of the value types in comparisons: numbers < text < logical values."""
    if is_number(v):
        return 0
    if is_text(v):
        return 1
    return 2


def empty_like(v):
    """What a blank operand stands for when compared with v."""
    if is_bool(v):
        return False
    if is_text(v):
        return ""
    return 0.0


def compare(a, b):
    """-1, 0 or 1 for two non-error values (SPEC.md section 6.4)."""
    if a is BLANK and b is BLANK:
        return 0
    if a is BLANK:
        a = empty_like(b)
    if b is BLANK:
        b = empty_like(a)
    ra, rb = type_rank(a), type_rank(b)
    if ra != rb:
        return -1 if ra < rb else 1
    if ra == 1:
        a, b = a.lower(), b.lower()
    return (a > b) - (a < b)
