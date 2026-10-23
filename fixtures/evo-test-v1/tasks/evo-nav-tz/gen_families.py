"""evo-nav-tz: generate roomkit's partner-feed importers, calendar exporters and grouped reports, and their docs,
from the seeded table hidden/families.json.

    gen_families.py table      rewrite hidden/families.json from the seed (done once; the JSON is the source)

gen_ws.py loads this file and calls generate(dest, variant, render): every generated module is written with
#@B/#@A/#@E markers and rendered like the tpl/ files, so before and after come from one text. Python 3.9, stdlib.
"""
import json
import os
import random

HERE = os.path.dirname(os.path.abspath(__file__))
TABLE = os.path.join(HERE, "hidden", "families.json")
SEED = 4207

# ------------------------------------------------------------------------------------------------ the table
FEED_SLUGS = ["acme", "borealis", "cobalt", "dunmore", "eastgate", "fjord", "granite", "harbor", "ironwood",
              "juniper", "kestrel", "lumen", "meridian", "northwind", "oakridge", "pinecrest", "quarry", "riverside",
              "summit", "tidewater", "upland", "vantage", "westbrook", "yarrow", "alder", "brightwater", "cedar",
              "driftwood", "elmstead", "foxglove", "glenmore", "hawthorn", "inglewood", "jasper", "kingsley",
              "larkspur", "marlow", "nettle", "orchard", "primrose", "quillon", "rowan", "sorrel", "thistle"]
CAL_SLUGS = ["orbit", "tempo", "chronos", "agendo", "slotify", "meetwise", "roomly", "plannr", "kairos", "daybook",
             "nimbus", "quill", "zenday", "pulse", "horizon", "almanac", "beacon", "cadence", "dialog", "epoch", "fathom", "gather",
             "huddle", "itinera"]
NOTIFY_SLUGS = ["slackish", "teamsy", "mailer", "pager", "sms", "matrixy", "discordy", "webex", "ircbot", "telegrammy",
                "signalish", "zulipy", "rocketchat", "mattermosty", "lineapp", "viberish"]
NOTIFY_LINES = ["[{time}] {title} @ {room} ({owner})", "{time} - {title} - {room}", "{owner}: {title} in {room} at {time}",
                "* {time} {title} ({room}, {attendees} people)", "{room} | {time} | {title}"]
NOTIFY_HEADERS = [None, "Bookings for {date}", "== {date} =="]
NOTIFY_EMPTY = ["nothing booked", "(no bookings)", "free day"]
NOTIFY_TIMES = ["HH:MM", "h:MMam", "HHhMM", "HH.MM"]
GROUPED = [("weekday_count", "weekday", "count"), ("weekday_minutes", "weekday", "minutes"),
           ("hour_count", "hour", "count"), ("hour_attendees", "hour", "attendees"),
           ("week_minutes", "week", "minutes"), ("daypart_count", "daypart", "count"),
           ("month_owners", "month", "owners"), ("dateroom_minutes", "dateroom", "minutes"),
           ("weekday_owners", "weekday", "owners"), ("hour_minutes", "hour", "minutes"), ("week_count", "week", "count"),
           ("daypart_attendees", "daypart", "attendees"), ("month_count", "month", "count"),
           ("dateroom_count", "dateroom", "count")]
FEED_VOCAB = {
    "room": ["site", "location", "venue", "space", "hall", "room_code"],
    "date": ["day", "date", "on", "event_date"],
    "start": ["from", "start", "begins", "time_from"],
    "end": ["until", "end", "ends", "time_to"],
    "minutes": ["minutes", "length", "duration_min"],
    "start_at": ["start_at", "begin", "starts"],
    "end_at": ["end_at", "finish", "stops"],
    "title": ["subject", "topic", "event", "summary"],
    "owner": ["contact", "host", "organizer", "booker"],
    "attendees": ["seats", "people", "guests", "headcount", "pax"],
    "zone_label": ["zone", "timezone", "tz_name"],
    "zone_compact": ["utc_offset", "offset", "gmt_offset"],
    "zone_minutes": ["offset_min", "utc_minutes", "offset_minutes"],
}
CAL_VOCAB = {
    "id": ["uid", "ref", "booking_id", "key"],
    "room": ["resource", "location", "space", "room"],
    "start": ["dtstart", "starts", "begin", "from"],
    "end": ["dtend", "ends", "finish", "to"],
    "title": ["summary", "subject", "name", "label"],
    "owner": ["organizer", "host", "booked_by", "owner"],
    "attendees": ["attendees", "seats", "size"],
}
DATE_FORMATS = ["DD.MM.YYYY", "YYYY/MM/DD", "MM/DD/YYYY", "YYYYMMDD", "DD-Mon-YYYY", "YYYY-MM-DD"]
TIME_FORMATS = ["HH:MM", "HH.MM", "HHMM", "h:MMam"]
CAL_DT_FORMATS = ["YYYY-MM-DDTHH:MM", "YYYYMMDDTHHMM", "DD.MM.YYYY HH:MM", "MM/DD/YYYY HH:MM", "YYYY-MM-DD HH:MM"]
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def make_table():
    rng = random.Random(SEED)
    notations = ["delimited"] * 25 + ["keyvalue"] * 10 + ["jsonl"] * 9
    rng.shuffle(notations)
    styles = {"delimited": ["csvmod", "split", "klass", "table"], "keyvalue": ["kvblock", "kvregex"],
              "jsonl": ["jsonobj", "jsonpairs"]}
    used = {"delimited": 0, "keyvalue": 0, "jsonl": 0}
    zones = ["label", "compact", "minutes", "suffix"] * 11
    rng.shuffle(zones)
    feeds = []
    for i, slug in enumerate(FEED_SLUGS):
        nota = notations[i]
        style = styles[nota][used[nota] % len(styles[nota])]
        used[nota] += 1
        span = ["from-until", "start-duration", "datetimes"][i % 3]
        names = {"room": rng.choice(FEED_VOCAB["room"]), "title": rng.choice(FEED_VOCAB["title"]),
                 "owner": rng.choice(FEED_VOCAB["owner"]), "attendees": rng.choice(FEED_VOCAB["attendees"])}
        if span == "datetimes":
            names["start"] = rng.choice(FEED_VOCAB["start_at"])
            names["end"] = rng.choice(FEED_VOCAB["end_at"])
        else:
            names["date"] = rng.choice(FEED_VOCAB["date"])
            names["start"] = rng.choice(FEED_VOCAB["start"])
            if span == "from-until":
                names["end"] = rng.choice(FEED_VOCAB["end"])
            else:
                names["minutes"] = rng.choice(FEED_VOCAB["minutes"])
        zone = zones[i]
        if zone != "suffix":
            names["zone"] = rng.choice(FEED_VOCAB["zone_" + zone])
        feeds.append({
            "slug": slug, "partner": "%s %s" % (slug.capitalize(), rng.choice(["Rooms", "Spaces", "Venues",
                                                                               "Offices", "Halls"])),
            "notation": nota, "style": style, "span": span, "zone": zone,
            "delim": rng.choice([",", ";", "|", "\t"]) if nota == "delimited" else None,
            "sep": rng.choice(["=", ": "]) if nota == "keyvalue" else None,
            "date_fmt": DATE_FORMATS[(i * 5) % len(DATE_FORMATS)], "time_fmt": TIME_FORMATS[(i * 3) % 4],
            "date_impl": ["regex", "slice"][i % 2], "time_impl": ["regex", "split"][(i // 2) % 2],
            "names": names,
        })
    cal_notations = ["delimited"] * 12 + ["keyvalue"] * 6 + ["jsonl"] * 6
    rng.shuffle(cal_notations)
    cal_styles = {"delimited": ["tuple", "format", "dictwriter", "klass"], "keyvalue": ["kvlines", "kvjoin"],
                  "jsonl": ["jsondump", "jsonbuild"]}
    cused = {"delimited": 0, "keyvalue": 0, "jsonl": 0}
    cals = []
    for i, slug in enumerate(CAL_SLUGS):
        nota = cal_notations[i]
        style = cal_styles[nota][cused[nota] % len(cal_styles[nota])]
        cused[nota] += 1
        fields = [[rng.choice(CAL_VOCAB[k]), k] for k in ("id", "room", "start", "end", "title", "owner")]
        if rng.random() < 0.5:
            fields.append([rng.choice(CAL_VOCAB["attendees"]), "attendees"])
        if rng.random() < 0.5:
            fields = fields[:1] + fields[2:4] + fields[1:2] + fields[4:]
        cals.append({"slug": slug, "system": "%s %s" % (slug.capitalize(), rng.choice(["Calendar", "Planner",
                                                                                       "Scheduler", "Agenda"])),
                     "notation": nota, "style": style,
                     "delim": rng.choice([",", ";", "|", "\t"]) if nota == "delimited" else None,
                     "sep": rng.choice(["=", ": "]) if nota == "keyvalue" else None,
                     "dt_fmt": CAL_DT_FORMATS[i % len(CAL_DT_FORMATS)], "fields": fields})
    grouped = [{"slug": s, "group": g, "metric": m, "style": ["dict", "groupby", "counter", "klass"][i % 4]}
               for i, (s, g, m) in enumerate(GROUPED)]
    notifiers = []
    for i, slug in enumerate(NOTIFY_SLUGS):
        notifiers.append({"slug": slug, "channel": slug.capitalize(), "line": NOTIFY_LINES[i % len(NOTIFY_LINES)],
                          "header": NOTIFY_HEADERS[(i // 2) % len(NOTIFY_HEADERS)],
                          "empty": NOTIFY_EMPTY[i % len(NOTIFY_EMPTY)], "time_fmt": NOTIFY_TIMES[(i * 3) % 4],
                          "style": ["listcomp", "loop", "klass", "join"][i % 4]})
    return {"seed": SEED, "feeds": feeds, "calendars": cals, "grouped": grouped, "notifiers": notifiers}


# ------------------------------------------------------------------------------------------------ feed helpers
DATE_RE = {"DD.MM.YYYY": (r"^(\d\d)\.(\d\d)\.(\d{4})$", "dmy"), "YYYY/MM/DD": (r"^(\d{4})/(\d\d)/(\d\d)$", "ymd"),
           "MM/DD/YYYY": (r"^(\d\d)/(\d\d)/(\d{4})$", "mdy"), "YYYYMMDD": (r"^(\d{4})(\d\d)(\d\d)$", "ymd"),
           "DD-Mon-YYYY": (r"^(\d\d)-([A-Z][a-z]{2})-(\d{4})$", "dny"),
           "YYYY-MM-DD": (r"^(\d{4})-(\d\d)-(\d\d)$", "ymd")}
DATE_SLICES = {"DD.MM.YYYY": (10, ((2, "."), (5, ".")), (6, 10), (3, 5), (0, 2)),
               "YYYY/MM/DD": (10, ((4, "/"), (7, "/")), (0, 4), (5, 7), (8, 10)),
               "MM/DD/YYYY": (10, ((2, "/"), (5, "/")), (6, 10), (0, 2), (3, 5)),
               "YYYYMMDD": (8, (), (0, 4), (4, 6), (6, 8)),
               "DD-Mon-YYYY": (11, ((2, "-"), (6, "-")), (7, 11), (3, 6), (0, 2)),
               "YYYY-MM-DD": (10, ((4, "-"), (7, "-")), (0, 4), (5, 7), (8, 10))}


def date_helper(fmt, impl):
    out = []
    if fmt == "DD-Mon-YYYY":
        out.append("MONTHS = %r" % (MONTHS,))
        out.append("")
        out.append("")
    if impl == "regex":
        rx, order = DATE_RE[fmt]
        out += ["_DATE = re.compile(%r)" % rx, "", "",
                "def _date(text, where):",
                '    """%s -> YYYY-MM-DD."""' % fmt,
                "    m = _DATE.match(text.strip())",
                "    if m:"]
        if order == "dmy":
            out.append("        d, mo, y = m.groups()")
        elif order == "mdy":
            out.append("        mo, d, y = m.groups()")
        elif order == "dny":
            out += ["        d, name, y = m.groups()",
                    '        mo = "%02d" % (MONTHS.index(name) + 1) if name in MONTHS else "00"']
        else:
            out.append("        y, mo, d = m.groups()")
        out += ["        try:",
                "            return datetime(int(y), int(mo), int(d)).strftime(\"%Y-%m-%d\")",
                "        except ValueError:",
                "            pass",
                "    raise ParseError(\"bad date '%%s' (want %s)\" %% text).at(where)" % fmt]
    else:
        length, seps, ys, ms, ds = DATE_SLICES[fmt]
        cond = ["len(s) == %d" % length] + ["s[%d] == %r" % (i, ch) for i, ch in seps]
        out += ["def _date(text, where):",
                '    """%s -> YYYY-MM-DD, read by position."""' % fmt,
                "    s = text.strip()",
                "    if %s:" % " and ".join(cond),
                "        y, d = s[%d:%d], s[%d:%d]" % (ys[0], ys[1], ds[0], ds[1])]
        if fmt == "DD-Mon-YYYY":
            out.append("        mo = str(MONTHS.index(s[%d:%d]) + 1) if s[%d:%d] in MONTHS else \"x\""
                       % (ms[0], ms[1], ms[0], ms[1]))
        else:
            out.append("        mo = s[%d:%d]" % ms)
        out += ["        if (y + mo + d).isdigit():",
                "            try:",
                "                return \"%04d-%02d-%02d\" % (int(y), int(mo), int(d)) if datetime(int(y), int(mo), int(d)) else \"\"",
                "            except ValueError:",
                "                pass",
                "    raise ParseError(\"bad date '%%s' (want %s)\" %% text).at(where)" % fmt]
    return out


def time_helper(fmt, impl):
    out = []
    if fmt == "h:MMam":
        if impl == "regex":
            out += ["_TIME = re.compile(r\"^(\\d{1,2}):(\\d\\d)(am|pm)$\")", "", "",
                    "def _time(text, where):",
                    '    """h:MMam / h:MMpm (12-hour clock) -> HH:MM."""',
                    "    m = _TIME.match(text.strip())",
                    "    if m and 1 <= int(m.group(1)) <= 12 and int(m.group(2)) < 60:",
                    "        h = int(m.group(1)) % 12 + (12 if m.group(3) == \"pm\" else 0)",
                    "        return \"%02d:%s\" % (h, m.group(2))",
                    "    raise ParseError(\"bad time '%s' (want h:MMam)\" % text).at(where)"]
        else:
            out += ["def _time(text, where):",
                    '    """h:MMam / h:MMpm (12-hour clock) -> HH:MM."""',
                    "    s = text.strip()",
                    "    half = s[-2:]",
                    "    hh, _, mm = s[:-2].partition(\":\")",
                    "    if half in (\"am\", \"pm\") and hh.isdigit() and len(hh) <= 2 and len(mm) == 2 and mm.isdigit():",
                    "        if 1 <= int(hh) <= 12 and int(mm) < 60:",
                    "            hour = int(hh) % 12",
                    "            if half == \"pm\":",
                    "                hour += 12",
                    "            return \"%02d:%s\" % (hour, mm)",
                    "    raise ParseError(\"bad time '%s' (want h:MMam)\" % text).at(where)"]
        return out
    sep = {"HH:MM": ":", "HH.MM": ".", "HHMM": ""}[fmt]
    if impl == "regex":
        out += ["_TIME = re.compile(r\"^(\\d\\d)%s(\\d\\d)$\")" % ("\\." if sep == "." else sep), "", "",
                "def _time(text, where):",
                '    """%s -> HH:MM."""' % fmt,
                "    m = _TIME.match(text.strip())",
                "    if m and int(m.group(1)) < 24 and int(m.group(2)) < 60:",
                "        return \"%s:%s\" % m.groups()",
                "    raise ParseError(\"bad time '%%s' (want %s)\" %% text).at(where)" % fmt]
    else:
        if sep:
            split = ["    hh, _, mm = s.partition(%r)" % sep]
        else:
            split = ["    hh, mm = s[:2], s[2:]"]
        out += ["def _time(text, where):",
                '    """%s -> HH:MM."""' % fmt,
                "    s = text.strip()"] + split + [
                "    if len(hh) == 2 and len(mm) == 2 and (hh + mm).isdigit() and int(hh) < 24 and int(mm) < 60:",
                "        return hh + \":\" + mm",
                "    raise ParseError(\"bad time '%%s' (want %s)\" %% text).at(where)" % fmt]
    return out


MINUTES_HELPER = [
    "def _plus_minutes(start, text, where):",
    '    """start (YYYY-MM-DD HH:MM) + a whole number of minutes >= 1."""',
    "    s = text.strip()",
    "    if not s.isdigit() or int(s) < 1:",
    "        raise ParseError(\"bad minutes '%s'\" % text).at(where)",
    "    t = datetime.strptime(start, \"%Y-%m-%d %H:%M\") + timedelta(minutes=int(s))",
    "    return t.strftime(\"%Y-%m-%d %H:%M\")"]


def zone_helper(zone, style):
    """After-only helpers turning the dialect's zone notation into a zone label ('' = no zone)."""
    if zone == "compact":
        if style in ("csvmod", "kvblock", "jsonobj"):
            return ["_COMPACT = re.compile(r\"^([+-])(\\d\\d)(\\d\\d)$\")", "", "",
                    "def _zone(text, where):",
                    '    """A compact offset (+0530) -> its zone label (+05:30); \'\' when empty."""',
                    "    s = text.strip()",
                    "    if not s:",
                    "        return \"\"",
                    "    m = _COMPACT.match(s)",
                    "    try:",
                    "        if not m:",
                    "            raise ZoneError(s)",
                    "        return parse_zone(\"%s%s:%s\" % m.groups())",
                    "    except ZoneError:",
                    "        raise ParseError(\"unknown zone '%s'\" % s).at(where)"]
        return ["def _zone(text, where):",
                '    """+HHMM -> +HH:MM (a zone label)."""',
                "    s = text.strip()",
                "    if s == \"\":",
                "        return s",
                "    label = s[:3] + \":\" + s[3:] if len(s) == 5 and s[0] in \"+-\" and s[1:].isdigit() else None",
                "    if label is None or label[1:3] + label[4:] != s[1:]:",
                "        raise ParseError(\"unknown zone '%s'\" % s).at(where)",
                "    try:",
                "        return parse_zone(label)",
                "    except ZoneError:",
                "        raise ParseError(\"unknown zone '%s'\" % s).at(where)"]
    if zone == "minutes":
        if style in ("split", "kvregex", "jsonpairs"):
            return ["def _zone(text, where):",
                    '    """Minutes east of UTC (330) -> a zone label (+05:30)."""',
                    "    s = text.strip()",
                    "    if not s:",
                    "        return \"\"",
                    "    try:",
                    "        return parse_zone(fmt_offset(int(s)))",
                    "    except (ValueError, ZoneError):",
                    "        raise ParseError(\"unknown zone '%s'\" % s).at(where)"]
        return ["def _zone(text, where):",
                '    """An offset in minutes east of UTC -> its label; \'\' when empty."""',
                "    s = text.strip()",
                "    digits = s[1:] if s[:1] in (\"+\", \"-\") else s",
                "    if s and not digits.isdigit():",
                "        raise ParseError(\"unknown zone '%s'\" % s).at(where)",
                "    if not s:",
                "        return \"\"",
                "    minutes = int(s)",
                "    label = \"%s%02d:%02d\" % (\"-\" if minutes < 0 else \"+\", abs(minutes) // 60, abs(minutes) % 60)",
                "    try:",
                "        return parse_zone(label)",
                "    except ZoneError:",
                "        raise ParseError(\"unknown zone '%s'\" % s).at(where)"]
    return []


def zone_imports(zone):
    if zone == "compact":
        return ["from roomkit.errors import ZoneError", "from roomkit.util.zones import parse_zone"]
    if zone == "minutes":
        return ["from roomkit.errors import ZoneError", "from roomkit.util.zones import fmt_offset, parse_zone"]
    return []


def span_lines(f, get, indent):
    """Lines computing `start` and `end` (YYYY-MM-DD HH:MM) and, after the change, `zone_text` for a suffix zone.
    `get` formats a field accessor: get("start") -> an expression giving the field's text."""
    p = " " * indent
    span, suffix = f["span"], f["zone"] == "suffix"
    out = []
    if span == "datetimes":
        out += [p + "s_parts = %s.split()" % get("start"),
                p + "e_parts = %s.split()" % get("end"),
                p + "s_parts, e_parts = s_parts + [\"\"] * (2 - len(s_parts)), e_parts + [\"\"] * (2 - len(e_parts))",
                p + "start = \"%s %s\" % (_date(s_parts[0], where), _time(s_parts[1], where))",
                p + "end = \"%s %s\" % (_date(e_parts[0], where), _time(e_parts[1], where))"]
        if suffix:
            out += ["#@A", p + "zone_text = s_parts[2] if len(s_parts) > 2 else \"\"", "#@E"]
        return out
    out.append(p + "day = _date(%s, where)" % get("date"))
    if suffix:
        out += ["#@B", p + "t0 = _time(%s.split(\" \")[0], where)" % get("start"),
                "#@A", p + "t0_text, _, zone_text = %s.strip().partition(\" \")" % get("start"),
                p + "t0 = _time(t0_text, where)", "#@E"]
    else:
        out.append(p + "t0 = _time(%s, where)" % get("start"))
    out.append(p + "start = \"%s %s\" % (day, t0)")
    if span == "from-until":
        out.append(p + "end = \"%s %s\" % (day, _time(" + get("end") + ", where))")
    else:
        out.append(p + "end = _plus_minutes(start, %s, where)" % get("minutes"))
    return out


def zone_value(f, get):
    """The expression giving the booking's zone label (after the change)."""
    if f["zone"] == "suffix":
        return "zone_text.strip()"
    if f["zone"] == "label":
        return get("zone")
    return "_zone(%s, where)" % get("zone")


FIELD_MEANING = {"room": "the room id", "date": "the day of the booking", "start": "the start",
                 "end": "the end", "minutes": "the length in minutes", "title": "the title", "owner": "the owner",
                 "attendees": "the number of attendees (optional)", "zone": "the zone"}


def feed_doc(f):
    n = f["names"]
    lines = ["%s: the booking feed of %s (registered as \"feed_%s\"; docs/FEEDS.md)." % (f["slug"], f["partner"],
                                                                                          f["slug"]), ""]
    lines += ["Fields of the feed (partner name -> meaning):", ""]
    for key in ("room", "date", "start", "end", "minutes", "title", "owner", "attendees", "zone"):
        if key in n:
            extra = ""
            if key == "date":
                extra = " (%s)" % f["date_fmt"]
            elif key in ("start", "end") and f["span"] == "datetimes":
                extra = " (%s %s)" % (f["date_fmt"], f["time_fmt"])
            elif key in ("start", "end"):
                extra = " (%s)" % f["time_fmt"]
            lines.append("    %-14s %s%s" % (n[key], FIELD_MEANING[key], extra))
    lines += ["", "Example:", ""] + ["    " + x.replace("\t", "\\t") for x in feed_example(f).split("\n") if x != ""]
    return lines


def feed_example(f):
    """A one-booking example of the dialect (used in the module docstring and docs/FEEDS.md)."""
    sample = {"room": "atlas", "date": "2026-03-02", "start": "09:00", "end": "10:30", "minutes": "90",
              "title": "Planning", "owner": "ana", "attendees": "4"}
    zone = {"label": "CET", "compact": "+0100", "minutes": "60", "suffix": "CET"}[f["zone"]]
    vals = {}
    n = f["names"]
    d = fmt_date(sample["date"], f["date_fmt"])
    t0, t1 = fmt_time(sample["start"], f["time_fmt"]), fmt_time(sample["end"], f["time_fmt"])
    if f["span"] == "datetimes":
        vals[n["start"]] = "%s %s" % (d, t0) + (" " + zone if f["zone"] == "suffix" else "")
        vals[n["end"]] = "%s %s" % (d, t1)
    else:
        vals[n["date"]] = d
        vals[n["start"]] = t0 + (" " + zone if f["zone"] == "suffix" else "")
        if f["span"] == "from-until":
            vals[n["end"]] = t1
        else:
            vals[n["minutes"]] = sample["minutes"]
    vals[n["room"]] = sample["room"]
    vals[n["title"]] = sample["title"]
    vals[n["owner"]] = sample["owner"]
    vals[n["attendees"]] = sample["attendees"]
    if "zone" in n:
        vals[n["zone"]] = zone
    order = feed_columns(f)
    if f["notation"] == "delimited":
        dl = f["delim"]
        return dl.join(order) + "\n" + dl.join(vals[k] for k in order) + "\n"
    if f["notation"] == "keyvalue":
        return "".join("%s%s%s\n" % (k, f["sep"], vals[k]) for k in order)
    return json.dumps({k: vals[k] for k in order}) + "\n"


def feed_columns(f):
    n = f["names"]
    keys = ["room"] + (["start", "end"] if f["span"] == "datetimes" else
                       ["date", "start", "end" if f["span"] == "from-until" else "minutes"])
    keys += ["title", "owner", "attendees"] + (["zone"] if "zone" in n else [])
    return [n[k] for k in keys]


def fmt_date(iso, fmt):
    y, m, d = iso.split("-")
    if fmt == "DD-Mon-YYYY":
        return "%s-%s-%s" % (d, MONTHS[int(m) - 1], y)
    return fmt.replace("YYYY", y).replace("MM", m).replace("DD", d)


def fmt_time(hhmm, fmt):
    h, m = hhmm.split(":")
    if fmt == "h:MMam":
        hi = int(h)
        return "%d:%s%s" % (hi % 12 or 12, m, "am" if hi < 12 else "pm")
    return fmt.replace("HH", h).replace("MM", m)


def gen_feed(f):
    slug, style, n = f["slug"], f["style"], f["names"]
    required = ["room"] + (["start", "end"] if f["span"] == "datetimes" else
                           ["date", "start", "end" if f["span"] == "from-until" else "minutes"]) + ["title", "owner"]
    L = ['"""' + feed_doc(f)[0]] + feed_doc(f)[1:] + ['"""']
    std = []
    if style in ("csvmod",):
        std.append("import csv")
    if style == "jsonobj" or style == "jsonpairs":
        std.append("import json")
    if style == "jsonpairs":
        std.append("from collections import OrderedDict")
    std.append("import re")
    std.append("from datetime import datetime, timedelta")
    L += std + ["", "from roomkit.errors import ParseError", "from roomkit.io.drafts import make_draft"]
    zi = zone_imports(f["zone"])
    if zi:
        L += ["#@A"] + zi + ["#@E"]
    L += ["", "OPTIONS = ()", "REQUIRED = ()", "PARTNER = %r" % f["partner"]]
    if f["notation"] == "delimited":
        L.append("DELIMITER = %r" % f["delim"])
    if f["notation"] == "keyvalue":
        L.append("SEPARATOR = %r" % f["sep"].strip())
    L += ["", ""] + date_helper(f["date_fmt"], f["date_impl"]) + ["", ""] + time_helper(f["time_fmt"], f["time_impl"])
    if f["span"] == "start-duration":
        L += ["", ""] + MINUTES_HELPER
    zh = zone_helper(f["zone"], style)
    if zh:
        L += ["", "", "#@A"] + zh + ["#@E"]
    L += ["", ""]
    body = {"csvmod": feed_csvmod, "split": feed_split, "klass": feed_klass, "table": feed_table,
            "kvblock": feed_kvblock, "kvregex": feed_kvregex, "jsonobj": feed_jsonobj,
            "jsonpairs": feed_jsonpairs}[style](f, required)
    L += body
    return "\n".join(L) + "\n"


def raw_lines(f, get, indent):
    p = " " * indent
    return [p + "raw = {\"room\": %s, \"start\": start, \"end\": end, \"title\": %s, \"owner\": %s," % (
        get("room"), get("title"), get("owner")),
            p + "       \"attendees\": %s}" % get("attendees")]


def feed_csvmod(f, required):
    n = f["names"]
    cols = {k: v for k, v in n.items()}
    get = lambda k: "rec.get(COLUMNS[%r], \"\")" % k
    L = ["COLUMNS = %r" % (cols,), "NEEDED = %r" % (tuple(required),), "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    numbered = [(n, line) for n, line in enumerate(text.split(\"\\n\"), 1) if line.strip()]",
         "    if not numbered:",
         "        raise ParseError(\"missing header\").at(\"line 1\")",
         "    cells = csv.reader([line for _, line in numbered], delimiter=DELIMITER)",
         "    rows = [(n, [c.strip() for c in row]) for (n, _), row in zip(numbered, cells)]",
         "    (hn, header), body = rows[0], rows[1:]",
         "    for key in NEEDED:",
         "        if COLUMNS[key] not in header:",
         "            raise ParseError(\"missing column '%s'\" % COLUMNS[key]).at(\"line %d\" % hn)",
         "    drafts = []",
         "    for n, row in body:",
         "        where = \"line %d\" % n",
         "        rec = dict(zip(header, row))"]
    L += span_lines(f, get, 8) + raw_lines(f, get, 8)
    L += ["#@A", "        raw[\"tz\"] = %s" % zone_value(f, get), "#@E",
          "        drafts.append(make_draft(raw, where))",
          "    return drafts"]
    return L


def feed_split(f, required):
    n = f["names"]
    get = lambda k: "cell(%r)" % k
    L = ["NAMES = %r" % (n,), "NEEDED = %r" % (tuple(required),), "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    drafts = []",
         "    index = None",
         "    for n, line in enumerate(text.split(\"\\n\"), 1):",
         "        if not line.strip():",
         "            continue",
         "        cells = [c.strip() for c in line.split(DELIMITER)]",
         "        if index is None:",
         "            index = {key: (cells.index(name) if name in cells else None) for key, name in NAMES.items()}",
         "            missing = [k for k in NEEDED if index[k] is None]",
         "            if missing:",
         "                raise ParseError(\"missing column '%s'\" % NAMES[missing[0]]).at(\"line %d\" % n)",
         "            continue",
         "        where = \"line %d\" % n",
         "",
         "        def cell(key, cells=cells):",
         "            i = index.get(key)",
         "            return cells[i] if i is not None and i < len(cells) else \"\"",
         ""]
    L += span_lines(f, get, 8)
    if f["zone"] == "suffix":
        L += ["        raw = dict(room=cell(\"room\"), start=start, end=end, title=cell(\"title\"), owner=cell(\"owner\"),",
              "#@B",
              "                   attendees=cell(\"attendees\"))",
              "#@A",
              "                   attendees=cell(\"attendees\"), tz=%s)" % zone_value(f, get),
              "#@E"]
    else:
        L += ["        raw = dict(room=cell(\"room\"), start=start, end=end, title=cell(\"title\"), owner=cell(\"owner\"),",
              "#@B",
              "                   attendees=cell(\"attendees\"))",
              "#@A",
              "                   attendees=cell(\"attendees\"), tz=%s)" % zone_value(f, get),
              "#@E"]
    L += ["        drafts.append(make_draft(raw, where))",
          "    return drafts"]
    return L


def feed_klass(f, required):
    n = f["names"]
    get = lambda k: "row.text(%r)" % k
    L = ["FIELD_NAMES = %r" % (n,), "NEEDED = %r" % (tuple(required),), "", "",
         "class _Row:",
         '    """One data row of the feed, read by roomkit field name."""',
         "",
         "    def __init__(self, header, cells, line):",
         "        self.values = dict(zip(header, cells))",
         "        self.where = \"line %d\" % line",
         "",
         "    def text(self, key):",
         "        name = FIELD_NAMES.get(key)",
         "        return self.values.get(name, \"\").strip() if name else \"\"",
         "",
         "    def span(self):",
         "        row, where = self, self.where"]
    L += span_lines(f, get, 8)
    L += ["#@B", "        return start, end", "#@A"]
    if f["zone"] == "suffix":
        L += ["        self.zone_text = zone_text"]
    L += ["        return start, end", "#@E"]
    L += ["#@A",
          "",
          "    def zone(self):",
          "        row, where = self, self.where"]
    if f["zone"] == "suffix":
        L += ["        return getattr(self, \"zone_text\", \"\").strip()"]
    else:
        L += ["        return %s" % zone_value(f, get)]
    L += ["#@E",
          "",
          "",
          "def parse_%s(text, options=None):" % f["slug"],
          "    lines = [(n, line) for n, line in enumerate(text.split(\"\\n\"), 1) if line.strip()]",
          "    if not lines:",
          "        raise ParseError(\"missing header\").at(\"line 1\")",
          "    header = [c.strip() for c in lines[0][1].split(DELIMITER)]",
          "    for key in NEEDED:",
          "        if FIELD_NAMES[key] not in header:",
          "            raise ParseError(\"missing column '%s'\" % FIELD_NAMES[key]).at(\"line %d\" % lines[0][0])",
          "    drafts = []",
          "    for n, line in lines[1:]:",
          "        row = _Row(header, [c.strip() for c in line.split(DELIMITER)], n)",
          "        start, end = row.span()",
          "        raw = {\"room\": row.text(\"room\"), \"start\": start, \"end\": end, \"title\": row.text(\"title\"),",
          "               \"owner\": row.text(\"owner\"), \"attendees\": row.text(\"attendees\")}",
          "#@A",
          "        raw[\"tz\"] = row.zone()",
          "#@E",
          "        drafts.append(make_draft(raw, row.where))",
          "    return drafts"]
    return L


def feed_table(f, required):
    n = f["names"]
    simple = [("room", n["room"]), ("title", n["title"]), ("owner", n["owner"]), ("attendees", n["attendees"])]
    get = lambda k: "values.get(%r, \"\")" % n[k]
    L = ["# roomkit field <- feed column, for the fields copied as they are",
         "COPIED = ["]
    for k, col in simple:
        L.append("    (%r, %r)," % (k, col))
    L += ["]",
          "SPAN_COLUMNS = %r" % ({k: n[k] for k in ("date", "start", "end", "minutes") if k in n},),
          "NEEDED_COLUMNS = %r" % (tuple(n[k] for k in required),)]
    if f["zone"] in ("label", "compact", "minutes"):
        conv = "lambda text, where: text.strip()" if f["zone"] == "label" else "_zone"
        L += ["#@A",
              "# zone column and how to read it",
              "ZONE_COLUMN = (%r, %s)" % (n["zone"], conv),
              "#@E"]
    L += ["", "",
          "def _read(values, where):",
          "    raw = {key: values.get(col, \"\") for key, col in COPIED}"]
    L += span_lines(f, get, 4)
    L += ["    raw[\"start\"], raw[\"end\"] = start, end"]
    if f["zone"] == "suffix":
        L += ["#@A", "    raw[\"tz\"] = zone_text.strip()", "#@E"]
    else:
        L += ["#@A", "    col, read = ZONE_COLUMN", "    raw[\"tz\"] = read(values.get(col, \"\"), where)", "#@E"]
    L += ["    return raw", "", "",
          "def parse_%s(text, options=None):" % f["slug"],
          "    header, drafts = None, []",
          "    for n, line in enumerate(text.split(\"\\n\"), 1):",
          "        if line.strip() == \"\":",
          "            continue",
          "        cells = [c.strip() for c in line.split(DELIMITER)]",
          "        if header is None:",
          "            header = cells",
          "            for col in NEEDED_COLUMNS:",
          "                if col not in header:",
          "                    raise ParseError(\"missing column '%s'\" % col).at(\"line %d\" % n)",
          "            continue",
          "        where = \"line %d\" % n",
          "        drafts.append(make_draft(_read(dict(zip(header, cells)), where), where))",
          "    if header is None:",
          "        raise ParseError(\"missing header\").at(\"line 1\")",
          "    return drafts"]
    return L


def feed_kvblock(f, required):
    n = f["names"]
    get = lambda k: "block.get(%r, \"\")" % n[k]
    L = ["def _blocks(text):",
         '    """[(first line, {key: value})] of the blank-line separated blocks."""',
         "    blocks, current = [], None",
         "    for n, line in enumerate(text.split(\"\\n\"), 1):",
         "        s = line.strip()",
         "        if s.startswith(\"#\"):",
         "            continue",
         "        if not s:",
         "            if current is not None:",
         "                blocks.append(current)",
         "                current = None",
         "            continue",
         "        if SEPARATOR not in s:",
         "            raise ParseError(\"bad line\").at(\"line %d\" % n)",
         "        key, _, value = s.partition(SEPARATOR)",
         "        if current is None:",
         "            current = (n, {})",
         "        current[1][key.strip()] = value.strip()",
         "    if current is not None:",
         "        blocks.append(current)",
         "    return blocks",
         "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    drafts = []",
         "    for first, block in _blocks(text):",
         "        where = \"line %d\" % first"]
    L += span_lines(f, get, 8) + raw_lines(f, get, 8)
    L += ["#@A", "        raw[\"tz\"] = %s" % zone_value(f, get), "#@E",
          "        drafts.append(make_draft(raw, where))",
          "    return drafts"]
    return L


def feed_kvregex(f, required):
    n = f["names"]
    get = lambda k: "fields.get(%r, \"\")" % n[k]
    sep = f["sep"].strip()
    L = ["_LINE = re.compile(r\"^\\s*([^%s\\s]+)\\s*%s\\s*(.*?)\\s*$\")" % (sep, sep),
         "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    drafts = []",
         "    first, fields = None, {}",
         "    lines = text.split(\"\\n\") + [\"\"]",
         "    for n, line in enumerate(lines, 1):",
         "        if line.strip().startswith(\"#\"):",
         "            continue",
         "        if line.strip():",
         "            m = _LINE.match(line)",
         "            if not m:",
         "                raise ParseError(\"bad line\").at(\"line %d\" % n)",
         "            if first is None:",
         "                first = n",
         "            fields[m.group(1)] = m.group(2)",
         "            continue",
         "        if first is None:",
         "            continue",
         "        where = \"line %d\" % first"]
    L += span_lines(f, get, 8) + raw_lines(f, get, 8)
    L += ["#@A", "        raw[\"tz\"] = %s" % zone_value(f, get), "#@E",
          "        drafts.append(make_draft(raw, where))",
          "        first, fields = None, {}",
          "    return drafts"]
    return L


def feed_jsonobj(f, required):
    n = f["names"]
    get = lambda k: "_text(obj, %r)" % n[k]
    L = ["def _text(obj, key):",
         "    v = obj.get(key, \"\")",
         "    return str(v) if isinstance(v, (str, int)) and not isinstance(v, bool) else \"\"",
         "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    drafts = []",
         "    for n, line in enumerate(text.split(\"\\n\"), 1):",
         "        if not line.strip():",
         "            continue",
         "        where = \"line %d\" % n",
         "        try:",
         "            obj = json.loads(line)",
         "        except ValueError:",
         "            raise ParseError(\"bad JSON\").at(where)",
         "        if not isinstance(obj, dict):",
         "            raise ParseError(\"expected an object\").at(where)"]
    L += span_lines(f, get, 8) + raw_lines(f, get, 8)
    L += ["#@A", "        raw[\"tz\"] = %s" % zone_value(f, get), "#@E",
          "        drafts.append(make_draft(raw, where))",
          "    return drafts"]
    return L


def feed_jsonpairs(f, required):
    n = f["names"]
    get = lambda k: "item.get(%r, \"\")" % n[k]
    L = ["KEYS = %r" % (tuple(sorted(n.values())),),
         "", "",
         "def _object(line, where):",
         "    try:",
         "        pairs = json.loads(line, object_pairs_hook=OrderedDict)",
         "    except ValueError:",
         "        raise ParseError(\"bad JSON\").at(where)",
         "    if not isinstance(pairs, dict):",
         "        raise ParseError(\"expected an object\").at(where)",
         "    return {k: (str(v) if isinstance(v, (str, int)) and not isinstance(v, bool) else \"\")",
         "            for k, v in pairs.items() if k in KEYS}",
         "", "",
         "def parse_%s(text, options=None):" % f["slug"],
         "    drafts = []",
         "    for n, line in enumerate(text.split(\"\\n\"), 1):",
         "        if line.strip():",
         "            where = \"line %d\" % n",
         "            item = _object(line, where)"]
    L += span_lines(f, get, 12) + raw_lines(f, get, 12)
    L += ["#@A", "            raw[\"tz\"] = %s" % zone_value(f, get), "#@E",
          "            drafts.append(make_draft(raw, where))",
          "    return drafts"]
    return L


# ------------------------------------------------------------------------------------------------ calendars
def cal_value_expr(kind, var, dtf):
    if kind == "id":
        return "str(%s.id)" % var
    if kind == "attendees":
        return "str(%s.attendees)" % var
    if kind in ("start", "end"):
        return "_stamp(%s.%s)" % (var, kind)
    return "%s.%s" % (var, kind)


def stamp_helper(dtf):
    py = {"YYYY-MM-DDTHH:MM": "%Y-%m-%dT%H:%M", "YYYYMMDDTHHMM": "%Y%m%dT%H%M", "DD.MM.YYYY HH:MM": "%d.%m.%Y %H:%M",
          "MM/DD/YYYY HH:MM": "%m/%d/%Y %H:%M", "YYYY-MM-DD HH:MM": "%Y-%m-%d %H:%M"}[dtf]
    return ["def _stamp(dt):",
            '    """%s."""' % dtf,
            "    return %r %% (%s)" % (py.replace("%Y", "%04d").replace("%m", "%02d").replace("%d", "%02d")
                                     .replace("%H", "%02d").replace("%M", "%02d"),
                                     ", ".join({"Y": "dt.year", "m": "dt.month", "d": "dt.day", "H": "dt.hour",
                                                "M": "dt.minute"}[c] for c in
                                               [py[i + 1] for i in range(len(py) - 1) if py[i] == "%"]))]


def cal_doc(c):
    lines = ['"""%s: bookings for %s (registered as exporter "cal_%s"; docs/CALENDARS.md).' % (
        c["slug"], c["system"], c["slug"]), "", "Fields, in the order written:", ""]
    for name, kind in c["fields"]:
        what = {"id": "the booking id", "room": "the room id", "start": "the start (%s)" % c["dt_fmt"],
                "end": "the end (%s)" % c["dt_fmt"], "title": "the title", "owner": "the owner",
                "attendees": "the number of attendees"}[kind]
        lines.append("    %-12s %s" % (name, what))
    lines.append('"""')
    return lines


def gen_calendar(c):
    style = c["style"]
    L = cal_doc(c)
    if style == "dictwriter":
        L += ["import csv", "import io"]
    if style in ("jsondump", "jsonbuild"):
        L += ["import json"]
    L += ["", "OPTIONS = ()", "REQUIRED = ()", "SYSTEM = %r" % c["system"]]
    if c["notation"] == "delimited":
        L.append("DELIMITER = %r" % c["delim"])
    if c["notation"] == "keyvalue":
        L.append("SEPARATOR = %r" % c["sep"])
    L += ["", ""] + stamp_helper(c["dt_fmt"]) + ["", ""]
    L += {"tuple": cal_tuple, "format": cal_format, "dictwriter": cal_dictwriter, "klass": cal_klass,
          "kvlines": cal_kvlines, "kvjoin": cal_kvjoin, "jsondump": cal_jsondump, "jsonbuild": cal_jsonbuild}[style](c)
    return "\n".join(L) + "\n"


def cal_tuple(c):
    names = [n for n, _ in c["fields"]]
    L = ["#@B", "FIELDS = %r" % (tuple(names),), "#@A", "FIELDS = %r" % (tuple(names + ["tz"]),), "#@E", "", "",
         "def _values(b):",
         "    vals = ["]
    for _, kind in c["fields"]:
        L.append("        %s," % cal_value_expr(kind, "b", c["dt_fmt"]))
    L += ["    ]", "#@A", "    vals.append(b.tz)", "#@E", "    return vals", "", "",
          "def export_%s(bookings, options=None):" % c["slug"],
          "    lines = [DELIMITER.join(FIELDS)] + [DELIMITER.join(_values(b)) for b in bookings]",
          "    return \"\\n\".join(lines) + \"\\n\""]
    return L


def cal_format(c):
    names = [n for n, _ in c["fields"]]
    d = c["delim"]
    tmpl = d.join("{%s}" % kind for _, kind in c["fields"])
    L = ["#@B", "HEADER = %r" % d.join(names), "ROW = %r" % tmpl, "#@A",
         "HEADER = %r" % d.join(names + ["tz"]), "ROW = %r" % (tmpl + d + "{tz}"), "#@E", "", "",
         "def export_%s(bookings, options=None):" % c["slug"],
         "    out = [HEADER]",
         "    for b in bookings:",
         "        out.append(ROW.format(id=b.id, room=b.room, start=_stamp(b.start), end=_stamp(b.end), title=b.title,",
         "#@B",
         "                              owner=b.owner, attendees=b.attendees))",
         "#@A",
         "                              owner=b.owner, attendees=b.attendees, tz=b.tz))",
         "#@E",
         "    return \"\\n\".join(out) + \"\\n\""]
    return L


def cal_dictwriter(c):
    L = ["COLUMNS = ["]
    for name, kind in c["fields"]:
        L.append("    (%r, %r)," % (name, kind))
    L += ["#@A", "    (\"tz\", \"tz\"),", "#@E", "]", "", "",
          "def _row(b):",
          "    row = {}",
          "    for name, kind in COLUMNS:",
          "        value = getattr(b, kind)",
          "        row[name] = _stamp(value) if kind in (\"start\", \"end\") else str(value)",
          "    return row",
          "", "",
          "def export_%s(bookings, options=None):" % c["slug"],
          "    buf = io.StringIO()",
          "    w = csv.DictWriter(buf, fieldnames=[name for name, _ in COLUMNS], delimiter=DELIMITER,",
          "                       lineterminator=\"\\n\")",
          "    w.writeheader()",
          "    for b in bookings:",
          "        w.writerow(_row(b))",
          "    return buf.getvalue()"]
    return L


def cal_klass(c):
    L = ["class _Writer:",
         '    """Collects the rows of the export."""',
         "",
         "    def __init__(self):",
         "        self.lines = []",
         "",
         "    def header(self):",
         "        names = %r" % ([n for n, _ in c["fields"]],),
         "#@A",
         "        names.append(\"tz\")",
         "#@E",
         "        self.lines.append(DELIMITER.join(names))",
         "",
         "    def booking(self, b):",
         "        cells = []"]
    for _, kind in c["fields"]:
        L.append("        cells.append(%s)" % cal_value_expr(kind, "b", c["dt_fmt"]))
    L += ["#@A", "        cells.append(b.tz)", "#@E",
          "        self.lines.append(DELIMITER.join(cells))",
          "",
          "    def text(self):",
          "        return \"\\n\".join(self.lines) + \"\\n\"",
          "", "",
          "def export_%s(bookings, options=None):" % c["slug"],
          "    w = _Writer()",
          "    w.header()",
          "    for b in bookings:",
          "        w.booking(b)",
          "    return w.text()"]
    return L


def cal_kvlines(c):
    L = ["def _block(b):",
         "    return ["]
    for name, kind in c["fields"]:
        L.append("        %r + SEPARATOR + %s," % (name, cal_value_expr(kind, "b", c["dt_fmt"])))
    L += ["#@A", "        \"tz\" + SEPARATOR + b.tz,", "#@E", "    ]", "", "",
          "def export_%s(bookings, options=None):" % c["slug"],
          "    blocks = [\"\\n\".join(_block(b)) for b in bookings]",
          "    return \"\\n\\n\".join(blocks) + \"\\n\" if blocks else \"\""]
    return L


def cal_kvjoin(c):
    L = ["#@B", "KEYS = %r" % ([[n, k] for n, k in c["fields"]],), "#@A",
         "KEYS = %r" % ([[n, k] for n, k in c["fields"]] + [["tz", "tz"]],), "#@E", "", "",
         "def export_%s(bookings, options=None):" % c["slug"],
         "    out = []",
         "    for b in bookings:",
         "        if out:",
         "            out.append(\"\")",
         "        for name, kind in KEYS:",
         "            value = getattr(b, kind)",
         "            out.append(\"%s%s%s\" % (name, SEPARATOR, _stamp(value) if kind in (\"start\", \"end\") else value))",
         "    return \"\\n\".join(out) + \"\\n\" if out else \"\""]
    return L


def cal_jsondump(c):
    L = ["def _object(b):",
         "    obj = {}"]
    for name, kind in c["fields"]:
        if kind in ("id", "attendees"):
            L.append("    obj[%r] = b.%s" % (name, kind))
        else:
            L.append("    obj[%r] = %s" % (name, cal_value_expr(kind, "b", c["dt_fmt"])))
    L += ["#@A", "    obj[\"tz\"] = b.tz", "#@E", "    return obj", "", "",
          "def export_%s(bookings, options=None):" % c["slug"],
          "    return \"\".join(json.dumps(_object(b)) + \"\\n\" for b in bookings)"]
    return L


def cal_jsonbuild(c):
    pairs = []
    for name, kind in c["fields"]:
        if kind in ("id", "attendees"):
            pairs.append("(%r, b.%s)" % (name, kind))
        else:
            pairs.append("(%r, %s)" % (name, cal_value_expr(kind, "b", c["dt_fmt"])))
    L = ["def export_%s(bookings, options=None):" % c["slug"],
         "    lines = []",
         "    for b in bookings:",
         "        pairs = [%s]" % ", ".join(pairs),
         "#@A",
         "        pairs.append((\"tz\", b.tz))",
         "#@E",
         "        lines.append(json.dumps(dict(pairs)))",
         "    return \"\".join(line + \"\\n\" for line in lines)"]
    return L


# ------------------------------------------------------------------------------------------------ grouped reports
GROUP_DOC = {
    "weekday": "the weekday of the start (Mon, Tue, ... Sun; rows in that order)",
    "hour": "the hour of the start (\"09\")",
    "week": "the ISO week of the start date (\"2026-W10\")",
    "daypart": "the part of day of the start: morning (hour < 12), afternoon (12-16), evening (17 and later); rows "
               "in that order",
    "month": "the month of the start (\"2026-03\")",
    "dateroom": "the start date and the room (\"2026-03-02 atlas\")",
}
METRIC_DOC = {"count": "the number of bookings", "minutes": "their minutes", "attendees": "their attendees",
              "owners": "the number of different owners"}


def group_expr(group, var):
    return {"weekday": "WEEKDAYS[%s.weekday()]" % var,
            "hour": "\"%%02d\" %% %s.hour" % var,
            "week": "\"%%04d-W%%02d\" %% %s.isocalendar()[:2]" % var,
            "daypart": "(\"morning\" if %s.hour < 12 else \"afternoon\" if %s.hour < 17 else \"evening\")" % (var, var),
            "month": "\"%%04d-%%02d\" %% (%s.year, %s.month)" % (var, var),
            "dateroom": "\"%%04d-%%02d-%%02d %%s\" %% (%s.year, %s.month, %s.day, b.room)" % (var, var, var)}[group]


def order_expr(group):
    if group == "weekday":
        return "WEEKDAYS.index(key)"
    if group == "daypart":
        return "(\"morning\", \"afternoon\", \"evening\").index(key)"
    return "key"


def gen_grouped(g):
    slug, group, metric, style = g["slug"], g["group"], g["metric"], g["style"]
    L = ['"""Grouped report by_%s: %s per group of bookings, grouped by %s (docs/GROUPED.md)."""' % (
        slug, METRIC_DOC[metric], GROUP_DOC[group].split(";")[0])]
    if style == "groupby":
        L.append("from itertools import groupby")
    if style == "counter":
        L.append("from collections import defaultdict")
    L += ["", "from roomkit.reports.common import in_range, opt_date, opt_text"]
    conv = {"dict": ["from roomkit.reports.common import opt_zone"],
            "groupby": ["from roomkit.util import zones"],
            "counter": ["from roomkit.reports.common import opt_zone", "from roomkit.util.zones import from_utc"],
            "klass": ["from roomkit.reports.common import opt_zone"]}[style]
    L += ["#@A"] + conv + ["#@E", ""]
    L += ["#@B", "OPTIONS = (\"date_from\", \"date_to\", \"room\")", "#@A",
          "OPTIONS = (\"date_from\", \"date_to\", \"room\", \"tz\")", "#@E", "REQUIRED = ()"]
    if group == "weekday":
        L.append("WEEKDAYS = [\"Mon\", \"Tue\", \"Wed\", \"Thu\", \"Fri\", \"Sat\", \"Sun\"]")
    L += ["", ""]
    mval = {"count": "1", "minutes": "b.minutes", "attendees": "b.attendees", "owners": "b.owner"}[metric]
    if style == "dict":
        L += ["def rows(cal, options):",
              "    first, last = opt_date(options, \"date_from\"), opt_date(options, \"date_to\")",
              "    only = opt_text(options, \"room\")",
              "#@A", "    zone = opt_zone(options)", "#@E",
              "    groups = {}",
              "    for b in cal.bookings_sorted():",
              "        if only and b.room != only:",
              "            continue",
              "#@B", "        start = b.start", "#@A", "        start = b.in_zone(zone)[0]", "#@E",
              "        if not in_range(start.date(), first, last):",
              "            continue",
              "        key = %s" % group_expr(group, "start")]
        if metric == "owners":
            L += ["        groups.setdefault(key, set()).add(b.owner)",
                  "    values = {k: len(v) for k, v in groups.items()}"]
        else:
            L += ["        groups[key] = groups.get(key, 0) + %s" % mval, "    values = groups"]
        L += ["    return [{\"group\": key, \"value\": values[key]} for key in sorted(values, key=lambda key: %s)]"
              % order_expr(group)]
    elif style == "groupby":
        L += ["def _start(b, options):",
              "#@B", "    return b.start", "#@A",
              "    label = zones.parse_zone(options[\"tz\"]) if options.get(\"tz\") else \"UTC\"",
              "    return zones.convert(b.start, b.tz, label)", "#@E",
              "", "",
              "def rows(cal, options):",
              "    first, last = opt_date(options, \"date_from\"), opt_date(options, \"date_to\")",
              "    only = opt_text(options, \"room\")",
              "    keyed = []",
              "    for b in cal.bookings_sorted():",
              "        start = _start(b, options)",
              "        if (not only or b.room == only) and in_range(start.date(), first, last):",
              "            key = %s" % group_expr(group, "start"),
              "            keyed.append((%s, key, b))" % order_expr(group),
              "    keyed.sort(key=lambda t: (t[0], t[2].id))",
              "    out = []",
              "    for (_, key), items in groupby(keyed, key=lambda t: (t[0], t[1])):",
              "        items = [b for _, _, b in items]"]
        if metric == "owners":
            L.append("        out.append({\"group\": key, \"value\": len({b.owner for b in items})})")
        else:
            L.append("        out.append({\"group\": key, \"value\": sum(%s for b in items)})" % mval)
        L.append("    return out")
    elif style == "counter":
        L += ["def rows(cal, options):",
              "    first, last = opt_date(options, \"date_from\"), opt_date(options, \"date_to\")",
              "    only = opt_text(options, \"room\")",
              "#@A", "    display = opt_zone(options)", "#@E",
              "    bucket = defaultdict(list)",
              "    for b in cal.bookings_sorted():",
              "#@B", "        start = b.start", "#@A", "        start = from_utc(b.utc_start(), display)", "#@E",
              "        if only and b.room != only or not in_range(start.date(), first, last):",
              "            continue",
              "        bucket[%s].append(b)" % group_expr(group, "start")]
        if metric == "owners":
            L.append("    totals = {key: len({b.owner for b in items}) for key, items in bucket.items()}")
        else:
            L.append("    totals = {key: sum(%s for b in items) for key, items in bucket.items()}" % mval)
        L += ["    order = sorted(totals, key=lambda key: %s)" % order_expr(group),
              "    return [{\"group\": key, \"value\": totals[key]} for key in order]"]
    else:
        L += ["class _Groups:",
              "    def __init__(self):",
              "        self.items = {}",
              "",
              "    def add(self, key, b):",
              "        self.items.setdefault(key, []).append(b)",
              "",
              "    def value(self, key):",
              "        items = self.items[key]"]
        if metric == "owners":
            L.append("        return len({b.owner for b in items})")
        else:
            L.append("        return sum(%s for b in items)" % mval)
        L += ["",
              "    def rows(self):",
              "        return [{\"group\": key, \"value\": self.value(key)} for key in sorted(self.items, key=lambda key: %s)]"
              % order_expr(group),
              "", "",
              "def rows(cal, options):",
              "    first, last = opt_date(options, \"date_from\"), opt_date(options, \"date_to\")",
              "    only = opt_text(options, \"room\")",
              "    groups = _Groups()",
              "    for b in cal.bookings_sorted():",
              "        if only and b.room != only:",
              "            continue",
              "#@B", "        when = b.start", "#@A", "        when, _ = b.in_zone(opt_zone(options))", "#@E",
              "        if in_range(when.date(), first, last):",
              "            groups.add(%s, b)" % group_expr(group, "when"),
              "    return groups.rows()"]
    return "\n".join(L) + "\n"


# ------------------------------------------------------------------------------------------------ notifiers
CLOCK = {"HH:MM": "\"%02d:%02d\" % (t.hour, t.minute)", "HH.MM": "\"%02d.%02d\" % (t.hour, t.minute)",
         "HHhMM": "\"%02dh%02d\" % (t.hour, t.minute)",
         "h:MMam": "\"%d:%02d%s\" % (t.hour % 12 or 12, t.minute, \"am\" if t.hour < 12 else \"pm\")"}
LINE_ARGS = "time=_clock(start), title=b.title, room=b.room, owner=b.owner, attendees=b.attendees"


def gen_notifier(nt):
    style = nt["style"]
    L = ['"""%s: the bookings of one day as a %s message (registered as notifier "%s"; docs/NOTIFY.md).' % (
        nt["slug"], nt["channel"], nt["slug"]), "", "Line: %s" % nt["line"], '"""']
    if style == "join":
        L += ["#@A", "from datetime import timedelta", "#@E", ""]
    L += ["from roomkit.reports.common import opt_text", "from roomkit.util.timefmt import fmt_date, parse_date"]
    L += ["#@A"] + {"listcomp": ["from roomkit.reports.common import opt_zone"],
                    "loop": ["from roomkit.util import zones"],
                    "klass": ["from roomkit.reports.common import opt_zone", "from roomkit.util.zones import from_utc"],
                    "join": ["from roomkit.util.zones import offset_minutes, parse_zone"]}[style] + ["#@E", ""]
    L += ["#@B", "OPTIONS = (\"date\", \"room\")", "#@A", "OPTIONS = (\"date\", \"room\", \"tz\")", "#@E",
          "REQUIRED = (\"date\",)", "CHANNEL = %r" % nt["channel"], "LINE = %r" % nt["line"],
          "HEADER = %r" % nt["header"], "EMPTY = %r" % nt["empty"], "", "",
          "def _clock(t):", '    """%s."""' % nt["time_fmt"], "    return " + CLOCK[nt["time_fmt"]], "", ""]
    if style == "listcomp":
        L += ["def _wrap(day, lines):",
              "    out = [HEADER.format(date=fmt_date(day))] if HEADER else []",
              "    return \"\\n\".join(out + (lines or [EMPTY])) + \"\\n\"",
              "", "",
              "def render(cal, options):",
              "    day = parse_date(options[\"date\"])",
              "    only = opt_text(options, \"room\")",
              "#@A", "    zone = opt_zone(options)", "#@E",
              "    picked = []",
              "    for b in cal.bookings_sorted():",
              "#@B", "        start, key = b.start, (b.start, b.id)",
              "#@A", "        start, key = b.in_zone(zone)[0], (b.utc_start(), b.id)", "#@E",
              "        if start.date() == day and (not only or b.room == only):",
              "            picked.append((key, start, b))",
              "    picked.sort(key=lambda t: t[0])",
              "    return _wrap(day, [LINE.format(%s) for _, start, b in picked])" % LINE_ARGS]
    elif style == "loop":
        L += ["def _local(b, options):",
              "#@B", "    return b.start",
              "#@A", "    return zones.convert(b.start, b.tz, zones.parse_zone(options.get(\"tz\") or \"UTC\"))", "#@E",
              "", "",
              "def _order(b):",
              "#@B", "    return (b.start, b.id)", "#@A", "    return (b.utc_start(), b.id)", "#@E",
              "", "",
              "def render(cal, options):",
              "    day = parse_date(options[\"date\"])",
              "    only = opt_text(options, \"room\")",
              "    text = [HEADER.format(date=fmt_date(day))] if HEADER else []",
              "    count = 0",
              "    for b in sorted(cal.bookings.values(), key=_order):",
              "        start = _local(b, options)",
              "        if start.date() != day or (only and b.room != only):",
              "            continue",
              "        text.append(LINE.format(%s))" % LINE_ARGS,
              "        count += 1",
              "    if not count:",
              "        text.append(EMPTY)",
              "    return \"\\n\".join(text) + \"\\n\""]
    elif style == "klass":
        L += ["class _Message:",
              "    def __init__(self, day):",
              "        self.day = day",
              "        self.items = []",
              "",
              "    def offer(self, key, start, b):",
              "        if start.date() == self.day:",
              "            self.items.append((key, start, b))",
              "",
              "    def text(self):",
              "        lines = [HEADER.format(date=fmt_date(self.day))] if HEADER else []",
              "        for _, start, b in sorted(self.items, key=lambda t: t[0]):",
              "            lines.append(LINE.format(%s))" % LINE_ARGS,
              "        if len(self.items) == 0:",
              "            lines.append(EMPTY)",
              "        return \"\\n\".join(lines) + \"\\n\"",
              "", "",
              "def render(cal, options):",
              "    msg = _Message(parse_date(options[\"date\"]))",
              "    only = opt_text(options, \"room\")",
              "#@A", "    display = opt_zone(options)", "#@E",
              "    for b in cal.bookings_sorted():",
              "        if only and b.room != only:",
              "            continue",
              "#@B", "        msg.offer((b.start, b.id), b.start, b)",
              "#@A", "        msg.offer((b.utc_start(), b.id), from_utc(b.utc_start(), display), b)", "#@E",
              "    return msg.text()"]
    else:
        L += ["def render(cal, options):",
              "    day = parse_date(options[\"date\"])",
              "    only = opt_text(options, \"room\")",
              "#@A", "    shift = timedelta(minutes=offset_minutes(parse_zone(options.get(\"tz\") or \"UTC\")))", "#@E",
              "    rows = []",
              "    for b in cal.bookings_sorted():",
              "#@B", "        start, key = b.start, (b.start, b.id)",
              "#@A", "        start, key = b.utc_start() + shift, (b.utc_start(), b.id)", "#@E",
              "        if start.date() == day and (not only or b.room == only):",
              "            rows.append((key, LINE.format(%s)))" % LINE_ARGS,
              "    head = [HEADER.format(date=fmt_date(day))] if HEADER else []",
              "    body = [line for _, line in sorted(rows)] or [EMPTY]",
              "    return \"\\n\".join(head + body) + \"\\n\""]
    return "\n".join(L) + "\n"


def notify_doc(table):
    out = ["# Notification channels", "",
           "Generated reference of the notification channels in roomkit/notify/ (one module per channel). Each is a",
           "registry notifier (`roomkit notify CHANNEL DATE [--room R]`, `GET /notify/CHANNEL?date=D[&room=R]`) that",
           "writes the bookings starting on `date` (option room: one room only) as a message: the header line if the",
           "channel has one, then one line per booking sorted by start, then id (or the channel's empty text when",
           "there is none), `\\n` after every line. In a line, `{time}` is the start in the channel's time format,",
           "`{title}`, `{room}`, `{owner}`, `{attendees}` the booking's; `{date}` in a header is `YYYY-MM-DD`.",
           "Time formats: `HH:MM`, `HH.MM`, `HHhMM` (`09h05`), `h:MMam` (12-hour clock, `9:05am`, `12:00pm` is noon).",
           ""]
    for nt in table["notifiers"]:
        out += ["## %s" % nt["slug"], "",
                "- line: `%s`" % nt["line"],
                "- header: %s" % ("none" if nt["header"] is None else "`%s`" % nt["header"]),
                "- empty text: `%s`" % nt["empty"],
                "- time format: `%s`" % nt["time_fmt"], ""]
    return "\n".join(out)


# ------------------------------------------------------------------------------------------------ docs
ZONE_DOC = {
    "label": "column `{z}`: a zone code or literal offset",
    "compact": "column `{z}`: a compact UTC offset `+HHMM`/`-HHMM` (`+0530` is +05:30)",
    "minutes": "column `{z}`: the UTC offset in whole minutes east of UTC (`330` is +05:30, `-180` is -03:00)",
    "suffix": "no zone column: the start time may be followed by one space and a zone code or literal offset "
              "(`09:00 CET`)",
}


def feeds_doc(table):
    out = ["# Partner feeds", "",
           "Generated reference of the partner-feed importers in roomkit/io/feeds/ (one module per partner). Each",
           "is a registry importer named `feed_<partner>` (`roomkit import feed_acme FILE`, `POST /import/feed_acme`,",
           "pipeline inputs) and turns the partner's text into drafts that are merged like every other import.", "",
           "## Common rules", "",
           "- Delimited feeds: the first non-empty line is the header; fields are separated by the delimiter and never",
           "  contain it (there is no quoting); cells are stripped; columns not listed are ignored; empty lines are",
           "  skipped. A data row's place is `line N` (the header is line 1 if it is the first line). A required",
           "  column missing from the header: `line N: missing column 'NAME'` (N = the header line).",
           "- Key-value feeds: blocks of `key<separator>value` lines separated by blank lines; lines starting with `#`",
           "  are skipped; unknown keys are ignored; a line without the separator: `line N: bad line`. A block's place",
           "  is `line N` of its first line.",
           "- JSON-lines feeds: one JSON object per non-empty line, values strings or integers; unknown keys are",
           "  ignored; `line N: bad JSON`, `line N: expected an object`.",
           "- Dates and times are written in the partner's format: errors `bad date 'X' (want FORMAT)`,",
           "  `bad time 'X' (want FORMAT)`. `h:MMam` is the 12-hour clock with `am`/`pm` in lower case and no",
           "  leading zero (`9:05am`, `12:00pm` is noon, `12:30am` is 00:30).",
           "- Spans: *date, start, end* books the date from the start time to the end time; *date, start, minutes*",
           "  books the date from the start time for that many minutes (an integer >= 1, else `bad minutes 'X'`; the",
           "  end may fall on a later day); *start, end* gives each as `DATE TIME` (one space between).",
           "- Required: room, the span fields, title, owner; attendees is optional (empty: 1). An empty required",
           "  value is reported by the field rules (`missing field 'room'`) or as a bad date/time.",
           "- Zones: each feed tells its zone in one of the notations listed with it. roomkit has no zones yet: the",
           "  zone column, or the zone after the start time, is accepted and not used (times are taken as written);",
           "  an empty zone or no suffix means no zone.", ""]
    for f in table["feeds"]:
        n = f["names"]
        if f["notation"] == "delimited":
            nota = "delimited by `%s`, with a header row" % ("\\t" if f["delim"] == "\t" else f["delim"])
        elif f["notation"] == "keyvalue":
            nota = "key-value blocks, separator `%s` (`key%svalue`)" % (f["sep"].strip(), f["sep"])
        else:
            nota = "JSON lines"
        if f["span"] == "datetimes":
            span = "start `%s`, end `%s` (each `%s %s`)" % (n["start"], n["end"], f["date_fmt"], f["time_fmt"])
        elif f["span"] == "from-until":
            span = "date `%s` (%s), start `%s`, end `%s` (%s)" % (n["date"], f["date_fmt"], n["start"], n["end"],
                                                                f["time_fmt"])
        else:
            span = "date `%s` (%s), start `%s` (%s), minutes `%s`" % (n["date"], f["date_fmt"], n["start"],
                                                                    f["time_fmt"], n["minutes"])
        out += ["## feed_%s (%s)" % (f["slug"], f["partner"]), "",
                "- format: %s" % nota,
                "- room `%s`, %s, title `%s`, owner `%s`, attendees `%s` (optional)" % (
                    n["room"], span, n["title"], n["owner"], n["attendees"]),
                "- zone: %s" % ZONE_DOC[f["zone"]].format(z=n.get("zone", "")),
                "", "Example:", ""]
        out += ["    " + x.replace("\t", "\\t") for x in feed_example(f).rstrip("\n").split("\n")] + [""]
    return "\n".join(out)


def calendars_doc(table):
    out = ["# Calendar exporters", "",
           "Generated reference of the exporters for other calendar systems in roomkit/io/calendars/ (one module per",
           "system). Each is a registry exporter named `cal_<system>` (`roomkit export cal_orbit FILE`, pipeline",
           "outputs); it writes the bookings in the order given, each booking's own times.", "",
           "- Delimited: a header row of the field names, then one row per booking, fields joined by the delimiter",
           "  (no quoting), `\\n` after every line.",
           "- Key-value: one block per booking, one `name<separator>value` line per field, blocks separated by one",
           "  blank line, a final `\\n` (nothing at all without bookings).",
           "- JSON lines: one object per booking (`json.dumps` defaults), keys in the order listed, `\\n` after each.",
           "  id and attendees are numbers there; everything else is text.", ""]
    for c in table["calendars"]:
        if c["notation"] == "delimited":
            nota = "delimited by `%s`" % ("\\t" if c["delim"] == "\t" else c["delim"])
        elif c["sep"] == ": ":
            nota = "key-value blocks, separator `: ` (a colon and one space)"
        elif c["notation"] == "keyvalue":
            nota = "key-value blocks, separator `%s`" % c["sep"]
        else:
            nota = "JSON lines"
        fields = ", ".join("`%s` (%s)" % (n, k) for n, k in c["fields"])
        out += ["## cal_%s (%s)" % (c["slug"], c["system"]), "",
                "- format: %s" % nota,
                "- fields in order: %s" % fields,
                "- start and end written as `%s`" % c["dt_fmt"], ""]
    return "\n".join(out)


def grouped_doc(table):
    out = ["# Grouped reports", "",
           "Generated reference of the grouped reports in roomkit/reports/grouped/. Each is a registry report named",
           "`by_<name>` (`roomkit report by_weekday_count`, `GET /reports/by_weekday_count`, pipeline report sinks)",
           "with the options date_from, date_to (both optional, inclusive: only bookings starting on a date in the",
           "range count) and room. Rows `{\"group\": ..., \"value\": ...}`, one per group with at least one booking,",
           "sorted by group (as text unless an order is given).", ""]
    for g in table["grouped"]:
        out += ["## by_%s" % g["slug"], "", "- group: %s" % GROUP_DOC[g["group"]],
                "- value: %s in the group" % METRIC_DOC[g["metric"]], ""]
    return "\n".join(out)


# ------------------------------------------------------------------------------------------------ generate
def init_module(doc, mapping_name, kind, prefix, entries):
    lines = ['"""%s"""' % doc, "", "%s = {" % mapping_name]
    for slug, target in entries:
        lines.append("    %r: %r," % (prefix + slug, target))
    lines.append("}")
    return "\n".join(lines) + "\n"


def generate(dest, variant, render):
    table = json.load(open(TABLE, encoding="utf-8"))
    files = {}
    for f in table["feeds"]:
        files["roomkit/io/feeds/%s.py" % f["slug"]] = gen_feed(f)
    files["roomkit/io/feeds/__init__.py"] = init_module(
        "Partner-feed importers, one module per partner (generated reference: docs/FEEDS.md).", "FEED_IMPORTERS",
        "importer", "feed_", [(f["slug"], "roomkit.io.feeds.%s:parse_%s" % (f["slug"], f["slug"]))
                              for f in table["feeds"]])
    for c in table["calendars"]:
        files["roomkit/io/calendars/%s.py" % c["slug"]] = gen_calendar(c)
    files["roomkit/io/calendars/__init__.py"] = init_module(
        "Exporters for other calendar systems (docs/CALENDARS.md).", "CALENDAR_EXPORTERS", "exporter", "cal_",
        [(c["slug"], "roomkit.io.calendars.%s:export_%s" % (c["slug"], c["slug"])) for c in table["calendars"]])
    for g in table["grouped"]:
        files["roomkit/reports/grouped/%s.py" % g["slug"]] = gen_grouped(g)
    files["roomkit/reports/grouped/__init__.py"] = init_module(
        "Grouped reports (docs/GROUPED.md).", "GROUPED_REPORTS", "report", "by_",
        [(g["slug"], "roomkit.reports.grouped.%s:rows" % g["slug"]) for g in table["grouped"]])
    for nt in table["notifiers"]:
        files["roomkit/notify/%s.py" % nt["slug"]] = gen_notifier(nt)
    files["roomkit/notify/__init__.py"] = init_module(
        "Notification channels: a day's bookings as a message (docs/NOTIFY.md).", "NOTIFIERS", "notifier", "",
        [(nt["slug"], "roomkit.notify.%s:render" % nt["slug"]) for nt in table["notifiers"]])
    files["docs/NOTIFY.md"] = notify_doc(table) + "\n"
    files["docs/FEEDS.md"] = feeds_doc(table) + "\n"
    files["docs/CALENDARS.md"] = calendars_doc(table) + "\n"
    files["docs/GROUPED.md"] = grouped_doc(table) + "\n"
    for rel in sorted(files):
        body = render(files[rel], variant, rel)
        out = os.path.join(dest, rel)
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(body)
        os.chmod(out, 0o644)


if __name__ == "__main__":
    import sys
    if sys.argv[1:] == ["table"]:
        with open(TABLE, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(make_table(), indent=1, sort_keys=True) + "\n")
    else:
        raise SystemExit("usage: gen_families.py table")
