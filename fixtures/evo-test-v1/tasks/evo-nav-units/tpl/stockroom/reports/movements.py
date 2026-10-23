"""Report "movements": per SKU, how many movements, how much came in, went out, and the net change.

#@B
Rows: sku, movements, received (sum of positive deltas), issued (sum of the negative ones, as a positive
number), net (received - issued); sorted by SKU. SKUs no longer in the inventory are included.
#@A
Rows: sku, movements, received (sum of positive deltas), unit, issued (sum of the negative ones, as a positive
number), net (received - issued); sorted by SKU. SKUs no longer in the inventory are included.
Every delta is converted from the unit it was recorded in into the row's unit before summing: the item's
current unit, or options["unit"] when the item's unit is of that dimension. A SKU no longer in the inventory
uses the unit of its latest movement. Deltas of another dimension are left out (and not counted).
#@E
"""
from ..util.qty import ZERO, qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

#@B
COLUMNS = ["sku", "movements", "received", "issued", "net"]
#@A
COLUMNS = ["sku", "movements", "received", "unit", "issued", "net"]
#@E


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
    by_sku = {}
    for m in inv.movements:
        by_sku.setdefault(m.sku, []).append(m)
    row_unit = {}
    for sku, moves in by_sku.items():
        unit = inv.items[sku].unit if sku in inv.items else moves[-1].unit
        if target is not None and compatible(unit, target):
            unit = target
        row_unit[sku] = unit
#@E
    groups = {}
    for m in inv.movements:
#@B
        delta = m.delta
#@A
        unit = row_unit[m.sku]
        if not compatible(m.unit, unit):
            continue
        delta = convert(m.delta, m.unit, unit)
#@E
        g = groups.setdefault(m.sku, {"count": 0, "in": ZERO, "out": ZERO})
        g["count"] += 1
        if delta > 0:
            g["in"] += delta
        else:
            g["out"] -= delta
    rows = []
    for sku in sorted(groups):
        g = groups[sku]
        rows.append({"sku": sku, "movements": g["count"],
#@A
                     "unit": row_unit[sku],
#@E
                     "received": qty_json(g["in"]), "issued": qty_json(g["out"]),
                     "net": qty_json(g["in"] - g["out"])})
#@B
    return Report("movements", COLUMNS, rows)
#@A
    return Report("movements", COLUMNS, rows, {"unit": target})
#@E
