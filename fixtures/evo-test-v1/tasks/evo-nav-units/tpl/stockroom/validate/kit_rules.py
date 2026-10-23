"""Kit rules (ruleset "kits"): every kit names an existing item and existing components in positive
quantities."""
from .problems import Problem
#@A
from ..units import UNITS, compatible
#@E


def check_kits(inv, config=None) -> list:
    out = []
    for kit in sorted(inv.kits):
        if kit not in inv.items:
            out.append(Problem("kit %s" % kit, None, "unknown kit item"))
        for line in inv.kits[kit]:
            subject = "kit %s line %s" % (kit, line.component)
            item = inv.items.get(line.component)
            if item is None:
                out.append(Problem(subject, "component", "unknown sku %s" % line.component))
                continue
            if line.quantity <= 0:
                out.append(Problem(subject, "quantity", "must be positive"))
#@A
            if line.unit not in UNITS:
                out.append(Problem(subject, "unit", "unknown unit %r" % line.unit))
            elif item.unit in UNITS and not compatible(line.unit, item.unit):
                out.append(Problem(subject, "unit", "%s does not match item unit %s" % (line.unit, item.unit)))
#@E
    return out
