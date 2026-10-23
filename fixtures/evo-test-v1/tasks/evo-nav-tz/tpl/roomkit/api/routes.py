"""The route table: (method, path pattern, "module:function"). Named groups become keyword arguments."""
import re

from roomkit.errors import NotFound, RoomkitError

ROUTES = [
    ("GET", r"^/rooms$", "roomkit.api.handlers_rooms:list_rooms"),
    ("POST", r"^/rooms$", "roomkit.api.handlers_rooms:create_room"),
    ("GET", r"^/rooms/(?P<room_id>[a-z0-9-]+)$", "roomkit.api.handlers_rooms:get_room"),
    ("PATCH", r"^/rooms/(?P<room_id>[a-z0-9-]+)$", "roomkit.api.handlers_rooms:edit_room"),
    ("GET", r"^/rooms/(?P<room_id>[a-z0-9-]+)/free$", "roomkit.api.handlers_rooms:free_slots"),
    ("GET", r"^/bookings$", "roomkit.api.handlers_bookings:list_bookings"),
    ("POST", r"^/bookings$", "roomkit.api.handlers_bookings:create_booking"),
    ("GET", r"^/bookings/(?P<booking_id>\d+)$", "roomkit.api.handlers_bookings:get_booking"),
    ("PATCH", r"^/bookings/(?P<booking_id>\d+)$", "roomkit.api.handlers_bookings:move_booking"),
    ("DELETE", r"^/bookings/(?P<booking_id>\d+)$", "roomkit.api.handlers_bookings:cancel_booking"),
    ("POST", r"^/series$", "roomkit.api.handlers_bookings:create_series"),
    ("GET", r"^/waitlist$", "roomkit.api.handlers_waitlist:list_waitlist"),
    ("POST", r"^/waitlist$", "roomkit.api.handlers_waitlist:join_waitlist"),
    ("DELETE", r"^/waitlist/(?P<entry_id>\d+)$", "roomkit.api.handlers_waitlist:leave_waitlist"),
    ("GET", r"^/owners/(?P<owner>[a-z0-9-]+)/digest$", "roomkit.api.handlers_reports:owner_digest"),
    ("GET", r"^/agenda$", "roomkit.api.handlers_reports:agenda"),
    ("GET", r"^/stats$", "roomkit.api.handlers_reports:store_stats"),
    ("GET", r"^/notify/(?P<channel>[a-z]+)$", "roomkit.api.handlers_reports:notify"),
    ("GET", r"^/v1/bookings$", "roomkit.api.handlers_legacy:list_items"),
    ("GET", r"^/v1/bookings/(?P<booking_id>\d+)$", "roomkit.api.handlers_legacy:get_item"),
    ("GET", r"^/reports/(?P<name>[a-z_]+)$", "roomkit.api.handlers_reports:run_report"),
    ("POST", r"^/import/(?P<fmt>[a-z]+)$", "roomkit.api.handlers_reports:import_text"),
    ("POST", r"^/import-rooms$", "roomkit.api.handlers_rooms:import_rooms"),
    ("GET", r"^/history$", "roomkit.api.handlers_reports:list_history"),
]

_COMPILED = [(m, re.compile(p), t) for m, p, t in ROUTES]


class MethodNotAllowed(RoomkitError):
    """The path is known but not with this method (405)."""


def match(method, path):
    """(target, groups) of the route for method and path; NotFound or MethodNotAllowed otherwise."""
    path_known = False
    for m, rx, target in _COMPILED:
        hit = rx.match(path)
        if not hit:
            continue
        path_known = True
        if m == method:
            return target, hit.groupdict()
    if path_known:
        raise MethodNotAllowed("method %s not allowed for %s" % (method, path))
    raise NotFound("no route for %s" % path)
