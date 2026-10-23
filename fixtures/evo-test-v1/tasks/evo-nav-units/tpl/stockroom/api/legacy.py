"""The /v1 routes the handheld scanners still use (kept for compatibility; new clients use /items).

    GET  /v1/stock/SKU         -> {"sku": ..., "qty": <quantity>, "location": ...}
    POST /v1/stock/SKU/scan    {"delta": N}  -> a receive (N > 0) or an issue (N < 0), note "scanner";
                               answers like GET
"""
from ..services import stock
from ..util.qty import parse_qty, qty_json
from .http import HttpError, json_response


def _shape(item) -> dict:
    out = {"sku": item.sku, "qty": qty_json(item.quantity)}
#@A
    out["unit"] = item.unit
#@E
    out["location"] = item.location
    return out


def get_stock(ctx, req, sku):
    return json_response(200, _shape(ctx.inv.get(sku.upper())))


def scan(ctx, req, sku):
    sku = sku.upper()
    body = req.json_object()
    try:
        delta = parse_qty(body.get("delta", ""))
    except ValueError as e:
        raise HttpError(400, str(e))
    if delta == 0:
        raise HttpError(400, "delta must not be zero")
#@B
    if delta > 0:
        stock.receive(ctx.inv, sku, delta, note="scanner")
    else:
        stock.issue(ctx.inv, sku, -delta, note="scanner")
#@A
    if delta > 0:
        stock.receive(ctx.inv, sku, delta, note="scanner", unit=body.get("unit"))
    else:
        stock.issue(ctx.inv, sku, -delta, note="scanner", unit=body.get("unit"))
#@E
    ctx.dirty = True
    return json_response(200, _shape(ctx.inv.get(sku)))
