"""The csv exporter, and the tab-separated picking list (registered as "tsv").

csv: a header row of COLUMNS, then one row per item in the text form of Item.to_record(). Option: delimiter.
tsv: the floor's picking list -- PICK_COLUMNS, sorted by location then SKU, no prices; an item without a
location is listed under "-".
"""
import csv
import io

from ..util.qty import format_qty

#@B
COLUMNS = ("sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost")
#@A
COLUMNS = ("sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost")
#@E

#@B
PICK_COLUMNS = ("LOCATION", "SKU", "NAME", "QTY")
#@A
PICK_COLUMNS = ("LOCATION", "SKU", "NAME", "QTY", "UNIT")
#@E


def write_csv(items, options=None) -> str:
    options = options or {}
    buf = io.StringIO()
    writer = csv.writer(buf, delimiter=options.get("delimiter", ","), lineterminator="\n")
    writer.writerow(COLUMNS)
    for item in items:
        rec = item.to_record()
        writer.writerow([rec[c] for c in COLUMNS])
    return buf.getvalue()


def write_tsv(items, options=None) -> str:
    lines = ["\t".join(PICK_COLUMNS)]
    for item in sorted(items, key=lambda i: (i.location, i.sku)):
        cells = [item.location or "-", item.sku, item.name, format_qty(item.quantity)]
#@A
        cells.append(item.unit)
#@E
        lines.append("\t".join(cells))
    return "\n".join(lines) + "\n"
