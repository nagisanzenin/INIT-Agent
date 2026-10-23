"""Report "totals": per category, how many items, how much stock and what it is worth.

#@B
Rows: category, items, quantity (sum of quantities), value (sum of item values); sorted by category.
#@A
Without a unit: one row per (category, unit) pair -- quantities are only summed within one unit.
With options["unit"]: only items of that unit's dimension, each converted into it before summing; one row per
category; the others are counted in "skipped". Values always come from the stored quantity and unit cost.
#@E
"""
from ..util.money import format_money, sum_money
from ..util.qty import ZERO, qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report


def build(inv, options=None) -> Report:
    options = options or {}
#@B
    groups = {}
    for item in inv.sorted_items():
        g = groups.setdefault(item.category, {"items": 0, "quantity": ZERO, "values": []})
        g["items"] += 1
        g["quantity"] += item.quantity
        g["values"].append(item.value())
    rows = []
    for category in sorted(groups):
        g = groups[category]
        rows.append({"category": category, "items": g["items"], "quantity": qty_json(g["quantity"]),
                     "value": format_money(sum_money(g["values"]))})
    return Report("totals", ["category", "items", "quantity", "value"], rows)
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
    groups, skipped = {}, 0
    for item in inv.sorted_items():
        if target is None:
            key, qty = (item.category, item.unit), item.quantity
        elif compatible(item.unit, target):
            key, qty = (item.category, target), convert(item.quantity, item.unit, target)
        else:
            skipped += 1
            continue
        g = groups.setdefault(key, {"items": 0, "quantity": ZERO, "values": []})
        g["items"] += 1
        g["quantity"] += qty
        g["values"].append(item.value())
    rows = []
    for category, unit in sorted(groups):
        g = groups[(category, unit)]
        rows.append({"category": category, "items": g["items"], "quantity": qty_json(g["quantity"]), "unit": unit,
                     "value": format_money(sum_money(g["values"]))})
    return Report("totals", ["category", "items", "quantity", "unit", "value"], rows,
                  {"unit": target, "skipped": skipped})
#@E
