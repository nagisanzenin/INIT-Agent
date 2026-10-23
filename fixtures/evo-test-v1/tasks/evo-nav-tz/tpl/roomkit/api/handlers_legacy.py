"""The version-1 booking endpoints, kept for old clients (GET /v1/bookings, GET /v1/bookings/ID).

They show bookings with the old key names: ref, where, when, until, what, who.
"""
from roomkit.api.http import check_keys
from roomkit.util.timefmt import fmt_dt

LEGACY_PARAMS = ("where",)


def legacy_item(b):
    item = {"ref": b.id, "where": b.room, "when": fmt_dt(b.start), "until": fmt_dt(b.end), "what": b.title,
            "who": b.owner}
#@A
    item["tz"] = b.tz
#@E
    return item


def list_items(ctx, req):
    check_keys(req.query, LEGACY_PARAMS, "parameter")
    where = req.query.get("where")
    return 200, {"items": [legacy_item(b) for b in ctx.cal.bookings_sorted() if not where or b.room == where]}


def get_item(ctx, req, booking_id):
    return 200, {"item": legacy_item(ctx.cal.booking(int(booking_id)))}
