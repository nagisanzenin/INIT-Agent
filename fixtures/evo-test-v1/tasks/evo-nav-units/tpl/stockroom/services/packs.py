"""Supplier packs, and the cheapest way to order a quantity of an item.

best_pack: for every pack of the item, the number of packs needed is the quantity wanted divided by what one
pack holds, rounded up; the cost is that number times the pack price. The cheapest pack wins (ties: the
supplier name, alphabetically).
"""
from decimal import Decimal, ROUND_CEILING

from ..errors import StockroomError
from ..models.pack import Pack
from ..util.money import parse_money, round_money
from ..util.qty import parse_qty
from ..util.text import clean
#@A
from ..units import compatible, convert, normalize_unit
#@E


#@B
def set_pack(inv, sku, supplier, quantity, price) -> Pack:
#@A
def set_pack(inv, sku, supplier, quantity, price, unit=None) -> Pack:
#@E
    item = inv.get(sku.strip().upper())
    supplier = clean(supplier)
    if not supplier:
        raise StockroomError("a pack needs a supplier")
    qty = parse_qty(quantity)
    if qty <= 0:
        raise StockroomError("a pack must hold a positive quantity")
#@B
    pack = Pack(sku=item.sku, supplier=supplier, quantity=qty, price=parse_money(price))
#@A
    unit = normalize_unit(unit) if unit else item.unit
    convert(qty, unit, item.unit)  # the pack's unit must be of the item's dimension
    pack = Pack(sku=item.sku, supplier=supplier, quantity=qty, price=parse_money(price), unit=unit)
#@E
    inv.set_pack(pack)
    return pack


def pack_size(inv, pack) -> Decimal:
    """What one pack holds, in the item's own unit."""
#@B
    return pack.quantity
#@A
    return convert(pack.quantity, pack.unit, inv.get(pack.sku).unit)
#@E


def best_pack(inv, sku, wanted):
    """(pack, number of packs, cost) of the cheapest way to get at least `wanted` (in the item's unit), or None
    when the item has no usable pack."""
    best = None
    item = inv.get(sku)
    for pack in inv.packs.get(sku, []):
#@A
        if not compatible(pack.unit, item.unit):
            continue
#@E
        size = pack_size(inv, pack)
        if size <= 0:
            continue
        n = int((wanted / size).to_integral_value(rounding=ROUND_CEILING))
        cost = round_money(pack.price * n)
        key = (cost, pack.supplier)
        if best is None or key < best[0]:
            best = (key, pack, n, cost)
    if best is None:
        return None
    return best[1], best[2], best[3]
