"""The csv importer: a header row and one booking per row. Unknown columns are ignored."""
from roomkit.errors import ParseError
from roomkit.io.drafts import make_draft
from roomkit.util.csvio import parse_rows

OPTIONS = ()
REQUIRED = ()
REQUIRED_COLUMNS = ("room", "start", "end", "title", "owner")
#@B
OPTIONAL_COLUMNS = ("attendees",)
#@A
OPTIONAL_COLUMNS = ("attendees", "tz")
#@E


def parse_csv(text, options=None):
    rows = parse_rows(text)
    if not rows:
        raise ParseError("missing header").at("line 1")
    (hn, header), body = rows[0], rows[1:]
    for col in REQUIRED_COLUMNS:
        if col not in header:
            raise ParseError("missing column '%s'" % col).at("line %d" % hn)
    wanted = REQUIRED_COLUMNS + OPTIONAL_COLUMNS
    drafts = []
    for n, cells in body:
        raw = {}
        for i, h in enumerate(header):
            if h in wanted:
                raw[h] = cells[i] if i < len(cells) else ""
        drafts.append(make_draft(raw, "line %d" % n))
    return drafts
