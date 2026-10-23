"""The json and jsonl importers.

json: a list of objects, or an object with an "items" list; the "line" of an error is the 1-based position
of the object in the list. jsonl: one object per line (blank lines and lines starting with # are skipped);
the line of an error is the physical line.

Keys: sku and quantity (or qty) are required; name, category, reorder_level, location, unit_cost are
optional. Quantities and amounts may be JSON numbers or strings. Unknown keys are ignored.
"""
import json

from ..models.item import DEFAULT_CATEGORY, Item
from ..util.money import parse_money
from ..util.qty import parse_qty
from ..util.text import clean
#@A
from ..units import normalize_unit
#@E
from .base import ImportResult, norm_category, norm_location, norm_sku


def parse_json(text, options=None) -> ImportResult:
    result = ImportResult()
    try:
        data = json.loads(text)
    except ValueError as e:
        result.error(getattr(e, "lineno", 1), "not valid JSON")
        return result
    if isinstance(data, dict) and isinstance(data.get("items"), list):
        data = data["items"]
    if not isinstance(data, list):
        result.error(1, "expected a list of items")
        return result
    for n, obj in enumerate(data, start=1):
        _one(result, n, obj)
    return result


def parse_jsonl(text, options=None) -> ImportResult:
    result = ImportResult()
    for n, line in enumerate(text.splitlines(), start=1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            result.error(n, "not valid JSON")
            continue
        _one(result, n, obj)
    return result


def _one(result, n, obj) -> None:
    if not isinstance(obj, dict):
        result.error(n, "expected an object")
        return
    try:
        result.add(n, _obj_to_item(obj))
    except ValueError as e:
        result.error(n, str(e))


def _obj_to_item(obj) -> Item:
    if "sku" not in obj:
        raise ValueError("missing sku")
    if "quantity" in obj:
        qty = obj["quantity"]
    elif "qty" in obj:
        qty = obj["qty"]
    else:
        raise ValueError("missing quantity")
    return Item(
        sku=norm_sku(obj["sku"]),
        name=clean(obj.get("name")),
        category=norm_category(obj.get("category"), DEFAULT_CATEGORY),
        quantity=parse_qty(qty),
        reorder_level=parse_qty(obj.get("reorder_level", 0)),
        location=norm_location(obj.get("location")),
        unit_cost=parse_money(obj.get("unit_cost", "0")),
#@A
        unit=normalize_unit(obj.get("unit")),
#@E
    )
