"""Harbor Foods' feed: JSON lines, one object per line:

    {"item": "RCE-0001", "name": "Rice", "count": 12, "uom": "C62", "price": "1.80"}

Blank lines are skipped; the line of an error is the physical line.
"""
import json
import re

from ...models.item import Item
from ...util.money import parse_money
from ...util.qty import parse_qty
from ...util.text import clean
from ..base import ImportResult, norm_sku
#@A
from . import common
#@E

PIECE_RE = re.compile(r"(EA|C62|H87)?", re.I)


def parse_harbor(text, options=None) -> ImportResult:
    result = ImportResult()
    for n, raw in enumerate(text.splitlines(), start=1):
        if not raw.strip():
            continue
        try:
            obj = json.loads(raw)
        except ValueError:
            result.error(n, "not valid JSON")
            continue
        if not isinstance(obj, dict):
            result.error(n, "expected an object")
            continue
        uom = str(obj.get("uom") or "").strip()
        try:
#@B
            if not PIECE_RE.fullmatch(uom):
                raise ValueError("unsupported unit of measure %r" % uom.upper())
#@E
            item = Item(sku=norm_sku(obj.get("item", "")), name=clean(obj.get("name")), category="food",
                        quantity=parse_qty(obj.get("count", 0)), unit_cost=parse_money(obj.get("price", "0")),
#@A
                        unit=common.rec20_unit(uom),
#@E
                        )
        except ValueError as e:
            result.error(n, str(e))
            continue
        result.add(n, item)
    return result
