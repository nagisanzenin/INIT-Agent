"""Inventory rules (ruleset "inventory"): what only the whole inventory can tell."""
from .problems import Problem


def check_inventory(inv, config=None) -> list:
    out = []
    seen = set()
    for m in inv.movements:
        subject = "movement %d" % m.id
        if m.id in seen:
            out.append(Problem(subject, "id", "duplicate id"))
        seen.add(m.id)
        if m.sku not in inv.items:
            out.append(Problem(subject, "sku", "unknown sku %s" % m.sku))
    by_location = {}
    for item in inv.sorted_items():
        if item.location:
            by_location.setdefault(item.location, []).append(item.sku)
    limit = ((config or {}).get("validate") or {}).get("max_skus_per_location")
    if limit:
        for loc in sorted(by_location):
            if len(by_location[loc]) > limit:
                out.append(Problem("location %s" % loc, None, "holds %d SKUs (limit %d)" % (
                    len(by_location[loc]), limit)))
    return out
