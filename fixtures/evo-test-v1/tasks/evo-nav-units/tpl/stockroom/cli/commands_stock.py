"""receive, issue, adjust, history, undo, transfer, count."""
import csv
import io

from ..errors import StockroomError
from ..api.serializers import movement_json
from ..reports.render import movement_table
from ..services import counts, stock, transfer, undo
from ..storage.factory import open_store
from ..util.qty import format_qty, signed_qty
from ..util.text import table
from .output import err, out, out_json


#@B
HISTORY_COLUMNS = ("id", "sku", "delta", "reason", "note")
#@A
HISTORY_COLUMNS = ("id", "sku", "delta", "unit", "reason", "note")
#@E


def register(sub):
    for name, helptext in (("receive", "add stock"), ("issue", "take stock out"),
                           ("adjust", "set the counted quantity")):
        p = sub.add_parser(name, help=helptext)
        p.add_argument("sku")
        p.add_argument("qty")
        p.add_argument("--note", default="")
#@A
        p.add_argument("--unit", default=None, help="the unit QTY is given in (default: the item's)")
#@E
        p.set_defaults(func=cmd_stock, op=name)

    p = sub.add_parser("history", help="movements of an item")
    p.add_argument("sku")
    fmt = p.add_mutually_exclusive_group()
    fmt.add_argument("--json", action="store_true")
    fmt.add_argument("--csv", action="store_true")
    p.set_defaults(func=cmd_history)

    p = sub.add_parser("undo", help="revert an item's latest movement")
    p.add_argument("sku")
    p.set_defaults(func=cmd_undo)

    p = sub.add_parser("transfer", help="move stock to another store")
    p.add_argument("sku")
    p.add_argument("qty")
    p.add_argument("--to", required=True, metavar="STORE", help="the target store (directory or .json file)")
#@A
    p.add_argument("--unit", default=None, help="the unit QTY is given in (default: the item's)")
#@E
    p.set_defaults(func=cmd_transfer)

    register_count(sub)


def cmd_stock(args, ctx):
    inv = ctx.load()
    sku = args.sku.upper()
    op = {"receive": stock.receive, "issue": stock.issue, "adjust": stock.adjust}[args.op]
    try:
#@B
        m = op(inv, sku, args.qty, note=args.note)
#@A
        m = op(inv, sku, args.qty, note=args.note, unit=args.unit)
#@E
    except ValueError as e:
        raise StockroomError(str(e)) from None
    item = inv.get(sku)
    if m is None:
#@B
        out("%s: %s (unchanged)" % (sku, format_qty(item.quantity)))
#@A
        out("%s: %s %s (unchanged)" % (sku, format_qty(item.quantity), item.unit))
#@E
        return 0
    ctx.save(inv)
#@B
    out("%s: %s (%s)" % (sku, format_qty(item.quantity), signed_qty(m.delta)))
#@A
    out("%s: %s %s (%s %s)" % (sku, format_qty(item.quantity), item.unit, signed_qty(m.delta), item.unit))
#@E
    return 0


def cmd_history(args, ctx):
    inv = ctx.load()
    sku = args.sku.upper()
    inv.get(sku)
    moves = inv.history(sku)
    if args.json:
        out_json([movement_json(m) for m in moves])
    elif args.csv:
        buf = io.StringIO()
        writer = csv.writer(buf, lineterminator="\n")
        writer.writerow(HISTORY_COLUMNS)
        for m in moves:
            rec = m.to_record()
            writer.writerow([rec[c] for c in HISTORY_COLUMNS])
        out(buf.getvalue())
    else:
        out(movement_table(moves))


def cmd_undo(args, ctx):
    inv = ctx.load()
    sku = args.sku.upper()
    m = undo.undo(inv, sku)
    ctx.save(inv)
    item = inv.get(sku)
#@B
    out("%s: %s (%s), undid #%s" % (sku, format_qty(item.quantity), signed_qty(m.delta), m.note.split("#")[-1]))
#@A
    out("%s: %s %s (%s %s), undid #%s" % (sku, format_qty(item.quantity), item.unit, signed_qty(m.delta), item.unit,
                                          m.note.split("#")[-1]))
#@E


def cmd_transfer(args, ctx):
    src = ctx.load()
    target_store = open_store(args.to)
    dst = target_store.load()
    sku = args.sku.upper()
    try:
#@B
        transfer.transfer(src, dst, sku, args.qty, ctx.store.path, args.to)
#@A
        transfer.transfer(src, dst, sku, args.qty, ctx.store.path, args.to, unit=args.unit)
#@E
    except ValueError as e:
        raise StockroomError(str(e)) from None
    ctx.save(src)
    target_store.save(dst)
    here, there = src.get(sku), dst.get(sku)
#@B
    out("%s: %s here, %s at %s" % (sku, format_qty(here.quantity), format_qty(there.quantity), args.to))
#@A
    out("%s: %s %s here, %s %s at %s" % (sku, format_qty(here.quantity), here.unit, format_qty(there.quantity),
                                         there.unit, args.to))
#@E


def register_count(sub):
    p = sub.add_parser("count", help="apply a stock count sheet (CSV: sku,counted[,note])")
    p.add_argument("file")
    p.add_argument("--dry-run", action="store_true", help="show the differences, change nothing")
    p.set_defaults(func=cmd_count)


def cmd_count(args, ctx):
    try:
        with open(args.file, encoding="utf-8", newline="") as f:
            text = f.read()
    except OSError as e:
        raise StockroomError("cannot read %s: %s" % (args.file, e.strerror or e))
    inv = ctx.load()
    rows, errors = counts.read_count_sheet(text)
    changes, apply_errors = counts.apply_counts(inv, rows, dry_run=args.dry_run)
    errors.extend(apply_errors)
    if changes and not args.dry_run:
        ctx.save(inv)
    for line, message in sorted(errors, key=lambda e: e[0]):
        err("%s:%d: %s" % (args.file, line, message))
    table_rows = []
    for sku, before, after, _ in changes:
#@B
        table_rows.append([sku, format_qty(before), format_qty(after), signed_qty(after - before)])
#@A
        unit = inv.get(sku).unit
        table_rows.append([sku, "%s %s" % (format_qty(before), unit), "%s %s" % (format_qty(after), unit),
                           "%s %s" % (signed_qty(after - before), unit)])
#@E
    out(table(["SKU", "BEFORE", "COUNTED", "CHANGE"], table_rows))
    return 1 if errors else 0
