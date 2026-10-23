"""The shop exporter: the product CSV the web shop imports (its own header names).

    Handle,Title,Variant SKU,Variant Inventory Qty,Variant Price

Handle is the lower-cased SKU; prices are the unit cost.
"""
import csv
import io

from ..util.money import format_money
from ..util.qty import format_qty

#@B
HEADER = ("Handle", "Title", "Variant SKU", "Variant Inventory Qty", "Variant Price")
#@A
HEADER = ("Handle", "Title", "Variant SKU", "Variant Inventory Qty", "unit", "Variant Price")
#@E


def write_shop(items, options=None) -> str:
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\n")
    writer.writerow(HEADER)
    for item in items:
        row = [item.sku.lower(), item.name, item.sku, format_qty(item.quantity)]
#@A
        row.append(item.unit)
#@E
        row.append(format_money(item.unit_cost))
        writer.writerow(row)
    return buf.getvalue()
