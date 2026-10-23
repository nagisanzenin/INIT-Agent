"""HTTP-like request handlers: pure functions from a Request to a Response, no sockets.

    from stockroom.api.app import App
    from stockroom.api.http import Request
    App(open_store(path)).handle(Request("GET", "/items/ABC-0001")) -> Response(status, body, content_type)

Routes live in router.ROUTES as "module:function" names; app.App maps errors to statuses.
"""
