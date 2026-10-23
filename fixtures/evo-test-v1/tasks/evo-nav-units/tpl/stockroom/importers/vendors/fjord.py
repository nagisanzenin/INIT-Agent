"""Fjord Handel's feed (Norwegian/German export tool): semicolon-separated, decimal commas, header:

    Artikel;Bezeichnung;Menge;Einheit;Preis
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common

COLUMNS = {"artikel": "sku", "bezeichnung": "name", "menge": "quantity", "einheit": "einheit", "preis": "preis"}
ALLOWED_EINHEIT = {"", "EA", "C62", "H87"}


def _price(text):
    return parse_money((text or "0").strip().replace(".", "").replace(",", ".") or "0")


def parse_fjord(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ";", COLUMNS):
        einheit = (rec.get("einheit") or "").strip()
#@B
        if einheit.upper() not in ALLOWED_EINHEIT:
            result.error(line, "unsupported unit of measure %r" % einheit.upper())
            continue
#@E
        try:
            item = Item(sku=norm_sku(rec.get("sku")), name=clean(rec.get("name")), category=DEFAULT_CATEGORY,
                        quantity=common.vendor_number(rec.get("quantity"), decimal_comma=True),
                        unit_cost=_price(rec.get("preis")))
#@A
            item.unit = common.rec20_unit(einheit)
#@E
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
