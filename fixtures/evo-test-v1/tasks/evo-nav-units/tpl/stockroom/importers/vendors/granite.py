"""Granite Supply's feed: comma-separated; the stock cell carries the Rec 20 code after the number
("25 KGM", "40 C62", or a bare "40"). Header:

    part,title,stock,price
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common

COLUMNS = {"part": "sku", "title": "name", "stock": "stock", "price": "price"}


def split_stock(cell):
    """"25 KGM" -> ("25", "KGM"); "40" -> ("40", "")."""
    number, _, code = (cell or "").strip().partition(" ")
    return number, code.strip()


def parse_granite(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ",", COLUMNS):
        number, code = split_stock(rec.get("stock"))
        try:
#@B
            common.check_count_uom(code)
#@E
            item = Item(sku=norm_sku(rec.get("sku")), name=clean(rec.get("name")), category=DEFAULT_CATEGORY,
                        quantity=common.vendor_number(number), unit_cost=parse_money(rec.get("price") or "0"),
#@A
                        unit=common.rec20_unit(code),
#@E
                        )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
