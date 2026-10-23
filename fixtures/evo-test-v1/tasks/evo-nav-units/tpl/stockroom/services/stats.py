"""Summary numbers of an inventory (`stockroom stats`, GET /stats)."""
from ..util.money import format_money, sum_money


def inventory_stats(inv) -> dict:
    """{"items", "categories", "locations", "movements", "value"} -- counts, and the total stock value."""
    stats = {
        "items": len(inv.items),
        "categories": len(inv.categories()),
        "locations": len({i.location for i in inv.items.values() if i.location}),
        "movements": len(inv.movements),
        "value": format_money(sum_money(i.value() for i in inv.items.values())),
    }
#@A
    units = {}
    for item in inv.items.values():
        units[item.unit] = units.get(item.unit, 0) + 1
    stats["units"] = {code: units[code] for code in sorted(units)}
#@E
    return stats


def category_counts(inv) -> dict:
    counts = {}
    for item in inv.items.values():
        counts[item.category] = counts.get(item.category, 0) + 1
    return {c: counts[c] for c in sorted(counts)}
