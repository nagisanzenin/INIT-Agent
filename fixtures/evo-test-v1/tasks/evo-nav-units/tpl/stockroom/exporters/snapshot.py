"""The snapshot exporter: a compact CSV of what is on hand and what it is worth, for the accountants.

    sku,name,quantity,reorder_level,value
"""
import csv
import io
from dataclasses import astuple

from ..util.money import format_money, round_money
from ..util.qty import format_qty

#@B
HEADER = ("sku", "name", "quantity", "reorder_level", "value")
#@A
HEADER = ("sku", "name", "quantity", "unit", "reorder_level", "value")
#@E


def write_snapshot(items, options=None) -> str:
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\n")
    writer.writerow(HEADER)
#@B
    for sku, name, _category, qty, reorder, _location, cost in map(astuple, items):
        writer.writerow([sku, name, format_qty(qty), format_qty(reorder), format_money(round_money(qty * cost))])
#@A
    for sku, name, _category, qty, reorder, _location, cost, unit in map(astuple, items):
        writer.writerow([sku, name, format_qty(qty), unit, format_qty(reorder), format_money(round_money(qty * cost))])
#@E
    return buf.getvalue()
