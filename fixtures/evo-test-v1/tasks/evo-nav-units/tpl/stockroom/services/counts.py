"""Stock counts: a count sheet lists what was physically on the shelves; applying it adjusts every listed
item to the counted quantity (reason "adjust", note "count" unless the sheet gives one).

Count sheet: CSV with a header row. Columns (case-insensitive): sku and counted are required, note is
#@B
optional. Blank rows are skipped. Line numbers count the header row as line 1.
#@A
optional, unit is optional (the unit `counted` is given in; blank means the item's own unit). Blank rows are
skipped. Line numbers count the header row as line 1.
#@E
"""
import csv
import io

from ..errors import NotFound, StockError, ValidationError
from ..util.qty import parse_qty
from ..util.text import clean
from . import stock

#@B
SHEET_COLUMNS = ("sku", "counted", "note")
#@A
SHEET_COLUMNS = ("sku", "counted", "note", "unit")
#@E


class CountRow:
    __slots__ = ("line", "sku", "counted", "note", "unit")

    def __init__(self, line, sku, counted, note="", unit=None):
        self.line = line
        self.sku = sku
        self.counted = counted
        self.note = note
        self.unit = unit


def read_count_sheet(text):
    """(rows, errors): CountRow objects and (line, message) pairs."""
    rows, errors = [], []
    reader = csv.reader(io.StringIO(text))
    header = next(reader, None)
    if header is None:
        return rows, [(1, "empty file")]
    names = [clean(h).lower() for h in header]
    missing = [c for c in ("sku", "counted") if c not in names]
    if missing:
        return rows, [(1, "missing column(s): %s" % ", ".join(missing))]
    for raw in reader:
        line = reader.line_num
        if not any(cell.strip() for cell in raw):
            continue
        rec = {n: cell for n, cell in zip(names, raw) if n in SHEET_COLUMNS}
        try:
            counted = parse_qty(rec.get("counted", ""))
        except ValueError as e:
            errors.append((line, str(e)))
            continue
        rows.append(CountRow(line, clean(rec.get("sku")).upper(), counted, clean(rec.get("note")),
                             clean(rec.get("unit")) or None))
    return rows, errors


def apply_counts(inv, rows, dry_run=False):
    """(changes, errors). changes: (sku, quantity before, quantity after, movement or None) for every row that
    applied; with dry_run the inventory is left as it was."""
    changes, errors = [], []
    for row in rows:
        try:
            item = inv.get(row.sku)
            before = item.quantity
            if dry_run:
                probe = _copy(inv, row.sku)
#@B
                stock.adjust(probe, row.sku, row.counted)
#@A
                stock.adjust(probe, row.sku, row.counted, unit=row.unit)
#@E
                changes.append((row.sku, before, probe.get(row.sku).quantity, None))
                continue
#@B
            m = stock.adjust(inv, row.sku, row.counted, note=row.note or "count")
#@A
            m = stock.adjust(inv, row.sku, row.counted, note=row.note or "count", unit=row.unit)
#@E
            changes.append((row.sku, before, inv.get(row.sku).quantity, m))
        except ValidationError as e:
            errors.append((row.line, "%s: %s" % (e.problems[0].field, e.problems[0].message)))
        except (NotFound, StockError, ValueError) as e:
            errors.append((row.line, str(e)))
    return changes, errors


def _copy(inv, sku):
    from ..models.inventory import Inventory

    probe = Inventory()
    probe.put(inv.get(sku))
    return probe
