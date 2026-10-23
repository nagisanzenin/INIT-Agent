"""Report "inbound": what came in per SKU -- the movements with reason receive or import.

#@B
Rows: sku, receipts (how many such movements), quantity (their total); sorted by SKU.
#@A
Rows: sku, receipts (how many such movements), quantity (their total), unit; sorted by SKU. Deltas are
converted from the unit they were recorded in into the row's unit before summing, as in "movements".
#@E
"""
from ..util.qty import ZERO, qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

INBOUND = ("receive", "import")
#@B
COLUMNS = ["sku", "receipts", "quantity"]
#@A
COLUMNS = ["sku", "receipts", "quantity", "unit"]
#@E


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
#@E
    totals = {}
    for m in inv.movements:
        if m.reason not in INBOUND:
            continue
#@B
        t = totals.setdefault(m.sku, [0, ZERO])
        t[0] += 1
        t[1] += m.delta
#@A
        t = totals.setdefault(m.sku, [0, ZERO, []])
        t[2].append(m)
#@E
    rows = []
    for sku in sorted(totals):
        t = totals[sku]
#@A
        unit = inv.items[sku].unit if sku in inv.items else t[2][-1].unit
        if target is not None and compatible(unit, target):
            unit = target
        for m in t[2]:
            if compatible(m.unit, unit):
                t[0] += 1
                t[1] += convert(m.delta, m.unit, unit)
#@E
        row = {"sku": sku, "receipts": t[0], "quantity": qty_json(t[1])}
#@A
        row["unit"] = unit
#@E
        rows.append(row)
#@B
    return Report("inbound", COLUMNS, rows)
#@A
    return Report("inbound", COLUMNS, rows, {"unit": target})
#@E
