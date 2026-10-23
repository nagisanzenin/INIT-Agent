"""kit-define, kit-show, assemble, pack-set, packs, reorder-mail."""
from ..api.serializers import item_json, kit_json, pack_json
from ..errors import StockroomError
from ..reports.reorder import suggested_order
from ..reports.render import kit_sheet, reorder_mail
from ..services import kits, packs
from ..util.qty import format_qty
#@A
from ..units import split_quantity
#@E
from .output import out, out_json


def register(sub):
    p = sub.add_parser("kit-define", help="set a kit's components: KIT COMPONENT=QTY ...")
    p.add_argument("kit")
    p.add_argument("lines", nargs="+", metavar="COMPONENT=QTY")
    p.set_defaults(func=cmd_kit_define)

    p = sub.add_parser("kit-show", help="a kit's components and how many could be assembled")
    p.add_argument("kit")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_kit_show)

    p = sub.add_parser("assemble", help="assemble kits from their components")
    p.add_argument("kit")
    p.add_argument("count")
    p.set_defaults(func=cmd_assemble)

    p = sub.add_parser("pack-set", help="record how a supplier sells an item")
    p.add_argument("sku")
    p.add_argument("supplier")
    p.add_argument("qty", help="how much one pack holds")
    p.add_argument("price", help="the price of one pack")
#@A
    p.add_argument("--unit", default=None, help="the unit QTY is given in (default: the item's)")
#@E
    p.set_defaults(func=cmd_pack_set)

    p = sub.add_parser("packs", help="the packs of an item")
    p.add_argument("sku")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_packs)

    p = sub.add_parser("reorder-mail", help="the reorder list with the cheapest packs, as a mail body")
    p.set_defaults(func=cmd_reorder_mail)


def _parse_line(text):
    component, sep, qty = text.partition("=")
    if not sep or not component.strip() or not qty.strip():
        raise StockroomError("bad kit line %r (want COMPONENT=QTY)" % text)
#@B
    return component, qty
#@A
    qty, unit = split_quantity(qty)
    return component, qty, unit
#@E


def cmd_kit_define(args, ctx):
    inv = ctx.load()
    try:
        lines = kits.define_kit(inv, args.kit, [_parse_line(t) for t in args.lines])
    except ValueError as e:
        raise StockroomError(str(e)) from None
    ctx.save(inv)
    out("%s: %d component%s" % (args.kit.upper(), len(lines), "" if len(lines) == 1 else "s"))


def cmd_kit_show(args, ctx):
    inv = ctx.load()
    kit = args.kit.strip().upper()
    lines = kits.lines_of(inv, kit)
    cap = kits.capacity(inv, kit)
    if args.json:
        out_json(kit_json(inv, kit, lines, cap))
    else:
        out(kit_sheet(inv, kit, lines, cap))


def cmd_assemble(args, ctx):
    inv = ctx.load()
    kit = args.kit.strip().upper()
    kits.assemble(inv, kit, args.count)
    ctx.save(inv)
    item = inv.get(kit)
#@B
    out("assembled %s x %s: %s now %s" % (args.count, kit, kit, format_qty(item.quantity)))
#@A
    out("assembled %s x %s: %s now %s %s" % (args.count, kit, kit, format_qty(item.quantity), item.unit))
#@E


def cmd_pack_set(args, ctx):
    inv = ctx.load()
    try:
#@B
        pack = packs.set_pack(inv, args.sku, args.supplier, args.qty, args.price)
#@A
        pack = packs.set_pack(inv, args.sku, args.supplier, args.qty, args.price, unit=args.unit)
#@E
    except ValueError as e:
        raise StockroomError(str(e)) from None
    ctx.save(inv)
    out_json(pack_json(pack))


def cmd_packs(args, ctx):
    inv = ctx.load()
    sku = args.sku.strip().upper()
    item = inv.get(sku)
    found = inv.packs.get(sku, [])
    if args.json:
        out_json({"item": item_json(item), "packs": [pack_json(p) for p in found]})
        return
    for p in found:
#@B
        out("%s  %s per pack  %s" % (p.supplier, format_qty(p.quantity), p.price))
#@A
        out("%s  %s %s per pack  %s" % (p.supplier, format_qty(p.quantity), p.unit, p.price))
#@E
    if not found:
        out("no packs for %s" % sku)


def cmd_reorder_mail(args, ctx):
    inv = ctx.load()
    rows = []
    for item in inv.sorted_items():
        if item.reorder_level <= 0 or not item.is_low():
            continue
        order = suggested_order(item)
        rows.append((item, order, packs.best_pack(inv, item.sku, order)))
    out(reorder_mail(rows))
