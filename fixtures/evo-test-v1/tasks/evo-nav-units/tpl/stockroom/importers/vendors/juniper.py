"""Juniper Packaging's feed: whole packs. Comma-separated, header:

    sku,name,packs,pack_size,pack_uom,cost_per_pack

The quantity is packs x pack_size; the unit cost is cost_per_pack / pack_size, rounded to cents.
"""
from ...models.item import DEFAULT_CATEGORY, Item
from ...util.money import parse_money, round_money
from ...util.qty import parse_qty
from ...util.text import clean
from ..base import ImportResult, norm_sku
from . import common

COLUMNS = {"sku": "sku", "name": "name", "packs": "packs", "pack_size": "pack_size", "pack_uom": "pack_uom",
           "cost_per_pack": "cost"}


def parse_juniper(text, options=None) -> ImportResult:
    result = ImportResult()
    for line, rec in common.read_rows(text, ",", COLUMNS):
        try:
#@B
            common.check_count_uom(rec.get("pack_uom"))
#@A
            unit = common.rec20_unit(rec.get("pack_uom"))
#@E
            packs = parse_qty(rec.get("packs") or "0")
            size = parse_qty(rec.get("pack_size") or "1")
            if size <= 0:
                raise ValueError("bad pack size %r" % rec.get("pack_size"))
            cost = parse_money(rec.get("cost") or "0")
            item = Item(sku=norm_sku(rec.get("sku")), name=clean(rec.get("name")), category=DEFAULT_CATEGORY,
                        quantity=packs * size, unit_cost=round_money(cost / size),
#@A
                        unit=unit,
#@E
                        )
        except ValueError as e:
            result.error(line, str(e))
            continue
        result.add(line, item)
    return result
