"""The markdown exporter: a table for wiki pages.

    | SKU | Name | Qty | Location |
    |---|---|---|---|
    | BOL-0001 | Hex bolt | 120 | A1 |
"""
from ..util.qty import format_qty


def write_markdown(items, options=None) -> str:
    lines = ["| SKU | Name | Qty | Location |", "|---|---|---|---|"]
    for item in items:
#@B
        qty = format_qty(item.quantity)
#@A
        qty = "%s %s" % (format_qty(item.quantity), item.unit)
#@E
        lines.append("| %s | %s | %s | %s |" % (item.sku, item.name.replace("|", "/"), qty, item.location or "-"))
    return "\n".join(lines) + "\n"
