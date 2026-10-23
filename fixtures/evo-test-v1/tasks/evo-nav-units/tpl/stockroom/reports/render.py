"""Text outputs: tables of items and movements (list, history), a kit sheet (kit-show) and the reorder mail."""
from ..util.money import format_money
from ..util.qty import format_qty, signed_qty
from ..util.text import table

#@B
ITEM_COLUMNS = (
    ("SKU", lambda i: i.sku),
    ("NAME", lambda i: i.name),
    ("CATEGORY", lambda i: i.category),
    ("QTY", lambda i: format_qty(i.quantity)),
    ("REORDER", lambda i: format_qty(i.reorder_level)),
    ("LOCATION", lambda i: i.location or "-"),
    ("COST", lambda i: format_money(i.unit_cost)),
)
#@A
ITEM_COLUMNS = (
    ("SKU", lambda i: i.sku),
    ("NAME", lambda i: i.name),
    ("CATEGORY", lambda i: i.category),
    ("QTY", lambda i: "%s %s" % (format_qty(i.quantity), i.unit)),
    ("REORDER", lambda i: "%s %s" % (format_qty(i.reorder_level), i.unit)),
    ("LOCATION", lambda i: i.location or "-"),
    ("COST", lambda i: format_money(i.unit_cost)),
)
#@E

MOVEMENT_COLUMNS = (
    ("ID", lambda m: str(m.id)),
    ("REASON", lambda m: m.reason),
#@B
    ("DELTA", lambda m: signed_qty(m.delta)),
#@A
    ("DELTA", lambda m: "%s %s" % (signed_qty(m.delta), m.unit)),
#@E
    ("NOTE", lambda m: m.note or "-"),
)


def item_table(items) -> str:
    return table([c[0] for c in ITEM_COLUMNS], [[get(i) for _, get in ITEM_COLUMNS] for i in items])


def movement_table(movements) -> str:
    return table([c[0] for c in MOVEMENT_COLUMNS], [[get(m) for _, get in MOVEMENT_COLUMNS] for m in movements])


def kit_sheet(inv, kit, lines, cap) -> str:
    """    KIT-0001 Bread kit: can assemble 3
      FLR-0001 Flour: 0.5
      YST-0002 Yeast: 7
    """
    item = inv.items.get(kit)
    out = ["%s %s: can assemble %d" % (kit, item.name if item else "?", cap)]
    for line in lines:
        comp = inv.items.get(line.component)
#@B
        out.append("  %s %s: %s" % (line.component, comp.name if comp else "?", format_qty(line.quantity)))
#@A
        out.append("  %s %s: %s %s" % (line.component, comp.name if comp else "?", format_qty(line.quantity),
                                       line.unit))
#@E
    return "\n".join(out) + "\n"


def reorder_mail(rows) -> str:
    """rows: (item, order quantity, best pack or None) for every item to reorder, by SKU.

    Reorder list (2 items)
    CHK-0002 Chickpeas: order 7.5 (have 2.5, reorder level 5)
      cheapest: 1 x Pulse Co pack of 10 = 32.00
    LNT-0007 Lentils: order 2.75 (have 1.25, reorder level 2)
      no pack on file
    """
    out = ["Reorder list (%d item%s)" % (len(rows), "" if len(rows) == 1 else "s")]
    for item, order, best in rows:
#@B
        out.append("%s %s: order %s (have %s, reorder level %s)" % (
            item.sku, item.name, format_qty(order), format_qty(item.quantity), format_qty(item.reorder_level)))
#@A
        u = item.unit
        out.append("%s %s: order %s %s (have %s %s, reorder level %s %s)" % (
            item.sku, item.name, format_qty(order), u, format_qty(item.quantity), u, format_qty(item.reorder_level), u))
#@E
        if best is None:
            out.append("  no pack on file")
        else:
            pack, n, cost = best
#@B
            out.append("  cheapest: %d x %s pack of %s = %s" % (n, pack.supplier, format_qty(pack.quantity),
                                                                format_money(cost)))
#@A
            out.append("  cheapest: %d x %s pack of %s %s = %s" % (n, pack.supplier, format_qty(pack.quantity),
                                                                   pack.unit, format_money(cost)))
#@E
    return "\n".join(out) + "\n"
