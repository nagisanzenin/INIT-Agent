"""Item rules (ruleset "items"). RULES is checked in order; every failing rule gives one problem."""
from . import fields
from .problems import Problem
#@A
from ..units import UNITS, dimension, is_whole_count
#@E

RULES = (
    ("sku", fields.sku_format),
    ("name", fields.not_blank),
    ("name", fields.max_length(60)),
    ("category", fields.category_name),
    ("quantity", fields.non_negative),
    ("reorder_level", fields.non_negative),
    ("location", fields.location_code),
    ("unit_cost", fields.non_negative),
#@A
    ("unit", fields.known_unit),
#@E
)


def validate_item(item) -> list:
    problems = []
    for field, check in RULES:
        message = check(getattr(item, field))
        if message:
            problems.append(Problem(item.sku, field, message))
#@A
    problems.extend(_whole_counts(item))
#@E
    return problems
#@A


def _whole_counts(item) -> list:
    """Count units (ea, dz): the quantity and the reorder level must be whole numbers of ea."""
    if item.unit not in UNITS or dimension(item.unit) != "count":
        return []
    return [Problem(item.sku, field, "must be a whole number of ea")
            for field in ("quantity", "reorder_level") if not is_whole_count(getattr(item, field), item.unit)]
#@E


def check_items(inv, config=None) -> list:
    """Ruleset "items": every item, by SKU."""
    out = []
    for item in inv.sorted_items():
        out.extend(validate_item(item))
    return out
