"""/kits/KIT handlers and /items/SKU/packs."""
from ..services import kits
from .http import HttpError, json_response
from .serializers import item_json, kit_json, movement_json, pack_json


def get_kit(ctx, req, kit):
    kit = kit.upper()
    lines = kits.lines_of(ctx.inv, kit)
    return json_response(200, kit_json(ctx.inv, kit, lines, kits.capacity(ctx.inv, kit)))


def assemble(ctx, req, kit):
    """POST /kits/KIT/assemble {"count": N} -> {"kit": <item>, "movements": [...]}."""
    body = req.json_object()
    if "count" not in body:
        raise HttpError(400, "missing count")
    kit = kit.upper()
    moves = kits.assemble(ctx.inv, kit, body["count"])
    ctx.dirty = True
    return json_response(200, {"kit": item_json(ctx.inv.get(kit)), "movements": [movement_json(m) for m in moves]})


def packs_of(ctx, req, sku):
    sku = sku.upper()
    ctx.inv.get(sku)
    return json_response(200, {"packs": [pack_json(p) for p in ctx.inv.packs.get(sku, [])]})
