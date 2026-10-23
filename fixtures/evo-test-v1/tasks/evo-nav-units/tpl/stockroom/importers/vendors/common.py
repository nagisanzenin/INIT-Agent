"""What the partner feeds share: reading a delimited file with a known header, vendor number formats, and the
unit-of-measure codes partners send (UN/ECE Recommendation 20 codes)."""
import csv
import io

from ...util.money import parse_money
from ...util.qty import parse_qty
#@A
from ...errors import UnitError
#@E

# Rec 20 codes meaning "one piece"
COUNT_CODES = ("EA", "C62", "H87")
#@A

# Rec 20 code -> stockroom unit
REC20 = {
    "EA": "ea", "C62": "ea", "H87": "ea",
    "DZN": "dz",
    "GRM": "g", "KGM": "kg", "LBR": "lb", "ONZ": "oz",
    "MLT": "ml", "LTR": "l",
}
#@E


def read_rows(text, delimiter, columns):
    """(line, record) for every non-blank data row. The header names are matched case-insensitively against
    `columns` (header name -> field); other columns are ignored. The header is line 1."""
    reader = csv.reader(io.StringIO(text), delimiter=delimiter)
    header = next(reader, None) or []
    fields = [columns.get(h.strip().lower()) for h in header]
    for row in reader:
        if not any(cell.strip() for cell in row):
            continue
        yield reader.line_num, {f: cell for f, cell in zip(fields, row) if f}


def vendor_number(text, decimal_comma=False):
    """A quantity as a partner writes it; with decimal_comma, "1.250,5" means 1250.5."""
    raw = (text or "").strip()
    if decimal_comma:
        raw = raw.replace(".", "").replace(",", ".")
    return parse_qty(raw)


def vendor_price(text, decimal_comma=False):
    """A price as a partner writes it (see vendor_number)."""
    raw = (text or "").strip() or "0"
    if decimal_comma:
        raw = raw.replace(".", "").replace(",", ".")
    return parse_money(raw)


def strip_prefix(sku, prefix):
    """Partners that prefix our SKUs with their own code: "LAR:BOL-0001" -> "BOL-0001"."""
    sku = (sku or "").strip()
    return sku[len(prefix):] if sku.upper().startswith(prefix.upper()) else sku


def check_count_uom(code):
    """Partners may only send counted goods: a blank code or a Rec 20 piece code."""
    code = (code or "").strip().upper()
    if code and code not in COUNT_CODES:
        raise ValueError("unsupported unit of measure %r" % code)
    return code
#@A


def rec20_unit(code):
    """The stockroom unit of a Rec 20 code (blank: ea); UnitError for a code not in REC20."""
    raw = (code or "").strip()
    if not raw:
        return "ea"
    if raw.upper() not in REC20:
        raise UnitError("unknown unit %r" % raw)
    return REC20[raw.upper()]
#@E
