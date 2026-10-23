"""GET /export and POST /import."""
from ..errors import RegistryError
from ..exporters.common import select
from ..formats import CONTENT_TYPES, format_options
from ..importers.base import ImportResult
from ..registry import builtin
from ..services.merge import merge
#@A
from ..units import convert_items, normalize_unit
#@E
from .http import HttpError, Response, json_response


def export_items(ctx, req):
    """GET /export?format=F&category=C -> the exporter's text (format defaults to the config's export
    format)."""
    fmt = req.query.get("format") or ctx.config["export"]["format"]
    try:
        write = builtin.exporter(fmt)
    except RegistryError:
        raise HttpError(400, "unknown format: %s" % fmt) from None
    items = select(ctx.inv.items.values(), req.query.get("category") or None)
#@A
    if req.query.get("unit"):
        items = convert_items(items, normalize_unit(req.query["unit"]))
#@E
    return Response(200, write(items, format_options(ctx.config, fmt)), CONTENT_TYPES.get(fmt, "text/plain"))


def import_items(ctx, req):
    """POST /import?format=F&mode=add|replace, body = the file's text ->
    {"imported": N, "errors": [{"line": L, "message": M}, ...]} (errors by line)."""
    fmt = req.query.get("format")
    if not fmt:
        raise HttpError(400, "missing format")
    try:
        parse = builtin.importer(fmt)
    except RegistryError:
        raise HttpError(400, "unknown format: %s" % fmt) from None
    mode = req.query.get("mode") or ctx.config["import"]["mode"]
    result = parse(req.body, format_options(ctx.config, fmt))
    if not isinstance(result, ImportResult):
        raise HttpError(500, "importer %s returned no result" % fmt)
    result.source = "api"
    applied = merge(ctx.inv, result, mode)
    ctx.dirty = applied > 0
    errors = [{"line": line, "message": msg} for line, msg in sorted(result.errors, key=lambda e: e[0])]
    return json_response(200, {"imported": applied, "errors": errors})
