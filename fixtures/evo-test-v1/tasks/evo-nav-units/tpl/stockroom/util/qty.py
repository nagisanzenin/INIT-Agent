"""Quantities: every quantity in stockroom is a decimal.Decimal with at most three decimal places.

Text form (store files, CSV, ledger, tables, CLI messages): plain notation, no exponent, trailing zeros removed
("12", "2.5", "0.125"). JSON form (exports, API, reports): an integer when the value is whole, else a number.
"""
from __future__ import annotations

from decimal import Decimal, InvalidOperation, ROUND_HALF_UP

PLACES = Decimal("0.001")
ZERO = Decimal(0)
_NOT_FINITE = ("nan", "snan", "inf", "+inf", "-inf", "infinity", "+infinity", "-infinity")


def round_qty(value) -> Decimal:
    """Rounds to three decimal places, halves away from zero."""
    return Decimal(value).quantize(PLACES, rounding=ROUND_HALF_UP)


def parse_qty(value) -> Decimal:
    """A quantity from a str, int, float or Decimal.

    Raises ValueError("bad quantity 'x'") for anything else, for blank or non-finite text, and
    ValueError("too many decimal places in quantity 'x'") beyond three places.
    """
    if isinstance(value, bool):
        raise ValueError("bad quantity %r" % (value,))
    if isinstance(value, Decimal):
        result = value
    elif isinstance(value, int):
        result = Decimal(value)
    elif isinstance(value, float):
        result = Decimal(repr(value))
    elif isinstance(value, str):
        text = value.strip()
        if not text or text.lower() in _NOT_FINITE:
            raise ValueError("bad quantity %r" % value)
        try:
            result = Decimal(text)
        except InvalidOperation:
            raise ValueError("bad quantity %r" % value) from None
    else:
        raise ValueError("bad quantity %r" % (value,))
    if not result.is_finite():
        raise ValueError("bad quantity %r" % (value,))
    if result != round_qty(result):
        raise ValueError("too many decimal places in quantity %r" % (str(value),))
    return result


def format_qty(value) -> str:
    """The text form: "12", "2.5", "0.125", "-3"."""
    text = format(round_qty(value), "f")
    if "." in text:
        text = text.rstrip("0").rstrip(".")
    if text in ("-0", ""):
        text = "0"
    return text


def qty_json(value):
    """The JSON form: an int when whole, else a float with at most three decimals."""
    text = format_qty(value)
    return int(text) if "." not in text else float(text)


def signed_qty(value) -> str:
    """"+2.5" / "-1" / "0": the form movement deltas are printed in."""
    text = format_qty(value)
    return text if text.startswith("-") or text == "0" else "+" + text


def sum_qty(values) -> Decimal:
    total = ZERO
    for v in values:
        total += v
    return round_qty(total)
