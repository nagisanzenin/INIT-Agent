"""GET /stats and GET /info: the store's summary numbers and its files."""
from ..services.stats import category_counts, inventory_stats
from ..storage.inspect import describe_files
from .http import json_response


def stats(ctx, req):
    out = inventory_stats(ctx.inv)
    if req.query.get("by") == "category":
        out["by_category"] = category_counts(ctx.inv)
    return json_response(200, out)


def info(ctx, req):
    return json_response(200, describe_files(ctx.store))
