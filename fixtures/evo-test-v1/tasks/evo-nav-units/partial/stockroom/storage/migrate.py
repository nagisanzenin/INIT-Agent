"""Upgrades data read from older files to the current schema, one version step at a time.

STEPS maps (file kind, version) to a function returning the data at version + 1. For inventory.json the data
is the whole decoded document; for the CSV files it is one row as a dict (column -> text).
"""
from ..errors import StorageError
from .schema import CURRENT


def _json_v0_to_v1(data):
    """v0 (stockroom < 1.2): no "schema" key, items keyed by SKU, "min" and "cost" for the reorder level and
    the unit cost."""
    items = []
    raw = data.get("items") or {}
    if not isinstance(raw, dict):
        raise StorageError("inventory.json: schema 0 items must be an object")
    for sku in sorted(raw):
        rec = dict(raw[sku])
        rec["sku"] = sku
        if "min" in rec:
            rec["reorder_level"] = rec.pop("min")
        if "cost" in rec:
            rec["unit_cost"] = rec.pop("cost")
        items.append(rec)
    return {"schema": 1, "items": items, "movements": list(data.get("movements") or []), "kits": [], "packs": []}


def _json_v1_to_v2(data):
    """v2 adds the unit of every item, movement, kit line and pack; everything older was counted in ea."""
    out = {"schema": 2}
    for key in ("items", "movements", "kits", "packs"):
        out[key] = [dict(rec, unit=rec.get("unit") or "ea") for rec in data.get(key) or []]
    return out


def _csv_v1_to_v2(rec):
    """v2 adds the unit column; v1 rows were counted in ea."""
    out = dict(rec)
    out["unit"] = "ea"
    return out


STEPS = {
    ("inventory.json", 0): _json_v0_to_v1,
    ("inventory.json", 1): _json_v1_to_v2,
    ("items.csv", 1): _csv_v1_to_v2,
    ("movements.csv", 1): _csv_v1_to_v2,
    ("kits.csv", 1): _csv_v1_to_v2,
    ("packs.csv", 1): _csv_v1_to_v2,
}


def upgrade(filename, version, data):
    """`data` read from a `filename` file at `version`, upgraded to the current version."""
    while version < CURRENT[filename]:
        step = STEPS.get((filename, version))
        if step is None:
            raise StorageError("%s: no upgrade from schema version %d" % (filename, version))
        data = step(data)
        version += 1
    return data
