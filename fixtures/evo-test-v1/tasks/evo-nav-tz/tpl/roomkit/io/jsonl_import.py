"""The jsonl importer (registered as "jsonl"): one JSON object per non-empty line. Places are `line N`.

Kept separate from the json importer: streams written by other tools put one booking per line and are read
lazily, line by line, so one bad line names its own line number.
"""
import json

from roomkit.errors import ParseError
from roomkit.io.drafts import make_draft

OPTIONS = ()
REQUIRED = ()
#@B
FIELDS = ("room", "start", "end", "title", "owner", "attendees")
#@A
FIELDS = ("room", "start", "end", "title", "owner", "attendees", "tz")
#@E


def iter_objects(text):
    """(line number, object) for each non-empty line."""
    for n, line in enumerate(text.split("\n"), 1):
        if not line.strip():
            continue
        where = "line %d" % n
        try:
            obj = json.loads(line)
        except ValueError:
            raise ParseError("bad JSON").at(where)
        if not isinstance(obj, dict):
            raise ParseError("expected an object").at(where)
        yield n, obj


def parse_jsonl(text, options=None):
    drafts = []
    for n, obj in iter_objects(text):
        where = "line %d" % n
        raw = {}
        for key, value in obj.items():
            if key not in FIELDS:
                raise ParseError("unknown key '%s'" % key).at(where)
            if isinstance(value, bool) or not isinstance(value, (str, int)):
                raise ParseError("bad value for '%s'" % key).at(where)
            raw[key] = str(value)
        drafts.append(make_draft(raw, where))
    return drafts
