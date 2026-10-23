"""Report "reorder": what to order so every low item gets back to twice its reorder level.

#@B
Rows: sku, name, quantity, reorder_level, order (2 x reorder_level - quantity); sorted by SKU. Only items at
or below a positive reorder level are listed.
#@A
Rows: sku, name, quantity, unit, reorder_level, order (2 x reorder_level - quantity); sorted by SKU. Only
items at or below a positive reorder level are listed. With options["unit"], the rows of items of that unit's
dimension show quantity, reorder level and order converted into it (each on its own).
#@E
"""
from ..util.qty import ZERO, qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

#@B
COLUMNS = ["sku", "name", "quantity", "reorder_level", "order"]
#@A
COLUMNS = ["sku", "name", "quantity", "unit", "reorder_level", "order"]
#@E


def suggested_order(item):
    return max(ZERO, item.reorder_level * 2 - item.quantity)


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
#@E
    rows = []
    for item in inv.sorted_items():
        if item.reorder_level <= 0 or not item.is_low():
            continue
        values = (item.quantity, item.reorder_level, suggested_order(item))
#@A
        unit = item.unit
        if target is not None and compatible(unit, target):
            values = tuple(convert(v, unit, target) for v in values)
            unit = target
#@E
        row = {"sku": item.sku, "name": item.name}
        row["quantity"] = qty_json(values[0])
#@A
        row["unit"] = unit
#@E
        row["reorder_level"] = qty_json(values[1])
        row["order"] = qty_json(values[2])
        rows.append(row)
#@B
    return Report("reorder", COLUMNS, rows)
#@A
    return Report("reorder", COLUMNS, rows, {"unit": target})
#@E
