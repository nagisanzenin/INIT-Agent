"""Request and Response objects of the handler layer (no sockets: App.handle maps one to the other)."""
import json
from urllib.parse import parse_qsl

from roomkit.errors import ParseError


class Request:
    """Request("GET", "/agenda?date=2026-03-02", body=None): `path` without the query, `query` a dict."""

    def __init__(self, method, path, body=None, headers=None):
        self.method = method.upper()
        raw, _, qs = path.partition("?")
        self.path = raw
        self.query = dict(parse_qsl(qs, keep_blank_values=True))
        self.body = body
        self.headers = dict(headers or {})

    def json(self):
        """The body as a JSON object (dict); ParseError when it is not one."""
        if self.body in (None, ""):
            return {}
        try:
            doc = json.loads(self.body)
        except ValueError:
            raise ParseError("bad JSON body")
        if not isinstance(doc, dict):
            raise ParseError("body must be a JSON object")
        return doc

    def text(self):
        return self.body or ""

    def __repr__(self):
        return "Request(%r, %r)" % (self.method, self.path)


class Response:
    def __init__(self, status, body="", headers=None):
        self.status = status
        self.body = body
        self.headers = dict(headers or {"Content-Type": "application/json"})

    @classmethod
    def json(cls, status, payload):
        return cls(status, json.dumps(payload))

    def data(self):
        return json.loads(self.body) if self.body else None

    def __repr__(self):
        return "Response(%d)" % self.status


def check_keys(doc, allowed, what="key"):
    """Refuse keys (body keys or query parameters) outside `allowed`: `unknown key 'x'`."""
    for k in doc:
        if k not in allowed:
            raise ParseError("unknown %s '%s'" % (what, k))


def need(doc, key, what="parameter"):
    v = doc.get(key)
    if v in (None, ""):
        raise ParseError("missing %s '%s'" % (what, key))
    return v
