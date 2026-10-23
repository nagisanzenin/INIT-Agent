"""Movement rules (ruleset "movements"): each recorded movement on its own."""
from ..models.movement import REASONS
from .problems import Problem
#@A
from ..units import UNITS, compatible
#@E


def check_movement(m, inv) -> list:
    subject = "movement %d" % m.id
    out = []
    if m.reason not in REASONS:
        out.append(Problem(subject, "reason", "unknown reason %r" % m.reason))
    if m.delta == 0:
        out.append(Problem(subject, "delta", "must not be zero"))
#@A
    item = inv.items.get(m.sku)
    if m.unit not in UNITS:
        out.append(Problem(subject, "unit", "unknown unit %r" % m.unit))
    elif item is not None and item.unit in UNITS and not compatible(m.unit, item.unit):
        out.append(Problem(subject, "unit", "%s does not match item unit %s" % (m.unit, item.unit)))
#@E
    return out


def check_movements(inv, config=None) -> list:
    out = []
    for m in inv.movements:
        out.extend(check_movement(m, inv))
    return out
