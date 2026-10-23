"""Report "slow-movers": items in stock that nothing has been issued from (no issue or assemble movement
taking from them), by value (highest first), then SKU.

#@B
Rows: sku, name, quantity, value.
#@A
Rows: sku, name, quantity, unit, value. With options["unit"], items of its dimension are shown converted.
#@E
"""
from ..util.money import format_money
from ..util.qty import qty_json
#@A
from ..units import compatible, convert, normalize_unit
#@E
from .base import Report

#@B
COLUMNS = ["sku", "name", "quantity", "value"]
#@A
COLUMNS = ["sku", "name", "quantity", "unit", "value"]
#@E


def build(inv, options=None) -> Report:
    options = options or {}
#@A
    target = normalize_unit(options["unit"]) if options.get("unit") else None
#@E
    taken = {m.sku for m in inv.movements if m.delta < 0 and m.reason in ("issue", "assemble")}
    items = [i for i in inv.items.values() if i.quantity > 0 and i.sku not in taken]
    rows = []
    for item in sorted(items, key=lambda i: (-i.value(), i.sku)):
#@B
        qty = item.quantity
#@A
        qty, unit = item.quantity, item.unit
        if target is not None and compatible(unit, target):
            qty, unit = convert(qty, unit, target), target
#@E
        row = {"sku": item.sku, "name": item.name, "quantity": qty_json(qty)}
#@A
        row["unit"] = unit
#@E
        row["value"] = format_money(item.value())
        rows.append(row)
#@B
    return Report("slow-movers", COLUMNS, rows)
#@A
    return Report("slow-movers", COLUMNS, rows, {"unit": target})
#@E
