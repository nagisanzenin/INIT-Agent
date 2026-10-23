"""ImportResult and the small normalizers every importer shares."""
from ..models.item import SKU_RE
from ..util.text import clean


class ImportResult:
    """(line, Item) rows in input order, and (line, message) errors. A row with an error is not in `rows`."""

    def __init__(self, source=""):
        self.source = source
        self.rows = []
        self.errors = []

    def add(self, line, item) -> None:
        self.rows.append((line, item))

    def error(self, line, message) -> None:
        self.errors.append((line, message))

    @property
    def items(self) -> list:
        return [item for _, item in self.rows]

    def ok(self) -> bool:
        return not self.errors

    def error_lines(self, name) -> list:
        """"<name>:<line>: <message>" for every error, by line."""
        return ["%s:%d: %s" % (name, line, msg) for line, msg in sorted(self.errors, key=lambda e: e[0])]


def norm_sku(value) -> str:
    """Upper-cased and checked against the SKU pattern; raises ValueError("bad sku 'x'")."""
    sku = clean(value).upper()
    if not SKU_RE.match(sku):
        raise ValueError("bad sku %r" % clean(value))
    return sku


def norm_location(value) -> str:
    return clean(value).upper()


def norm_category(value, default) -> str:
    return clean(value).lower() or default
