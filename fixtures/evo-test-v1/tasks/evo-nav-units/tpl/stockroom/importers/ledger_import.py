"""The ledger importer: the plain-text format stock clerks keep by hand.

    # comment lines start with '#' or ';'
    [HEX-0001]
    name = Hex bolt M6
    qty = 120
    category = fasteners
    reorder = 50
    location = A1
    cost = 0.12

A record starts at a [SKU] line and runs to the next one; `qty` is required. Keys are case-insensitive. An
unknown key, or a line without "=", is an error at its own line and drops its record; a problem with the
record as a whole (bad SKU, missing qty, a bad value) is an error at the [SKU] line.
"""
import re

from ..models.item import DEFAULT_CATEGORY, Item
from ..util.money import parse_money
from ..util.qty import parse_qty
from ..util.text import clean
#@A
from ..units import normalize_unit
#@E
from .base import ImportResult, norm_category, norm_location, norm_sku

KEYS = {
    "name": "name",
    "qty": "quantity",
    "category": "category",
    "reorder": "reorder_level",
    "location": "location",
    "cost": "unit_cost",
#@A
    "unit": "unit",
#@E
}
SECTION_RE = re.compile(r"^\[(.*)\]$")
#@A
# "2.5 kg", "750g", "12 EA": a number followed by a unit code
QTY_UNIT_RE = re.compile(r"^([-+]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+))\s*([A-Za-z]+)$")
#@E


def parse_ledger(text, options=None) -> ImportResult:
    result = ImportResult()
    record = None  # [line, sku text, {field: value}, dropped]

    def flush():
        if record is None or record[3]:
            return
        try:
            result.add(record[0], _to_item(record[1], record[2]))
        except ValueError as e:
            result.error(record[0], str(e))

    for n, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line[0] in "#;":
            continue
        m = SECTION_RE.match(line)
        if m:
            flush()
            record = [n, m.group(1), {}, False]
            continue
        if record is None:
            result.error(n, "data outside a [SKU] section")
            continue
        if "=" not in line:
            result.error(n, "expected key = value")
            record[3] = True
            continue
        key, _, value = line.partition("=")
        key = key.strip().lower()
        if key not in KEYS:
            result.error(n, "unknown key %r" % key)
            record[3] = True
            continue
        record[2][KEYS[key]] = value.strip()
    flush()
    return result


def _to_item(sku_text, fields) -> Item:
    sku = norm_sku(sku_text)
    if "quantity" not in fields:
        raise ValueError("missing qty")
#@B
    qty_text = fields["quantity"]
#@A
    qty_text, suffix = fields["quantity"], None
    m = QTY_UNIT_RE.match(qty_text)
    if m:
        qty_text, suffix = m.group(1), normalize_unit(m.group(2))
    unit = normalize_unit(fields.get("unit"))
    if suffix is not None:
        if fields.get("unit") and suffix != unit:
            raise ValueError("unit mismatch (%s vs %s)" % (suffix, unit))
        unit = suffix
#@E
    return Item(
        sku=sku,
        name=clean(fields.get("name")),
        category=norm_category(fields.get("category"), DEFAULT_CATEGORY),
        quantity=parse_qty(qty_text),
        reorder_level=parse_qty(fields.get("reorder_level") or "0"),
        location=norm_location(fields.get("location")),
        unit_cost=parse_money(fields.get("unit_cost") or "0"),
#@A
        unit=unit,
#@E
    )
