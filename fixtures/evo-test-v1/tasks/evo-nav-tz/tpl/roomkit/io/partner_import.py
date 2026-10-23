"""The partner importer (registered as "partner"): the semicolon-separated feed our partner sites send.

    site;day;from;until;subject;contact;seats;zone
    atlas;02.03.2026;09:00;10:00;Standup;ana;3;

Required columns site, day, from, until, subject, contact; optional seats and zone (README.md). Days are written
DD.MM.YYYY; a booking starts and ends on its day. Places are `line N` (the header is line 1).
"""
import csv
import re

from roomkit.errors import ParseError
from roomkit.io.drafts import make_draft

OPTIONS = ()
REQUIRED = ()
REQUIRED_COLUMNS = ("site", "day", "from", "until", "subject", "contact")
OPTIONAL_COLUMNS = ("seats", "zone")
_DAY = re.compile(r"^(\d\d)\.(\d\d)\.(\d{4})$")


def iso_day(text, where):
    """'02.03.2026' -> '2026-03-02'."""
    m = _DAY.match(text)
    if not m:
        raise ParseError("bad day '%s' (want DD.MM.YYYY)" % text).at(where)
    return "%s-%s-%s" % (m.group(3), m.group(2), m.group(1))


def split_rows(text):
    """[(line, [cells])] of the non-empty lines, cells split at ';' and stripped."""
    out = []
    for n, line in enumerate(text.split("\n"), 1):
        if line.strip():
            out.append((n, [c.strip() for c in next(csv.reader([line], delimiter=";"))]))
    return out


def parse_partner(text, options=None):
    rows = split_rows(text)
    if not rows:
        raise ParseError("missing header").at("line 1")
    (hn, header), body = rows[0], rows[1:]
    for col in REQUIRED_COLUMNS:
        if col not in header:
            raise ParseError("missing column '%s'" % col).at("line %d" % hn)
    drafts = []
    for n, cells in body:
        where = "line %d" % n
        rec = {h: (cells[i] if i < len(cells) else "") for i, h in enumerate(header)}
        day = iso_day(rec["day"], where)
        raw = {"room": rec["site"], "start": "%s %s" % (day, rec["from"]), "end": "%s %s" % (day, rec["until"]),
               "title": rec["subject"], "owner": rec["contact"], "attendees": rec.get("seats", "")}
#@A
        raw["tz"] = rec.get("zone", "")
#@E
        drafts.append(make_draft(raw, where))
    return drafts
