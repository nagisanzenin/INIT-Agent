"""Stock operations. Each one changes an item's quantity and records a movement.

Quantities arrive as text or numbers and are parsed with util.qty.parse_qty (ValueError on bad input).
"""
from ..errors import StockError
from ..util.qty import format_qty, parse_qty
#@A
from ..units import convert, normalize_unit
from ..validate.runner import require_valid
#@E


#@B
def receive(inv, sku, qty, note=""):
#@A
def receive(inv, sku, qty, note="", unit=None):
#@E
    """Adds a positive quantity; returns the movement."""
    item = inv.get(sku)
#@B
    qty = parse_qty(qty)
#@A
    qty = _in_item_unit(item, parse_qty(qty), unit)
#@E
    if qty <= 0:
        raise StockError("quantity must be positive")
    changed = item.with_changes(quantity=item.quantity + qty)
#@A
    require_valid(changed)
#@E
    inv.put(changed)
    return inv.record(sku, qty, "receive", note)


#@B
def issue(inv, sku, qty, note=""):
#@A
def issue(inv, sku, qty, note="", unit=None):
#@E
    """Takes a positive quantity out; refuses to go below zero."""
    item = inv.get(sku)
#@B
    qty = parse_qty(qty)
#@A
    qty = _in_item_unit(item, parse_qty(qty), unit)
#@E
    if qty <= 0:
        raise StockError("quantity must be positive")
    if qty > item.quantity:
        raise StockError("insufficient stock for %s: have %s, need %s" % (
            sku, format_qty(item.quantity), format_qty(qty)))
    changed = item.with_changes(quantity=item.quantity - qty)
#@A
    require_valid(changed)
#@E
    inv.put(changed)
    return inv.record(sku, -qty, "issue", note)


#@B
def adjust(inv, sku, new_qty, note=""):
#@A
def adjust(inv, sku, new_qty, note="", unit=None):
#@E
    """Sets the quantity after a count; records the difference. Returns None when nothing changed."""
    item = inv.get(sku)
#@B
    new_qty = parse_qty(new_qty)
#@A
    new_qty = _in_item_unit(item, parse_qty(new_qty), unit)
#@E
    if new_qty < 0:
        raise StockError("quantity must not be negative")
    delta = new_qty - item.quantity
    if delta == 0:
        return None
    changed = item.with_changes(quantity=new_qty)
#@A
    require_valid(changed)
#@E
    inv.put(changed)
    return inv.record(sku, delta, "adjust", note)
#@A


def _in_item_unit(item, qty, unit):
    """qty given in `unit` (the item's own unit when None or blank), expressed in the item's unit."""
    if unit is None or not str(unit).strip():
        return qty
    return convert(qty, normalize_unit(unit), item.unit)
#@E
