"""The csv importer: a header row, then one item per row.

Header names are matched case-insensitively after trimming (inner spaces become underscores); ALIASES maps
the short names spreadsheets use. Required columns: sku and quantity. Unknown columns are ignored, blank rows
skipped. Line numbers count the header row as line 1. Option: delimiter (default ",").
"""
import csv
import io

from ..models.item import DEFAULT_CATEGORY, Item
from ..util.money import parse_money
from ..util.qty import parse_qty
from ..util.text import clean
#@A
from ..units import normalize_unit
#@E
from .base import ImportResult, norm_category, norm_location, norm_sku

ALIASES = {
    "qty": "quantity",
    "reorder": "reorder_level",
    "min": "reorder_level",
    "cost": "unit_cost",
    "price": "unit_cost",
    "loc": "location",
    "bin": "location",
    "description": "name",
#@A
    "uom": "unit",
#@E
}
#@B
KNOWN = ("sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost")
#@A
KNOWN = ("sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost")
#@E
REQUIRED = ("sku", "quantity")


def parse_csv(text, options=None) -> ImportResult:
    options = options or {}
    result = ImportResult()
    reader = csv.reader(io.StringIO(text), delimiter=options.get("delimiter", ","))
    header = next(reader, None)
    if header is None:
        result.error(1, "empty file")
        return result
    columns = [_column(h) for h in header]
    missing = [c for c in REQUIRED if c not in columns]
    if missing:
        result.error(1, "missing column(s): %s" % ", ".join(missing))
        return result
    for row in reader:
        line = reader.line_num
        if not any(cell.strip() for cell in row):
            continue
        rec = {c: cell for c, cell in zip(columns, row) if c in KNOWN}
        try:
            result.add(line, _to_item(rec))
        except ValueError as e:
            result.error(line, str(e))
    return result


def _column(name) -> str:
    key = "_".join(name.strip().lower().split())
    return ALIASES.get(key, key)


def _to_item(rec) -> Item:
    return Item(
        sku=norm_sku(rec.get("sku")),
        name=clean(rec.get("name")),
        category=norm_category(rec.get("category"), DEFAULT_CATEGORY),
        quantity=parse_qty(rec.get("quantity", "")),
        reorder_level=parse_qty(rec.get("reorder_level") or "0"),
        location=norm_location(rec.get("location")),
        unit_cost=parse_money(rec.get("unit_cost") or "0"),
#@A
        unit=normalize_unit(rec.get("unit")),
#@E
    )
