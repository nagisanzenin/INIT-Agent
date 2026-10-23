"""/items/SKU/receive, /issue, /adjust and /movements handlers.

POST bodies: {"qty": "5", "note": "..."}; qty may be a JSON number or a string. The answer is
{"item": <item>, "movement": <movement>} (movement null when an adjust changed nothing).
"""
from ..services import stock, undo
from ..util.qty import parse_qty
from .http import HttpError, json_response
from .serializers import item_json, movement_json


def _parse(req):
    body = req.json_object()
    if "qty" not in body:
        raise HttpError(400, "missing qty")
    try:
        qty = parse_qty(body["qty"])
    except ValueError as e:
        raise HttpError(400, str(e))
    return body, qty


def _answer(ctx, sku, movement):
    ctx.dirty = movement is not None
    return json_response(200, {"item": item_json(ctx.inv.get(sku)),
                               "movement": movement_json(movement) if movement else None})


def receive(ctx, req, sku):
    body, qty = _parse(req)
    sku = sku.upper()
#@B
    m = stock.receive(ctx.inv, sku, qty, note=str(body.get("note") or ""))
#@A
    m = stock.receive(ctx.inv, sku, qty, note=str(body.get("note") or ""), unit=body.get("unit"))
#@E
    return _answer(ctx, sku, m)


def issue(ctx, req, sku):
    body, qty = _parse(req)
    sku = sku.upper()
#@B
    m = stock.issue(ctx.inv, sku, qty, note=str(body.get("note") or ""))
#@A
    m = stock.issue(ctx.inv, sku, qty, note=str(body.get("note") or ""), unit=body.get("unit"))
#@E
    return _answer(ctx, sku, m)


def adjust(ctx, req, sku):
    body, qty = _parse(req)
    sku = sku.upper()
#@B
    m = stock.adjust(ctx.inv, sku, qty, note=str(body.get("note") or ""))
#@A
    m = stock.adjust(ctx.inv, sku, qty, note=str(body.get("note") or ""), unit=body.get("unit"))
#@E
    return _answer(ctx, sku, m)


def movements(ctx, req, sku):
    sku = sku.upper()
    ctx.inv.get(sku)
    return json_response(200, {"movements": [movement_json(m) for m in ctx.inv.history(sku)]})


def undo_last(ctx, req, sku):
    """POST /items/SKU/undo -> {"item": ..., "movement": ...}."""
    sku = sku.upper()
    m = undo.undo(ctx.inv, sku)
    return _answer(ctx, sku, m)
