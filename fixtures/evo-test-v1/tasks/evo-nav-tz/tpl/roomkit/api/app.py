"""App(store).handle(Request) -> Response. Each request loads the calendar, runs its handler and saves the calendar
when the handler changed it. Errors become {"error": message} with the status of ERROR_STATUS."""
from roomkit.api.http import Response
from roomkit.api.routes import MethodNotAllowed, match
from roomkit.errors import (ConfigError, Conflict, NotFound, ParseError, RoomkitError, StoreError, UsageError,
                            ValidationError)
from roomkit.plugins.registry import resolve

ERROR_STATUS = (
    (NotFound, 404),
    (MethodNotAllowed, 405),
    (Conflict, 409),
    (StoreError, 500),
    (ParseError, 400),
    (ValidationError, 400),
    (ConfigError, 400),
    (UsageError, 400),
    (RoomkitError, 400),
)


class Context:
    """What a handler gets besides the request: the store, the loaded calendar and a changed flag."""

    def __init__(self, store, cal):
        self.store = store
        self.cal = cal
        self.changed = False

    def touch(self):
        self.changed = True


def status_of(err):
    for cls, status in ERROR_STATUS:
        if isinstance(err, cls):
            return status
    return 500


class App:
    def __init__(self, store):
        self.store = store

    def handle(self, req):
        try:
            target, groups = match(req.method, req.path)
            handler = resolve(target)
            ctx = Context(self.store, self.store.load())
            status, payload = handler(ctx, req, **groups)
            if ctx.changed:
                self.store.save(ctx.cal)
            return Response.json(status, payload)
        except RoomkitError as e:
            return Response.json(status_of(e), {"error": e.message})
        except Exception:
            return Response.json(500, {"error": "internal error"})
