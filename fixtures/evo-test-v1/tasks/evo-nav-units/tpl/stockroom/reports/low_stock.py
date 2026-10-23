"""Report "low-stock": items at or below their reorder level, by location then SKU.

#@B
Rows: sku, name, location, quantity, reorder_level, shortfall (reorder_level - quantity).
#@A
Rows: sku, name, location, quantity, unit, reorder_level, shortfall (reorder_level - quantity). With
options["unit"], the rows of items of that unit's dimension show quantity, reorder level and shortfall converted
into it (each converted on its own); the other rows keep their own unit. Selection never depends on the unit.
#@E
"""
from ..util.qty import qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

#@B
COLUMNS = ["sku", "name", "location", "quantity", "reorder_level", "shortfall"]
#@A
COLUMNS = ["sku", "name", "location", "quantity", "unit", "reorder_level", "shortfall"]
#@E


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
#@E
    rows = []
    for item in sorted(inv.items.values(), key=lambda i: (i.location, i.sku)):
        if not item.is_low():
            continue
        qty, level, short = item.quantity, item.reorder_level, item.shortfall()
#@A
        unit = item.unit
        if target is not None and compatible(unit, target):
            qty, level, short = (convert(v, unit, target) for v in (qty, level, short))
            unit = target
#@E
        rows.append({
            "sku": item.sku,
            "name": item.name,
            "location": item.location or None,
            "quantity": qty_json(qty),
#@A
            "unit": unit,
#@E
            "reorder_level": qty_json(level),
            "shortfall": qty_json(short),
        })
#@B
    return Report("low-stock", COLUMNS, rows)
#@A
    return Report("low-stock", COLUMNS, rows, {"unit": target})
#@E
