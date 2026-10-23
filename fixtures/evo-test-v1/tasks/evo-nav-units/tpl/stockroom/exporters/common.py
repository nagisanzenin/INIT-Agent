"""What every export path does before calling an exporter."""


def select(items, category=None) -> list:
    """Items of `category` (case-insensitive; all when None), sorted by SKU."""
    out = [i for i in items if category is None or i.category == category.strip().lower()]
    return sorted(out, key=lambda i: i.sku)


def header_comment(title) -> str:
    return "# stockroom %s export" % title
