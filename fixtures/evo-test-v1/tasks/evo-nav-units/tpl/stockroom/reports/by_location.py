"""Report "by-location": what each location holds -- one row per item with a location, sorted by location
then SKU; items without a location are left out.

#@B
Rows: location, sku, name, quantity.
#@A
Rows: location, sku, name, quantity, unit. With options["unit"], items of its dimension are shown converted.
#@E
"""
from ..util.qty import qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

#@B
COLUMNS = ["location", "sku", "name", "quantity"]
#@A
COLUMNS = ["location", "sku", "name", "quantity", "unit"]
#@E


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
#@E
    rows = []
    for item in sorted((i for i in inv.items.values() if i.location), key=lambda i: (i.location, i.sku)):
        row = {"location": item.location, "sku": item.sku, "name": item.name, "quantity": qty_json(item.quantity)}
#@A
        row["unit"] = item.unit
        if target is not None and compatible(item.unit, target):
            row["quantity"], row["unit"] = qty_json(convert(item.quantity, item.unit, target)), target
#@E
        rows.append(row)
#@B
    return Report("by-location", COLUMNS, rows)
#@A
    return Report("by-location", COLUMNS, rows, {"unit": target})
#@E
