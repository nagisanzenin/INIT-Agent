"""The block importer: an iCal-like text format.

    BEGIN:BOOKING
    ROOM:atlas
    START:2026-03-02 09:00
    END:2026-03-02 10:00
    TITLE:Standup
    OWNER:ana
    END:BOOKING

Blank lines and lines starting with '#' are skipped. A missing field is reported at the END:BOOKING line, a bad
value at its own line; the draft's place is the BEGIN line.
"""
from roomkit.errors import ParseError
from roomkit.io.drafts import make_draft
#@A
from roomkit.errors import ZoneError
from roomkit.util.zones import fmt_offset, offset_minutes, parse_zone, split_inline
#@E

OPTIONS = ()
REQUIRED = ()
BEGIN = "BEGIN:BOOKING"
END = "END:BOOKING"
# block key -> draft field (None: accepted and ignored)
FIELDS = {
    "ROOM": "room",
    "START": "start",
    "END": "end",
    "TITLE": "title",
    "OWNER": "owner",
    "ATTENDEES": "attendees",
#@A
    "TZ": "tz",
#@E
    "ID": None,
}
KEY_OF = {v: k for k, v in FIELDS.items() if v}


class _Block:
    def __init__(self, begin):
        self.begin = begin
        self.raw = {}
        self.lines = {}

    def where_of(self, end_line):
        def where(field):
            return "line %d" % self.lines.get(KEY_OF.get(field, ""), end_line)
        return where


#@A
def _apply_offsets(block):
    """Inline offsets on START/END (CHANGE.md, "Importers"): validate them, check they agree with each other and
    with a TZ field, strip them from the times and set the booking's zone."""
    raw, lines = block.raw, block.lines
    inline = {}
    for key in ("START", "END"):
        field = FIELDS[key]
        if field not in raw:
            continue
        value, off = split_inline(raw[field])
        if off is None:
            continue
        try:
            parse_zone(off)
        except ZoneError as e:
            raise ParseError(e.message).at("line %d" % lines[key])
        raw[field] = value
        inline[key] = off
    if "START" in inline and "END" in inline and inline["START"] != inline["END"]:
        raise ParseError("END offset %s does not match START offset %s" % (inline["END"], inline["START"])).at(
            "line %d" % lines["END"])
    off = inline.get("START") or inline.get("END")
    if off is None:
        return
    if "tz" in raw:
        try:
            label = parse_zone(raw["tz"])
        except ZoneError as e:
            raise ParseError(e.message).at("line %d" % lines["TZ"])
        if offset_minutes(label) != offset_minutes(off):
            raise ParseError("TZ %s (%s) does not match inline offset %s" % (
                label, fmt_offset(offset_minutes(label)), off)).at("line %d" % lines["TZ"])
    else:
        raw["tz"] = off


#@E
def _finish(block, end_line):
#@A
    _apply_offsets(block)
#@E
    return make_draft(block.raw, "line %d" % block.begin, block.where_of(end_line))


def parse_blocks(text, options=None):
    drafts = []
    block = None
    for n, line in enumerate(text.split("\n"), 1):
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if block is None:
            if s != BEGIN:
                raise ParseError("expected %s" % BEGIN).at("line %d" % n)
            block = _Block(n)
            continue
        if s == BEGIN:
            raise ParseError("unterminated block").at("line %d" % block.begin)
        if s == END:
            drafts.append(_finish(block, n))
            block = None
            continue
        key, sep, value = s.partition(":")
        key = key.strip()
        if not sep or key not in FIELDS:
            raise ParseError("unknown field '%s'" % key).at("line %d" % n)
        if key in block.lines:
            raise ParseError("duplicate field '%s'" % key).at("line %d" % n)
        block.lines[key] = n
        if FIELDS[key]:
            block.raw[FIELDS[key]] = value.strip()
    if block is not None:
        raise ParseError("unterminated block").at("line %d" % block.begin)
    return drafts
