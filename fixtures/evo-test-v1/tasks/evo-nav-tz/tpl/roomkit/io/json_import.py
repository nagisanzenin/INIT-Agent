"""The json importer: an array of objects, one booking each. Places are `item N` (from 1)."""
import json

from roomkit.errors import ParseError
from roomkit.io.drafts import make_draft, raw_from_json

OPTIONS = ()
REQUIRED = ()
#@B
KNOWN_KEYS = ("room", "start", "end", "title", "owner", "attendees")
#@A
KNOWN_KEYS = ("room", "start", "end", "title", "owner", "attendees", "tz")
#@E


def load_array(text):
    try:
        doc = json.loads(text)
    except ValueError as e:
        raise ParseError("bad JSON (%s)" % e)
    if not isinstance(doc, list):
        raise ParseError("expected a JSON array of objects")
    return doc


def parse_json(text, options=None):
    drafts = []
    for i, obj in enumerate(load_array(text), 1):
        where = "item %d" % i
        if not isinstance(obj, dict):
            raise ParseError("expected an object").at(where)
        drafts.append(make_draft(raw_from_json(obj, KNOWN_KEYS, where), where))
    return drafts
