"""The inventory table."""


def render(items):
    lines = ["%-8s %-20s %-10s %5s %10s" % ("SKU", "NAME", "CATEGORY", "QTY", "VALUE")]
    for i in sorted(items, key=lambda i: i["sku"]):
        v = i["qty"] * i["price_cents"]
        lines.append("%-8s %-20s %-10s %5d %10s" % (i["sku"], i["name"], i["category"], i["qty"],
                                                    "%d.%02d" % divmod(v, 100)))
    return "\n".join(lines) + "\n"
