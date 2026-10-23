"""The ledger exporter: the hand-kept text format the ledger importer reads.

    # stockroom ledger export

    [HEX-0001]
    name = Hex bolt M6
    qty = 120
    category = fasteners
    reorder = 50
    location = A1
    cost = 0.12

One blank line before each record; `location` is left out when empty, every other key is always written.
"""
from ..util.money import format_money
from ..util.qty import format_qty
from .common import header_comment


def write_ledger(items, options=None) -> str:
    out = [header_comment("ledger")]
    for item in items:
        out.append("")
        out.append("[%s]" % item.sku)
        out.append("name = %s" % item.name)
        out.append("qty = %s" % format_qty(item.quantity))
#@A
        out.append("unit = %s" % item.unit)
#@E
        out.append("category = %s" % item.category)
        out.append("reorder = %s" % format_qty(item.reorder_level))
        if item.location:
            out.append("location = %s" % item.location)
        out.append("cost = %s" % format_money(item.unit_cost))
    return "\n".join(out) + "\n"
