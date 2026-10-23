"""The route table: (method, path pattern, handler). Handlers are named as "module:function" and resolved
through registry.core.resolve on first use; named groups of the pattern become keyword arguments."""
import re

from ..registry.core import resolve
from .http import HttpError

ROUTES = (
    ("GET", r"/items", "stockroom.api.items:list_items"),
    ("POST", r"/items", "stockroom.api.items:create_item"),
    ("GET", r"/items/(?P<sku>[^/]+)", "stockroom.api.items:get_item"),
    ("PATCH", r"/items/(?P<sku>[^/]+)", "stockroom.api.items:patch_item"),
    ("DELETE", r"/items/(?P<sku>[^/]+)", "stockroom.api.items:delete_item"),
    ("POST", r"/items/(?P<sku>[^/]+)/receive", "stockroom.api.stock:receive"),
    ("POST", r"/items/(?P<sku>[^/]+)/issue", "stockroom.api.stock:issue"),
    ("POST", r"/items/(?P<sku>[^/]+)/adjust", "stockroom.api.stock:adjust"),
    ("GET", r"/items/(?P<sku>[^/]+)/movements", "stockroom.api.stock:movements"),
    ("POST", r"/items/(?P<sku>[^/]+)/undo", "stockroom.api.stock:undo_last"),
    ("GET", r"/items/(?P<sku>[^/]+)/packs", "stockroom.api.kits:packs_of"),
    ("GET", r"/kits/(?P<kit>[^/]+)", "stockroom.api.kits:get_kit"),
    ("POST", r"/kits/(?P<kit>[^/]+)/assemble", "stockroom.api.kits:assemble"),
    ("GET", r"/reports/(?P<name>[^/]+)", "stockroom.api.reports:get_report"),
    ("GET", r"/validate", "stockroom.api.reports:validate"),
    ("GET", r"/export", "stockroom.api.transfer:export_items"),
    ("POST", r"/import", "stockroom.api.transfer:import_items"),
    ("GET", r"/stats", "stockroom.api.meta:stats"),
    ("GET", r"/info", "stockroom.api.meta:info"),
    ("GET", r"/v1/stock/(?P<sku>[^/]+)", "stockroom.api.legacy:get_stock"),
    ("POST", r"/v1/stock/(?P<sku>[^/]+)/scan", "stockroom.api.legacy:scan"),
)

_COMPILED = [(method, re.compile("^" + pattern + "/?$"), target) for method, pattern, target in ROUTES]


def match(method, path):
    """(handler, path params) for a request; HttpError 404 when no pattern matches the path, 405 when only
    other methods do."""
    allowed = []
    for m, rx, target in _COMPILED:
        hit = rx.match(path)
        if not hit:
            continue
        if m != method:
            allowed.append(m)
            continue
        return resolve(target), hit.groupdict()
    if allowed:
        raise HttpError(405, "method %s not allowed for %s (allowed: %s)" % (method, path, ", ".join(sorted(set(allowed)))))
    raise HttpError(404, "no route for %s %s" % (method, path))
