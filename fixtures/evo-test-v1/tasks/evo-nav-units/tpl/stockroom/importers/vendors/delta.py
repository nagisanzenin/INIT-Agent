"""Delta Supply's feed: pipe-separated, with a reorder level per line:

    code|name|quantity|uom|reorder|location
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.qty import parse_qty
from ...util.text import clean
from ..base import ImportResult, norm_location, norm_sku
from . import common

COLUMNS = {"code": "sku", "name": "name", "quantity": "quantity", "uom": "uom", "reorder": "reorder",
           "location": "location"}
PIECE = {"EA", "C62", "H87"}


def parse_delta(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, "|", COLUMNS):
        try:
            uom = (rec.get("uom") or "").strip()
#@B
            if uom and uom.upper() not in PIECE:
                raise ValueError("unsupported unit of measure %r" % uom.upper())
#@A
            unit = common.rec20_unit(uom)
#@E
            item = Item(
                sku=norm_sku(rec.get("sku")),
                name=clean(rec.get("name")),
                category=DEFAULT_CATEGORY,
                quantity=common.vendor_number(rec.get("quantity")),
                reorder_level=parse_qty((rec.get("reorder") or "0").strip() or "0"),
                location=norm_location(rec.get("location")),
#@A
                unit=unit,
#@E
            )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
