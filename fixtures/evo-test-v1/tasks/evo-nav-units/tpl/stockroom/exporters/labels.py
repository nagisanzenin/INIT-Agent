"""The labels exporter: shelf labels for the label printer, one block per item, a blank line between:

    [ BOL-0001 ] Hex bolt
    qty: 120   loc: A1
"""
from ..util.qty import format_qty


def write_labels(items, options=None) -> str:
    blocks = []
    for item in items:
#@B
        qty = format_qty(item.quantity)
#@A
        qty = "%s %s" % (format_qty(item.quantity), item.unit)
#@E
        blocks.append("[ %s ] %s\nqty: %s   loc: %s" % (item.sku, item.name, qty, item.location or "-"))
    return "\n\n".join(blocks) + ("\n" if blocks else "")
