"""Cargo & Co's warehouse feed: tab-separated, header row in capitals:

    SKU	NAME	QTY	UNIT_CODE	COST	CATEGORY
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_category, norm_sku
from . import common

COLUMNS = {"sku": "sku", "name": "name", "qty": "quantity", "unit_code": "unit_code", "cost": "cost",
           "category": "category"}


def parse_cargo(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, "\t", COLUMNS):
        try:
#@B
            common.check_count_uom(rec.get("unit_code"))
#@A
            unit = common.rec20_unit(rec.get("unit_code"))
#@E
            item = Item(
                sku=norm_sku(rec.get("sku")),
                name=clean(rec.get("name")),
                category=norm_category(rec.get("category"), DEFAULT_CATEGORY),
                quantity=common.vendor_number(rec.get("quantity")),
                unit_cost=parse_money(rec.get("cost") or "0"),
#@A
                unit=unit,
#@E
            )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
