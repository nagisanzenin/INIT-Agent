"""evo-nav-tz check (cwd = the workspace). One scenario per named item: a Python snippet that drives the
workspace's roomkit package (CLI main(), the request handler, the stores, pipelines) in a fresh process (cwd = a
scratch dir of its own, PYTHONPATH = the workspace) and records what it observed in OUT. This process compares OUT
with the expected values, which never reach workspace code. Prints one PASS/FAIL line per item."""
import json
import os
import subprocess
import sys
import threading
from datetime import date, datetime, timedelta

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
DRIVERS = os.path.join(SCRATCH, "_drivers")

HARNESS = r'''
import contextlib, io, json, os, sys, traceback
OUT = {}

def cli(*argv):
    from roomkit.cli.main import main
    o, e = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(o), contextlib.redirect_stderr(e):
        try:
            code = main([str(a) for a in argv])
        except SystemExit as x:
            code = x.code if isinstance(x.code, int) else (0 if x.code is None else 1)
        except Exception as x:
            code = "crash"
            e.write("".join(traceback.format_exception_only(type(x), x)))
    try:
        j = json.loads(o.getvalue())
    except ValueError:
        j = None
    return {"code": code, "out": o.getvalue(), "err": e.getvalue(), "json": j}

def api(store, method, path, body=None):
    from roomkit.api.app import App
    from roomkit.api.http import Request
    from roomkit.storage.factory import open_store
    if body is not None and not isinstance(body, str):
        body = json.dumps(body)
    r = App(open_store(store)).handle(Request(method, path, body=body))
    try:
        j = json.loads(r.body) if r.body else None
    except ValueError:
        j = None
    return {"status": r.status, "body": r.body, "json": j}

def write(path, text):
    d = os.path.dirname(path)
    if d:
        os.makedirs(d, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)

def read(path):
    try:
        with open(path, encoding="utf-8", newline="") as f:
            return f.read()
    except OSError:
        return None

def _dt(x):
    return "%04d-%02d-%02d %02d:%02d" % (x.year, x.month, x.day, x.hour, x.minute)

def bookings(store):
    from roomkit.storage.factory import open_store
    cal = open_store(store).load()
    return [{"id": b.id, "room": b.room, "start": _dt(b.start), "end": _dt(b.end), "title": b.title,
             "owner": b.owner, "attendees": b.attendees, "tz": getattr(b, "tz", None)}
            for b in sorted(cal.bookings.values(), key=lambda b: b.id)]

def rooms(store):
    from roomkit.storage.factory import open_store
    cal = open_store(store).load()
    return {r.id: {"name": r.name, "capacity": r.capacity, "opens": "%02d:%02d" % (r.opens.hour, r.opens.minute),
                   "closes": "%02d:%02d" % (r.closes.hour, r.closes.minute), "tags": list(r.tags),
                   "tz": getattr(r, "tz", None)} for r in cal.rooms.values()}

ROOMS_V3 = ("#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\n"
            "atlas,Atlas,8,08:00,18:00,video,UTC\nberlin,Berlin,10,07:00,19:00,video;board,CET\n"
            "kyoto,Kyoto,6,09:00,17:00,,JST\npune,Pune,4,08:00,20:00,,IST\n")

def seed(store, rows=None, extra_rooms=""):
    write(store + "/rooms.csv", ROOMS_V3 + extra_rooms)
    if rows is not None:
        write(store + "/bookings.csv", "#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
              + "".join(r + "\n" for r in rows))

try:
    exec(compile(SCENARIO, "scenario", "exec"))
except BaseException as x:
    OUT["__error__"] = "".join(traceback.format_exception_only(type(x), x))[-500:]
sys.stdout.write("\n@@OUT@@" + json.dumps(OUT, default=str) + "\n")
'''


def run_scenario(name, snippet):
    d = os.path.join(SCRATCH, "s_" + name.replace("/", "_").replace(":", "_").replace(".", "_"))
    os.makedirs(d, exist_ok=True)
    os.makedirs(DRIVERS, exist_ok=True)
    path = os.path.join(DRIVERS, name.replace("/", "_") + ".py")
    with open(path, "w", encoding="utf-8") as f:
        f.write("SCENARIO = %r\n" % snippet + HARNESS)
    env = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "TMPDIR": SCRATCH, "LANG": "C", "LC_ALL": "C",
           "PYTHONPATH": WS, "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1"}
    try:
        p = subprocess.run([sys.executable, "-S", "-s", "-B", path], cwd=d, env=env, stdin=subprocess.DEVNULL,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=12)
    except (OSError, subprocess.TimeoutExpired):
        return None
    for line in reversed(p.stdout.decode("utf-8", "replace").splitlines()):
        if line.startswith("@@OUT@@"):
            try:
                return json.loads(line[len("@@OUT@@"):])
            except ValueError:
                return None
    return None


ITEMS = []


def item(name, snippet):
    def deco(fn):
        ITEMS.append((name, snippet, fn))
        return fn
    return deco


def sub(d, want):
    """True when dict d has every key/value of want."""
    return isinstance(d, dict) and all(k in d and d[k] == v for k, v in want.items())


def res(r, code, out=None, err=None):
    return (isinstance(r, dict) and r.get("code") == code and (out is None or r.get("out") == out)
            and (err is None or r.get("err") == err))


def st(r, status, payload=None):
    return isinstance(r, dict) and r.get("status") == status and (payload is None or r.get("json") == payload)


def B(i, room, start, end, title, owner, attendees, tz):
    return {"id": i, "room": room, "start": start, "end": end, "title": title, "owner": owner,
            "attendees": attendees, "tz": tz}


def R(i, room, start, end, title, owner):
    return {"id": i, "room": room, "start": start, "end": end, "title": title, "owner": owner}


BOOKING_KEYS = ["id", "room", "start", "end", "title", "owner", "attendees", "tz"]
ROOM_KEYS = ["id", "name", "capacity", "opens", "closes", "tags", "tz"]

# Bookings shared by several scenarios (written straight into a version-3 store).
AGENDA_ROWS = [
    "1,atlas,2026-03-02 23:30,2026-03-03 00:30,Late,ana,1,UTC",
    "2,kyoto,2026-03-03 09:00,2026-03-03 10:00,Early,ken,1,JST",
    "3,pune,2026-03-03 08:00,2026-03-03 09:00,Chai,raj,1,IST",
    "4,berlin,2026-03-02 08:00,2026-03-02 09:00,Kickoff,eva,1,CET",
    "5,berlin,2026-03-02 15:30,2026-03-02 16:30,Wrap,eva,1,CET",
]
BUSY_ROWS = [
    "1,pune,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,IST",
    "2,atlas,2026-03-02 03:00,2026-03-02 04:00,B,bob,1,UTC",
    "3,kyoto,2026-03-02 12:00,2026-03-02 13:00,C,cyd,1,JST",
    "4,atlas,2026-03-02 20:00,2026-03-02 21:00,D,dan,1,UTC",
]
FREE_ROWS = [
    "1,kyoto,2026-03-02 01:00,2026-03-02 02:00,Busy,ana,1,UTC",
    "2,kyoto,2026-03-02 14:00,2026-03-02 15:00,Busy2,bob,1,JST",
]
EXPORT_ROWS = [
    "2,kyoto,2026-03-02 10:00,2026-03-02 11:00,Review,ken,3,JST",
    '1,pune,2026-03-02 09:15,2026-03-02 10:00,"Stand, up",raj,1,+05:45',
]

# JST/PST/UTC agenda rows of AGENDA_ROWS
AG_0303_JST = [R(1, "atlas", "08:30", "09:30", "Late", "ana"), R(2, "kyoto", "09:00", "10:00", "Early", "ken"),
               R(3, "pune", "11:30", "12:30", "Chai", "raj")]
AG_0302_JST = [R(4, "berlin", "16:00", "17:00", "Kickoff", "eva"), R(5, "berlin", "23:30", "00:30+1", "Wrap", "eva")]
AG_0302_PST = [R(5, "berlin", "06:30", "07:30", "Wrap", "eva"), R(1, "atlas", "15:30", "16:30", "Late", "ana"),
               R(2, "kyoto", "16:00", "17:00", "Early", "ken"), R(3, "pune", "18:30", "19:30", "Chai", "raj")]
AG_0302_UTC = [R(4, "berlin", "07:00", "08:00", "Kickoff", "eva"), R(5, "berlin", "14:30", "15:30", "Wrap", "eva"),
               R(1, "atlas", "23:30", "00:30+1", "Late", "ana")]
AG_0303_UTC = [R(2, "kyoto", "00:00", "01:00", "Early", "ken"), R(3, "pune", "02:30", "03:30", "Chai", "raj")]

FREE_JST = ["2026-03-02 09:00", "2026-03-02 11:00", "2026-03-02 12:00", "2026-03-02 13:00", "2026-03-02 15:00",
            "2026-03-02 16:00"]
FREE_UTC = ["2026-03-02 00:00", "2026-03-02 02:00", "2026-03-02 03:00", "2026-03-02 04:00", "2026-03-02 06:00",
            "2026-03-02 07:00"]
FREE_PST = ["2026-03-01 16:00", "2026-03-01 18:00", "2026-03-01 19:00", "2026-03-01 20:00", "2026-03-01 22:00",
            "2026-03-01 23:00"]

# ------------------------------------------------------------------------------------------------- items
# regression: behaviour that exists before the change and must keep working


@item("reg/cli_book_cancel", r'''
OUT["a"] = cli("--store", "s", "add-room", "atlas", "--name", "Atlas", "--capacity", "4")
OUT["b1"] = cli("--store", "s", "book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "Standup",
                "--owner", "ana", "--attendees", "3")
OUT["b2"] = cli("--store", "s", "book", "atlas", "2026-03-02 09:30", "2026-03-02 10:30", "--title", "Clash",
                "--owner", "bob")
OUT["b3"] = cli("--store", "s", "book", "atlas", "2026-03-02T10:00", "2026-03-02 11:00", "--title", "Next",
                "--owner", "bob", "--json")
OUT["bad"] = cli("--store", "s", "book", "atlas", "2026-03-02 9:00", "2026-03-02 10:00", "--title", "X",
                 "--owner", "bob")
OUT["list"] = cli("--store", "s", "list", "--json")
OUT["c"] = cli("--store", "s", "cancel", "1")
OUT["c2"] = cli("--store", "s", "cancel", "1")
OUT["left"] = [b["id"] for b in bookings("s")]
''')
def _(o):
    lst = (o["list"]["json"] or {}).get("bookings") or []
    return (res(o["a"], 0, "added room atlas\n") and res(o["b1"], 0, "booked 1\n")
            and res(o["b2"], 1, "", "error: room 'atlas' is busy: conflicts with booking 1\n")
            and o["b3"]["code"] == 0 and o["b3"]["json"]["booking"]["id"] == 2
            and o["b3"]["json"]["booking"]["start"] == "2026-03-02 10:00"
            and res(o["bad"], 2, "", "error: bad datetime '2026-03-02 9:00' (want YYYY-MM-DD HH:MM)\n")
            and len(lst) == 2 and sub(lst[0], {"id": 1, "room": "atlas", "start": "2026-03-02 09:00",
                                               "end": "2026-03-02 10:00", "title": "Standup", "owner": "ana",
                                               "attendees": 3})
            and res(o["c"], 0, "cancelled 1\n") and res(o["c2"], 1, "", "error: no booking 1\n")
            and o["left"] == [2])


@item("reg/store_v1_csv", r'''
write("s/rooms.csv", "id,name,capacity,hours\natlas,Atlas,6,08:30-17:30\nbern,Bern,3,07:00-12:00\n")
write("s/bookings.csv", "id,room,date,from,to,title,owner\n4,atlas,2026-03-02,09:00,10:30,Plan,ana\n"
      "7,bern,2026-03-03,07:30,08:00,Sync,bob\n")
OUT["rooms"] = rooms("s")
OUT["bookings"] = bookings("s")
OUT["m"] = cli("--store", "s", "migrate")
OUT["rtext"] = read("s/rooms.csv")
OUT["btext"] = read("s/bookings.csv")
OUT["bookings2"] = bookings("s")
OUT["next"] = cli("--store", "s", "book", "bern", "2026-03-03 09:00", "2026-03-03 10:00", "--title", "X",
                  "--owner", "cyd")
''')
def _(o):
    r, b = o["rooms"], o["bookings"]
    keep = ("id", "room", "start", "end", "title", "owner", "attendees")
    strip = lambda rows: [{k: x[k] for k in keep} for x in rows]
    rl, bl = o["rtext"].split("\n"), o["btext"].split("\n")
    return (sub(r.get("atlas"), {"name": "Atlas", "capacity": 6, "opens": "08:30", "closes": "17:30", "tags": []})
            and sub(r.get("bern"), {"capacity": 3, "opens": "07:00", "closes": "12:00"})
            and strip(b) == [{"id": 4, "room": "atlas", "start": "2026-03-02 09:00", "end": "2026-03-02 10:30",
                              "title": "Plan", "owner": "ana", "attendees": 1},
                             {"id": 7, "room": "bern", "start": "2026-03-03 07:30", "end": "2026-03-03 08:00",
                              "title": "Sync", "owner": "bob", "attendees": 1}]
            and res(o["m"], 0, "migrated s\n")
            and rl[0].startswith("#roomkit-schema: ") and rl[1].startswith("id,name,capacity,opens,closes,tags")
            and bl[0].startswith("#roomkit-schema: ") and bl[1].startswith("id,room,start,end,title,owner,attendees")
            and strip(o["bookings2"]) == strip(b) and res(o["next"], 0, "booked 8\n"))


@item("reg/booking_rules", r'''
cli("--store", "s", "add-room", "atlas", "--name", "Atlas", "--capacity", "4")
cli("--store", "s", "add-room", "long", "--name", "Long", "--capacity", "9", "--opens", "00:00", "--closes", "23:59")
def bk(room, a, b, *more):
    return cli("--store", "s", "book", room, a, b, "--title", "T", "--owner", "ana", *more)
OUT["hours"] = bk("atlas", "2026-03-02 07:30", "2026-03-02 08:30")
OUT["order"] = bk("atlas", "2026-03-02 10:00", "2026-03-02 09:00")
OUT["day"] = bk("atlas", "2026-03-02 09:00", "2026-03-03 09:00")
OUT["long"] = bk("long", "2026-03-02 01:00", "2026-03-02 10:00")
OUT["cap"] = bk("atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--attendees", "5")
OUT["room"] = bk("nowhere", "2026-03-02 09:00", "2026-03-02 10:00")
OUT["title"] = cli("--store", "s", "book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "",
                   "--owner", "ana")
OUT["ok"] = bk("long", "2026-03-02 00:00", "2026-03-02 08:00")
''')
def _(o):
    return (res(o["hours"], 1, "", "error: outside opening hours 08:00-18:00\n")
            and res(o["order"], 1, "", "error: end must be after start\n")
            and res(o["day"], 1, "", "error: booking must start and end on the same day\n")
            and res(o["long"], 1, "", "error: booking longer than 8 hours\n")
            and res(o["cap"], 1, "", "error: 5 attendees exceed capacity 4 of room 'atlas'\n")
            and res(o["room"], 1, "", "error: unknown room 'nowhere'\n")
            and res(o["title"], 2, "", "error: missing field 'title'\n")
            and res(o["ok"], 0, "booked 1\n"))


@item("reg/reports_v2_store", r'''
write("s/rooms.csv", "#roomkit-schema: 2\nid,name,capacity,opens,closes,tags\natlas,Atlas,8,08:00,18:00,\n"
      "bern,Bern,3,09:00,12:00,quiet\n")
write("s/bookings.csv", "#roomkit-schema: 2\nid,room,start,end,title,owner,attendees\n"
      "1,atlas,2026-03-02 09:00,2026-03-02 10:30,A,ana,2\n2,atlas,2026-03-03 13:00,2026-03-03 14:00,B,bob,1\n"
      "3,atlas,2026-03-05 10:00,2026-03-05 11:00,C,cyd,1\n4,bern,2026-03-02 09:00,2026-03-02 09:20,D,dan,1\n")
OUT["u"] = cli("--store", "s", "report", "utilization", "--from", "2026-03-02", "--to", "2026-03-03", "--json")
OUT["need"] = cli("--store", "s", "report", "utilization", "--to", "2026-03-03", "--json")
OUT["ag"] = cli("--store", "s", "agenda", "2026-03-02", "--json")
OUT["cf"] = cli("--store", "s", "report", "conflicts", "--json")
OUT["unk"] = cli("--store", "s", "report", "nope", "--json")
''')
def _(o):
    return (o["u"]["code"] == 0 and o["u"]["json"] == {"report": "utilization", "rows": [
                {"room": "atlas", "open_minutes": 1200, "booked_minutes": 150, "percent": "12.5"},
                {"room": "bern", "open_minutes": 360, "booked_minutes": 20, "percent": "5.6"}]}
            and res(o["need"], 2, "", "error: report 'utilization' needs option 'date_from'\n")
            and o["ag"]["code"] == 0 and o["ag"]["json"] == {"date": "2026-03-02", "rows": [
                R(1, "atlas", "09:00", "10:30", "A", "ana"), R(4, "bern", "09:00", "09:20", "D", "dan")]}
            and o["cf"]["json"] == {"report": "conflicts", "rows": []}
            and res(o["unk"], 2, "", "error: unknown report 'nope'\n"))


@item("reg/api_errors", r'''
cli("--store", "s", "add-room", "atlas", "--name", "Atlas", "--capacity", "4")
cli("--store", "s", "book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "A", "--owner", "ana")
body = {"room": "atlas", "start": "2026-03-02 09:30", "end": "2026-03-02 10:00", "title": "X", "owner": "bob"}
OUT["r404"] = api("s", "GET", "/rooms/nope")
OUT["noroute"] = api("s", "GET", "/nothing")
OUT["r405"] = api("s", "DELETE", "/rooms")
OUT["conf"] = api("s", "POST", "/bookings", body)
body2 = dict(body, start="2026-03-02 11:00", end="2026-03-02 12:00")
OUT["badkey"] = api("s", "POST", "/bookings", dict(body2, color="red"))
OUT["ok"] = api("s", "POST", "/bookings", dict(body2, attendees=2))
OUT["get"] = api("s", "GET", "/bookings/2")
OUT["del"] = api("s", "DELETE", "/bookings/2")
OUT["get2"] = api("s", "GET", "/bookings/2")
OUT["bad"] = api("s", "POST", "/bookings", dict(body2, end="2026-03-02 10:59"))
''')
def _(o):
    ok = (o["ok"]["json"] or {}).get("booking") or {}
    return (st(o["r404"], 404, {"error": "unknown room 'nope'"}) and st(o["noroute"], 404) and st(o["r405"], 405)
            and st(o["conf"], 409, {"error": "room 'atlas' is busy: conflicts with booking 1"})
            and st(o["badkey"], 400, {"error": "unknown key 'color'"})
            and st(o["ok"], 201) and sub(ok, {"id": 2, "attendees": 2, "start": "2026-03-02 11:00"})
            and st(o["get"], 200) and st(o["del"], 200, {"cancelled": 2, "promoted": []})
            and st(o["get2"], 404, {"error": "no booking 2"})
            and st(o["bad"], 400, {"error": "end must be after start"}))


# zones ------------------------------------------------------------------------------------------------------
@item("zones/codes_and_literals", r'''
oks = ["jst", "+05:45", "+00:00", "--tz=-12:00", "+14:00", "nzst", "Brt"]
for i, z in enumerate(oks):
    flag = [z] if z.startswith("--") else ["--tz", z]
    OUT["ok%d" % i] = cli("--store", "s", "add-room", "r%d" % i, "--name", "R", "--capacity", "2", *flag)["code"]
bads = ["+14:15", "+05:20", "+5:30", "GMT", "--tz=-12:15", "UTC+1", "05:30"]
for i, z in enumerate(bads):
    flag = [z] if z.startswith("--") else ["--tz", z]
    r = cli("--store", "s", "add-room", "b%d" % i, "--name", "B", "--capacity", "2", *flag)
    OUT["bad%d" % i] = [r["code"], r["err"]]
OUT["plain"] = cli("--store", "s", "add-room", "plain", "--name", "P", "--capacity", "2")["code"]
OUT["rooms"] = cli("--store", "s", "rooms", "--json")["json"]
''')
def _(o):
    want = {"r0": "JST", "r1": "+05:45", "r2": "+00:00", "r3": "-12:00", "r4": "+14:00", "r5": "NZST",
            "r6": "BRT", "plain": "UTC"}
    got = {r["id"]: r.get("tz") for r in (o["rooms"] or {}).get("rooms", [])}
    bad_text = ["+14:15", "+05:20", "+5:30", "GMT", "-12:15", "UTC+1", "05:30"]
    return (all(o["ok%d" % i] == 0 for i in range(7)) and o["plain"] == 0 and got == want
            and all(o["bad%d" % i] == [2, "error: unknown zone '%s'\n" % z] for i, z in enumerate(bad_text)))


# stores -----------------------------------------------------------------------------------------------------
@item("store/csv_v3_written", r'''
cli("--store", "s", "add-room", "kyoto", "--name", "Kyoto", "--capacity", "6", "--opens", "09:00", "--closes",
    "17:00", "--tz", "JST")
cli("--store", "s", "add-room", "atlas", "--name", "Atlas", "--capacity", "8", "--tags", "video;board")
OUT["b1"] = cli("--store", "s", "book", "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "--title", "Review",
                "--owner", "ken")
OUT["b2"] = cli("--store", "s", "book", "atlas", "2026-03-02 10:00", "2026-03-02 11:00", "--title", "Plan, v2",
                "--owner", "ana", "--tz", "+01:00")
OUT["rooms"] = read("s/rooms.csv")
OUT["bookings"] = read("s/bookings.csv")
''')
def _(o):
    return (res(o["b1"], 0, "booked 1\n") and res(o["b2"], 0, "booked 2\n")
            and o["rooms"] == ("#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\n"
                               "atlas,Atlas,8,08:00,18:00,video;board,UTC\nkyoto,Kyoto,6,09:00,17:00,,JST\n")
            and o["bookings"] == ("#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
                                  "1,kyoto,2026-03-02 10:00,2026-03-02 11:00,Review,ken,1,JST\n"
                                  '2,atlas,2026-03-02 10:00,2026-03-02 11:00,"Plan, v2",ana,1,+01:00\n'))


@item("store/csv_mixed_versions", r'''
seed("s1")
write("s1/bookings.csv", "#roomkit-schema: 2\nid,room,start,end,title,owner,attendees\n"
      "1,berlin,2026-03-02 09:00,2026-03-02 10:00,A,ana,2\n2,atlas,2026-03-02 09:00,2026-03-02 10:00,B,bob,1\n"
      "3,ghost,2026-03-02 09:00,2026-03-02 10:00,C,cyd,1\n")
OUT["s1"] = bookings("s1")
seed("s2")
write("s2/bookings.csv", "id,room,date,from,to,title,owner\n5,kyoto,2026-03-04,10:00,11:00,D,dan\n"
      "6,pune,2026-03-04,12:00,12:30,E,eve\n")
OUT["s2"] = bookings("s2")
write("s3/rooms.csv", "id,name,capacity,hours\natlas,Atlas,8,08:00-18:00\n")
write("s3/bookings.csv", "#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
      "1,atlas,2026-03-02 09:00,2026-03-02 10:00,F,fay,1,EST\n2,atlas,2026-03-02 11:00,2026-03-02 12:00,G,gus,1,+05:45\n")
OUT["s3"] = bookings("s3")
OUT["s3rooms"] = rooms("s3")
OUT["m"] = cli("--store", "s1", "migrate")["code"]
OUT["s1text"] = read("s1/bookings.csv")
''')
def _(o):
    return ([(b["id"], b["tz"], b["attendees"]) for b in o["s1"]] == [(1, "CET", 2), (2, "UTC", 1), (3, "UTC", 1)]
            and o["s2"] == [B(5, "kyoto", "2026-03-04 10:00", "2026-03-04 11:00", "D", "dan", 1, "JST"),
                            B(6, "pune", "2026-03-04 12:00", "2026-03-04 12:30", "E", "eve", 1, "IST")]
            and [(b["id"], b["tz"]) for b in o["s3"]] == [(1, "EST"), (2, "+05:45")]
            and sub(o["s3rooms"].get("atlas"), {"tz": "UTC", "opens": "08:00"})
            and o["m"] == 0
            and o["s1text"] == ("#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
                                "1,berlin,2026-03-02 09:00,2026-03-02 10:00,A,ana,2,CET\n"
                                "2,atlas,2026-03-02 09:00,2026-03-02 10:00,B,bob,1,UTC\n"
                                "3,ghost,2026-03-02 09:00,2026-03-02 10:00,C,cyd,1,UTC\n"))


@item("store/csv_version_and_zone_errors", r'''
seed("s1")
write("s1/bookings.csv", "#roomkit-schema: 4\nid,room,start,end,title,owner,attendees,tz\n")
OUT["v4"] = cli("--store", "s1", "list", "--json")
write("s2/rooms.csv", "#roomkit-schema: 5\nid,name,capacity,opens,closes,tags,tz\n")
OUT["v5"] = cli("--store", "s2", "rooms")
write("s3/rooms.csv", "#roomkit-schema: 3\nid,name,capacity,opens,closes,tags,tz\natlas,Atlas,8,08:00,18:00,,XYZ\n")
OUT["zr"] = cli("--store", "s3", "rooms")
seed("s4", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,UTC",
            "2,atlas,2026-03-02 11:00,2026-03-02 12:00,B,bob,1,+05:20"])
OUT["zb"] = cli("--store", "s4", "list")
seed("s5", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,cet"])
OUT["low"] = bookings("s5")
''')
def _(o):
    return (res(o["v4"], 1, "", "error: bookings.csv: unsupported schema version 4 (newest is 3)\n")
            and res(o["v5"], 1, "", "error: rooms.csv: unsupported schema version 5 (newest is 3)\n")
            and res(o["zr"], 1, "", "error: rooms.csv line 3: unknown zone 'XYZ'\n")
            and res(o["zb"], 1, "", "error: bookings.csv line 4: unknown zone '+05:20'\n")
            and [b["tz"] for b in o["low"]] == ["CET"])


@item("store/json_v3_written", r'''
OUT["a"] = cli("--store", "plan.json", "add-room", "sf", "--name", "SF", "--capacity", "5", "--tz", "PST",
               "--tags", "a;b")
OUT["b"] = cli("--store", "plan.json", "book", "sf", "2026-03-02 09:00", "2026-03-02 09:45", "--title", "Sync",
               "--owner", "ana")
OUT["text"] = read("plan.json")
OUT["back"] = bookings("plan.json")
''')
def _(o):
    doc = json.loads(o["text"])
    room = {"id": "sf", "name": "SF", "capacity": 5, "opens": "08:00", "closes": "18:00", "tags": ["a", "b"],
            "tz": "PST"}
    bk = B(1, "sf", "2026-03-02 09:00", "2026-03-02 09:45", "Sync", "ana", 1, "PST")
    return (o["a"]["code"] == 0 and res(o["b"], 0, "booked 1\n") and doc.get("schema") == 3
            and doc["rooms"] == [room] and list(doc["rooms"][0]) == ROOM_KEYS
            and doc["bookings"] == [bk] and list(doc["bookings"][0]) == BOOKING_KEYS
            and o["back"] == [bk])


@item("store/json_legacy_versions", r'''
write("old.json", json.dumps({"schema": 2, "rooms": [{"id": "atlas", "name": "Atlas", "capacity": 8,
    "opens": "08:00", "closes": "18:00", "tags": []}], "bookings": [{"id": 3, "room": "atlas",
    "start": "2026-03-02 09:00", "end": "2026-03-02 10:00", "title": "T", "owner": "ana", "attendees": 2}]}))
write("v1.json", json.dumps({"version": 1, "rooms": [{"id": "bern", "name": "Bern", "capacity": 3,
    "hours": "07:00-12:00"}], "bookings": [{"id": 1, "room": "bern", "date": "2026-03-03", "from": "08:00",
    "to": "09:00", "title": "U", "owner": "bob"}]}))
write("new.json", json.dumps({"schema": 4, "rooms": [], "bookings": []}))
OUT["old"] = bookings("old.json")
OUT["oldrooms"] = rooms("old.json")
OUT["v1"] = bookings("v1.json")
OUT["v1rooms"] = rooms("v1.json")
OUT["m"] = cli("--store", "old.json", "migrate")["code"]
OUT["oldtext"] = read("old.json")
OUT["new"] = cli("--store", "new.json", "list")
''')
def _(o):
    doc = json.loads(o["oldtext"])
    return (o["old"] == [B(3, "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "T", "ana", 2, "UTC")]
            and o["oldrooms"]["atlas"]["tz"] == "UTC"
            and o["v1"] == [B(1, "bern", "2026-03-03 08:00", "2026-03-03 09:00", "U", "bob", 1, "UTC")]
            and sub(o["v1rooms"]["bern"], {"tz": "UTC", "opens": "07:00", "closes": "12:00"})
            and o["m"] == 0 and doc["schema"] == 3 and doc["rooms"][0]["tz"] == "UTC"
            and doc["bookings"][0]["tz"] == "UTC" and list(doc["bookings"][0]) == BOOKING_KEYS
            and res(o["new"], 1, "", "error: new.json: unsupported schema version 4 (newest is 3)\n"))


# booking rules and rulesets ---------------------------------------------------------------------------------
@item("rules/opening_hours_room_zone", r'''
seed("s")
def bk(room, a, b, tz):
    return cli("--store", "s", "book", room, "2026-03-02 " + a, "2026-03-02 " + b, "--title", "T", "--owner",
               "ana", "--tz", tz)
OUT["b1"] = bk("berlin", "06:30", "07:30", "UTC")
OUT["b2"] = bk("berlin", "18:30", "19:30", "UTC")
OUT["b3"] = bk("atlas", "08:00", "09:00", "IST")
OUT["b4"] = bk("atlas", "15:00", "16:00", "IST")
OUT["b5"] = bk("kyoto", "07:00", "08:00", "ICT")
OUT["all"] = bookings("s")
''')
def _(o):
    return (res(o["b1"], 0, "booked 1\n") and res(o["b2"], 1, "", "error: outside opening hours 07:00-19:00\n")
            and res(o["b3"], 1, "", "error: outside opening hours 08:00-18:00\n")
            and res(o["b4"], 0, "booked 2\n") and res(o["b5"], 0, "booked 3\n")
            and o["all"] == [B(1, "berlin", "2026-03-02 06:30", "2026-03-02 07:30", "T", "ana", 1, "UTC"),
                             B(2, "atlas", "2026-03-02 15:00", "2026-03-02 16:00", "T", "ana", 1, "IST"),
                             B(3, "kyoto", "2026-03-02 07:00", "2026-03-02 08:00", "T", "ana", 1, "ICT")])


@item("rules/same_day_room_zone", r'''
seed("s", extra_rooms="night,Night,5,00:00,23:59,,UTC\n")
def bk(a, b, tz):
    return cli("--store", "s", "book", "night", a, b, "--title", "T", "--owner", "ana", "--tz", tz)
OUT["b1"] = bk("2026-03-02 08:00", "2026-03-02 10:00", "JST")
OUT["b2"] = bk("2026-03-02 10:00", "2026-03-02 12:00", "JST")
OUT["b3"] = bk("2026-03-02 23:30", "2026-03-03 00:45", "CET")
OUT["all"] = bookings("s")
''')
def _(o):
    return (res(o["b1"], 1, "", "error: booking must start and end on the same day\n")
            and res(o["b2"], 0, "booked 1\n") and res(o["b3"], 0, "booked 2\n")
            and [(b["start"], b["end"], b["tz"]) for b in o["all"]] == [
                ("2026-03-02 10:00", "2026-03-02 12:00", "JST"), ("2026-03-02 23:30", "2026-03-03 00:45", "CET")])


@item("rules/check_strict_absolute", r'''
seed("s", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,One,ana,1,UTC",
           "2,atlas,2026-03-02 10:30,2026-03-02 11:30,Two,bob,1,CET",
           "3,atlas,2026-03-02 11:00,2026-03-02 12:00,Three,cyd,1,CET",
           "4,berlin,2026-03-02 12:00,2026-03-02 13:00,Four,dan,1,CET",
           "5,berlin,2026-03-02 10:15,2026-03-02 10:45,Five,ana,1,CET",
           "6,berlin,2026-03-02 12:00,2026-03-02 12:30,Six,eve,1,UTC"])
OUT["strict"] = cli("--store", "s", "check", "--ruleset", "strict", "--json")
OUT["lenient"] = cli("--store", "s", "check", "--ruleset", "lenient")
''')
def _(o):
    ov = [{"rule": "overlaps", "bookings": [1, 2], "message": "bookings 1 and 2 overlap in room 'atlas' (30 min)"},
          {"rule": "overlaps", "bookings": [2, 3], "message": "bookings 2 and 3 overlap in room 'atlas' (30 min)"}]
    ow = [{"rule": "owner_overlap", "bookings": [1, 5], "message": "owner 'ana' double-booked: bookings 1 and 5"}]
    return (o["strict"]["code"] == 1 and o["strict"]["json"] == {"ruleset": "strict", "issues": ov + ow}
            and res(o["lenient"], 1, "overlaps: %s\noverlaps: %s\n" % (ov[0]["message"], ov[1]["message"])))


@item("rules/check_booking_rule_zone", r'''
seed("s", ["1,kyoto,2026-03-02 10:00,2026-03-02 11:00,A,ana,1,UTC",
           "2,kyoto,2026-03-02 10:00,2026-03-02 11:00,B,bob,1,JST",
           "3,pune,2026-03-02 03:00,2026-03-02 04:00,C,cyd,1,UTC",
           "4,atlas,2026-03-02 17:00,2026-03-02 18:00,D,dan,1,EST",
           "5,berlin,2026-03-02 23:30,2026-03-03 00:30,E,eve,1,UTC"])
OUT["c"] = cli("--store", "s", "check", "--json")
''')
def _(o):
    want = [{"rule": "booking", "bookings": [1], "message": "booking 1: outside opening hours 09:00-17:00"},
            {"rule": "booking", "bookings": [4], "message": "booking 4: outside opening hours 08:00-18:00"},
            {"rule": "booking", "bookings": [5], "message": "booking 5: outside opening hours 07:00-19:00"}]
    return o["c"]["code"] == 1 and o["c"]["json"] == {"ruleset": "default", "issues": want}


# command line -----------------------------------------------------------------------------------------------
@item("cli/book_zone_json", r'''
seed("s")
OUT["b1"] = cli("--store", "s", "book", "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "--title", "Review",
                "--owner", "ken", "--json")
OUT["b2"] = cli("--store", "s", "book", "pune", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "X",
                "--owner", "ana", "--tz", "utc", "--json")
OUT["b3"] = cli("--store", "s", "book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "Y",
                "--owner", "ana", "--tz", "XYZ")
OUT["rooms"] = cli("--store", "s", "rooms", "--json")
OUT["list"] = cli("--store", "s", "list", "--json")
OUT["rtab"] = cli("--store", "s", "rooms")["out"]
OUT["ltab"] = cli("--store", "s", "list")["out"]
''')
def _(o):
    b1 = o["b1"]["json"]["booking"]
    rl = o["rooms"]["json"]["rooms"]
    rlines, llines = o["rtab"].split("\n"), o["ltab"].split("\n")
    return (b1 == B(1, "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "Review", "ken", 1, "JST")
            and list(b1) == BOOKING_KEYS and o["b2"]["json"]["booking"]["tz"] == "UTC"
            and res(o["b3"], 2, "", "error: unknown zone 'XYZ'\n")
            and [(r["id"], r["tz"]) for r in rl] == [("atlas", "UTC"), ("berlin", "CET"), ("kyoto", "JST"),
                                                    ("pune", "IST")]
            and list(rl[1]) == ROOM_KEYS
            and [b["tz"] for b in o["list"]["json"]["bookings"]] == ["JST", "UTC"]
            and rlines[0].split()[-1] == "tz" and [l.split()[-1] for l in rlines[2:6]] == ["UTC", "CET", "JST", "IST"]
            and llines[0].split()[-1] == "tz" and [l.split()[-1] for l in llines[2:4]] == ["JST", "UTC"])


@item("cli/conflicts_across_zones", r'''
seed("s")
def bk(a, b, *tz):
    return cli("--store", "s", "book", "atlas", "2026-03-02 " + a, "2026-03-02 " + b, "--title", "T",
               "--owner", "ana", *tz)
OUT["b1"] = bk("09:00", "10:00")
OUT["b2"] = bk("10:30", "11:00", "--tz", "CET")
OUT["b3"] = bk("11:00", "12:00", "--tz", "CET")
OUT["b4"] = bk("14:00", "15:00")
OUT["b5"] = bk("14:30", "15:30", "--tz", "+05:30")
OUT["b6"] = bk("06:00", "06:30", "--tz=-03:00")
OUT["ids"] = [(b["id"], b["tz"]) for b in bookings("s")]
''')
def _(o):
    busy = "error: room 'atlas' is busy: conflicts with booking %d\n"
    return (res(o["b1"], 0, "booked 1\n") and res(o["b2"], 1, "", busy % 1) and res(o["b3"], 0, "booked 2\n")
            and res(o["b4"], 0, "booked 3\n") and res(o["b5"], 1, "", busy % 1) and res(o["b6"], 1, "", busy % 1)
            and o["ids"] == [[1, "UTC"], [2, "CET"], [3, "UTC"]])


@item("cli/move_zones", r'''
seed("s")
cli("--store", "s", "book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "M", "--owner", "ana")
OUT["m1"] = cli("--store", "s", "move", "1", "2026-03-02 11:00", "--tz", "CET")
OUT["s1"] = bookings("s")
OUT["m2"] = cli("--store", "s", "move", "1", "2026-03-02 13:00")
OUT["s2"] = bookings("s")
OUT["m3"] = cli("--store", "s", "move", "1", "2026-03-02 08:00", "--room", "berlin")
OUT["s3"] = bookings("s")
OUT["b2"] = cli("--store", "s", "book", "berlin", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "N",
                "--owner", "bob", "--tz", "UTC")
OUT["m4"] = cli("--store", "s", "move", "1", "2026-03-02 10:30")
OUT["m5"] = cli("--store", "s", "move", "1", "2026-03-02 09:00", "--tz", "XYZ")
OUT["m6"] = cli("--store", "s", "move", "1", "2026-03-02 05:30", "--tz", "UTC")
OUT["end"] = bookings("s")
''')
def _(o):
    one = lambda rows: [(b["room"], b["start"], b["end"], b["tz"]) for b in rows if b["id"] == 1]
    return (res(o["m1"], 0, "moved 1\n") and one(o["s1"]) == [("atlas", "2026-03-02 11:00", "2026-03-02 12:00", "CET")]
            and res(o["m2"], 0) and one(o["s2"]) == [("atlas", "2026-03-02 13:00", "2026-03-02 14:00", "CET")]
            and res(o["m3"], 0) and one(o["s3"]) == [("berlin", "2026-03-02 08:00", "2026-03-02 09:00", "CET")]
            and res(o["b2"], 0, "booked 2\n")
            and res(o["m4"], 1, "", "error: room 'berlin' is busy: conflicts with booking 2\n")
            and res(o["m5"], 2, "", "error: unknown zone 'XYZ'\n")
            and res(o["m6"], 1, "", "error: outside opening hours 07:00-19:00\n")
            and one(o["end"]) == [("berlin", "2026-03-02 08:00", "2026-03-02 09:00", "CET")])


@item("cli/free_slots_zones", r'''
seed("s", ROWS)
def free(*more):
    return cli("--store", "s", "free", "kyoto", "2026-03-02", "--duration", "60", "--step", "60", "--json", *more)
OUT["local"] = free()
OUT["utc"] = free("--tz", "UTC")
OUT["pst"] = free("--tz", "PST")
OUT["npt"] = cli("--store", "s", "free", "kyoto", "2026-03-02", "--duration", "120", "--step", "60", "--tz", "NPT")
OUT["bad"] = free("--tz", "Mars")
'''.replace("ROWS", repr(FREE_ROWS)))
def _(o):
    j = lambda r: r["json"] if r["code"] == 0 else None
    return (j(o["local"]) == {"room": "kyoto", "date": "2026-03-02", "slots": FREE_JST}
            and j(o["utc"]) == {"room": "kyoto", "date": "2026-03-02", "slots": FREE_UTC}
            and j(o["pst"]) == {"room": "kyoto", "date": "2026-03-02", "slots": FREE_PST}
            and res(o["npt"], 0, "2026-03-02 07:45\n2026-03-02 08:45\n2026-03-02 11:45\n")
            and res(o["bad"], 2, "", "error: unknown zone 'Mars'\n"))


@item("cli/agenda_tz", r'''
seed("s", ROWS)
OUT["jst3"] = cli("--store", "s", "agenda", "2026-03-03", "--tz", "JST", "--json")
OUT["jst2"] = cli("--store", "s", "agenda", "2026-03-02", "--tz", "jst", "--json")
OUT["pst2"] = cli("--store", "s", "agenda", "2026-03-02", "--tz", "PST", "--json")
OUT["room"] = cli("--store", "s", "agenda", "2026-03-03", "--tz", "JST", "--room", "pune", "--json")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    return (o["jst3"]["json"] == {"date": "2026-03-03", "rows": AG_0303_JST}
            and o["jst2"]["json"] == {"date": "2026-03-02", "rows": AG_0302_JST}
            and o["pst2"]["json"] == {"date": "2026-03-02", "rows": AG_0302_PST}
            and o["room"]["json"] == {"date": "2026-03-03", "rows": AG_0303_JST[2:]})


@item("cli/agenda_default_utc", r'''
seed("s", ROWS)
OUT["d2"] = cli("--store", "s", "agenda", "2026-03-02", "--json")
OUT["d3"] = cli("--store", "s", "agenda", "2026-03-03", "--json")
OUT["r2"] = cli("--store", "s", "report", "agenda", "--date", "2026-03-02", "--json")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    return (o["d2"]["json"] == {"date": "2026-03-02", "rows": AG_0302_UTC}
            and o["d3"]["json"] == {"date": "2026-03-03", "rows": AG_0303_UTC}
            and o["r2"]["json"] == {"report": "agenda", "rows": AG_0302_UTC})


@item("cli/report_busiest_tz", r'''
seed("s", ROWS)
OUT["utc"] = cli("--store", "s", "report", "busiest", "--json")
OUT["ist"] = cli("--store", "s", "report", "busiest", "--tz", "IST", "--json")
OUT["istday"] = cli("--store", "s", "report", "busiest", "--tz", "IST", "--from", "2026-03-02", "--to",
                    "2026-03-02", "--json")
OUT["util"] = cli("--store", "s", "report", "utilization", "--from", "2026-03-02", "--to", "2026-03-02",
                  "--tz", "JST")
'''.replace("ROWS", repr(BUSY_ROWS)))
def _(o):
    h = lambda pairs: [{"hour": a, "bookings": n} for a, n in pairs]
    return (o["utc"]["json"] == {"report": "busiest", "rows": h([("03", 3), ("04", 1), ("20", 1)])}
            and o["ist"]["json"] == {"report": "busiest", "rows": h([("09", 3), ("08", 2), ("01", 1), ("02", 1)])}
            and o["istday"]["json"] == {"report": "busiest", "rows": h([("09", 3), ("08", 2)])}
            and res(o["util"], 2, "", "error: report 'utilization' does not take option 'tz'\n"))


@item("report/utilization_room_day", r'''
seed("s", ["1,kyoto,2026-03-02 00:30,2026-03-02 02:30,A,ana,1,UTC",
           "2,kyoto,2026-03-01 23:30,2026-03-02 00:30,B,bob,1,UTC",
           "3,kyoto,2026-03-02 08:00,2026-03-02 09:00,C,cyd,1,UTC",
           "4,kyoto,2026-03-02 16:00,2026-03-02 17:30,D,dan,1,UTC",
           "5,kyoto,2026-03-02 14:00,2026-03-02 14:45,E,eve,1,JST",
           "6,berlin,2026-03-02 23:30,2026-03-03 00:30,F,fay,1,UTC",
           "7,berlin,2026-03-01 23:30,2026-03-02 00:30,G,gus,1,UTC"])
OUT["one"] = cli("--store", "s", "report", "utilization", "--from", "2026-03-02", "--to", "2026-03-02", "--json")
OUT["two"] = cli("--store", "s", "report", "utilization", "--from", "2026-03-02", "--to", "2026-03-03", "--room",
                 "kyoto", "--json")
''')
def _(o):
    u = lambda room, op, bk, pc: {"room": room, "open_minutes": op, "booked_minutes": bk, "percent": pc}
    return (o["one"]["json"] == {"report": "utilization", "rows": [
                u("atlas", 600, 0, "0.0"), u("berlin", 720, 60, "8.3"), u("kyoto", 480, 285, "59.4"),
                u("pune", 720, 0, "0.0")]}
            and o["two"]["json"] == {"report": "utilization", "rows": [u("kyoto", 960, 375, "39.1")]})


@item("report/conflicts_absolute", r'''
seed("s", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,UTC",
           "2,atlas,2026-03-02 10:30,2026-03-02 11:30,B,bob,1,CET",
           "3,atlas,2026-03-02 09:45,2026-03-02 10:15,C,cyd,1,UTC",
           "4,berlin,2026-03-02 12:00,2026-03-02 13:00,D,dan,1,CET",
           "5,berlin,2026-03-02 12:00,2026-03-02 12:30,E,eve,1,UTC",
           "6,pune,2026-03-02 15:00,2026-03-02 16:00,F,fay,1,IST",
           "7,pune,2026-03-02 09:15,2026-03-02 10:00,G,gus,1,UTC"])
OUT["all"] = cli("--store", "s", "report", "conflicts", "--json")
OUT["pune"] = cli("--store", "s", "report", "conflicts", "--room", "pune", "--json")
''')
def _(o):
    c = lambda room, a, b, m: {"room": room, "a": a, "b": b, "overlap_minutes": m}
    rows = [c("atlas", 1, 2, 30), c("atlas", 1, 3, 15), c("atlas", 2, 3, 30), c("pune", 6, 7, 30)]
    return (o["all"]["json"] == {"report": "conflicts", "rows": rows}
            and o["pune"]["json"] == {"report": "conflicts", "rows": rows[3:]})


# importers --------------------------------------------------------------------------------------------------
@item("import/csv_tz_column", r'''
seed("s")
write("in.csv", "room,tz,start,end,title,owner\natlas,cet,2026-03-02 10:00,2026-03-02 11:00,Alpha,ana\n"
      "kyoto,,2026-03-02 10:00,2026-03-02 11:00,Beta,ken\npune,+05:30,2026-03-02 12:00,2026-03-02 13:00,Gamma,raj\n"
      "berlin,UTC,2026-03-02 17:30,2026-03-02 18:00,Delta,eva\n")
OUT["imp"] = cli("--store", "s", "import", "csv", "in.csv", "--json")
OUT["all"] = bookings("s")
''')
def _(o):
    return (o["imp"]["code"] == 0 and o["imp"]["json"] == {"added": [1, 2, 3, 4], "skipped": []}
            and o["all"] == [B(1, "atlas", "2026-03-02 10:00", "2026-03-02 11:00", "Alpha", "ana", 1, "CET"),
                             B(2, "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "Beta", "ken", 1, "JST"),
                             B(3, "pune", "2026-03-02 12:00", "2026-03-02 13:00", "Gamma", "raj", 1, "+05:30"),
                             B(4, "berlin", "2026-03-02 17:30", "2026-03-02 18:00", "Delta", "eva", 1, "UTC")])


@item("import/csv_bad_zone", r'''
seed("s")
write("in.csv", "room,start,end,title,owner,tz\natlas,2026-03-02 10:00,2026-03-02 11:00,A,ana,EST\n\n"
      "atlas,2026-03-02 12:00,2026-03-02 13:00,B,bob,Mars\n")
OUT["imp"] = cli("--store", "s", "import", "csv", "in.csv")
OUT["all"] = bookings("s")
write("ok.csv", "room,start,end,title,owner,tz\natlas,2026-03-02 10:00,2026-03-02 11:00,A,ana,EST\n")
OUT["ok"] = cli("--store", "s", "import", "csv", "ok.csv")
OUT["all2"] = bookings("s")
''')
def _(o):
    return (res(o["imp"], 1, "", "error: line 4: unknown zone 'Mars'\n") and o["all"] == []
            and res(o["ok"], 0, "imported 1, skipped 0\n")
            and o["all2"] == [B(1, "atlas", "2026-03-02 10:00", "2026-03-02 11:00", "A", "ana", 1, "EST")])


@item("import/json_tz", r'''
seed("s")
write("a.json", json.dumps([
    {"room": "atlas", "start": "2026-03-02 09:00", "end": "2026-03-02 10:00", "title": "A", "owner": "ana",
     "tz": "EST"},
    {"room": "berlin", "start": "2026-03-02 09:00", "end": "2026-03-02 10:00", "title": "B", "owner": "bob",
     "attendees": 3}]))
OUT["a"] = cli("--store", "s", "import", "json", "a.json", "--json")
write("b.json", json.dumps([
    {"room": "atlas", "start": "2026-03-02 11:00", "end": "2026-03-02 12:00", "title": "C", "owner": "ana",
     "tz": "EST"},
    {"room": "atlas", "start": "2026-03-02 12:00", "end": "2026-03-02 13:00", "title": "D", "owner": "ana",
     "tz": "Mars"}]))
OUT["b"] = cli("--store", "s", "import", "json", "b.json")
OUT["all"] = bookings("s")
''')
def _(o):
    return (o["a"]["json"] == {"added": [1, 2], "skipped": []}
            and res(o["b"], 1, "", "error: item 2: unknown zone 'Mars'\n")
            and o["all"] == [B(1, "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "A", "ana", 1, "EST"),
                             B(2, "berlin", "2026-03-02 09:00", "2026-03-02 10:00", "B", "bob", 3, "CET")])


@item("import/jsonl_tz", r'''
seed("s")
row = lambda **k: json.dumps(dict({"title": "T", "owner": "ana"}, **k))
write("c.jsonl", row(room="kyoto", start="2026-03-02 01:00", end="2026-03-02 02:00", tz="UTC") + "\n\n"
      + row(room="pune", start="2026-03-02 10:00", end="2026-03-02 11:00") + "\n")
OUT["c"] = cli("--store", "s", "import", "jsonl", "c.jsonl", "--json")
write("d.jsonl", row(room="pune", start="2026-03-02 12:00", end="2026-03-02 13:00") + "\n"
      + row(room="pune", start="2026-03-02 14:00", end="2026-03-02 15:00", tz="+05:20") + "\n")
OUT["d"] = cli("--store", "s", "import", "jsonl", "d.jsonl")
OUT["all"] = bookings("s")
''')
def _(o):
    return (o["c"]["json"] == {"added": [1, 2], "skipped": []}
            and res(o["d"], 1, "", "error: line 2: unknown zone '+05:20'\n")
            and [(b["room"], b["tz"]) for b in o["all"]] == [("kyoto", "UTC"), ("pune", "IST")])


BLOCKS_OK = """# bookings from the partner office
BEGIN:BOOKING
ROOM:pune
START:2026-03-02 09:00+05:30
END:2026-03-02 10:00
TITLE:Inline
OWNER:raj
END:BOOKING

BEGIN:BOOKING
ROOM:kyoto
TZ:npt
START:2026-03-02 07:00
END:2026-03-02 08:00
TITLE:Coded
OWNER:ken
END:BOOKING

BEGIN:BOOKING
ROOM:kyoto
START:2026-03-02 13:00+09:00
END:2026-03-02 14:00+09:00
TZ:JST
TITLE:Both
OWNER:yui
ATTENDEES:2
END:BOOKING
BEGIN:BOOKING
ROOM:berlin
START:2026-03-02 15:00
END:2026-03-02 16:00+01:00
TITLE:EndOnly
OWNER:eva
END:BOOKING
"""


@item("import/block_offsets", r'''
seed("s")
write("in.txt", TEXT)
OUT["imp"] = cli("--store", "s", "import", "block", "in.txt", "--json")
OUT["all"] = bookings("s")
'''.replace("TEXT", repr(BLOCKS_OK)))
def _(o):
    return (o["imp"]["json"] == {"added": [1, 2, 3, 4], "skipped": []}
            and o["all"] == [B(1, "pune", "2026-03-02 09:00", "2026-03-02 10:00", "Inline", "raj", 1, "+05:30"),
                             B(2, "kyoto", "2026-03-02 07:00", "2026-03-02 08:00", "Coded", "ken", 1, "NPT"),
                             B(3, "kyoto", "2026-03-02 13:00", "2026-03-02 14:00", "Both", "yui", 2, "JST"),
                             B(4, "berlin", "2026-03-02 15:00", "2026-03-02 16:00", "EndOnly", "eva", 1, "+01:00")])


@item("import/block_offset_errors", r'''
seed("s")
def imp(text):
    write("in.txt", text)
    return cli("--store", "s", "import", "block", "in.txt")
OUT["a"] = imp("BEGIN:BOOKING\nROOM:pune\nSTART:2026-03-02 09:00+05:30\nTITLE:X\nEND:2026-03-02 10:00+09:00\n"
               "OWNER:raj\nEND:BOOKING\n")
OUT["b"] = imp("BEGIN:BOOKING\nROOM:kyoto\nTZ:JST\nSTART:2026-03-02 10:00+05:30\nEND:2026-03-02 11:00\n"
               "TITLE:X\nOWNER:ken\nEND:BOOKING\n")
OUT["c"] = imp("BEGIN:BOOKING\nROOM:kyoto\nSTART:2026-03-02 10:00+14:30\nEND:2026-03-02 11:00\nTITLE:X\n"
               "OWNER:ken\nEND:BOOKING\n")
OUT["d"] = imp("# x\nBEGIN:BOOKING\nROOM:kyoto\nTZ:Mars\nSTART:2026-03-02 10:00\nEND:2026-03-02 11:00\n"
               "TITLE:X\nOWNER:ken\nEND:BOOKING\n")
OUT["e"] = imp("BEGIN:BOOKING\nROOM:pune\nSTART:2026-03-02 10:00+05:30\nEND:2026-03-02 11:00+05:30\n"
               "TZ:+05:30\nTITLE:X\nOWNER:raj\nEND:BOOKING\n")
OUT["all"] = bookings("s")
''')
def _(o):
    return (res(o["a"], 1, "", "error: line 5: END offset +09:00 does not match START offset +05:30\n")
            and res(o["b"], 1, "", "error: line 3: TZ JST (+09:00) does not match inline offset +05:30\n")
            and res(o["c"], 1, "", "error: line 3: unknown zone '+14:30'\n")
            and res(o["d"], 1, "", "error: line 4: unknown zone 'Mars'\n")
            and res(o["e"], 0, "imported 1, skipped 0\n")
            and [(b["start"], b["tz"]) for b in o["all"]] == [("2026-03-02 10:00", "+05:30")])


@item("import/merge_across_zones", r'''
seed("s", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,Base,ana,1,UTC"])
text = ("room,start,end,title,owner,tz\natlas,2026-03-02 10:30,2026-03-02 11:00,Clash,bob,CET\n"
        "atlas,2026-03-02 11:00,2026-03-02 12:00,Touch,cyd,CET\nberlin,2026-03-02 09:00,2026-03-02 10:00,Local,dan,\n"
        "atlas,2026-03-02 10:15,2026-03-02 10:45,Late,eve,UTC\nkyoto,2026-03-02 08:00,2026-03-02 09:00,Early,fay,\n")
OUT["skip"] = api("s", "POST", "/import/csv?policy=skip", text)
OUT["mid"] = bookings("s")
OUT["fail"] = api("s", "POST", "/import/csv?policy=fail",
                  "room,start,end,title,owner,tz\npune,2026-03-02 15:00,2026-03-02 16:00,P,raj,IST\n"
                  "atlas,2026-03-02 11:30,2026-03-02 12:30,Q,raj,+01:00\n")
OUT["end"] = bookings("s")
''')
def _(o):
    busy = "room 'atlas' is busy: conflicts with booking %d"
    return (st(o["skip"], 200, {"added": [2, 3], "skipped": [
                {"where": "line 2", "reason": busy % 1}, {"where": "line 5", "reason": busy % 2},
                {"where": "line 6", "reason": "outside opening hours 09:00-17:00"}]})
            and [(b["id"], b["tz"]) for b in o["mid"]] == [(1, "UTC"), (2, "CET"), (3, "CET")]
            and st(o["fail"], 409, {"error": "line 3: " + busy % 2}) and o["end"] == o["mid"])


# exporters --------------------------------------------------------------------------------------------------
EXPORT_SETUP = "seed('s', %r)\n" % EXPORT_ROWS


@item("export/csv_tz", EXPORT_SETUP + r'''
OUT["e"] = cli("--store", "s", "export", "csv", "out.csv")
OUT["text"] = read("out.csv")
''')
def _(o):
    return (res(o["e"], 0, "exported 2 bookings to out.csv\n")
            and o["text"] == ("id,room,start,end,title,owner,attendees,tz\n"
                              '1,pune,2026-03-02 09:15,2026-03-02 10:00,"Stand, up",raj,1,+05:45\n'
                              "2,kyoto,2026-03-02 10:00,2026-03-02 11:00,Review,ken,3,JST\n"))


@item("export/json_tz", EXPORT_SETUP + r'''
OUT["e"] = cli("--store", "s", "export", "json", "out.json")
OUT["text"] = read("out.json")
''')
def _(o):
    doc = json.loads(o["text"])
    return (o["e"]["code"] == 0 and o["text"].endswith("]\n")
            and doc == [B(1, "pune", "2026-03-02 09:15", "2026-03-02 10:00", "Stand, up", "raj", 1, "+05:45"),
                        B(2, "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "Review", "ken", 3, "JST")]
            and all(list(x) == BOOKING_KEYS for x in doc))


@item("export/block_tz", EXPORT_SETUP + r'''
OUT["e"] = cli("--store", "s", "export", "block", "out.txt")
OUT["text"] = read("out.txt")
''')
def _(o):
    return o["e"]["code"] == 0 and o["text"] == (
        "BEGIN:BOOKING\nID:1\nROOM:pune\nSTART:2026-03-02 09:15\nEND:2026-03-02 10:00\nTZ:+05:45\nTITLE:Stand, up\n"
        "OWNER:raj\nATTENDEES:1\nEND:BOOKING\n\nBEGIN:BOOKING\nID:2\nROOM:kyoto\nSTART:2026-03-02 10:00\n"
        "END:2026-03-02 11:00\nTZ:JST\nTITLE:Review\nOWNER:ken\nATTENDEES:3\nEND:BOOKING\n")


@item("export/agenda_tz_option", r'''
seed("s", ROWS)
OUT["jst"] = cli("--store", "s", "export", "agenda", "a.tsv", "--opt", "tz=JST")
OUT["a"] = read("a.tsv")
OUT["utc"] = cli("--store", "s", "export", "agenda", "b.tsv")
OUT["b"] = read("b.tsv")
OUT["est"] = cli("--store", "s", "export", "agenda", "c.tsv", "--opt", "tz=EST", "--opt", "room=berlin")
OUT["c"] = read("c.tsv")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    head = "date\ttime\troom\ttitle\towner\n"
    return (o["jst"]["code"] == 0 and o["utc"]["code"] == 0 and o["est"]["code"] == 0
            and o["a"] == head + ("2026-03-02\t16:00-17:00\tberlin\tKickoff\teva\n"
                                  "2026-03-02\t23:30-00:30+1\tberlin\tWrap\teva\n"
                                  "2026-03-03\t08:30-09:30\tatlas\tLate\tana\n"
                                  "2026-03-03\t09:00-10:00\tkyoto\tEarly\tken\n"
                                  "2026-03-03\t11:30-12:30\tpune\tChai\traj\n")
            and o["b"] == head + ("2026-03-02\t07:00-08:00\tberlin\tKickoff\teva\n"
                                  "2026-03-02\t14:30-15:30\tberlin\tWrap\teva\n"
                                  "2026-03-02\t23:30-00:30+1\tatlas\tLate\tana\n"
                                  "2026-03-03\t00:00-01:00\tkyoto\tEarly\tken\n"
                                  "2026-03-03\t02:30-03:30\tpune\tChai\traj\n")
            and o["c"] == head + ("2026-03-02\t02:00-03:00\tberlin\tKickoff\teva\n"
                                  "2026-03-02\t09:30-10:30\tberlin\tWrap\teva\n"))


@item("e2e/block_csv_json", r'''
seed("a")
write("in.txt", "BEGIN:BOOKING\nROOM:berlin\nSTART:2026-03-02 08:00-03:00\nEND:2026-03-02 09:00-03:00\n"
      "TITLE:Remote\nOWNER:lia\nEND:BOOKING\nBEGIN:BOOKING\nROOM:atlas\nTZ:AEST\nSTART:2026-03-02 19:00\n"
      "END:2026-03-02 20:00\nTITLE:Sydney\nOWNER:max\nEND:BOOKING\n")
OUT["i1"] = cli("--store", "a", "import", "block", "in.txt")
OUT["x"] = cli("--store", "a", "export", "csv", "x.csv")
seed("b")
OUT["i2"] = cli("--store", "b", "import", "csv", "x.csv")
OUT["y"] = cli("--store", "b", "export", "json", "y.json")
OUT["doc"] = read("y.json")
OUT["ag"] = cli("--store", "b", "agenda", "2026-03-02", "--tz", "BRT", "--json")
''')
def _(o):
    return (res(o["i1"], 0, "imported 2, skipped 0\n") and res(o["i2"], 0, "imported 2, skipped 0\n")
            and json.loads(o["doc"]) == [
                B(1, "berlin", "2026-03-02 08:00", "2026-03-02 09:00", "Remote", "lia", 1, "-03:00"),
                B(2, "atlas", "2026-03-02 19:00", "2026-03-02 20:00", "Sydney", "max", 1, "AEST")]
            and o["ag"]["json"] == {"date": "2026-03-02", "rows": [
                R(2, "atlas", "06:00", "07:00", "Sydney", "max"), R(1, "berlin", "08:00", "09:00", "Remote", "lia")]})


# request handler --------------------------------------------------------------------------------------------
@item("api/rooms_bookings_tz", r'''
OUT["r1"] = api("s", "POST", "/rooms", {"id": "syd", "name": "Sydney", "capacity": 4, "tz": "aest"})
OUT["r2"] = api("s", "POST", "/rooms", {"id": "lon", "name": "London", "capacity": 4})
OUT["r3"] = api("s", "POST", "/rooms", {"id": "bad", "name": "Bad", "capacity": 1, "tz": "XYZ"})
bk = {"start": "2026-03-02 09:00", "end": "2026-03-02 10:00", "title": "A", "owner": "ana"}
OUT["b1"] = api("s", "POST", "/bookings", dict(bk, room="syd"))
OUT["b2"] = api("s", "POST", "/bookings", dict(bk, room="lon", tz="est"))
OUT["b3"] = api("s", "POST", "/bookings", dict(bk, room="lon", tz="+05:20"))
OUT["rooms"] = api("s", "GET", "/rooms")
OUT["one"] = api("s", "GET", "/bookings/1")
OUT["lon"] = api("s", "GET", "/bookings?room=lon")
''')
def _(o):
    syd = {"id": "syd", "name": "Sydney", "capacity": 4, "opens": "08:00", "closes": "18:00", "tags": [],
           "tz": "AEST"}
    b1 = B(1, "syd", "2026-03-02 09:00", "2026-03-02 10:00", "A", "ana", 1, "AEST")
    b2 = B(2, "lon", "2026-03-02 09:00", "2026-03-02 10:00", "A", "ana", 1, "EST")
    rl = (o["rooms"]["json"] or {}).get("rooms") or []
    return (st(o["r1"], 201, {"room": syd}) and st(o["r2"], 201)
            and st(o["r3"], 400, {"error": "unknown zone 'XYZ'"})
            and st(o["b1"], 201, {"booking": b1}) and st(o["b2"], 201, {"booking": b2})
            and st(o["b3"], 400, {"error": "unknown zone '+05:20'"})
            and [(r["id"], r["tz"]) for r in rl] == [("lon", "UTC"), ("syd", "AEST")] and list(rl[0]) == ROOM_KEYS
            and st(o["one"], 200, {"booking": b1}) and st(o["lon"], 200, {"bookings": [b2]}))


@item("api/move_tz", r'''
seed("s", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,UTC",
           "2,berlin,2026-03-02 10:00,2026-03-02 11:00,B,bob,1,CET"])
OUT["m1"] = api("s", "PATCH", "/bookings/1", {"start": "2026-03-02 12:00", "tz": "CET"})
OUT["m2"] = api("s", "PATCH", "/bookings/1", {"start": "2026-03-02 09:30", "room": "berlin", "tz": "UTC"})
OUT["m3"] = api("s", "PATCH", "/bookings/1", {"start": "2026-03-02 10:00", "room": "berlin", "tz": "UTC"})
OUT["m4"] = api("s", "PATCH", "/bookings/1", {"start": "2026-03-02 13:00"})
OUT["m5"] = api("s", "PATCH", "/bookings/2", {"start": "2026-03-02 10:00", "tz": "Mars"})
OUT["end"] = bookings("s")
''')
def _(o):
    bj = lambda r: ((r["json"] or {}).get("booking") or {})
    return (st(o["m1"], 200) and sub(bj(o["m1"]), {"start": "2026-03-02 12:00", "end": "2026-03-02 13:00",
                                                   "tz": "CET", "room": "atlas"})
            and st(o["m2"], 409, {"error": "room 'berlin' is busy: conflicts with booking 2"})
            and st(o["m3"], 200) and sub(bj(o["m3"]), {"room": "berlin", "start": "2026-03-02 10:00", "tz": "UTC"})
            and st(o["m4"], 200) and sub(bj(o["m4"]), {"start": "2026-03-02 13:00", "end": "2026-03-02 14:00",
                                                       "tz": "UTC"})
            and st(o["m5"], 400, {"error": "unknown zone 'Mars'"})
            and o["end"] == [B(1, "berlin", "2026-03-02 13:00", "2026-03-02 14:00", "A", "ana", 1, "UTC"),
                             B(2, "berlin", "2026-03-02 10:00", "2026-03-02 11:00", "B", "bob", 1, "CET")])


@item("api/agenda_query_tz", r'''
seed("s", ROWS)
OUT["jst"] = api("s", "GET", "/agenda?date=2026-03-03&tz=JST")
OUT["utc"] = api("s", "GET", "/agenda?date=2026-03-02")
OUT["room"] = api("s", "GET", "/agenda?date=2026-03-02&tz=JST&room=berlin")
OUT["bad"] = api("s", "GET", "/agenda?date=2026-03-02&tz=XYZ")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    return (st(o["jst"], 200, {"date": "2026-03-03", "rows": AG_0303_JST})
            and st(o["utc"], 200, {"date": "2026-03-02", "rows": AG_0302_UTC})
            and st(o["room"], 200, {"date": "2026-03-02", "rows": AG_0302_JST})
            and st(o["bad"], 400, {"error": "unknown zone 'XYZ'"}))


@item("api/reports_query_tz", r'''
seed("s", ROWS)
OUT["ist"] = api("s", "GET", "/reports/busiest?tz=IST")
OUT["istday"] = api("s", "GET", "/reports/busiest?tz=IST&date_from=2026-03-02&date_to=2026-03-02")
OUT["ag"] = api("s", "GET", "/reports/agenda?date=2026-03-02&tz=JST")
OUT["cf"] = api("s", "GET", "/reports/conflicts?tz=JST")
OUT["ut"] = api("s", "GET", "/reports/utilization?date_from=2026-03-02&date_to=2026-03-02&tz=UTC")
'''.replace("ROWS", repr(BUSY_ROWS)))
def _(o):
    h = lambda pairs: [{"hour": a, "bookings": n} for a, n in pairs]
    return (st(o["ist"], 200, {"report": "busiest", "rows": h([("09", 3), ("08", 2), ("01", 1), ("02", 1)])})
            and st(o["istday"], 200, {"report": "busiest", "rows": h([("09", 3), ("08", 2)])})
            and st(o["ag"], 200, {"report": "agenda", "rows": [
                R(2, "atlas", "12:00", "13:00", "B", "bob"), R(3, "kyoto", "12:00", "13:00", "C", "cyd"),
                R(1, "pune", "12:30", "13:30", "A", "ana")]})
            and st(o["cf"], 400, {"error": "report 'conflicts' does not take option 'tz'"})
            and st(o["ut"], 400, {"error": "report 'utilization' does not take option 'tz'"}))


@item("api/free_query_tz", r'''
seed("s", ROWS)
base = "/rooms/kyoto/free?date=2026-03-02&duration=60&step=60"
OUT["local"] = api("s", "GET", base)
OUT["utc"] = api("s", "GET", base + "&tz=UTC")
OUT["ict"] = api("s", "GET", "/rooms/kyoto/free?date=2026-03-02&duration=90&step=30&tz=ICT")
OUT["bad"] = api("s", "GET", base + "&tz=XYZ")
'''.replace("ROWS", repr(FREE_ROWS)))
def _(o):
    ict = ["2026-03-02 09:00", "2026-03-02 09:30", "2026-03-02 10:00", "2026-03-02 10:30", "2026-03-02 13:00",
           "2026-03-02 13:30"]
    return (st(o["local"], 200, {"room": "kyoto", "date": "2026-03-02", "slots": FREE_JST})
            and st(o["utc"], 200, {"room": "kyoto", "date": "2026-03-02", "slots": FREE_UTC})
            and st(o["ict"], 200, {"room": "kyoto", "date": "2026-03-02", "slots": ict})
            and st(o["bad"], 400, {"error": "unknown zone 'XYZ'"}))


# pipelines --------------------------------------------------------------------------------------------------
PIPE_ROWS = AGENDA_ROWS[:3] + [AGENDA_ROWS[3]]


@item("pipe/to_zone_step", r'''
seed("s", ROWS)
write("p.json", json.dumps({"input": {"store": "s"}, "steps": [{"step": "to_zone", "tz": "jst"}],
                            "output": {"format": "json", "path": "o.json"}}))
OUT["p"] = cli("pipeline", "p.json")
OUT["o"] = read("o.json")
write("q.json", json.dumps({"input": {"store": "s"}, "steps": [{"step": "to_zone", "tz": "-03:00"},
                            {"step": "where", "field": "id", "op": "eq", "value": 1}],
                            "output": {"format": "csv", "path": "q.csv"}}))
OUT["q"] = cli("pipeline", "q.json")
OUT["qt"] = read("q.csv")
write("bad.json", json.dumps({"input": {"store": "s"}, "steps": [
    {"step": "where", "field": "room", "op": "ne", "value": "x"}, {"step": "to_zone", "tz": "Mars"}],
    "output": {"format": "json", "path": "bad.out"}}))
OUT["bad"] = cli("pipeline", "bad.json")
'''.replace("ROWS", repr(PIPE_ROWS)))
def _(o):
    return (res(o["p"], 0, "pipeline: read 4, wrote 4 to o.json\n")
            and json.loads(o["o"]) == [
                B(1, "atlas", "2026-03-03 08:30", "2026-03-03 09:30", "Late", "ana", 1, "JST"),
                B(2, "kyoto", "2026-03-03 09:00", "2026-03-03 10:00", "Early", "ken", 1, "JST"),
                B(3, "pune", "2026-03-03 11:30", "2026-03-03 12:30", "Chai", "raj", 1, "JST"),
                B(4, "berlin", "2026-03-02 16:00", "2026-03-02 17:00", "Kickoff", "eva", 1, "JST")]
            and res(o["q"], 0)
            and o["qt"] == ("id,room,start,end,title,owner,attendees,tz\n"
                            "1,atlas,2026-03-02 20:30,2026-03-02 21:30,Late,ana,1,-03:00\n")
            and res(o["bad"], 2, "", "error: step 2 (to_zone): unknown zone 'Mars'\n"))


@item("pipe/where_tz", r'''
seed("s", ROWS)
write("p.json", json.dumps({"input": {"store": "s"}, "steps": [{"step": "where", "field": "tz", "op": "eq",
                            "value": "CET"}], "output": {"format": "csv", "path": "o.csv"}}))
OUT["p"] = cli("pipeline", "p.json")
OUT["o"] = read("o.csv")
write("q.json", json.dumps({"input": {"store": "s"}, "steps": [{"step": "where", "field": "tz", "op": "ne",
                            "value": "UTC"}, {"step": "sort", "by": "owner", "desc": True}],
                            "output": {"format": "json", "path": "o.json"}}))
OUT["q"] = cli("pipeline", "q.json")
OUT["qo"] = read("o.json")
'''.replace("ROWS", repr(PIPE_ROWS)))
def _(o):
    return (res(o["p"], 0, "pipeline: read 4, wrote 1 to o.csv\n")
            and o["o"] == ("id,room,start,end,title,owner,attendees,tz\n"
                           "4,berlin,2026-03-02 08:00,2026-03-02 09:00,Kickoff,eva,1,CET\n")
            and res(o["q"], 0) and [b["id"] for b in json.loads(o["qo"])] == [3, 2, 4])


@item("pipe/sort_start_absolute", r'''
seed("s", ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,A,ana,1,UTC",
           "2,kyoto,2026-03-02 17:00,2026-03-02 18:00,B,bob,1,JST",
           "3,pune,2026-03-02 14:00,2026-03-02 15:00,C,cyd,1,IST",
           "4,berlin,2026-03-02 10:00,2026-03-02 11:00,D,dan,1,CET",
           "5,atlas,2026-03-02 01:00,2026-03-02 02:00,E,eve,1,PST"])
def run(steps, name):
    write(name + ".json", json.dumps({"input": {"store": "s"}, "steps": steps,
                                      "output": {"format": "json", "path": name + ".out"}}))
    r = cli("pipeline", name + ".json")
    return [r["code"], [b["id"] for b in json.loads(read(name + ".out") or "[]")]]
OUT["asc"] = run([{"step": "sort", "by": "start"}], "asc")
OUT["desc"] = run([{"step": "sort", "by": "start", "desc": True}], "desc")
OUT["top"] = run([{"step": "sort", "by": "start"}, {"step": "limit", "n": 2}], "top")
''')
def _(o):
    return o["asc"] == [0, [2, 3, 1, 4, 5]] and o["desc"] == [0, [1, 4, 5, 3, 2]] and o["top"] == [0, [2, 3]]


@item("pipe/report_sinks_tz", r'''
seed("s", ROWS)
def run(cfg, name):
    write(name + ".json", json.dumps(cfg))
    return cli("pipeline", name + ".json")
OUT["ag"] = run({"input": {"store": "s"}, "report": {"name": "agenda", "path": "r.json",
                 "options": {"date": "2026-03-03", "tz": "JST"}}}, "ag")
OUT["r"] = read("r.json")
OUT["bu"] = run({"input": {"store": "s"}, "steps": [{"step": "where", "field": "room", "op": "eq", "value": "berlin"}],
                 "report": {"name": "busiest", "path": "b.json", "options": {"tz": "JST"}}}, "bu")
OUT["b"] = read("b.json")
OUT["ex"] = run({"input": {"store": "s"}, "output": {"format": "agenda", "path": "a.tsv",
                 "options": {"tz": "JST", "room": "berlin"}}}, "ex")
OUT["a"] = read("a.tsv")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    h = lambda pairs: [{"hour": a, "bookings": n} for a, n in pairs]
    return (res(o["ag"], 0) and json.loads(o["r"]) == {"report": "agenda", "rows": AG_0303_JST}
            and res(o["bu"], 0) and json.loads(o["b"]) == {"report": "busiest",
                                                           "rows": h([("00", 1), ("16", 1), ("23", 1)])}
            and res(o["ex"], 0) and o["a"] == ("date\ttime\troom\ttitle\towner\n"
                                               "2026-03-02\t16:00-17:00\tberlin\tKickoff\teva\n"
                                               "2026-03-02\t23:30-00:30+1\tberlin\tWrap\teva\n"))


@item("pipe/file_input_zones", r'''
seed("rs")
write("in.csv", "room,start,end,title,owner\nkyoto,2026-03-02 10:00,2026-03-02 11:00,A,ana\n"
      "zzz,2026-03-02 10:00,2026-03-02 11:00,B,bob\n")
write("in2.csv", "room,start,end,title,owner,tz\nberlin,2026-03-02 10:00,2026-03-02 11:00,C,cyd,EST\n"
      "berlin,2026-03-02 12:00,2026-03-02 13:00,D,dan,\n")
def run(cfg, name):
    write(name + ".json", json.dumps(cfg))
    r = cli("pipeline", name + ".json")
    return [r["code"], [(b["id"], b["room"], b["tz"]) for b in json.loads(read(name + ".out") or "[]")]]
OUT["a"] = run({"input": {"format": "csv", "path": "in.csv"}, "rooms": "rs",
                "output": {"format": "json", "path": "a.out"}}, "a")
OUT["b"] = run({"input": {"format": "csv", "path": "in.csv"}, "output": {"format": "json", "path": "b.out"}}, "b")
OUT["c"] = run({"input": {"format": "csv", "path": "in2.csv"}, "rooms": "rs",
                "output": {"format": "json", "path": "c.out"}}, "c")
''')
def _(o):
    return (o["a"] == [0, [[1, "kyoto", "JST"], [2, "zzz", "UTC"]]]
            and o["b"] == [0, [[1, "kyoto", "UTC"], [2, "zzz", "UTC"]]]
            and o["c"] == [0, [[1, "berlin", "EST"], [2, "berlin", "CET"]]])


# features the change does not name ------------------------------------------------------------------------------
@item("reg/more_features_utc", r'''
cli("--store", "s", "add-room", "atlas", "--name", "Atlas", "--capacity", "4")
def bk(*a):
    return cli("--store", "s", *a)
OUT["b1"] = bk("book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "A", "--owner", "ana")
OUT["w1"] = bk("wait", "atlas", "2026-03-02 09:30", "2026-03-02 10:30", "--title", "W", "--owner", "bob")
OUT["w2"] = bk("wait", "atlas", "2026-03-02 11:00", "2026-03-02 12:00", "--title", "X", "--owner", "bob")
OUT["c"] = bk("cancel", "1")
OUT["hist"] = bk("history", "--json")["json"]
OUT["ser"] = bk("book-series", "atlas", "2026-03-03 09:00", "2026-03-03 10:00", "--title", "S", "--owner", "ana",
                "--count", "2", "--every", "1")
OUT["dg"] = bk("digest", "ana", "--from", "2026-03-02", "--to", "2026-03-04", "--json")["json"]
OUT["daily"] = bk("report", "daily_bookings", "--from", "2026-03-02", "--to", "2026-03-04", "--json")["json"]
OUT["stats"] = bk("stats", "--json")["json"]
write("p.txt", "site;day;from;until;subject;contact;seats;zone\natlas;05.03.2026;09:00;10:00;P;cyd;2;\n")
OUT["pi"] = bk("import", "partner", "p.txt")
OUT["e1"] = bk("edit-room", "atlas", "--closes", "09:45")
OUT["e2"] = bk("edit-room", "atlas", "--name", "Big", "--capacity", "6")
OUT["rem"] = bk("reminders", "2026-03-03", "--before", "30")
OUT["v1"] = api("s", "GET", "/v1/bookings/2")
write("r.csv", "id,name,capacity\nbern,Bern,3\natlas,A,2\n")
OUT["ri"] = bk("import-rooms", "r.csv", "--json")
OUT["all"] = bookings("s")
''')
def _(o):
    keep = ("seq", "action", "booking", "room", "start", "end")
    hist = [{k: h[k] for k in keep} for h in (o["hist"] or {}).get("history", [])]
    dg = o["dg"] or {}
    item_ = ((o["v1"]["json"] or {}).get("item")) or {}
    return (res(o["b1"], 0, "booked 1\n") and res(o["w1"], 0, "waitlisted 1\n")
            and res(o["w2"], 1, "", "error: room 'atlas' is free at that time: book it instead\n")
            and res(o["c"], 0, "cancelled 1\nbooked 1 from waitlist entry 1\n")
            and hist == [{"seq": 1, "action": "book", "booking": 1, "room": "atlas", "start": "2026-03-02 09:00",
                          "end": "2026-03-02 10:00"},
                         {"seq": 2, "action": "cancel", "booking": 1, "room": "atlas", "start": "2026-03-02 09:00",
                          "end": "2026-03-02 10:00"},
                         {"seq": 3, "action": "waitlist", "booking": 1, "room": "atlas", "start": "2026-03-02 09:30",
                          "end": "2026-03-02 10:30"}]
            and res(o["ser"], 0, "booked 2, 3\n")
            and dg == {"owner": "ana", "days": [
                {"date": "2026-03-03", "items": [{"id": 2, "room": "atlas", "time": "09:00-10:00", "title": "S"}]},
                {"date": "2026-03-04", "items": [{"id": 3, "room": "atlas", "time": "09:00-10:00", "title": "S"}]}]}
            and o["daily"] == {"report": "daily_bookings", "rows": [{"date": "2026-03-02", "value": 1},
                                                                    {"date": "2026-03-03", "value": 1},
                                                                    {"date": "2026-03-04", "value": 1}]}
            and sub(o["stats"], {"rooms": 1, "bookings": 3, "waitlist": 0, "first_start": "2026-03-02 09:30",
                                 "last_end": "2026-03-04 10:00", "busiest_room": "atlas"})
            and res(o["pi"], 0, "imported 1, skipped 0\n")
            and res(o["e1"], 1, "", "error: booking 1 would break: outside opening hours 08:00-09:45\n")
            and res(o["e2"], 0, "updated room atlas\n")
            and res(o["rem"], 0, "2026-03-03 08:30 ana: S in atlas at 09:00\n")
            and sub(item_, {"ref": 2, "where": "atlas", "when": "2026-03-03 09:00", "until": "2026-03-03 10:00",
                            "what": "S", "who": "ana"})
            and o["ri"]["json"] == {"added": ["bern"], "skipped": [{"where": "line 3", "reason": "room 'atlas' exists"}]}
            and [(b["id"], b["start"], b["attendees"]) for b in o["all"]] == [
                (1, "2026-03-02 09:30", 1), (2, "2026-03-03 09:00", 1), (3, "2026-03-04 09:00", 1),
                (4, "2026-03-05 09:00", 2)])


@item("waitlist/zones_and_promotion", r'''
seed("s")
def c(*a):
    return cli("--store", "s", *a)
OUT["b1"] = c("book", "atlas", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "A", "--owner", "ana")
c("book", "berlin", "2026-03-02 12:00", "2026-03-02 13:00", "--title", "K", "--owner", "kim")
OUT["w1"] = c("wait", "atlas", "2026-03-02 10:15", "2026-03-02 10:45", "--title", "W1", "--owner", "bob", "--tz", "CET")
OUT["w2"] = c("wait", "atlas", "2026-03-02 09:30", "2026-03-02 10:00", "--title", "W2", "--owner", "cyd")
OUT["w3"] = c("wait", "atlas", "2026-03-02 11:00", "2026-03-02 12:00", "--title", "W3", "--owner", "dan", "--tz", "CET")
OUT["w4"] = c("wait", "atlas", "2026-03-02 13:00", "2026-03-02 13:30", "--title", "W4", "--owner", "eve", "--tz", "JST")
OUT["list1"] = c("waitlist", "--json")["json"]
OUT["cancel"] = c("cancel", "1")
OUT["list2"] = c("waitlist", "--json")["json"]
OUT["mid"] = bookings("s")
OUT["move"] = c("move", "3", "2026-03-02 11:00")
OUT["end"] = bookings("s")
OUT["list3"] = c("waitlist", "--json")["json"]
''')
def _(o):
    e1 = {"id": 1, "room": "atlas", "start": "2026-03-02 10:15", "end": "2026-03-02 10:45", "title": "W1",
          "owner": "bob", "attendees": 1, "tz": "CET"}
    e2 = {"id": 2, "room": "atlas", "start": "2026-03-02 09:30", "end": "2026-03-02 10:00", "title": "W2",
          "owner": "cyd", "attendees": 1, "tz": "UTC"}
    return (res(o["w1"], 0, "waitlisted 1\n") and res(o["w2"], 0, "waitlisted 2\n")
            and res(o["w3"], 1, "", "error: room 'atlas' is free at that time: book it instead\n")
            and res(o["w4"], 1, "", "error: outside opening hours 08:00-18:00\n")
            and o["list1"] == {"waitlist": [e1, e2]} and list(o["list1"]["waitlist"][0]) == BOOKING_KEYS
            and res(o["cancel"], 0, "cancelled 1\nbooked 3 from waitlist entry 1\n")
            and o["list2"] == {"waitlist": [e2]}
            and o["mid"][1:] == [B(3, "atlas", "2026-03-02 10:15", "2026-03-02 10:45", "W1", "bob", 1, "CET")]
            and res(o["move"], 0, "moved 3\nbooked 4 from waitlist entry 2\n")
            and o["end"][1:] == [B(3, "atlas", "2026-03-02 11:00", "2026-03-02 11:30", "W1", "bob", 1, "CET"),
                                 B(4, "atlas", "2026-03-02 09:30", "2026-03-02 10:00", "W2", "cyd", 1, "UTC")]
            and o["list3"] == {"waitlist": []})


@item("waitlist/stores_and_api", r'''
seed("s", ["1,pune,2026-03-02 10:00,2026-03-02 11:00,B,bob,1,IST"])
write("s/waitlist.csv", "#roomkit-schema: 2\nid,room,start,end,title,owner,attendees\n"
      "1,kyoto,2026-03-02 10:00,2026-03-02 11:00,W,ana,1\n")
write("s/history.csv", "#roomkit-schema: 2\nseq,action,booking,room,start,end\n"
      "1,book,1,pune,2026-03-02 10:00,2026-03-02 11:00\n")
OUT["m"] = cli("--store", "s", "migrate")["code"]
OUT["wl"] = read("s/waitlist.csv")
OUT["hi"] = read("s/history.csv")
seed("a", ["1,berlin,2026-03-02 10:00,2026-03-02 11:00,B,bob,1,CET",
           "2,atlas,2026-03-02 12:00,2026-03-02 13:00,K,kim,1,UTC"])
body = {"room": "berlin", "start": "2026-03-02 09:30", "end": "2026-03-02 10:30", "title": "W", "owner": "eve"}
OUT["j1"] = api("a", "POST", "/waitlist", dict(body, tz="UTC"))
OUT["j2"] = api("a", "POST", "/waitlist", dict(body, tz="XYZ"))
OUT["del"] = api("a", "DELETE", "/bookings/1")
OUT["wl2"] = api("a", "GET", "/waitlist")
OUT["b2"] = api("a", "GET", "/bookings/3")
api("w.json", "POST", "/rooms", {"id": "sf", "name": "SF", "capacity": 3, "tz": "PST"})
api("w.json", "POST", "/bookings", {"room": "sf", "start": "2026-03-02 09:00", "end": "2026-03-02 10:00",
                                     "title": "A", "owner": "ana"})
OUT["j3"] = api("w.json", "POST", "/waitlist", {"room": "sf", "start": "2026-03-02 09:30", "end": "2026-03-02 10:00",
                                                 "title": "W", "owner": "bob"})
OUT["doc"] = read("w.json")
''')
def _(o):
    doc = json.loads(o["doc"] or "{}")
    ent = (o["j1"]["json"] or {}).get("entry") or {}
    return (o["m"] == 0
            and o["wl"] == ("#roomkit-schema: 3\nid,room,start,end,title,owner,attendees,tz\n"
                            "1,kyoto,2026-03-02 10:00,2026-03-02 11:00,W,ana,1,JST\n")
            and o["hi"] == ("#roomkit-schema: 3\nseq,action,booking,room,start,end,tz\n"
                            "1,book,1,pune,2026-03-02 10:00,2026-03-02 11:00,IST\n")
            and st(o["j1"], 201) and sub(ent, {"id": 1, "start": "2026-03-02 09:30", "tz": "UTC"})
            and st(o["j2"], 400, {"error": "unknown zone 'XYZ'"})
            and st(o["del"], 200, {"cancelled": 1, "promoted": [{"entry": 1, "booking": 3}]})
            and st(o["wl2"], 200, {"waitlist": []})
            and st(o["b2"], 200, {"booking": B(3, "berlin", "2026-03-02 09:30", "2026-03-02 10:30", "W", "eve", 1,
                                               "UTC")})
            and st(o["j3"], 201) and doc.get("schema") == 3 and len(doc.get("waitlist", [])) == 1
            and list(doc["waitlist"][0]) == BOOKING_KEYS and doc["waitlist"][0]["tz"] == "PST"
            and [h["tz"] for h in doc.get("history", [])] == ["PST"] and list(doc["history"][0])[-1] == "tz")


@item("history/records_tz", r'''
seed("s")
cli("--store", "s", "book", "kyoto", "2026-03-02 01:00", "2026-03-02 02:00", "--title", "A", "--owner", "ana",
    "--tz", "UTC")
cli("--store", "s", "move", "1", "2026-03-02 11:00", "--tz", "JST")
cli("--store", "s", "cancel", "1")
OUT["h"] = cli("--store", "s", "history", "--json")["json"]
OUT["tab"] = cli("--store", "s", "history")["out"]
OUT["file"] = read("s/history.csv")
OUT["api"] = api("s", "GET", "/history?booking=1")
''')
def _(o):
    h = lambda seq, act, s, e, tz: {"seq": seq, "action": act, "booking": 1, "room": "kyoto", "start": s, "end": e,
                                    "tz": tz}
    want = [h(1, "book", "2026-03-02 01:00", "2026-03-02 02:00", "UTC"),
            h(2, "move", "2026-03-02 11:00", "2026-03-02 12:00", "JST"),
            h(3, "cancel", "2026-03-02 11:00", "2026-03-02 12:00", "JST")]
    lines = o["tab"].split("\n")
    return (o["h"] == {"history": want} and all(list(x) == list(want[0]) for x in o["h"]["history"])
            and lines[0].split()[-1] == "tz" and [l.split()[-1] for l in lines[2:5]] == ["UTC", "JST", "JST"]
            and o["file"] == ("#roomkit-schema: 3\nseq,action,booking,room,start,end,tz\n"
                              "1,book,1,kyoto,2026-03-02 01:00,2026-03-02 02:00,UTC\n"
                              "2,move,1,kyoto,2026-03-02 11:00,2026-03-02 12:00,JST\n"
                              "3,cancel,1,kyoto,2026-03-02 11:00,2026-03-02 12:00,JST\n")
            and st(o["api"], 200, {"history": want}))


@item("series/zones", r'''
seed("s")
def c(*a):
    return cli("--store", "s", *a)
c("book", "atlas", "2026-03-03 09:00", "2026-03-03 10:00", "--title", "A", "--owner", "ana")
OUT["s1"] = c("book-series", "kyoto", "2026-03-02 01:00", "2026-03-02 02:00", "--title", "S", "--owner", "ken",
              "--count", "3", "--every", "1", "--tz", "UTC")
OUT["s2"] = c("book-series", "atlas", "2026-03-02 10:30", "2026-03-02 11:00", "--title", "T", "--owner", "bob",
              "--count", "2", "--every", "1", "--tz", "CET")
OUT["s3"] = c("book-series", "pune", "2026-03-02 09:00", "2026-03-02 10:00", "--title", "P", "--owner", "raj",
              "--count", "2")
OUT["all"] = [(b["id"], b["room"], b["start"], b["tz"]) for b in bookings("s")]
body = {"room": "berlin", "start": "2026-03-02 08:00", "end": "2026-03-02 09:00", "title": "A", "owner": "eva",
        "count": 2, "every": 7}
OUT["a1"] = api("s", "POST", "/series", dict(body, tz="UTC"))
OUT["a2"] = api("s", "POST", "/series", dict(body, tz="Mars"))
''')
def _(o):
    made = (o["a1"]["json"] or {}).get("bookings") or []
    return (res(o["s1"], 0, "booked 2, 3, 4\n")
            and res(o["s2"], 1, "", "error: occurrence 2: room 'atlas' is busy: conflicts with booking 1\n")
            and res(o["s3"], 0, "booked 5, 6\n")
            and o["all"] == [[1, "atlas", "2026-03-03 09:00", "UTC"], [2, "kyoto", "2026-03-02 01:00", "UTC"],
                             [3, "kyoto", "2026-03-03 01:00", "UTC"], [4, "kyoto", "2026-03-04 01:00", "UTC"],
                             [5, "pune", "2026-03-02 09:00", "IST"], [6, "pune", "2026-03-09 09:00", "IST"]]
            and st(o["a1"], 201) and [(b["id"], b["start"], b["tz"]) for b in made] == [
                (7, "2026-03-02 08:00", "UTC"), (8, "2026-03-09 08:00", "UTC")]
            and st(o["a2"], 400, {"error": "unknown zone 'Mars'"}))


@item("import/partner_zone", r'''
seed("s")
write("p.txt", "site;day;from;until;subject;contact;seats;zone\nkyoto;02.03.2026;01:00;02:00;A;ana;2;UTC\n"
      "pune;02.03.2026;10:00;11:00;B;bob;;\nberlin;02.03.2026;08:00;09:00;C;cyd;;-03:00\n")
OUT["ok"] = cli("--store", "s", "import", "partner", "p.txt", "--json")
OUT["all"] = bookings("s")
write("q.txt", "site;day;from;until;subject;contact;zone\natlas;02.03.2026;09:00;10:00;X;ana;Mars\n")
OUT["bad"] = cli("--store", "s", "import", "partner", "q.txt")
''')
def _(o):
    return (o["ok"]["json"] == {"added": [1, 2, 3], "skipped": []}
            and o["all"] == [B(1, "kyoto", "2026-03-02 01:00", "2026-03-02 02:00", "A", "ana", 2, "UTC"),
                             B(2, "pune", "2026-03-02 10:00", "2026-03-02 11:00", "B", "bob", 1, "IST"),
                             B(3, "berlin", "2026-03-02 08:00", "2026-03-02 09:00", "C", "cyd", 1, "-03:00")]
            and res(o["bad"], 1, "", "error: line 2: unknown zone 'Mars'\n"))


@item("digest/zones", r'''
seed("s", ROWS)
OUT["eva"] = cli("--store", "s", "digest", "eva", "--from", "2026-03-02", "--to", "2026-03-03", "--tz", "JST", "--json")
OUT["ana"] = cli("--store", "s", "digest", "ana", "--from", "2026-03-03", "--to", "2026-03-03", "--tz", "JST", "--json")
OUT["utc"] = cli("--store", "s", "digest", "ana", "--from", "2026-03-02", "--to", "2026-03-02", "--json")
OUT["text"] = cli("--store", "s", "digest", "eva", "--from", "2026-03-02", "--to", "2026-03-02", "--tz", "JST")
OUT["api"] = api("s", "GET", "/owners/eva/digest?date_from=2026-03-01&date_to=2026-03-01&tz=PST")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    it = lambda i, room, time, title: {"id": i, "room": room, "time": time, "title": title}
    return (o["eva"]["json"] == {"owner": "eva", "days": [{"date": "2026-03-02", "items": [
                it(4, "berlin", "16:00-17:00", "Kickoff"), it(5, "berlin", "23:30-00:30+1", "Wrap")]}]}
            and o["ana"]["json"] == {"owner": "ana", "days": [{"date": "2026-03-03", "items": [
                it(1, "atlas", "08:30-09:30", "Late")]}]}
            and o["utc"]["json"] == {"owner": "ana", "days": [{"date": "2026-03-02", "items": [
                it(1, "atlas", "23:30-00:30+1", "Late")]}]}
            and res(o["text"], 0, "2026-03-02\n  16:00-17:00 berlin Kickoff\n  23:30-00:30+1 berlin Wrap\n")
            and st(o["api"], 200, {"owner": "eva", "days": [{"date": "2026-03-01", "items": [
                it(4, "berlin", "23:00-00:00+1", "Kickoff")]}]}))


DAILY_ROWS = [
    "1,atlas,2026-03-02 14:00,2026-03-02 15:00,A,ana,2,UTC",
    "2,atlas,2026-03-02 15:30,2026-03-02 16:00,B,bob,3,UTC",
    "3,kyoto,2026-03-03 09:00,2026-03-03 11:00,C,ana,4,JST",
    "4,pune,2026-03-02 22:00,2026-03-02 23:30,D,cyd,1,IST",
    "5,berlin,2026-03-02 08:00,2026-03-02 08:45,E,dan,5,CET",
]


def V(*pairs):
    return [{"date": d, "value": v} for d, v in pairs]


@item("daily/counts_tz", r'''
seed("s", ROWS)
def rep(name, *more):
    r = cli("--store", "s", "report", name, "--from", "2026-03-02", "--to", "2026-03-03", "--json", *more)
    return r["json"]["rows"] if r["code"] == 0 else r["code"]
OUT["bk"] = rep("daily_bookings", "--tz", "JST")
OUT["mi"] = rep("daily_minutes", "--tz", "JST")
OUT["at"] = rep("daily_attendees", "--tz", "JST")
OUT["utc"] = rep("daily_bookings")
'''.replace("ROWS", repr(DAILY_ROWS)))
def _(o):
    return (o["bk"] == V(("2026-03-02", 2), ("2026-03-03", 3)) and o["mi"] == V(("2026-03-02", 105), ("2026-03-03", 240))
            and o["at"] == V(("2026-03-02", 7), ("2026-03-03", 8)) and o["utc"] == V(("2026-03-02", 4), ("2026-03-03", 1)))


@item("daily/times_tz", r'''
seed("s", ROWS)
def rep(name, *more):
    r = cli("--store", "s", "report", name, "--from", "2026-03-01", "--to", "2026-03-03", "--json", *more)
    return r["json"]["rows"] if r["code"] == 0 else r["code"]
OUT["fs"] = rep("daily_first_start", "--tz", "JST")
OUT["le"] = rep("daily_last_end", "--tz", "JST")
OUT["le_utc"] = rep("daily_last_end")
'''.replace("ROWS", repr(DAILY_ROWS)))
def _(o):
    return (o["fs"] == V(("2026-03-01", None), ("2026-03-02", "16:00"), ("2026-03-03", "00:30"))
            and o["le"] == V(("2026-03-01", None), ("2026-03-02", "00:00+1"), ("2026-03-03", "11:00"))
            and o["le_utc"] == V(("2026-03-01", None), ("2026-03-02", "18:00"), ("2026-03-03", "02:00")))


@item("daily/api_and_sink_tz", r'''
seed("s", ROWS)
q = "?date_from=2026-03-02&date_to=2026-03-03&tz=JST"
for name in ("daily_rooms", "daily_owners", "daily_longest", "daily_average"):
    OUT[name] = api("s", "GET", "/reports/" + name + q)
OUT["bad"] = api("s", "GET", "/reports/daily_rooms?date_from=2026-03-02&date_to=2026-03-03&tz=XYZ")
write("p.json", json.dumps({"input": {"store": "s"}, "report": {"name": "daily_average", "path": "r.json",
    "options": {"date_from": "2026-03-02", "date_to": "2026-03-03", "tz": "JST", "room": "atlas"}}}))
OUT["p"] = cli("pipeline", "p.json")["code"]
OUT["r"] = read("r.json")
'''.replace("ROWS", repr(DAILY_ROWS)))
def _(o):
    rows = lambda name: (o[name]["json"] or {}).get("rows") if o[name]["status"] == 200 else None
    return (rows("daily_rooms") == V(("2026-03-02", 2), ("2026-03-03", 3))
            and rows("daily_owners") == V(("2026-03-02", 2), ("2026-03-03", 3))
            and rows("daily_longest") == V(("2026-03-02", 60), ("2026-03-03", 120))
            and rows("daily_average") == V(("2026-03-02", "52.5"), ("2026-03-03", "80.0"))
            and st(o["bad"], 400, {"error": "unknown zone 'XYZ'"})
            and o["p"] == 0 and json.loads(o["r"] or "{}") == {"report": "daily_average",
                                                               "rows": V(("2026-03-02", "60.0"), ("2026-03-03", "30.0"))})


@item("rooms/edit_zone_revalidates", r'''
seed("s", ["1,kyoto,2026-03-02 10:00,2026-03-02 11:00,A,ana,1,JST",
           "2,kyoto,2026-03-02 05:00,2026-03-02 06:00,B,bob,1,UTC"])
def c(*a):
    return cli("--store", "s", "edit-room", "kyoto", *a)
OUT["e1"] = c("--tz", "UTC")
OUT["e2"] = c("--tz", "ICT")
OUT["e3"] = c("--tz", "ICT", "--opens", "08:00")
OUT["e4"] = c("--closes", "12:30")
OUT["e5"] = c("--tz", "Mars")
OUT["rooms"] = rooms("s")
OUT["all"] = [(b["id"], b["start"], b["tz"]) for b in bookings("s")]
OUT["p1"] = api("s", "PATCH", "/rooms/pune", {"tz": "NPT"})
OUT["p2"] = api("s", "PATCH", "/rooms/pune", {"tz": "Mars"})
''')
def _(o):
    pr = ((o["p1"]["json"] or {}).get("room")) or {}
    return (res(o["e1"], 1, "", "error: booking 1 would break: outside opening hours 09:00-17:00\n")
            and res(o["e2"], 1, "", "error: booking 1 would break: outside opening hours 09:00-17:00\n")
            and res(o["e3"], 0, "updated room kyoto\n")
            and res(o["e4"], 1, "", "error: booking 2 would break: outside opening hours 08:00-12:30\n")
            and res(o["e5"], 2, "", "error: unknown zone 'Mars'\n")
            and sub(o["rooms"]["kyoto"], {"tz": "ICT", "opens": "08:00", "closes": "17:00"})
            and o["all"] == [[1, "2026-03-02 10:00", "JST"], [2, "2026-03-02 05:00", "UTC"]]
            and st(o["p1"], 200) and pr.get("tz") == "NPT" and list(pr)[-1] == "tz"
            and st(o["p2"], 400, {"error": "unknown zone 'Mars'"}))


@item("reminders/zones", r'''
seed("s", ROWS)
OUT["jst"] = cli("--store", "s", "reminders", "2026-03-03", "--tz", "JST", "--before", "45", "--json")
OUT["utc"] = cli("--store", "s", "reminders", "2026-03-03", "--before", "30")
OUT["api"] = api("s", "GET", "/reports/reminders?date=2026-03-02&tz=JST&room=berlin")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    rm = lambda at, i, owner, title, room, starts: {"at": at, "id": i, "owner": owner, "title": title, "room": room,
                                                    "starts": starts}
    return (o["jst"]["json"] == {"date": "2026-03-03", "reminders": [
                rm("2026-03-03 07:45", 1, "ana", "Late", "atlas", "08:30"),
                rm("2026-03-03 08:15", 2, "ken", "Early", "kyoto", "09:00"),
                rm("2026-03-03 10:45", 3, "raj", "Chai", "pune", "11:30")]}
            and res(o["utc"], 0, "2026-03-02 23:30 ken: Early in kyoto at 00:00\n"
                                 "2026-03-03 02:00 raj: Chai in pune at 02:30\n")
            and st(o["api"], 200, {"report": "reminders", "rows": [
                rm("2026-03-02 15:45", 4, "eva", "Kickoff", "berlin", "16:00"),
                rm("2026-03-02 23:15", 5, "eva", "Wrap", "berlin", "23:30")]}))


@item("stats/zones", r'''
seed("s", ROWS)
OUT["utc"] = cli("--store", "s", "stats", "--json")["json"]
OUT["jst"] = cli("--store", "s", "stats", "--tz", "JST", "--json")["json"]
OUT["api"] = api("s", "GET", "/stats?tz=PST")
'''.replace("ROWS", repr(AGENDA_ROWS)))
def _(o):
    base = {"rooms": 4, "bookings": 5, "waitlist": 0, "busiest_room": "berlin"}
    return (o["utc"] == dict(base, first_start="2026-03-02 07:00", last_end="2026-03-03 03:30")
            and o["jst"] == dict(base, first_start="2026-03-02 16:00", last_end="2026-03-03 12:30")
            and st(o["api"], 200, dict(base, first_start="2026-03-01 23:00", last_end="2026-03-02 19:30")))


@item("api/legacy_v1_tz", EXPORT_SETUP + r'''
OUT["all"] = api("s", "GET", "/v1/bookings")
OUT["kyoto"] = api("s", "GET", "/v1/bookings?where=kyoto")
''')
def _(o):
    i1 = {"ref": 1, "where": "pune", "when": "2026-03-02 09:15", "until": "2026-03-02 10:00", "what": "Stand, up",
          "who": "raj", "tz": "+05:45"}
    i2 = {"ref": 2, "where": "kyoto", "when": "2026-03-02 10:00", "until": "2026-03-02 11:00", "what": "Review",
          "who": "ken", "tz": "JST"}
    items = (o["all"]["json"] or {}).get("items") or []
    return (st(o["all"], 200, {"items": [i1, i2]}) and all(list(x)[-1] == "tz" for x in items)
            and st(o["kyoto"], 200, {"items": [i2]}))


@item("rooms/import_tz", r'''
write("r.csv", "id,name,capacity,tz,opens\nlima,Lima,3,-05:00,07:00\nsyd,Sydney,4,aest,\nrome,Rome,2,,\n")
OUT["ok"] = cli("--store", "s", "import-rooms", "r.csv", "--json")
OUT["rooms"] = rooms("s")
write("bad.csv", "id,name,capacity,tz\noslo,Oslo,3,Mars\n")
OUT["bad"] = cli("--store", "s", "import-rooms", "bad.csv")
OUT["api"] = api("s", "POST", "/import-rooms", "id,name,capacity,tz\nkiev,Kyiv,5,EET\n")
OUT["kiev"] = api("s", "GET", "/rooms/kiev")
''')
def _(o):
    r = o["rooms"]
    return (o["ok"]["json"] == {"added": ["lima", "syd", "rome"], "skipped": []}
            and sub(r.get("lima"), {"tz": "-05:00", "opens": "07:00"}) and sub(r.get("syd"), {"tz": "AEST"})
            and sub(r.get("rome"), {"tz": "UTC"})
            and res(o["bad"], 1, "", "error: line 2: unknown zone 'Mars'\n")
            and st(o["api"], 200, {"added": ["kiev"], "skipped": []})
            and ((o["kiev"]["json"] or {}).get("room") or {}).get("tz") == "EET")


# generated families (hidden/families.json) ---------------------------------------------------------------------
FAM = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "families.json"), encoding="utf-8"))
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
ZOFF = {"UTC": 0, "CET": 60, "EET": 120, "IST": 330, "NPT": 345, "JST": 540, "BRT": -180, "PST": -480, "EST": -300,
        "AEST": 600, "ICT": 420}


def zoff(label):
    if label in ZOFF:
        return ZOFF[label]
    sign = -1 if label[0] == "-" else 1
    return sign * (int(label[1:3]) * 60 + int(label[4:6]))


def fdate(iso, fmt):
    y, m, d = iso.split("-")
    if fmt == "DD-Mon-YYYY":
        return "%s-%s-%s" % (d, MONTHS[int(m) - 1], y)
    return fmt.replace("YYYY", y).replace("MM", m).replace("DD", d)


def ftime(hhmm, fmt):
    h, m = hhmm.split(":")
    if fmt == "h:MMam":
        hi = int(h)
        return "%d:%s%s" % (hi % 12 or 12, m, "am" if hi < 12 else "pm")
    return fmt.replace("HH", h).replace("MM", m)


def feed_cols(f):
    n = f["names"]
    keys = ["room"] + (["start", "end"] if f["span"] == "datetimes" else
                       ["date", "start", "end" if f["span"] == "from-until" else "minutes"])
    keys += ["title", "owner", "attendees"] + (["zone"] if "zone" in n else [])
    return [n[k] for k in keys]


def feed_text(f, rows):
    """The feed text of `rows` ({room, date, start, end, minutes, title, owner, attendees, zone}) in dialect f."""
    n, recs = f["names"], []
    for r in rows:
        v = {}
        d, t0, t1 = fdate(r["date"], f["date_fmt"]), ftime(r["start"], f["time_fmt"]), ftime(r["end"], f["time_fmt"])
        suf = (" " + r["zone"]) if f["zone"] == "suffix" and r["zone"] else ""
        if f["span"] == "datetimes":
            v[n["start"]], v[n["end"]] = d + " " + t0 + suf, d + " " + t1
        else:
            v[n["date"]], v[n["start"]] = d, t0 + suf
            if f["span"] == "from-until":
                v[n["end"]] = t1
            else:
                v[n["minutes"]] = str(r["minutes"])
        v[n["room"]], v[n["title"]], v[n["owner"]], v[n["attendees"]] = r["room"], r["title"], r["owner"], r["attendees"]
        if "zone" in n:
            v[n["zone"]] = r["zone"]
        recs.append(v)
    cols = feed_cols(f)
    if f["notation"] == "delimited":
        return "\n".join([f["delim"].join(cols)] + [f["delim"].join(v[c] for c in cols) for v in recs]) + "\n"
    if f["notation"] == "keyvalue":
        return "\n\n".join("\n".join("%s%s%s" % (c, f["sep"], v[c]) for c in cols) for v in recs) + "\n"
    return "".join(json.dumps({c: v[c] for c in cols}) + "\n" for v in recs)


def feed_line(f, i):
    """The place (line number) of row i (from 0) in feed_text."""
    if f["notation"] == "delimited":
        return i + 2
    if f["notation"] == "keyvalue":
        return 1 + i * (len(feed_cols(f)) + 1)
    return i + 1


FEED_ROWS = [
    {"room": "kyoto", "date": "2026-03-02", "start": "01:00", "end": "02:00", "minutes": 60, "title": "Sync",
     "owner": "ana", "attendees": "2"},
    {"room": "pune", "date": "2026-03-02", "start": "10:00", "end": "11:30", "minutes": 90, "title": "Review",
     "owner": "bob", "attendees": ""},
    {"room": "berlin", "date": "2026-03-02", "start": "05:00", "end": "06:00", "minutes": 60, "title": "Remote",
     "owner": "cyd", "attendees": "3"},
    {"room": "atlas", "date": "2026-03-02", "start": "16:00", "end": "17:00", "minutes": 60, "title": "Late",
     "owner": "dan", "attendees": ""},
]
FEED_ZONES = {"label": (["utc", "+05:30", "BRT", "npt"], ["UTC", "+05:30", "BRT", "NPT"]),
              "compact": (["+0000", "+0530", "-0300", "+0545"], ["+00:00", "+05:30", "-03:00", "+05:45"]),
              "minutes": (["0", "330", "-180", "345"], ["+00:00", "+05:30", "-03:00", "+05:45"]),
              "suffix": (["UTC", "IST", "-03:00", "+05:45"], ["UTC", "IST", "-03:00", "+05:45"])}
FEED_BAD = {"label": "Mars", "compact": "+0520", "minutes": "320", "suffix": "Mars"}


def feed_case(zone_style):
    feeds = [f for f in FAM["feeds"] if f["zone"] == zone_style]
    texts, written = {}, FEED_ZONES[zone_style][0]
    for f in feeds:
        texts[f["slug"]] = feed_text(f, [dict(r, zone=z) for r, z in zip(FEED_ROWS, written)])
    snippet = r'''
for slug, text in TEXTS.items():
    seed("s_" + slug)
    write(slug + ".txt", text)
    r = cli("--store", "s_" + slug, "import", "feed_" + slug, slug + ".txt", "--json")
    OUT[slug] = [r["code"], r["json"], r["err"], bookings("s_" + slug)]
'''.replace("TEXTS", repr(texts))
    labels = FEED_ZONES[zone_style][1]
    want = [B(i + 1, r["room"], r["date"] + " " + r["start"], r["date"] + " " + r["end"], r["title"], r["owner"],
              int(r["attendees"] or 1), labels[i]) for i, r in enumerate(FEED_ROWS)]

    def pred(o):
        return all(o[f["slug"]][:2] == [0, {"added": [1, 2, 3, 4], "skipped": []}] and o[f["slug"]][3] == want
                   for f in feeds)
    return snippet, pred


for _style in ("label", "compact", "minutes", "suffix"):
    _snip, _pred = feed_case(_style)
    item("feeds/%s_zones" % _style, _snip)(_pred)


def _bad_feeds():
    picks = []
    for style in ("label", "compact", "minutes", "suffix"):
        for nota in ("jsonl", "keyvalue", "delimited"):
            cands = [f for f in FAM["feeds"] if f["zone"] == style and f["notation"] == nota and f not in picks]
            if cands:
                picks.append(cands[0])
                break
    return picks


BAD_FEEDS = _bad_feeds()
_miss_row = {"room": "pune", "date": "2026-03-03", "start": "09:00", "end": "10:00", "minutes": 60, "title": "Plain",
             "owner": "eve", "attendees": "", "zone": ""}
_texts = {}
for _f in BAD_FEEDS:
    _ok = dict(FEED_ROWS[1], zone=FEED_ZONES[_f["zone"]][0][1])
    _bad = dict(FEED_ROWS[3], zone=FEED_BAD[_f["zone"]])
    _texts[_f["slug"]] = [feed_text(_f, [_miss_row]), feed_text(_f, [_ok, _bad])]


@item("feeds/missing_and_bad_zones", r'''
for slug, (plain, bad) in TEXTS.items():
    seed("s_" + slug)
    write(slug + "_a.txt", plain)
    write(slug + "_b.txt", bad)
    a = cli("--store", "s_" + slug, "import", "feed_" + slug, slug + "_a.txt")
    b = cli("--store", "s_" + slug, "import", "feed_" + slug, slug + "_b.txt")
    OUT[slug] = [a["code"], b["code"], b["err"], bookings("s_" + slug)]
'''.replace("TEXTS", repr(_texts)))
def _(o):
    for f in BAD_FEEDS:
        code_a, code_b, err, rows = o[f["slug"]]
        want_err = "error: line %d: unknown zone '%s'\n" % (feed_line(f, 1), FEED_BAD[f["zone"]])
        if not (code_a == 0 and code_b == 1 and err == want_err
                and rows == [B(1, "pune", "2026-03-03 09:00", "2026-03-03 10:00", "Plain", "eve", 1, "IST")]):
            return False
    return True


CAL_ROWS = ["1,pune,2026-03-02 09:15,2026-03-02 10:00,Standup,raj,1,+05:45",
            "2,kyoto,2026-03-02 10:00,2026-03-02 11:00,Review,ken,3,JST",
            "3,atlas,2026-03-03 23:30,2026-03-04 00:15,Night,ana,2,UTC"]
CAL_BOOKINGS = [(1, "pune", "2026-03-02 09:15", "2026-03-02 10:00", "Standup", "raj", 1, "+05:45"),
                (2, "kyoto", "2026-03-02 10:00", "2026-03-02 11:00", "Review", "ken", 3, "JST"),
                (3, "atlas", "2026-03-03 23:30", "2026-03-04 00:15", "Night", "ana", 2, "UTC")]


def cstamp(text, fmt):
    d, t = text.split(" ")
    y, m, dd = d.split("-")
    hh, mm = t.split(":")
    return {"YYYY-MM-DDTHH:MM": "%s-%s-%sT%s:%s", "YYYYMMDDTHHMM": "%s%s%sT%s%s"}.get(fmt, "") % (y, m, dd, hh, mm) \
        if fmt in ("YYYY-MM-DDTHH:MM", "YYYYMMDDTHHMM") else {
            "DD.MM.YYYY HH:MM": "%s.%s.%s %s:%s" % (dd, m, y, hh, mm),
            "MM/DD/YYYY HH:MM": "%s/%s/%s %s:%s" % (m, dd, y, hh, mm),
            "YYYY-MM-DD HH:MM": text}[fmt]


def cal_values(c, bk):
    i, room, s, e, title, owner, att, tz = bk
    val = {"id": i, "room": room, "start": cstamp(s, c["dt_fmt"]), "end": cstamp(e, c["dt_fmt"]), "title": title,
           "owner": owner, "attendees": att}
    return [(name, val[kind]) for name, kind in c["fields"]] + [("tz", tz)]


def cal_expected(c):
    rows = [cal_values(c, bk) for bk in CAL_BOOKINGS]
    if c["notation"] == "delimited":
        head = c["delim"].join(name for name, _ in rows[0])
        return "\n".join([head] + [c["delim"].join(str(v) for _, v in r) for r in rows]) + "\n"
    if c["notation"] == "keyvalue":
        return "\n\n".join("\n".join("%s%s%s" % (k, c["sep"], v) for k, v in r) for r in rows) + "\n"
    return [[[k, v] for k, v in r] for r in rows]


def cal_case(notation):
    cals = [c for c in FAM["calendars"] if c["notation"] == notation]
    snippet = r'''
seed("s", ROWS)
for slug in SLUGS:
    r = cli("--store", "s", "export", "cal_" + slug, slug + ".out")
    text = read(slug + ".out")
    if JSONL and text is not None:
        try:
            from collections import OrderedDict
            text = [[[k, v] for k, v in json.loads(l, object_pairs_hook=OrderedDict).items()] for l in text.split("\n") if l]
        except ValueError:
            text = None
    OUT[slug] = [r["code"], text]
'''.replace("ROWS", repr(CAL_ROWS)).replace("SLUGS", repr([c["slug"] for c in cals])).replace(
        "JSONL", repr(notation == "jsonl"))

    def pred(o):
        return all(o[c["slug"]] == [0, cal_expected(c)] for c in cals)
    return snippet, pred


for _nota in ("delimited", "keyvalue", "jsonl"):
    _snip, _pred = cal_case(_nota)
    item("calendars/%s_tz" % _nota, _snip)(_pred)


GROUP_ROWS = [
    "1,atlas,2026-03-02 14:00,2026-03-02 15:00,A,ana,2,UTC",
    "2,atlas,2026-03-02 15:30,2026-03-02 16:00,B,bob,3,UTC",
    "3,kyoto,2026-03-03 09:00,2026-03-03 11:00,C,ana,4,JST",
    "4,pune,2026-03-02 22:00,2026-03-02 23:30,D,cyd,1,IST",
    "5,berlin,2026-03-02 08:00,2026-03-02 08:45,E,dan,5,CET",
    "6,berlin,2026-03-08 23:30,2026-03-09 00:30,F,eve,2,UTC",
    "7,kyoto,2026-03-31 20:00,2026-03-31 21:00,G,fay,1,UTC",
]
WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]


def _bk_rows(rows):
    out = []
    for r in rows:
        i, room, s, e, title, owner, att, tz = r.split(",")
        out.append((int(i), room, datetime.strptime(s, "%Y-%m-%d %H:%M"), datetime.strptime(e, "%Y-%m-%d %H:%M"),
                    title, owner, int(att), tz))
    return out


def group_rows(g, rows, zone="UTC", first=None, last=None, room=None):
    groups = {}
    for i, rm, s, e, title, owner, att, tz in _bk_rows(rows):
        start = s - timedelta(minutes=zoff(tz)) + timedelta(minutes=zoff(zone))
        if room and rm != room:
            continue
        if (first and start.date() < first) or (last and start.date() > last):
            continue
        key = {"weekday": WEEKDAYS[start.weekday()], "hour": "%02d" % start.hour,
               "week": "%04d-W%02d" % start.isocalendar()[:2],
               "daypart": "morning" if start.hour < 12 else "afternoon" if start.hour < 17 else "evening",
               "month": "%04d-%02d" % (start.year, start.month),
               "dateroom": "%s %s" % (start.strftime("%Y-%m-%d"), rm)}[g["group"]]
        groups.setdefault(key, []).append((int((e - s).total_seconds() // 60), att, owner))
    def val(items):
        return {"count": len(items), "minutes": sum(x[0] for x in items), "attendees": sum(x[1] for x in items),
                "owners": len({x[2] for x in items})}[g["metric"]]
    order = {"weekday": lambda k: WEEKDAYS.index(k), "daypart": lambda k: ["morning", "afternoon", "evening"].index(k)}
    keys = sorted(groups, key=order.get(g["group"], lambda k: k))
    return [{"group": k, "value": val(groups[k])} for k in keys]


@item("grouped/display_zone_cli", r'''
seed("s", ROWS)
for slug in SLUGS:
    r = cli("--store", "s", "report", "by_" + slug, "--tz", "JST", "--json")
    OUT[slug] = r["json"]["rows"] if r["code"] == 0 and r["json"] else [r["code"], r["err"]]
'''.replace("ROWS", repr(GROUP_ROWS)).replace("SLUGS", repr([g["slug"] for g in FAM["grouped"]])))
def _(o):
    return all(o[g["slug"]] == group_rows(g, GROUP_ROWS, "JST") for g in FAM["grouped"])


@item("grouped/range_and_room_api", r'''
seed("s", ROWS)
for slug in SLUGS:
    a = api("s", "GET", "/reports/by_" + slug + "?tz=PST&date_from=2026-03-02&date_to=2026-03-08")
    b = api("s", "GET", "/reports/by_" + slug + "?room=kyoto&date_from=2026-03-03")
    OUT[slug] = [a["status"], (a["json"] or {}).get("rows"), b["status"], (b["json"] or {}).get("rows")]
'''.replace("ROWS", repr(GROUP_ROWS)).replace("SLUGS", repr([g["slug"] for g in FAM["grouped"]])))
def _(o):
    d2, d3, d8 = date(2026, 3, 2), date(2026, 3, 3), date(2026, 3, 8)
    return all(o[g["slug"]] == [200, group_rows(g, GROUP_ROWS, "PST", d2, d8), 200,
                                 group_rows(g, GROUP_ROWS, "UTC", d3, None, "kyoto")] for g in FAM["grouped"])


def clock(t, fmt):
    return {"HH:MM": "%02d:%02d" % (t.hour, t.minute), "HH.MM": "%02d.%02d" % (t.hour, t.minute),
            "HHhMM": "%02dh%02d" % (t.hour, t.minute),
            "h:MMam": "%d:%02d%s" % (t.hour % 12 or 12, t.minute, "am" if t.hour < 12 else "pm")}[fmt]


def notify_text(nt, rows, day, zone="UTC", room=None):
    picked = []
    for i, rm, s, e, title, owner, att, tz in _bk_rows(rows):
        utc = s - timedelta(minutes=zoff(tz))
        start = utc + timedelta(minutes=zoff(zone))
        if start.date() == day and (not room or rm == room):
            picked.append(((utc, i), nt["line"].format(time=clock(start, nt["time_fmt"]), title=title, room=rm,
                                                       owner=owner, attendees=att)))
    lines = [nt["header"].format(date=day.strftime("%Y-%m-%d"))] if nt["header"] else []
    lines += [x for _, x in sorted(picked)] or [nt["empty"]]
    return "\n".join(lines) + "\n"


NOTIFY_ROWS = [r.replace(",Late,", ",Late night,") for r in AGENDA_ROWS]


@item("notify/cli_display_zone", r'''
seed("s", ROWS)
for slug in SLUGS:
    a = cli("--store", "s", "notify", slug, "2026-03-03", "--tz", "JST")
    b = cli("--store", "s", "notify", slug, "2026-03-01", "--tz", "PST")
    OUT[slug] = [a["code"], a["out"], b["code"], b["out"]]
'''.replace("ROWS", repr(NOTIFY_ROWS)).replace("SLUGS", repr([nt["slug"] for nt in FAM["notifiers"]])))
def _(o):
    return all(o[nt["slug"]] == [0, notify_text(nt, NOTIFY_ROWS, date(2026, 3, 3), "JST"), 0,
                                  notify_text(nt, NOTIFY_ROWS, date(2026, 3, 1), "PST")] for nt in FAM["notifiers"])


@item("notify/api_default_and_room", r'''
seed("s", ROWS)
for slug in SLUGS:
    a = api("s", "GET", "/notify/" + slug + "?date=2026-03-02")
    b = api("s", "GET", "/notify/" + slug + "?date=2026-03-02&tz=AEST&room=berlin")
    OUT[slug] = [a["status"], (a["json"] or {}).get("text"), b["status"], (b["json"] or {}).get("text")]
'''.replace("ROWS", repr(NOTIFY_ROWS)).replace("SLUGS", repr([nt["slug"] for nt in FAM["notifiers"]])))
def _(o):
    return all(o[nt["slug"]] == [200, notify_text(nt, NOTIFY_ROWS, date(2026, 3, 2)), 200,
                                  notify_text(nt, NOTIFY_ROWS, date(2026, 3, 2), "AEST", "berlin")]
               for nt in FAM["notifiers"])


_REG_FEEDS = [[f for f in FAM["feeds"] if f["notation"] == nota][0] for nota in ("delimited", "keyvalue", "jsonl")]
_reg_rows = [{"room": "atlas", "date": "2026-03-02", "start": "09:00", "end": "10:00", "minutes": 60, "title": "One",
              "owner": "ana", "attendees": "2", "zone": ""},
             {"room": "atlas", "date": "2026-03-02", "start": "13:30", "end": "15:00", "minutes": 90, "title": "Two",
              "owner": "bob", "attendees": "", "zone": ""}]
_reg_texts = {f["slug"]: feed_text(f, _reg_rows) for f in _REG_FEEDS}
_REG_STORE = ["1,atlas,2026-03-02 09:00,2026-03-02 10:00,One,ana,2", "2,atlas,2026-03-03 17:30,2026-03-03 18:00,Two,bob,1"]


@item("reg/families_utc", r'''
for slug, text in TEXTS.items():
    write("s_" + slug + "/rooms.csv", "#roomkit-schema: 2\nid,name,capacity,opens,closes,tags\natlas,Atlas,8,08:00,18:00,\n")
    write(slug + ".txt", text)
    r = cli("--store", "s_" + slug, "import", "feed_" + slug, slug + ".txt", "--json")
    OUT[slug] = [r["code"], r["json"], [(b["start"], b["end"], b["attendees"]) for b in bookings("s_" + slug)]]
write("g/rooms.csv", "#roomkit-schema: 2\nid,name,capacity,opens,closes,tags\natlas,Atlas,8,08:00,18:00,\n")
write("g/bookings.csv", "#roomkit-schema: 2\nid,room,start,end,title,owner,attendees\n" + "\n".join(STORE) + "\n")
for slug in GSLUGS:
    r = cli("--store", "g", "report", "by_" + slug, "--json")
    OUT["by_" + slug] = r["json"]["rows"] if r["code"] == 0 and r["json"] else r["err"]
for slug in NSLUGS:
    OUT["n_" + slug] = cli("--store", "g", "notify", slug, "2026-03-03")["out"]
'''.replace("TEXTS", repr(_reg_texts)).replace("STORE", repr(_REG_STORE)).replace(
    "GSLUGS", repr([g["slug"] for g in FAM["grouped"][:4]])).replace(
    "NSLUGS", repr([nt["slug"] for nt in FAM["notifiers"][:4]])))
def _(o):
    rows = [r + ",UTC" for r in _REG_STORE]
    return (all(o[f["slug"]] == [0, {"added": [1, 2], "skipped": []},
                                  [["2026-03-02 09:00", "2026-03-02 10:00", 2], ["2026-03-02 13:30", "2026-03-02 15:00", 1]]]
                for f in _REG_FEEDS)
            and all(o["by_" + g["slug"]] == group_rows(g, rows) for g in FAM["grouped"][:4])
            and all(o["n_" + nt["slug"]] == notify_text(nt, rows, date(2026, 3, 3)) for nt in FAM["notifiers"][:4]))


def main():
    results = {}
    lock = threading.Lock()
    queue = list(ITEMS)

    def worker():
        while True:
            with lock:
                if not queue:
                    return
                name, snippet, pred = queue.pop(0)
            out = run_scenario(name, snippet)
            ok = False
            if isinstance(out, dict) and "__error__" not in out:
                try:
                    ok = bool(pred(out))
                except Exception:
                    ok = False
            with lock:
                results[name] = ok

    threads = [threading.Thread(target=worker) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    for name, _, _ in ITEMS:
        print("%s %s" % ("PASS" if results.get(name) else "FAIL", name))


main()
