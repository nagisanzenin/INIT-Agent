"""Undo: reverts an item's latest movement by recording its opposite as an "adjust" (note "undo #<id>")."""
from ..errors import StockError
#@A
from ..units import convert
from ..validate.runner import require_valid
#@E


def undo(inv, sku):
    """Returns the new movement."""
    item = inv.get(sku)
    history = inv.history(sku)
    if not history:
        raise StockError("nothing to undo for %s" % sku)
    last = history[-1]
#@B
    delta = -last.delta
#@A
    delta = -convert(last.delta, last.unit, item.unit)
#@E
    if item.quantity + delta < 0:
        raise StockError("cannot undo movement %d: the stock would go negative" % last.id)
    changed = item.with_changes(quantity=item.quantity + delta)
#@A
    require_valid(changed)
#@E
    inv.put(changed)
    return inv.record(sku, delta, "adjust", "undo #%d" % last.id)
