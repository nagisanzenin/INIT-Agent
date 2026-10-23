"""The JSON shapes of items, movements, kits and packs (the API and the CLI's --json outputs use them)."""
from ..util.money import format_money
from ..util.qty import qty_json


def item_json(item) -> dict:
    out = {
        "sku": item.sku,
        "name": item.name,
        "category": item.category,
        "quantity": qty_json(item.quantity),
    }
#@A
    out["unit"] = item.unit
#@E
    out.update({
        "reorder_level": qty_json(item.reorder_level),
        "location": item.location,
        "unit_cost": format_money(item.unit_cost),
        "value": format_money(item.value()),
    })
    return out


def movement_json(m) -> dict:
    out = {"id": m.id, "sku": m.sku, "delta": qty_json(m.delta)}
#@A
    out["unit"] = m.unit
#@E
    out["reason"] = m.reason
    out["note"] = m.note
    return out


def kit_json(inv, kit, lines, capacity) -> dict:
    item = inv.items.get(kit)
    out_lines = []
    for line in lines:
        comp = inv.items.get(line.component)
        entry = {"component": line.component, "name": comp.name if comp else None, "quantity": qty_json(line.quantity)}
#@A
        entry["unit"] = line.unit
#@E
        out_lines.append(entry)
    return {"kit": kit, "name": item.name if item else None, "capacity": capacity, "lines": out_lines}


def pack_json(p) -> dict:
    out = {"sku": p.sku, "supplier": p.supplier, "quantity": qty_json(p.quantity)}
#@A
    out["unit"] = p.unit
#@E
    out["price"] = format_money(p.price)
    return out
