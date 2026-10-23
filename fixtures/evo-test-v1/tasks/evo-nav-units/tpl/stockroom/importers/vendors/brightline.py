"""Brightline Foods' feed: semicolon-separated, decimal commas ("2,5", "1.250,75"), prices in euro, header:

    article;label;amount;measure;price_eur

Brightline only ever sends food, so every item lands in the "food" category.
"""
from ...models.item import Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common

COLUMNS = {"article": "sku", "label": "name", "amount": "quantity", "measure": "measure", "price_eur": "price"}


def parse_brightline(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ";", COLUMNS):
        measure = (rec.get("measure") or "").strip().upper()
        try:
#@B
            if measure not in ("", "EA", "C62", "H87"):
                raise ValueError("unsupported unit of measure %r" % measure)
#@A
            unit = common.rec20_unit(rec.get("measure"))
#@E
            price = (rec.get("price") or "0").strip().replace(".", "").replace(",", ".")
            item = Item(
                sku=norm_sku(rec.get("sku")),
                name=clean(rec.get("name")),
                category="food",
                quantity=common.vendor_number(rec.get("quantity"), decimal_comma=True),
                unit_cost=parse_money(price or "0"),
#@A
                unit=unit,
#@E
            )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
