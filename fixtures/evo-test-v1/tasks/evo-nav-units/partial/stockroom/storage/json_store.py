"""The JSON file store: one document {"schema": N, "items": [...], "movements": [...], "kits": [...],
"packs": [...]}.

Quantities and amounts are kept as decimal strings ("2.5", "12.50") so nothing is lost to floats. Items are
written sorted by SKU, movements in id order, with two-space indentation and a final newline.
"""
import json
import os

from ..errors import DuplicateError, StorageError
from ..models.inventory import Inventory
from ..models.item import DEFAULT_CATEGORY, Item
from ..models.kit import KitLine
from ..models.movement import Movement
from ..models.pack import Pack
from ..util.money import format_money, parse_money
from ..util.qty import format_qty, parse_qty
from . import migrate, schema
from .base import Store, atomic_write, read_text


class JsonStore(Store):
    kind = "json"

    def exists(self) -> bool:
        return os.path.isfile(self.path)

    def load(self) -> Inventory:
        if not self.exists():
            raise StorageError("no store at %s (run `stockroom init`)" % self.path)
        try:
            data = json.loads(read_text(self.path, "inventory.json"))
        except ValueError as e:
            raise StorageError("inventory.json: not valid JSON (%s)" % e)
        if not isinstance(data, dict):
            raise StorageError("inventory.json: the document must be an object")
        version = schema.check_version("inventory.json", data.get("schema", 0))
        data = migrate.upgrade("inventory.json", version, data)
        inv = Inventory()
        for obj in data.get("items") or []:
            try:
                inv.add(_item_from_json(obj))
            except DuplicateError as e:
                raise StorageError("inventory.json: %s" % e)
        for obj in data.get("movements") or []:
            inv.movements.append(_movement_from_json(obj))
        kits = {}
        for obj in data.get("kits") or []:
            kl = _record(KitLine, obj, "kit line")
            kits.setdefault(kl.kit, []).append(kl)
        for kit, lines in kits.items():
            inv.set_kit(kit, lines)
        for obj in data.get("packs") or []:
            inv.set_pack(_record(Pack, obj, "pack"))
        return inv

    def save(self, inv: Inventory) -> None:
        data = {
            "schema": schema.INVENTORY_JSON,
            "items": [_item_to_json(i) for i in inv.sorted_items()],
            "movements": [_movement_to_json(m) for m in inv.movements],
            "kits": [_kit_line_to_json(kl) for kl in inv.kit_lines()],
            "packs": [_pack_to_json(p) for p in inv.pack_list()],
        }
        atomic_write(self.path, json.dumps(data, indent=2) + "\n")


def _item_to_json(item) -> dict:
    return {
        "sku": item.sku,
        "name": item.name,
        "category": item.category,
        "quantity": format_qty(item.quantity),
        "unit": item.unit,
        "reorder_level": format_qty(item.reorder_level),
        "location": item.location,
        "unit_cost": format_money(item.unit_cost),
    }


def _item_from_json(obj) -> Item:
    if not isinstance(obj, dict) or not obj.get("sku"):
        raise StorageError("inventory.json: every item needs a sku")
    try:
        return Item(
            sku=str(obj["sku"]).strip().upper(),
            name=str(obj.get("name") or ""),
            category=str(obj.get("category") or DEFAULT_CATEGORY),
            quantity=parse_qty(obj.get("quantity", "0")),
            reorder_level=parse_qty(obj.get("reorder_level", "0")),
            location=str(obj.get("location") or "").upper(),
            unit_cost=parse_money(obj.get("unit_cost", "0")),
            unit=str(obj.get("unit") or "").strip().lower() or "ea",
        )
    except ValueError as e:
        raise StorageError("inventory.json: item %s: %s" % (obj.get("sku"), e))


def _movement_to_json(m) -> dict:
    out = {"id": m.id, "sku": m.sku, "delta": format_qty(m.delta)}
    out["unit"] = m.unit
    out["reason"] = m.reason
    out["note"] = m.note
    return out


def _movement_from_json(obj) -> Movement:
    if not isinstance(obj, dict):
        raise StorageError("inventory.json: every movement must be an object")
    try:
        return Movement(
            id=int(obj["id"]),
            sku=str(obj.get("sku", "")).upper(),
            delta=parse_qty(obj.get("delta", "0")),
            reason=str(obj.get("reason", "")),
            note=str(obj.get("note") or ""),
            unit=str(obj.get("unit") or "").strip().lower() or "ea",
        )
    except (KeyError, TypeError, ValueError) as e:
        raise StorageError("inventory.json: bad movement %r: %s" % (obj.get("id"), e))


def _kit_line_to_json(kl) -> dict:
    out = {"kit": kl.kit, "component": kl.component, "quantity": format_qty(kl.quantity)}
    out["unit"] = kl.unit
    return out


def _pack_to_json(p) -> dict:
    out = {"sku": p.sku, "supplier": p.supplier, "quantity": format_qty(p.quantity)}
    out["unit"] = p.unit
    out["price"] = format_money(p.price)
    return out


def _record(cls, obj, what):
    if not isinstance(obj, dict):
        raise StorageError("inventory.json: every %s must be an object" % what)
    try:
        return cls.from_record(obj)
    except ValueError as e:
        raise StorageError("inventory.json: bad %s: %s" % (what, e))
