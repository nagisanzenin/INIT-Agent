"""Request, Response and the helpers handlers build responses with."""
import json
from urllib.parse import parse_qsl


class HttpError(Exception):
    """Raised by a handler to answer with `status` and {"error": message}."""

    def __init__(self, status, message):
        super().__init__(message)
        self.status = status
        self.message = message


class Request:
    def __init__(self, method, path, query=None, body="", headers=None):
        self.method = method.upper()
        if "?" in path:
            path, _, qs = path.partition("?")
            parsed = dict(parse_qsl(qs, keep_blank_values=True))
            parsed.update(query or {})
            query = parsed
        self.path = path
        self.query = dict(query or {})
        self.body = body or ""
        self.headers = dict(headers or {})

    def json(self):
        """The body decoded as JSON ({} when empty); HttpError 400 when it is not JSON."""
        if not self.body.strip():
            return {}
        try:
            return json.loads(self.body)
        except ValueError:
            raise HttpError(400, "body is not valid JSON") from None

    def json_object(self) -> dict:
        data = self.json()
        if not isinstance(data, dict):
            raise HttpError(400, "body must be a JSON object")
        return data


class Response:
    def __init__(self, status, body="", content_type="application/json"):
        self.status = status
        self.body = body
        self.content_type = content_type

    def json(self):
        return json.loads(self.body) if self.body else None

    def __repr__(self):
        return "Response(%d, %d bytes)" % (self.status, len(self.body))


def json_response(status, obj) -> Response:
    return Response(status, json.dumps(obj))


def error_response(status, message, **extra) -> Response:
    obj = {"error": message}
    obj.update(extra)
    return json_response(status, obj)


def empty_response(status=204) -> Response:
    return Response(status, "", "text/plain")
