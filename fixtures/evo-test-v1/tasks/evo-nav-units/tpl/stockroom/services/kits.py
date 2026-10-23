"""Kits: defining a kit's components, assembling kits, and how many could be assembled from stock.

Assembling N kits takes, for every line, N x the line's quantity of the component (an "assemble" movement per
component, note "kit <KIT> x <N>") and adds N to the kit item (an "assemble" movement, same note). It is all or
nothing: when any component is short, nothing changes.
"""
from decimal import Decimal, ROUND_FLOOR

from ..errors import NotFound, StockError, StockroomError
from ..models.kit import KitLine
from ..util.qty import format_qty, parse_qty
#@A
from ..units import convert, normalize_unit
from ..validate.runner import require_valid
#@E


def define_kit(inv, kit, lines) -> list:
    """lines: (component, quantity text[, unit]) tuples. Replaces the kit's lines; returns them."""
    kit = kit.strip().upper()
    inv.get(kit)
    out = []
    seen = set()
    for spec in lines:
        component, qty = spec[0].strip().upper(), parse_qty(spec[1])
        item = inv.get(component)
        if component == kit:
            raise StockroomError("a kit cannot contain itself: %s" % kit)
        if component in seen:
            raise StockroomError("component listed twice: %s" % component)
        if qty <= 0:
            raise StockroomError("kit quantities must be positive: %s" % component)
        seen.add(component)
#@B
        out.append(KitLine(kit=kit, component=component, quantity=qty))
#@A
        unit = normalize_unit(spec[2]) if len(spec) > 2 and spec[2] else item.unit
        convert(qty, unit, item.unit)  # the line's unit must be of the component's dimension
        out.append(KitLine(kit=kit, component=component, quantity=qty, unit=unit))
#@E
    if not out:
        raise StockroomError("a kit needs at least one component")
    inv.set_kit(kit, out)
    return inv.kits[kit]


def lines_of(inv, kit) -> list:
    kit = kit.strip().upper()
    if kit not in inv.kits:
        raise NotFound("unknown kit: %s" % kit)
    return inv.kits[kit]


def per_kit(inv, line) -> Decimal:
    """How much of the component one kit takes, in the component's own unit."""
#@B
    return line.quantity
#@A
    return convert(line.quantity, line.unit, inv.get(line.component).unit)
#@E


def capacity(inv, kit) -> int:
    """How many kits the components in stock allow (whole kits)."""
    counts = []
    for line in lines_of(inv, kit):
        need = per_kit(inv, line)
        have = inv.get(line.component).quantity
        counts.append(int((have / need).to_integral_value(rounding=ROUND_FLOOR)) if need > 0 else 0)
    return min(counts) if counts else 0


def assemble(inv, kit, count) -> list:
    """Assembles `count` kits; returns the movements recorded."""
    kit = kit.strip().upper()
    try:
        count = int(str(count).strip())
    except ValueError:
        count = 0
    if count <= 0:
        raise StockError("the number of kits must be a positive whole number")
    lines = lines_of(inv, kit)
    kit_item = inv.get(kit)
    plan = []
    for line in lines:
        item = inv.get(line.component)
        need = per_kit(inv, line) * count
        if need > item.quantity:
            raise StockError("insufficient stock for %s: have %s, need %s" % (
                item.sku, format_qty(item.quantity), format_qty(need)))
        plan.append((item.with_changes(quantity=item.quantity - need), -need))
    built = kit_item.with_changes(quantity=kit_item.quantity + count)
#@A
    for changed, _ in plan:
        require_valid(changed)
    require_valid(built)
#@E
    note = "kit %s x %d" % (kit, count)
    moves = []
    for changed, delta in plan:
        inv.put(changed)
        moves.append(inv.record(changed.sku, delta, "assemble", note))
    inv.put(built)
    moves.append(inv.record(kit, Decimal(count), "assemble", note))
    return moves
