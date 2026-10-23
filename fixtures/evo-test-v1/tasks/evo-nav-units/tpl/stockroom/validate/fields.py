"""Field checks: each takes a value and returns an error message, or None when the value is fine."""
from ..models.item import LOCATION_RE, SKU_RE
#@A
from ..units import UNITS
#@E


def sku_format(value):
    if not SKU_RE.match(value or ""):
        return "must look like ABC-1234"
    return None


def not_blank(value):
    if not str(value or "").strip():
        return "must not be blank"
    return None


def max_length(n):
    def check(value):
        if len(str(value or "")) > n:
            return "must be at most %d characters" % n
        return None
    return check


def non_negative(value):
    if value < 0:
        return "must not be negative"
    return None


def positive(value):
    if value <= 0:
        return "must be positive"
    return None


def location_code(value):
    if value and not LOCATION_RE.match(value):
        return "must look like A1 or B12"
    return None


def category_name(value):
    if not value or value != value.strip().lower():
        return "must be lower-case and not blank"
    return None
#@A


def known_unit(value):
    if value not in UNITS:
        return "unknown unit %r" % value
    return None
#@E
