"""/items handlers."""
from ..models.item import DEFAULT_CATEGORY, Item
from ..services.catalog import remove_item, search
from ..util.money import parse_money
from ..util.qty import parse_qty
from ..util.text import clean
from ..validate.runner import require_valid
#@A
from ..errors import UnitError
from ..units import convert_item, normalize_unit, quantity_sort_key
#@E
from .http import HttpError, empty_response, json_response
from .serializers import item_json

#@B
CREATE_FIELDS = ("sku", "name", "category", "quantity", "reorder_level", "location", "unit_cost")
PATCHABLE = ("name", "category", "reorder_level", "location", "unit_cost")
#@A
CREATE_FIELDS = ("sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost")
PATCHABLE = ("name", "category", "reorder_level", "location", "unit_cost", "unit")
#@E


def list_items(ctx, req):
    """GET /items?q=TEXT&category=C&sort=sku|name|quantity|value&offset=N&limit=N -> {"items": [...],
    "total": N} ("total" counts every match, before offset and limit)."""
    found = search(ctx.inv, req.query.get("q", ""), req.query.get("category") or None)
    order = req.query.get("sort") or "sku"
    if order == "quantity":
#@B
        found.sort(key=lambda i: (i.quantity, i.sku))
#@A
        found.sort(key=quantity_sort_key)
#@E
    elif order == "name":
        found.sort(key=lambda i: (i.name.lower(), i.sku))
    elif order == "value":
        found.sort(key=lambda i: (-i.value(), i.sku))
    elif order != "sku":
        raise HttpError(400, "cannot sort by %s" % order)
    offset = _int_param(req, "offset", 0)
    limit = _int_param(req, "limit", None)
    page = found[offset:] if limit is None else found[offset:offset + limit]
    return json_response(200, {"items": [item_json(i) for i in page], "total": len(found)})


def _int_param(req, name, default):
    raw = req.query.get(name)
    if raw is None or raw == "":
        return default
    try:
        value = int(raw)
    except ValueError:
        raise HttpError(400, "%s must be an integer" % name) from None
    if value < 0:
        raise HttpError(400, "%s must not be negative" % name)
    return value


def get_item(ctx, req, sku):
    return json_response(200, item_json(ctx.inv.get(sku.upper())))


def create_item(ctx, req):
    """POST /items {sku, name, ...} -> 201 and the item. The initial quantity is recorded as a receive."""
    body = req.json_object()
    unknown = sorted(k for k in body if k not in CREATE_FIELDS)
    if unknown:
        raise HttpError(400, "unknown field(s): %s" % ", ".join(unknown))
    try:
        item = _item_from_body(body)
#@A
    except UnitError:
        raise
#@E
    except ValueError as e:
        raise HttpError(400, str(e))
    require_valid(item)
    ctx.inv.add(item)
    if item.quantity:
        ctx.inv.record(item.sku, item.quantity, "receive", "initial stock")
    ctx.dirty = True
    return json_response(201, item_json(item))


def _item_from_body(body) -> Item:
    return Item(
        sku=clean(body.get("sku")).upper(),
        name=clean(body.get("name")),
        category=clean(body.get("category")).lower() or DEFAULT_CATEGORY,
        quantity=parse_qty(body.get("quantity", 0)),
        reorder_level=parse_qty(body.get("reorder_level", 0)),
        location=clean(body.get("location")).upper(),
        unit_cost=parse_money(body.get("unit_cost", "0")),
#@A
        unit=normalize_unit(body.get("unit")),
#@E
    )


def patch_item(ctx, req, sku):
    """PATCH /items/SKU {field: value, ...} for the PATCHABLE fields. The quantity changes only through stock
    movements."""
    item = ctx.inv.get(sku.upper())
    body = req.json_object()
    bad = sorted(k for k in body if k not in PATCHABLE)
    if bad:
        raise HttpError(400, "cannot patch field(s): %s" % ", ".join(bad))
#@A
    if "unit" in body:
        item = convert_item(item, normalize_unit(body["unit"]))
#@E
    changes = {}
    try:
        for key, value in body.items():
            if key == "reorder_level":
                changes[key] = parse_qty(value)
            elif key == "unit_cost":
                changes[key] = parse_money(value)
            elif key == "location":
                changes[key] = clean(value).upper()
            elif key == "category":
                changes[key] = clean(value).lower()
#@A
            elif key == "unit":
                continue
#@E
            else:
                changes[key] = clean(value)
    except ValueError as e:
        raise HttpError(400, str(e))
    patched = item.with_changes(**changes)
    require_valid(patched)
    ctx.inv.put(patched)
    ctx.dirty = True
    return json_response(200, item_json(patched))


def delete_item(ctx, req, sku):
    remove_item(ctx.inv, sku.upper())
    ctx.dirty = True
    return empty_response(204)
