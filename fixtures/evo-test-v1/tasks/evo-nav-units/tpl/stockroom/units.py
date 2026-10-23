#@A
"""Units of measure (CHANGE.md): the unit table, normalization and conversion.

Every quantity is in the unit of its item. Conversion multiplies by the source factor, divides by the target
factor and rounds to three decimal places (half up); it is defined only within one dimension.
"""
from __future__ import annotations

from decimal import Decimal

from .errors import UnitError
from .util.money import round_money
from .util.qty import parse_qty, round_qty

DEFAULT_UNIT = "ea"

# code -> (dimension, factor in the dimension's base unit)
UNITS = {
    "ea": ("count", Decimal("1")),
    "dz": ("count", Decimal("12")),
    "g": ("mass", Decimal("1")),
    "kg": ("mass", Decimal("1000")),
    "lb": ("mass", Decimal("453.59237")),
    "oz": ("mass", Decimal("28.349523125")),
    "ml": ("volume", Decimal("1")),
    "l": ("volume", Decimal("1000")),
}


def normalize_unit(text) -> str:
    """The canonical code of `text` (trimmed, lower-case); None or blank means the default unit, ea."""
    if text is None:
        return DEFAULT_UNIT
    code = str(text).strip().lower()
    if not code:
        return DEFAULT_UNIT
    if code not in UNITS:
        raise UnitError("unknown unit %r" % str(text).strip())
    return code


def is_known(code) -> bool:
    return code in UNITS


def dimension(code) -> str:
    try:
        return UNITS[code][0]
    except KeyError:
        raise UnitError("unknown unit %r" % code) from None


def compatible(a, b) -> bool:
    """True when both codes are known and of the same dimension."""
    return a in UNITS and b in UNITS and UNITS[a][0] == UNITS[b][0]


def factor_ratio(from_unit, to_unit) -> Decimal:
    if from_unit not in UNITS:
        raise UnitError("unknown unit %r" % from_unit)
    if to_unit not in UNITS:
        raise UnitError("unknown unit %r" % to_unit)
    if UNITS[from_unit][0] != UNITS[to_unit][0]:
        raise UnitError("cannot convert %s to %s" % (from_unit, to_unit))
    return UNITS[from_unit][1] / UNITS[to_unit][1]


def convert(qty, from_unit, to_unit) -> Decimal:
    """qty (in from_unit) expressed in to_unit, rounded to three decimal places half up."""
    qty = parse_qty(qty) if not isinstance(qty, Decimal) else qty
    if from_unit == to_unit and from_unit in UNITS:
        return qty
    return round_qty(qty * factor_ratio(from_unit, to_unit))


def convert_cost(cost, from_unit, to_unit) -> Decimal:
    """A price per from_unit expressed per to_unit, rounded to cents half up."""
    if from_unit == to_unit and from_unit in UNITS:
        return cost
    return round_money(cost / factor_ratio(from_unit, to_unit))


def is_whole_count(qty, unit) -> bool:
    """For a count unit: the quantity is a whole number of ea."""
    value = qty * UNITS[unit][1]
    return value == value.to_integral_value()


def convert_item(item, unit):
    """The item expressed in `unit`: quantity, reorder level and unit cost converted. Raises UnitError when the
    dimensions differ."""
    if item.unit == unit:
        return item
    return item.with_changes(
        quantity=convert(item.quantity, item.unit, unit),
        reorder_level=convert(item.reorder_level, item.unit, unit),
        unit_cost=convert_cost(item.unit_cost, item.unit, unit),
        unit=unit,
    )


def convert_items(items, unit) -> list:
    """Items of `unit`'s dimension converted into it; the others unchanged (order kept)."""
    return [convert_item(i, unit) if compatible(i.unit, unit) else i for i in items]
#@E
#@A


DIMENSION_ORDER = {"count": 0, "mass": 1, "volume": 2}


def quantity_sort_key(item):
    """Orders items by quantity across units: count, then mass, then volume, then unknown units; within a
    dimension by the quantity in its base unit; ties by SKU."""
    if item.unit in UNITS:
        dim, factor = UNITS[item.unit]
        return (DIMENSION_ORDER[dim], item.quantity * factor, item.sku)
    return (len(DIMENSION_ORDER), item.quantity, item.sku)


def split_quantity(text):
    """"2.5 kg" / "750g" / "12" -> (quantity text, unit code or None)."""
    import re

    m = re.match(r"^\s*([-+]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+))\s*([A-Za-z]*)\s*$", text or "")
    if not m:
        return (text or "").strip(), None
    return m.group(1), (m.group(2) or None)
#@E
