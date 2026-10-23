"""Moving stock from one store to another.

The quantity leaves the source as an "issue" (note "transfer to <target>") and arrives in the target as a
"receive" (note "transfer from <source>"). A SKU the target does not have is created there with the source
item's fields and no stock first. Both inventories are changed in memory; the caller saves both.
"""
from ..errors import StockError
from ..util.qty import format_qty, parse_qty
#@A
from ..units import convert, normalize_unit
from ..validate.runner import require_valid
#@E


#@B
def transfer(src, dst, sku, qty, src_name, dst_name):
#@A
def transfer(src, dst, sku, qty, src_name, dst_name, unit=None):
#@E
    """Returns (source movement, target movement)."""
    item = src.get(sku)
#@B
    qty = parse_qty(qty)
#@A
    qty = parse_qty(qty)
    if unit is not None and str(unit).strip():
        qty = convert(qty, normalize_unit(unit), item.unit)
#@E
    if qty <= 0:
        raise StockError("quantity must be positive")
    if qty > item.quantity:
        raise StockError("insufficient stock for %s: have %s, need %s" % (
            sku, format_qty(item.quantity), format_qty(qty)))
    target = dst.items.get(sku)
    if target is None:
        target = item.with_changes(quantity=item.quantity - item.quantity)
#@B
    incoming = qty
#@A
    incoming = convert(qty, item.unit, target.unit)
#@E
    left = item.with_changes(quantity=item.quantity - qty)
    arrived = target.with_changes(quantity=target.quantity + incoming)
#@A
    require_valid(left)
    require_valid(arrived)
#@E
    src.put(left)
    out_move = src.record(sku, -qty, "issue", "transfer to %s" % dst_name)
    dst.put(arrived)
    in_move = dst.record(sku, incoming, "receive", "transfer from %s" % src_name)
    return out_move, in_move
