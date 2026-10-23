"""Elm Wholesale's nightly feed: comma-separated, lower-case header:

    sku,desc,on_hand,uom_code,cost
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common

COLUMNS = {"sku": "sku", "desc": "name", "on_hand": "quantity", "uom_code": "uom", "cost": "cost"}


def parse_elm(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ",", COLUMNS):
        try:
#@B
            common.check_count_uom(rec.get("uom"))
            unit_args = {}
#@A
            unit_args = {"unit": common.rec20_unit(rec.get("uom"))}
#@E
            result.add(line, Item(sku=norm_sku(rec.get("sku")), name=clean(rec.get("name")), category=DEFAULT_CATEGORY,
                                  quantity=common.vendor_number(rec.get("quantity")),
                                  unit_cost=parse_money(rec.get("cost") or "0"), **unit_args))
        except ValueError as e:
            result.error(line, str(e))
    return result
