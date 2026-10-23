"""Iris Office Supplies' feed: fixed-width text, no header, one item per line:

    columns  1-8   SKU
            10-29  name
            31-38  quantity (right-aligned)
            40-42  Rec 20 unit code (blank = piece)

Lines starting with '*' are comments.
"""
from ...models.item import Item
from ...util.qty import parse_qty
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common


def fields(line):
    return line[0:8], line[9:29], line[30:38], line[39:42]


def parse_iris(text, options=None) -> ImportResult:
    result = ImportResult()
    for n, raw in enumerate(text.splitlines(), start=1):
        if not raw.strip() or raw.startswith("*"):
            continue
        sku, name, qty, uom = fields(raw.ljust(42))
        uom = uom.strip().upper()
        try:
#@B
            if uom not in ("", "EA") and uom not in common.COUNT_CODES:
                raise ValueError("unsupported unit of measure %r" % uom)
#@E
            item = Item(sku=norm_sku(sku), name=clean(name), category="office", quantity=parse_qty(qty.strip()),
#@A
                        unit=common.rec20_unit(uom),
#@E
                        )
        except ValueError as e:
            result.error(n, str(e))
            continue
        result.add(n, item)
    return result
