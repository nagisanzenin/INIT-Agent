"""Acme Industrial's stock feed: comma-separated, a decimal point, and this header row:

    Item No,Description,Qty,UOM,Unit Price,Bin

"Item No" is our SKU, "Bin" our location.
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_location, norm_sku
from . import common

COLUMNS = {
    "item no": "sku",
    "description": "name",
    "qty": "quantity",
    "uom": "uom",
    "unit price": "unit_cost",
    "bin": "location",
}


def parse_acme(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ",", COLUMNS):
        try:
#@B
            common.check_count_uom(rec.get("uom"))
#@A
            unit = common.rec20_unit(rec.get("uom"))
#@E
            item = Item(
                sku=norm_sku(rec.get("sku")),
                name=clean(rec.get("name")),
                category=DEFAULT_CATEGORY,
                quantity=common.vendor_number(rec.get("quantity")),
                location=norm_location(rec.get("location")),
                unit_cost=parse_money(rec.get("unit_cost") or "0"),
#@A
                unit=unit,
#@E
            )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
