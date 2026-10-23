"""Money: decimal.Decimal amounts with two decimal places (unit costs, stock values)."""
from __future__ import annotations

from decimal import Decimal, InvalidOperation, ROUND_HALF_UP

CENTS = Decimal("0.01")


def round_money(value) -> Decimal:
    return Decimal(value).quantize(CENTS, rounding=ROUND_HALF_UP)


def parse_money(value) -> Decimal:
    """An amount from a str, int, float or Decimal; at most two decimal places. Raises ValueError("bad amount
    'x'")."""
    if isinstance(value, bool):
        raise ValueError("bad amount %r" % (value,))
    try:
        if isinstance(value, float):
            amount = Decimal(repr(value))
        elif isinstance(value, (int, Decimal)):
            amount = Decimal(value)
        elif isinstance(value, str) and value.strip():
            amount = Decimal(value.strip())
        else:
            raise ValueError("bad amount %r" % (value,))
    except InvalidOperation:
        raise ValueError("bad amount %r" % (value,)) from None
    if not amount.is_finite() or amount != round_money(amount):
        raise ValueError("bad amount %r" % (value,))
    return round_money(amount)


def format_money(value) -> str:
    """"12.50", "0.05", "1200.00"."""
    return format(round_money(value), "f")


def sum_money(values) -> Decimal:
    total = Decimal("0.00")
    for v in values:
        total += v
    return round_money(total)
