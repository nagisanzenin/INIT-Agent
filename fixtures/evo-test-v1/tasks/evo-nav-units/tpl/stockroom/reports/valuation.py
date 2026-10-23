"""Report "valuation": stock value per location (items without a location are grouped under "-"), sorted by
location, with the grand total in "total"."""
from ..util.money import format_money, sum_money
from .base import Report


def build(inv, options=None) -> Report:
    groups = {}
    for item in inv.sorted_items():
        g = groups.setdefault(item.location or "-", {"items": 0, "values": []})
        g["items"] += 1
        g["values"].append(item.value())
    rows = []
    for loc in sorted(groups):
        g = groups[loc]
        rows.append({"location": loc, "items": g["items"], "value": format_money(sum_money(g["values"]))})
    total = sum_money(item.value() for item in inv.items.values())
    return Report("valuation", ["location", "items", "value"], rows, {"total": format_money(total)})
