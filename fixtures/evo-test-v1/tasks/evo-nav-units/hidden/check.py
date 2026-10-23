"""evo-nav-units check (cwd = the workspace). One scenario per named item: a Python snippet that drives the
workspace's stockroom package (CLI main(), the request handler, the stores) in a fresh process (cwd = a scratch
dir of its own, PYTHONPATH = the workspace) and records what it observed in OUT. This process compares OUT with
the expected values, which never reach workspace code. Prints one PASS/FAIL line per item."""
import json
import os
import subprocess
import sys
import threading
from decimal import Decimal

WS = os.getcwd()
SCRATCH = os.environ["S1_SCRATCH"]
DRIVERS = os.path.join(SCRATCH, "_drivers")

HARNESS = r'''
import contextlib, io, json, os, sys, traceback
OUT = {}

def cli(*argv):
    from stockroom.cli.main import main
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
    from stockroom.api.app import App
    from stockroom.api.http import Request
    from stockroom.storage.factory import open_store
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

def items(store):
    from stockroom.storage.factory import open_store
    inv = open_store(store).load()
    return {i.sku: {"name": i.name, "category": i.category, "quantity": str(i.quantity),
                    "reorder_level": str(i.reorder_level), "location": i.location, "unit_cost": str(i.unit_cost),
                    "unit": getattr(i, "unit", None)} for i in inv.items.values()}

def moves(store):
    from stockroom.storage.factory import open_store
    inv = open_store(store).load()
    return [{"id": m.id, "sku": m.sku, "delta": str(m.delta), "reason": m.reason, "note": m.note,
             "unit": getattr(m, "unit", None)} for m in inv.movements]

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


def D(x):
    """Decimal from a str/int/float (floats through repr), None for anything else."""
    if isinstance(x, bool) or x is None:
        return None
    try:
        return Decimal(repr(x)) if isinstance(x, float) else Decimal(str(x))
    except Exception:
        return None


def deq(a, b):
    da, db = D(a), D(b)
    return da is not None and db is not None and da == db


ITEMS = []


def item(name, snippet):
    def deco(fn):
        ITEMS.append((name, snippet, fn))
        return fn
    return deco


# ------------------------------------------------------------------------------------------------- fixtures
import textwrap  # noqa: E402

V1_ITEMS = ("#stockroom-items v1\nsku,name,category,quantity,reorder_level,location,unit_cost\n"
            "NUT-0001,Hex nut,fasteners,250,100,A1,0.05\nOIL-0002,Gear oil,fluids,3.5,1,B2,7.90\n")
V1_MOVES = ("#stockroom-movements v1\nid,sku,delta,reason,note\n"
            "1,NUT-0001,250,receive,initial stock\n2,OIL-0002,3.5,receive,initial stock\n")
S_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
BOL-0001,Hex bolt,fasteners,120,ea,50,A1,0.12
CHK-0002,Chickpeas,food,2.5,kg,5,C3,3.20
LNT-0007,Lentils,food,1.25,kg,2,C3,2.80
MLK-0003,Oat milk,food,6,l,8,C1,1.95
PEP-0004,Peppercorn,food,250,g,100,C3,0.04
RIB-0005,Ribbon,packing,1.5,dz,1,,4.80
SLT-0006,Sea salt,food,3,lb,1,C3,2.00
"""
S_MOVES = """#stockroom-movements v2
id,sku,delta,unit,reason,note
1,BOL-0001,120,ea,receive,initial stock
2,CHK-0002,3,kg,receive,initial stock
3,CHK-0002,-500,g,issue,
4,PEP-0004,0.3,kg,receive,
5,PEP-0004,-50,g,issue,
6,SLT-0006,3,lb,receive,
7,LNT-0007,1.25,kg,receive,
"""
S_SETUP = 'write("st/items.csv", S_ITEMS)\nwrite("st/movements.csv", S_MOVES)\n'
ITEM_KEYS = ["sku", "name", "category", "quantity", "unit", "reorder_level", "location", "unit_cost"]


def scen(code, **fixtures):
    head = "".join("%s = %r\n" % (k, v) for k, v in sorted(fixtures.items()))
    return head + textwrap.dedent(code)


def S(code):
    return scen(S_SETUP + textwrap.dedent(code), S_ITEMS=S_ITEMS, S_MOVES=S_MOVES)


def units_of(items):
    return {k: v.get("unit") for k, v in items.items()}


def rows_eq(rows, expected):
    """rows (list of dicts) equal expected (list of dicts) with numeric values compared as decimals."""
    if not isinstance(rows, list) or len(rows) != len(expected):
        return False
    for r, e in zip(rows, expected):
        if not isinstance(r, dict) or set(r) != set(e):
            return False
        for k, v in e.items():
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                if not deq(r[k], v) or isinstance(r[k], (str, bool)):
                    return False
            elif r[k] != v:
                return False
    return True


def errlines(text):
    return [line for line in (text or "").splitlines() if line.strip()]


# ------------------------------------------------------------------------------------------------- units

@item("units.convert", scen("""
    from decimal import Decimal
    from stockroom import units
    cases = [("1.5", "kg", "g"), ("1", "lb", "kg"), ("10", "oz", "g"), ("3", "dz", "ea"), ("2.5", "l", "ml"),
             ("250", "g", "lb"), ("1", "kg", "lb"), ("0.75", "kg", "kg"), ("100", "ml", "l"), ("5", "lb", "oz"),
             ("18", "ea", "dz")]
    OUT["c"] = [str(units.convert(Decimal(q), f, t)) for q, f, t in cases]
    OUT["n"] = [units.normalize_unit(x) for x in [" KG ", "Lb", None, "", "  ", "oz", "DZ"]]
    OUT["tbl"] = sorted(units.UNITS)
"""))
def _(o):
    want = ["1500", "0.454", "283.495", "36", "2500", "0.551", "2.205", "0.75", "0.1", "80", "1.5"]
    return (all(deq(a, b) for a, b in zip(o["c"], want)) and len(o["c"]) == len(want)
            and o["n"] == ["kg", "lb", "ea", "ea", "ea", "oz", "dz"]
            and o["tbl"] == ["dz", "ea", "g", "kg", "l", "lb", "ml", "oz"])


@item("units.errors", scen("""
    from decimal import Decimal
    from stockroom import errors, units
    def catch(fn):
        try:
            fn()
            return None
        except Exception as e:
            return [type(e).__name__, str(e), isinstance(e, errors.StockroomError)]
    OUT["a"] = catch(lambda: units.normalize_unit(" Stone "))
    OUT["b"] = catch(lambda: units.convert(Decimal(1), "kg", "ea"))
    OUT["c"] = catch(lambda: units.convert(Decimal(2), "l", "g"))
    OUT["sub"] = issubclass(errors.UnitError, errors.StockroomError)
"""))
def _(o):
    return (o["a"] == ["UnitError", "unknown unit 'Stone'", True]
            and o["b"] == ["UnitError", "cannot convert kg to ea", True]
            and o["c"] == ["UnitError", "cannot convert l to g", True] and o["sub"] is True)


# ------------------------------------------------------------------------------------------------- stores

@item("store.csv_legacy", scen("""
    write("st/items.csv", V1_ITEMS)
    write("st/movements.csv", V1_MOVES)
    OUT["items"] = items("st")
    OUT["moves"] = moves("st")
    OUT["show"] = cli("--store", "st", "show", "OIL-0002")["json"]
    OUT["hist"] = cli("--store", "st", "history", "NUT-0001", "--json")["json"]
""", V1_ITEMS=V1_ITEMS, V1_MOVES=V1_MOVES))
def _(o):
    it, mv = o["items"], o["moves"]
    return (units_of(it) == {"NUT-0001": "ea", "OIL-0002": "ea"} and deq(it["OIL-0002"]["quantity"], "3.5")
            and deq(it["NUT-0001"]["reorder_level"], 100) and it["OIL-0002"]["location"] == "B2"
            and [m["unit"] for m in mv] == ["ea", "ea"]
            and o["show"]["unit"] == "ea" and deq(o["show"]["quantity"], "3.5")
            and [m.get("unit") for m in o["hist"]] == ["ea"])


@item("store.csv_save_v2", scen("""
    write("st/items.csv", V1_ITEMS)
    write("st/movements.csv", V1_MOVES)
    write("st/kits.csv", "#stockroom-kits v1\\nkit,component,quantity\\nOIL-0002,NUT-0001,4\\n")
    write("st/packs.csv", "#stockroom-packs v1\\nsku,supplier,quantity,price\\nNUT-0001,Fastco,100,4.50\\n")
    OUT["code"] = cli("--store", "st", "receive", "OIL-0002", "1.5")["code"]
    OUT["items_csv"] = read("st/items.csv")
    OUT["moves_csv"] = read("st/movements.csv")
    OUT["kits_csv"] = read("st/kits.csv")
    OUT["packs_csv"] = read("st/packs.csv")
    OUT["init"] = cli("--store", "fresh", "init")["code"]
    OUT["fresh_items"] = read("fresh/items.csv")
    OUT["fresh_moves"] = read("fresh/movements.csv")
""", V1_ITEMS=V1_ITEMS, V1_MOVES=V1_MOVES))
def _(o):
    return (o["code"] == 0 and o["items_csv"] == (
        "#stockroom-items v2\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
        "NUT-0001,Hex nut,fasteners,250,ea,100,A1,0.05\nOIL-0002,Gear oil,fluids,5,ea,1,B2,7.90\n")
        and o["moves_csv"] == (
        "#stockroom-movements v2\nid,sku,delta,unit,reason,note\n1,NUT-0001,250,ea,receive,initial stock\n"
        "2,OIL-0002,3.5,ea,receive,initial stock\n3,OIL-0002,1.5,ea,receive,\n")
        and o["kits_csv"] == "#stockroom-kits v2\nkit,component,quantity,unit\nOIL-0002,NUT-0001,4,ea\n"
        and o["packs_csv"] == "#stockroom-packs v2\nsku,supplier,quantity,unit,price\nNUT-0001,Fastco,100,ea,4.50\n"
        and o["init"] == 0
        and o["fresh_items"] == "#stockroom-items v2\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
        and o["fresh_moves"] == "#stockroom-movements v2\nid,sku,delta,unit,reason,note\n")


@item("store.csv_versions", scen('''
    write("a/items.csv", "#stockroom-items v2\\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\\n"
          "FLR-0001,Flour,baking,12.5,KG,5,C1,0.90\\nMLK-0002,Milk,dairy,6,,2,C2,1.10\\nSUG-0003,Sugar,baking,2, Lb ,1,C1,1.50\\n")
    write("a/movements.csv", "#stockroom-movements v1\\nid,sku,delta,reason,note\\n1,MLK-0002,6,receive,initial stock\\n")
    OUT["a"] = items("a")
    OUT["am"] = moves("a")
    write("b/items.csv", "#stockroom-items v3\\nsku,name,category,quantity,unit,reorder_level,location,unit_cost,density\\n")
    r = cli("--store", "b", "list")
    OUT["b"] = [r["code"], r["err"]]
    write("c/items.csv", V1_ITEMS)
    write("c/movements.csv", "#stockroom-movements v2\\nid,sku,delta,unit,reason,note\\n1,NUT-0001,2,DZ,receive,\\n")
    OUT["c"] = [moves("c"), {k: v["unit"] for k, v in items("c").items()}]
    r = cli("--store", "c", "info", "--json")
    OUT["info"] = r["json"]
''', V1_ITEMS=V1_ITEMS))
def _(o):
    a, am = o["a"], o["am"]
    info = {f["file"]: (f["version"], f["current"]) for f in o["info"]["files"]}
    return (units_of(a) == {"FLR-0001": "kg", "MLK-0002": "ea", "SUG-0003": "lb"} and deq(a["FLR-0001"]["quantity"], "12.5")
            and [m["unit"] for m in am] == ["ea"]
            and o["b"][0] == 2 and "items.csv: unsupported schema version 3" in o["b"][1]
            and [m["unit"] for m in o["c"][0]] == ["dz"] and o["c"][1] == {"NUT-0001": "ea", "OIL-0002": "ea"}
            and info == {"items.csv": (1, 2), "movements.csv": (2, 2)})


@item("store.json_legacy", scen("""
    import json
    write("s1.json", json.dumps({"schema": 1, "items": [
        {"sku": "TEA-0001", "name": "Green tea", "category": "drinks", "quantity": "40", "reorder_level": "10",
         "location": "D4", "unit_cost": "2.50"}],
        "movements": [{"id": 1, "sku": "TEA-0001", "delta": "40", "reason": "receive", "note": ""}],
        "kits": [{"kit": "TEA-0001", "component": "TEA-0001", "quantity": "1"}],
        "packs": [{"sku": "TEA-0001", "supplier": "Leafy", "quantity": "20", "price": "30.00"}]}))
    write("s0.json", json.dumps({"items": {"CUP-0009": {"name": "Paper cup", "category": "supplies",
                                 "quantity": "300", "min": "100", "cost": "0.03"}}, "movements": []}))
    OUT["i1"] = items("s1.json")
    OUT["m1"] = moves("s1.json")
    OUT["i0"] = items("s0.json")
    OUT["code"] = cli("--store", "s1.json", "issue", "TEA-0001", "4")["code"]
    doc = json.loads(read("s1.json"))
    OUT["schema"] = doc.get("schema")
    OUT["item_keys"] = list(doc["items"][0])
    OUT["item"] = doc["items"][0]
    OUT["move_keys"] = [list(m) for m in doc["movements"]]
    OUT["moves"] = doc["movements"]
    OUT["kp"] = [doc.get("kits"), doc.get("packs"), [list(x) for x in doc.get("kits", [])], [list(x) for x in doc.get("packs", [])]]
    OUT["up"] = cli("--store", "s0.json", "upgrade")["code"]
    OUT["s0"] = json.loads(read("s0.json"))
"""))
def _(o):
    mk = ["id", "sku", "delta", "unit", "reason", "note"]
    s0 = o["s0"]
    return (units_of(o["i1"]) == {"TEA-0001": "ea"} and [m["unit"] for m in o["m1"]] == ["ea"]
            and units_of(o["i0"]) == {"CUP-0009": "ea"} and deq(o["i0"]["CUP-0009"]["reorder_level"], 100)
            and o["code"] == 0 and o["schema"] == 2 and o["item_keys"] == ITEM_KEYS
            and o["item"]["unit"] == "ea" and deq(o["item"]["quantity"], 36)
            and o["move_keys"] == [mk, mk]
            and o["moves"][1] == {"id": 2, "sku": "TEA-0001", "delta": "-4", "unit": "ea", "reason": "issue", "note": ""}
            and o["kp"][0] == [{"kit": "TEA-0001", "component": "TEA-0001", "quantity": "1", "unit": "ea"}]
            and o["kp"][1] == [{"sku": "TEA-0001", "supplier": "Leafy", "quantity": "20", "unit": "ea", "price": "30.00"}]
            and o["kp"][2] == [["kit", "component", "quantity", "unit"]] and o["kp"][3] == [["sku", "supplier", "quantity", "unit", "price"]]
            and o["up"] == 0 and s0.get("schema") == 2 and s0["items"][0]["unit"] == "ea"
            and s0["items"][0]["sku"] == "CUP-0009" and deq(s0["items"][0]["reorder_level"], 100))


@item("store.json_versions", scen("""
    import json
    write("s2.json", json.dumps({"schema": 2, "items": [
        {"sku": "RCE-0001", "name": "Rice", "category": "food", "quantity": "20", "unit": " KG",
         "reorder_level": "5", "location": "", "unit_cost": "1.80"},
        {"sku": "EGG-0002", "name": "Eggs", "category": "food", "quantity": "3", "unit": "DZ",
         "reorder_level": "1", "location": "", "unit_cost": "2.40"},
        {"sku": "SPN-0003", "name": "Sponge", "category": "cleaning", "quantity": "14", "reorder_level": "4",
         "location": "B1", "unit_cost": "0.60"},
        {"sku": "VIN-0004", "name": "Vinegar", "category": "food", "quantity": "2", "unit": None,
         "reorder_level": "1", "location": "", "unit_cost": "1.20"}],
        "movements": [{"id": 1, "sku": "RCE-0001", "delta": "20", "unit": "kg", "reason": "receive", "note": ""},
                      {"id": 2, "sku": "EGG-0002", "delta": "3", "reason": "receive", "note": ""}]}))
    OUT["i"] = items("s2.json")
    OUT["m"] = moves("s2.json")
    write("s3.json", json.dumps({"schema": 3, "items": [], "movements": []}))
    r = cli("--store", "s3.json", "list")
    OUT["s3"] = [r["code"], r["err"]]
    OUT["info"] = cli("--store", "s2.json", "info", "--json")["json"]
"""))
def _(o):
    f = o["info"]["files"][0]
    return (units_of(o["i"]) == {"RCE-0001": "kg", "EGG-0002": "dz", "SPN-0003": "ea", "VIN-0004": "ea"}
            and [m["unit"] for m in o["m"]] == ["kg", "ea"]
            and o["s3"][0] == 2 and "inventory.json: unsupported schema version 3" in o["s3"][1]
            and (f["version"], f["current"]) == (2, 2))


# ------------------------------------------------------------------------------------------------- importers

@item("import.csv_unit", scen('''
    cli("--store", "st", "init")
    write("in.csv", "SKU,Name,Category,Qty,Unit,Reorder,Location,Cost\\nFLR-0101,Rye flour,baking,12.5, KG ,5,C1,0.95\\n"
                    "OIL-0102,Olive oil,pantry,3,l,1,C2,8.40\\nBOX-0103,Box,packing,40,,10,D1,0.30\\n")
    r = cli("--store", "st", "import", "in.csv")
    OUT["r"] = [r["code"], r["err"]]
    write("in2.csv", "sku,name,qty,UoM\\nSLT-0104,Salt,750,G\\n")
    OUT["r2"] = cli("--store", "st", "import", "in2.csv")["code"]
    OUT["items"] = items("st")
    OUT["moves"] = moves("st")
'''))
def _(o):
    it = o["items"]
    flr = [m for m in o["moves"] if m["sku"] == "FLR-0101"]
    return (o["r"] == [0, ""] and o["r2"] == 0
            and units_of(it) == {"FLR-0101": "kg", "OIL-0102": "l", "BOX-0103": "ea", "SLT-0104": "g"}
            and deq(it["FLR-0101"]["quantity"], "12.5") and deq(it["SLT-0104"]["quantity"], 750)
            and len(flr) == 1 and flr[0]["unit"] == "kg" and deq(flr[0]["delta"], "12.5") and flr[0]["reason"] == "import")


@item("import.json_unit", scen('''
    import json
    cli("--store", "st", "init")
    write("in.json", json.dumps([
        {"sku": "HNY-0201", "name": "Honey", "qty": 2.5, "unit": "Kg", "unit_cost": 6.4},
        {"sku": "JAR-0202", "name": "Jar", "quantity": "24", "unit": None},
        {"sku": "WTR-0203", "name": "Water", "quantity": 1500, "unit": " ML", "category": "drinks"},
        {"sku": "BAR-0204", "name": "Barley", "quantity": "0.5", "unit": "lb"}]))
    r = cli("--store", "st", "import", "in.json")
    OUT["r"] = [r["code"], r["err"]]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["r"] == [0, ""] and units_of(it) == {"HNY-0201": "kg", "JAR-0202": "ea", "WTR-0203": "ml", "BAR-0204": "lb"}
            and deq(it["HNY-0201"]["quantity"], "2.5") and deq(it["BAR-0204"]["quantity"], "0.5"))


@item("import.jsonl_unit", scen('''
    cli("--store", "st", "init")
    write("feed.jsonl", '{"sku": "SDA-0301", "name": "Soda", "quantity": 6, "unit": "L"}\\n'
                        '# from the till\\n'
                        '{"sku": "CAN-0302", "name": "Can", "quantity": 24}\\n'
                        '{"sku": "SLT-0303", "name": "Salt", "quantity": "1.25", "unit": "lb"}\\n'
                        '{"sku": "TAR-0304", "name": "Tar", "quantity": 1, "unit": "barrel"}\\n')
    r = cli("--store", "st", "import", "feed.jsonl")
    OUT["r"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["r"] == [1, ["feed.jsonl:5: unknown unit 'barrel'"]]
            and units_of(it) == {"SDA-0301": "l", "CAN-0302": "ea", "SLT-0303": "lb"} and deq(it["SLT-0303"]["quantity"], "1.25"))


LEDGER_IN = """# delivery 14
[FLR-0401]
name = Spelt flour
qty = 2.5 kg
category = baking

[SUG-0402]
name = Cane sugar
qty = 750g
cost = 0.40

[EGG-0403]
name = Eggs
qty = 12 EA
unit = ea

[MLK-0404]
name = Milk
qty = 4
unit = L

[BTR-0405]
name = Butter
qty = 2 kg
unit = g
"""


@item("import.ledger_units", scen('''
    cli("--store", "st", "init")
    write("in.ledger", LEDGER_IN)
    r = cli("--store", "st", "import", "in.ledger")
    OUT["r"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
''', LEDGER_IN=LEDGER_IN))
def _(o):
    it = o["items"]
    return (o["r"] == [1, ["in.ledger:22: unit mismatch (kg vs g)"]]
            and units_of(it) == {"FLR-0401": "kg", "SUG-0402": "g", "EGG-0403": "ea", "MLK-0404": "l"}
            and deq(it["FLR-0401"]["quantity"], "2.5") and deq(it["SUG-0402"]["quantity"], 750)
            and deq(it["SUG-0402"]["unit_cost"], "0.40") and deq(it["EGG-0403"]["quantity"], 12))


@item("import.unknown_unit", scen('''
    import json
    cli("--store", "st", "init")
    write("in.csv", "sku,name,quantity,unit\\nAAA-0501,Apples,5,kg\\nBBB-0502,Bricks,3,stone\\nCCC-0503,Corks,7,\\n")
    r1 = cli("--store", "st", "import", "in.csv")
    write("in.ledger", "[DDD-0504]\\nname = Dates\\nqty = 2 stone\\n\\n[EEE-0505]\\nname = Eels\\nqty = 1\\nunit = bushel\\n\\n"
                       "[FFF-0506]\\nname = Figs\\nqty = 9 oz\\n")
    r2 = cli("--store", "st", "import", "in.ledger")
    write("in.json", json.dumps([{"sku": "GGG-0507", "name": "Gum", "quantity": 1, "unit": "pcs"},
                                 {"sku": "HHH-0508", "name": "Hops", "quantity": 2, "unit": "OZ"}]))
    r3 = cli("--store", "st", "import", "in.json")
    OUT["r"] = [[r["code"], [l for l in r["err"].splitlines() if l.strip()]] for r in (r1, r2, r3)]
    OUT["items"] = items("st")
'''))
def _(o):
    return (o["r"] == [[1, ["in.csv:3: unknown unit 'stone'"]],
                       [1, ["in.ledger:1: unknown unit 'stone'", "in.ledger:5: unknown unit 'bushel'"]],
                       [1, ["in.json:1: unknown unit 'pcs'"]]]
            and units_of(o["items"]) == {"AAA-0501": "kg", "CCC-0503": "ea", "FFF-0506": "oz", "HHH-0508": "oz"})


MERGE_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
FLR-0601,Flour,baking,10,kg,2,C1,0.90
OAT-0603,Oats,baking,4,lb,1,C1,1.10
TEA-0602,Tea,drinks,5,ea,1,C2,2.00
"""


@item("import.merge_convert", scen('''
    write("st/items.csv", MERGE_ITEMS)
    write("st/movements.csv", "#stockroom-movements v2\\nid,sku,delta,unit,reason,note\\n")
    write("add.csv", "sku,name,quantity,unit\\nFLR-0601,Flour,500,g\\nTEA-0602,Tea,1,kg\\nOAT-0603,Oats,1,KG\\n")
    r = cli("--store", "st", "import", "add.csv")
    OUT["add"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["after_add"] = items("st")
    write("rep.csv", "sku,name,category,quantity,unit,reorder_level,location,unit_cost\\nFLR-0601,Flour,baking,20,lb,5,C1,0.45\\n"
                     "TEA-0602,Tea,drinks,2,g,1,C2,2.00\\n")
    r = cli("--store", "st", "import", "rep.csv", "--mode", "replace")
    OUT["rep"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
    OUT["moves"] = moves("st")
''', MERGE_ITEMS=MERGE_ITEMS))
def _(o):
    a, it, mv = o["after_add"], o["items"], o["moves"]
    want_moves = [("FLR-0601", "0.5", "kg", "add.csv"), ("OAT-0603", "2.205", "lb", "add.csv"),
                  ("FLR-0601", "-3.149", "lb", "rep.csv")]
    got = [(m["sku"], m["delta"], m["unit"], m["note"]) for m in mv]
    return (o["add"] == [1, ["add.csv:3: cannot convert kg to ea"]]
            and deq(a["FLR-0601"]["quantity"], "10.5") and a["FLR-0601"]["unit"] == "kg"
            and deq(a["OAT-0603"]["quantity"], "6.205") and a["OAT-0603"]["unit"] == "lb"
            and deq(a["TEA-0602"]["quantity"], 5) and a["TEA-0602"]["unit"] == "ea"
            and o["rep"] == [1, ["rep.csv:3: cannot convert g to ea"]]
            and it["FLR-0601"]["unit"] == "lb" and deq(it["FLR-0601"]["quantity"], 20) and deq(it["FLR-0601"]["unit_cost"], "0.45")
            and it["TEA-0602"]["unit"] == "ea" and deq(it["TEA-0602"]["quantity"], 5)
            and len(got) == 3 and all(g[0] == w[0] and deq(g[1], w[1]) and g[2] == w[2] and g[3] == w[3]
                                      for g, w in zip(got, want_moves))
            and all(m["reason"] == "import" for m in mv))


@item("import.whole_counts", scen('''
    cli("--store", "st", "init")
    write("in.csv", "sku,name,quantity,unit,reorder\\nBOL-0701,Bolt,2.5,ea,1\\nBOL-0702,Bolt pack,1.5,dz,0\\n"
                    "BOL-0703,Bolt box,0.1,dz,0\\nBOL-0704,Bolt mass,2.5,kg,0.5\\nBOL-0705,Bolt tray,3,dz,0.2\\n")
    r = cli("--store", "st", "import", "in.csv")
    OUT["r"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
'''))
def _(o):
    return (o["r"] == [1, ["in.csv:2: quantity: must be a whole number of ea",
                           "in.csv:4: quantity: must be a whole number of ea",
                           "in.csv:6: reorder_level: must be a whole number of ea"]]
            and units_of(o["items"]) == {"BOL-0702": "dz", "BOL-0704": "kg"})


# ------------------------------------------------------------------------------------------------- exporters

S_CSV = """sku,name,category,quantity,unit,reorder_level,location,unit_cost
BOL-0001,Hex bolt,fasteners,120,ea,50,A1,0.12
CHK-0002,Chickpeas,food,2.5,kg,5,C3,3.20
LNT-0007,Lentils,food,1.25,kg,2,C3,2.80
MLK-0003,Oat milk,food,6,l,8,C1,1.95
PEP-0004,Peppercorn,food,250,g,100,C3,0.04
RIB-0005,Ribbon,packing,1.5,dz,1,,4.80
SLT-0006,Sea salt,food,3,lb,1,C3,2.00
"""


@item("export.csv", S('''
    OUT["code"] = cli("--store", "st", "export", "out.csv")["code"]
    OUT["text"] = read("out.csv")
    OUT["food"] = cli("--store", "st", "export", "-", "--format", "csv", "--category", "food")["out"]
'''))
def _(o):
    lines = S_CSV.splitlines(True)
    food = lines[0] + "".join(l for l in lines[1:] if ",food," in l)
    return o["code"] == 0 and o["text"] == S_CSV and o["food"] == food


@item("export.json", S('''
    import json
    OUT["code"] = cli("--store", "st", "export", "out.json")["code"]
    doc = json.loads(read("out.json"))
    OUT["keys"] = [list(x) for x in doc]
    OUT["doc"] = doc
'''))
def _(o):
    doc = o["doc"]
    by = {x["sku"]: x for x in doc}
    return (o["code"] == 0 and all(k == ITEM_KEYS for k in o["keys"]) and len(doc) == 7
            and [x["sku"] for x in doc] == ["BOL-0001", "CHK-0002", "LNT-0007", "MLK-0003", "PEP-0004", "RIB-0005", "SLT-0006"]
            and {s: x["unit"] for s, x in by.items()} == {"BOL-0001": "ea", "CHK-0002": "kg", "LNT-0007": "kg",
                                                         "MLK-0003": "l", "PEP-0004": "g", "RIB-0005": "dz", "SLT-0006": "lb"}
            and by["CHK-0002"]["quantity"] == 2.5 and by["PEP-0004"]["quantity"] == 250
            and by["RIB-0005"]["reorder_level"] == 1 and by["SLT-0006"]["unit_cost"] == "2.00")


@item("export.ledger_roundtrip", S('''
    OUT["code"] = cli("--store", "st", "export", "out.ledger")["code"]
    OUT["text"] = read("out.ledger")
    cli("--store", "st2", "init")
    r = cli("--store", "st2", "import", "out.ledger")
    OUT["imp"] = [r["code"], r["err"]]
    OUT["a"] = items("st")
    OUT["b"] = items("st2")
'''))
def _(o):
    blocks = o["text"].split("\n\n")
    chk = [b for b in blocks if b.startswith("[CHK-0002]")]
    rib = [b for b in blocks if b.startswith("[RIB-0005]")]
    return (o["code"] == 0 and len(chk) == 1
            and chk[0].splitlines()[1:4] == ["name = Chickpeas", "qty = 2.5", "unit = kg"]
            and rib and rib[0].splitlines()[2:4] == ["qty = 1.5", "unit = dz"]
            and o["imp"] == [0, ""] and o["a"] == o["b"] and len(o["b"]) == 7)


@item("export.tsv", S('''
    r = cli("--store", "st", "export", "-", "--format", "tsv")
    OUT["r"] = [r["code"], r["out"]]
    OUT["file"] = cli("--store", "st", "export", "pick.tsv", "--category", "packing")["code"]
    OUT["pick"] = read("pick.tsv")
'''))
def _(o):
    want = ("LOCATION\tSKU\tNAME\tQTY\tUNIT\n-\tRIB-0005\tRibbon\t1.5\tdz\nA1\tBOL-0001\tHex bolt\t120\tea\n"
            "C1\tMLK-0003\tOat milk\t6\tl\nC3\tCHK-0002\tChickpeas\t2.5\tkg\nC3\tLNT-0007\tLentils\t1.25\tkg\n"
            "C3\tPEP-0004\tPeppercorn\t250\tg\nC3\tSLT-0006\tSea salt\t3\tlb\n")
    return o["r"] == [0, want] and o["file"] == 0 and o["pick"] == "LOCATION\tSKU\tNAME\tQTY\tUNIT\n-\tRIB-0005\tRibbon\t1.5\tdz\n"


@item("export.unit_option", S('''
    import json
    r = cli("--store", "st", "export", "-", "--format", "json", "--unit", "KG")
    OUT["kg"] = [r["code"], json.loads(r["out"]) if r["code"] == 0 else None]
    r = cli("--store", "st", "export", "-", "--format", "csv", "--unit", "oz", "--category", "food")
    OUT["oz"] = [r["code"], r["out"]]
    r = cli("--store", "st", "export", "-", "--format", "csv", "--unit", "stone")
    OUT["bad"] = [r["code"], r["err"], r["out"]]
    OUT["items"] = items("st")
'''))
def _(o):
    kg = {x["sku"]: x for x in o["kg"][1]}
    def is_(sku, q, u, r, c):
        x = kg[sku]
        return deq(x["quantity"], q) and x["unit"] == u and deq(x["reorder_level"], r) and x["unit_cost"] == c
    want_oz = ("sku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
               "CHK-0002,Chickpeas,food,88.185,oz,176.37,C3,0.09\nLNT-0007,Lentils,food,44.092,oz,70.548,C3,0.08\n"
               "MLK-0003,Oat milk,food,6,l,8,C1,1.95\nPEP-0004,Peppercorn,food,8.818,oz,3.527,C3,1.13\n"
               "SLT-0006,Sea salt,food,48,oz,16,C3,0.13\n")
    return (o["kg"][0] == 0 and is_("CHK-0002", "2.5", "kg", 5, "3.20") and is_("PEP-0004", "0.25", "kg", "0.1", "40.00")
            and is_("SLT-0006", "1.361", "kg", "0.454", "4.41") and is_("BOL-0001", 120, "ea", 50, "0.12")
            and is_("MLK-0003", 6, "l", 8, "1.95") and is_("RIB-0005", "1.5", "dz", 1, "4.80")
            and o["oz"] == [0, want_oz]
            and o["bad"][0] == 2 and "error: unknown unit 'stone'" in o["bad"][1] and o["bad"][2] == ""
            and o["items"]["PEP-0004"]["unit"] == "g" and deq(o["items"]["PEP-0004"]["quantity"], 250))


# ------------------------------------------------------------------------------------------------- validation

@item("validate.item_units", scen('''
    write("st/items.csv", "#stockroom-items v2\\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\\n"
          "ANV-0801,Anvil,tools,2,Stone,0,A1,90.00\\nBOL-0802,Bolt,fasteners,2.5,ea,1,A1,0.10\\n"
          "CRT-0803,Crate,packing,1.5,dz,0.2,B1,12.00\\nDOW-0804,Dowel,parts,0.1,dz,0,B2,1.00\\n"
          "FLR-0805,Flour,food,2.5,kg,0.5,C1,0.90\\nGLU-0806,Glue,parts,0.75,l,0.25,B2,4.00\\n")
    r = cli("--store", "st", "validate")
    OUT["r"] = [r["code"], [l for l in r["out"].splitlines() if l.strip()]]
    OUT["api"] = api("st", "GET", "/validate")["json"]
'''))
def _(o):
    want = {"ANV-0801 unit: unknown unit 'stone'", "BOL-0802 quantity: must be a whole number of ea",
            "CRT-0803 reorder_level: must be a whole number of ea", "DOW-0804 quantity: must be a whole number of ea"}
    return (o["r"][0] == 1 and set(o["r"][1]) == want and len(o["r"][1]) == 4
            and o["api"]["ok"] is False and set(o["api"]["problems"]) == want)


@item("validate.movement_units", scen('''
    write("st/items.csv", "#stockroom-items v2\\nsku,name,category,quantity,unit,reorder_level,location,unit_cost\\n"
          "BOL-0901,Bolt,fasteners,10,ea,0,A1,0.10\\nFLR-0902,Flour,food,2,kg,0,C1,0.90\\n")
    write("st/movements.csv", "#stockroom-movements v2\\nid,sku,delta,unit,reason,note\\n1,BOL-0901,10,ea,receive,\\n"
          "2,FLR-0902,2500,g,receive,\\n3,FLR-0902,-0.5,bushel,issue,\\n4,BOL-0901,1,kg,adjust,\\n"
          "5,XYZ-0001,1,ea,receive,\\n6,FLR-0902,0.5,L,adjust,\\n")
    r = cli("--store", "st", "validate")
    OUT["r"] = [r["code"], [l for l in r["out"].splitlines() if l.strip()]]
'''))
def _(o):
    want = {"movement 3 unit: unknown unit 'bushel'", "movement 4 unit: kg does not match item unit ea",
            "movement 5 sku: unknown sku XYZ-0001", "movement 6 unit: l does not match item unit kg"}
    return o["r"][0] == 1 and set(o["r"][1]) == want and len(o["r"][1]) == 4


# ------------------------------------------------------------------------------------------------- command line

@item("cli.add_unit", scen('''
    cli("--store", "st", "init")
    rs = [cli("--store", "st", "add", "FLR-1001", "Bread flour", "--category", "baking", "--qty", "2.5",
              "--reorder", "1", "--cost", "0.95", "--unit", "KG"),
          cli("--store", "st", "add", "EGG-1002", "Eggs", "--qty", "1.5", "--unit", "dz"),
          cli("--store", "st", "add", "ANV-1003", "Anvil", "--unit", "stone"),
          cli("--store", "st", "add", "BOL-1004", "Bolt", "--qty", "2.5"),
          cli("--store", "st", "add", "PIN-1005", "Pin", "--qty", "30")]
    OUT["rs"] = [[r["code"], r["out"].strip(), r["err"].strip()] for r in rs]
    OUT["show"] = cli("--store", "st", "show", "FLR-1001")["json"]
    OUT["hist"] = cli("--store", "st", "history", "EGG-1002", "--json")["json"]
    OUT["stats"] = cli("--store", "st", "stats", "--json")["json"]
    OUT["items"] = items("st")
'''))
def _(o):
    rs = o["rs"]
    return (rs[0][:2] == [0, "added FLR-1001 (2.5 kg)"] and rs[1][:2] == [0, "added EGG-1002 (1.5 dz)"]
            and rs[2][0] == 2 and rs[2][2] == "error: unknown unit 'stone'"
            and rs[3][0] == 1 and "invalid: BOL-1004 quantity: must be a whole number of ea" in rs[3][2]
            and rs[4][:2] == [0, "added PIN-1005 (30 ea)"]
            and o["show"]["unit"] == "kg" and list(o["show"])[:5] == ITEM_KEYS[:5]
            and o["hist"][0]["unit"] == "dz" and deq(o["hist"][0]["delta"], "1.5")
            and o["stats"]["units"] == {"dz": 1, "ea": 1, "kg": 1}
            and sorted(o["items"]) == ["EGG-1002", "FLR-1001", "PIN-1005"])


STOCK_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
BOL-1102,Bolt,fasteners,20,ea,5,A1,0.10
FLR-1101,Flour,food,10,kg,2,C1,0.90
"""


@item("cli.stock_unit", scen('''
    write("st/items.csv", STOCK_ITEMS)
    write("st/movements.csv", "#stockroom-movements v2\\nid,sku,delta,unit,reason,note\\n")
    s = "st"
    rs = [cli("--store", s, "receive", "FLR-1101", "500", "--unit", "g"),
          cli("--store", s, "issue", "FLR-1101", "1", "--unit", "lb"),
          cli("--store", s, "adjust", "FLR-1101", "9000", "--unit", "G"),
          cli("--store", s, "receive", "BOL-1102", "1", "--unit", "kg"),
          cli("--store", s, "issue", "BOL-1102", "2", "--unit", "dz"),
          cli("--store", s, "receive", "FLR-1101", "1", "--unit", "stone"),
          cli("--store", s, "issue", "BOL-1102", "1", "--unit", "dz")]
    OUT["rs"] = [[r["code"], r["out"].strip(), r["err"].strip()] for r in rs]
    write("count.csv", "sku,counted,note,unit\\nFLR-1101,8500,shelf,g\\nBOL-1102,1,,DZ\\nBOL-1102,3,,kg\\nFLR-1101,7,,bushel\\n")
    r = cli("--store", s, "count", "count.csv")
    OUT["count"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items(s)
    OUT["moves"] = moves(s)
''', STOCK_ITEMS=STOCK_ITEMS))
def _(o):
    rs, it = o["rs"], o["items"]
    want = [("FLR-1101", "0.5", "kg", "receive", ""), ("FLR-1101", "-0.454", "kg", "issue", ""),
            ("FLR-1101", "-1.046", "kg", "adjust", ""), ("BOL-1102", "-12", "ea", "issue", ""),
            ("FLR-1101", "-0.5", "kg", "adjust", "shelf"), ("BOL-1102", "4", "ea", "adjust", "count")]
    got = [(m["sku"], m["delta"], m["unit"], m["reason"], m["note"]) for m in o["moves"]]
    return (rs[0][:2] == [0, "FLR-1101: 10.5 kg (+0.5 kg)"] and rs[1][:2] == [0, "FLR-1101: 10.046 kg (-0.454 kg)"]
            and rs[2][:2] == [0, "FLR-1101: 9 kg (-1.046 kg)"]
            and rs[3][0] == 2 and rs[3][2] == "error: cannot convert kg to ea"
            and rs[4][0] == 2 and rs[4][2] == "error: insufficient stock for BOL-1102: have 20, need 24"
            and rs[5][0] == 2 and rs[5][2] == "error: unknown unit 'stone'"
            and rs[6][:2] == [0, "BOL-1102: 8 ea (-12 ea)"]
            and o["count"] == [1, ["count.csv:4: cannot convert kg to ea", "count.csv:5: unknown unit 'bushel'"]]
            and deq(it["FLR-1101"]["quantity"], "8.5") and deq(it["BOL-1102"]["quantity"], 12)
            and len(got) == len(want) and all(g[0] == w[0] and deq(g[1], w[1]) and g[2:] == w[2:] for g, w in zip(got, want)))


@item("cli.list_text", S('''
    OUT["list"] = cli("--store", "st", "list")["out"]
    OUT["hist"] = cli("--store", "st", "history", "CHK-0002")["out"]
    OUT["stats"] = cli("--store", "st", "stats", "--json")["json"]
'''))
def _(o):
    rows = {l.split()[0]: l for l in o["list"].splitlines() if l.strip()}
    def has(sku, *parts):
        line = rows.get(sku, "")
        return all(p in line for p in parts)
    hist = [l for l in o["hist"].splitlines() if l.strip()]
    return (has("CHK-0002", "  2.5 kg  ", "  5 kg  ") and has("RIB-0005", "  1.5 dz  ", "  1 dz  ")
            and has("BOL-0001", "  120 ea  ", "  50 ea  ") and has("PEP-0004", "  250 g  ", "  100 g  ")
            and len(hist) == 3 and "+3 kg" in hist[1] and "-500 g" in hist[2]
            and o["stats"]["units"] == {"dz": 1, "ea": 1, "g": 1, "kg": 2, "l": 1, "lb": 1})


# ------------------------------------------------------------------------------------------------- reports

@item("report.totals", S('''
    OUT["r"] = cli("--store", "st", "report", "totals", "--json")["json"]
    OUT["text"] = cli("--store", "st", "report", "totals")["out"]
'''))
def _(o):
    r = o["r"]
    want = [{"category": "fasteners", "unit": "ea", "items": 1, "quantity": 120, "value": "14.40"},
            {"category": "food", "unit": "g", "items": 1, "quantity": 250, "value": "10.00"},
            {"category": "food", "unit": "kg", "items": 2, "quantity": 3.75, "value": "11.50"},
            {"category": "food", "unit": "l", "items": 1, "quantity": 6, "value": "11.70"},
            {"category": "food", "unit": "lb", "items": 1, "quantity": 3, "value": "6.00"},
            {"category": "packing", "unit": "dz", "items": 1, "quantity": 1.5, "value": "7.20"}]
    return (r["report"] == "totals" and rows_eq(r["rows"], want) and r["unit"] is None and r["skipped"] == 0
            and all(list(x) == ["category", "items", "quantity", "unit", "value"] for x in r["rows"])
            and "UNIT" in o["text"].splitlines()[0])


@item("report.totals_unit", S('''
    OUT["kg"] = cli("--store", "st", "report", "totals", "--json", "--unit", "KG")["json"]
    OUT["ea"] = cli("--store", "st", "report", "totals", "--json", "--unit", "ea")["json"]
    r = cli("--store", "st", "report", "totals", "--unit", "stone")
    OUT["bad"] = [r["code"], r["err"].strip()]
'''))
def _(o):
    kg, ea = o["kg"], o["ea"]
    return (rows_eq(kg["rows"], [{"category": "food", "unit": "kg", "items": 4, "quantity": 5.361, "value": "27.50"}])
            and kg["unit"] == "kg" and kg["skipped"] == 3
            and rows_eq(ea["rows"], [{"category": "fasteners", "unit": "ea", "items": 1, "quantity": 120, "value": "14.40"},
                                     {"category": "packing", "unit": "ea", "items": 1, "quantity": 18, "value": "7.20"}])
            and ea["unit"] == "ea" and ea["skipped"] == 5
            and o["bad"] == [2, "error: unknown unit 'stone'"])


@item("report.low_stock_reorder", S('''
    OUT["plain"] = cli("--store", "st", "report", "low-stock", "--json")["json"]
    OUT["g"] = cli("--store", "st", "report", "low-stock", "--json", "--unit", "g")["json"]
    OUT["ro"] = cli("--store", "st", "report", "reorder", "--json")["json"]
    OUT["ro_lb"] = cli("--store", "st", "report", "reorder", "--json", "--unit", "lb")["json"]
'''))
def _(o):
    def low(q1, u1, r1, s1, q2, u2, r2, s2):
        return [{"sku": "MLK-0003", "name": "Oat milk", "location": "C1", "quantity": 6, "unit": "l", "reorder_level": 8, "shortfall": 2},
                {"sku": "CHK-0002", "name": "Chickpeas", "location": "C3", "quantity": q1, "unit": u1, "reorder_level": r1, "shortfall": s1},
                {"sku": "LNT-0007", "name": "Lentils", "location": "C3", "quantity": q2, "unit": u2, "reorder_level": r2, "shortfall": s2}]

    def ro(q1, u1, r1, o1, q2, u2, r2, o2):
        return [{"sku": "CHK-0002", "name": "Chickpeas", "quantity": q1, "unit": u1, "reorder_level": r1, "order": o1},
                {"sku": "LNT-0007", "name": "Lentils", "quantity": q2, "unit": u2, "reorder_level": r2, "order": o2},
                {"sku": "MLK-0003", "name": "Oat milk", "quantity": 6, "unit": "l", "reorder_level": 8, "order": 10}]
    return (rows_eq(o["plain"]["rows"], low(2.5, "kg", 5, 2.5, 1.25, "kg", 2, 0.75)) and o["plain"]["unit"] is None
            and rows_eq(o["g"]["rows"], low(2500, "g", 5000, 2500, 1250, "g", 2000, 750)) and o["g"]["unit"] == "g"
            and list(o["plain"]["rows"][0])[3:5] == ["quantity", "unit"]
            and rows_eq(o["ro"]["rows"], ro(2.5, "kg", 5, 7.5, 1.25, "kg", 2, 2.75)) and o["ro"]["unit"] is None
            and rows_eq(o["ro_lb"]["rows"], ro(5.512, "lb", 11.023, 16.535, 2.756, "lb", 4.409, 6.063))
            and o["ro_lb"]["unit"] == "lb")


@item("report.movements", S('''
    with open("st/movements.csv", "a") as f:
        f.write("8,OLD-0099,2,kg,receive,\\n9,OLD-0099,-500,g,issue,\\n10,BOL-0001,1,kg,adjust,\\n")
    OUT["plain"] = cli("--store", "st", "report", "movements", "--json")["json"]
    OUT["kg"] = cli("--store", "st", "report", "movements", "--json", "--unit", "kg")["json"]
'''))
def _(o):
    def row(sku, n, unit, rec, iss, net):
        return {"sku": sku, "movements": n, "unit": unit, "received": rec, "issued": iss, "net": net}
    plain = [row("BOL-0001", 1, "ea", 120, 0, 120), row("CHK-0002", 2, "kg", 3, 0.5, 2.5),
             row("LNT-0007", 1, "kg", 1.25, 0, 1.25), row("OLD-0099", 2, "g", 2000, 500, 1500),
             row("PEP-0004", 2, "g", 300, 50, 250), row("SLT-0006", 1, "lb", 3, 0, 3)]
    kg = [row("BOL-0001", 1, "ea", 120, 0, 120), row("CHK-0002", 2, "kg", 3, 0.5, 2.5),
          row("LNT-0007", 1, "kg", 1.25, 0, 1.25), row("OLD-0099", 2, "kg", 2, 0.5, 1.5),
          row("PEP-0004", 2, "kg", 0.3, 0.05, 0.25), row("SLT-0006", 1, "kg", 1.361, 0, 1.361)]
    return (rows_eq(o["plain"]["rows"], plain) and o["plain"]["unit"] is None
            and rows_eq(o["kg"]["rows"], kg) and o["kg"]["unit"] == "kg"
            and list(o["plain"]["rows"][0]) == ["sku", "movements", "received", "unit", "issued", "net"])


# ------------------------------------------------------------------------------------------------- request handlers

@item("api.items_unit", S('''
    OUT["one"] = api("st", "GET", "/items/CHK-0002")["json"]
    OUT["food"] = api("st", "GET", "/items?category=food")["json"]
    r = api("st", "POST", "/items", {"sku": "HNY-1101", "name": "Honey", "quantity": "1.5", "unit": " KG ",
                                     "unit_cost": "6.40"})
    OUT["new"] = [r["status"], r["json"]]
    r = api("st", "POST", "/items", {"sku": "ANV-1102", "name": "Anvil", "quantity": 1, "unit": "stone"})
    OUT["bad"] = [r["status"], r["json"]]
    r = api("st", "POST", "/items", {"sku": "BOL-1103", "name": "Bolt", "quantity": "2.5"})
    OUT["half"] = [r["status"], r["json"]]
    OUT["mv"] = api("st", "GET", "/items/HNY-1101/movements")["json"]
    OUT["items"] = items("st")
'''))
def _(o):
    one, new = o["one"], o["new"]
    return (list(one) == ITEM_KEYS + ["value"] and one["unit"] == "kg" and one["value"] == "8.00"
            and [x["unit"] for x in o["food"]["items"]] == ["kg", "kg", "l", "g", "lb"]
            and new[0] == 201 and new[1]["unit"] == "kg" and new[1]["quantity"] == 1.5
            and o["bad"] == [422, {"error": "unknown unit 'stone'"}]
            and o["half"][0] == 422 and "BOL-1103 quantity: must be a whole number of ea" in o["half"][1]["problems"]
            and len(o["mv"]["movements"]) == 1 and o["mv"]["movements"][0]["unit"] == "kg"
            and list(o["mv"]["movements"][0]) == ["id", "sku", "delta", "unit", "reason", "note"]
            and "ANV-1102" not in o["items"] and o["items"]["HNY-1101"]["unit"] == "kg")


@item("api.patch_unit", S('''
    r = api("st", "PATCH", "/items/PEP-0004", {"unit": "kg"})
    OUT["a"] = [r["status"], r["json"]]
    r = api("st", "PATCH", "/items/PEP-0004", {"unit": "ea"})
    OUT["b"] = [r["status"], r["json"]]
    r = api("st", "PATCH", "/items/PEP-0004", {"unit": "stone", "name": "Black pepper"})
    OUT["c"] = [r["status"], r["json"]]
    r = api("st", "PATCH", "/items/SLT-0006", {"unit": "OZ", "reorder_level": "20"})
    OUT["d"] = [r["status"], r["json"]]
    OUT["items"] = items("st")
    OUT["n"] = len(moves("st"))
'''))
def _(o):
    a, d, it = o["a"][1], o["d"][1], o["items"]
    return (o["a"][0] == 200 and a["unit"] == "kg" and deq(a["quantity"], "0.25") and deq(a["reorder_level"], "0.1")
            and a["unit_cost"] == "40.00"
            and o["b"] == [422, {"error": "cannot convert kg to ea"}]
            and o["c"] == [422, {"error": "unknown unit 'stone'"}]
            and o["d"][0] == 200 and d["unit"] == "oz" and deq(d["quantity"], 48) and deq(d["reorder_level"], 20)
            and d["unit_cost"] == "0.13"
            and it["PEP-0004"]["unit"] == "kg" and it["PEP-0004"]["name"] == "Peppercorn" and deq(it["PEP-0004"]["quantity"], "0.25")
            and it["SLT-0006"]["unit"] == "oz" and o["n"] == 7)


@item("api.stock_unit", S('''
    calls = [("/items/CHK-0002/receive", {"qty": "250", "unit": "g"}),
             ("/items/MLK-0003/issue", {"qty": 500, "unit": "ML", "note": "cafe"}),
             ("/items/BOL-0001/issue", {"qty": 1, "unit": "kg"}),
             ("/items/RIB-0005/adjust", {"qty": 24, "unit": "ea"}),
             ("/items/BOL-0001/receive", {"qty": 1, "unit": "bushel"}),
             ("/items/BOL-0001/receive", {"qty": 3})]
    OUT["rs"] = [[r["status"], r["json"]] for r in (api("st", "POST", p, b) for p, b in calls)]
    OUT["items"] = items("st")
'''))
def _(o):
    rs, it = o["rs"], o["items"]
    m0, m3 = rs[0][1]["movement"], rs[3][1]["movement"]
    return (rs[0][0] == 200 and deq(rs[0][1]["item"]["quantity"], "2.75") and rs[0][1]["item"]["unit"] == "kg"
            and deq(m0["delta"], "0.25") and m0["unit"] == "kg" and m0["reason"] == "receive"
            and rs[1][0] == 200 and deq(rs[1][1]["item"]["quantity"], "5.5") and rs[1][1]["movement"]["unit"] == "l"
            and rs[2] == [422, {"error": "cannot convert kg to ea"}]
            and rs[3][0] == 200 and deq(rs[3][1]["item"]["quantity"], 2) and deq(m3["delta"], "0.5") and m3["unit"] == "dz"
            and rs[4] == [422, {"error": "unknown unit 'bushel'"}]
            and rs[5][0] == 200 and deq(rs[5][1]["item"]["quantity"], 123) and rs[5][1]["movement"]["unit"] == "ea"
            and deq(it["BOL-0001"]["quantity"], 123) and deq(it["CHK-0002"]["quantity"], "2.75"))


@item("api.reports_transfer", S('''
    OUT["tot"] = api("st", "GET", "/reports/totals?unit=kg")["json"]
    OUT["low"] = api("st", "GET", "/reports/low-stock?unit=ml")["json"]
    r = api("st", "GET", "/reports/totals?unit=stone")
    OUT["bad"] = [r["status"], r["json"]]
    r = api("st", "GET", "/export?format=csv&category=food&unit=lb")
    OUT["csv"] = [r["status"], r["body"]]
    r = api("st", "GET", "/export?format=json&unit=parsec")
    OUT["badx"] = r["status"]
    body = ("[CHK-0002]\\nname = Chickpeas\\nqty = 500 g\\n\\n[NEW-1201]\\nname = Paprika\\nqty = 80g\\n\\n"
            "[BOL-0001]\\nname = Hex bolt\\nqty = 1 kg\\n")
    r = api("st", "POST", "/import?format=ledger&mode=add", body)
    OUT["imp"] = [r["status"], r["json"]]
    OUT["items"] = items("st")
    OUT["stats"] = api("st", "GET", "/stats")["json"]
'''))
def _(o):
    tot, low, it = o["tot"], o["low"], o["items"]
    want_csv = ("sku,name,category,quantity,unit,reorder_level,location,unit_cost\n"
                "CHK-0002,Chickpeas,food,5.512,lb,11.023,C3,1.45\nLNT-0007,Lentils,food,2.756,lb,4.409,C3,1.27\n"
                "MLK-0003,Oat milk,food,6,l,8,C1,1.95\nPEP-0004,Peppercorn,food,0.551,lb,0.22,C3,18.14\n"
                "SLT-0006,Sea salt,food,3,lb,1,C3,2.00\n")
    mlk = [r for r in low["rows"] if r["sku"] == "MLK-0003"]
    chk = [r for r in low["rows"] if r["sku"] == "CHK-0002"]
    return (rows_eq(tot["rows"], [{"category": "food", "unit": "kg", "items": 4, "quantity": 5.361, "value": "27.50"}])
            and tot["skipped"] == 3 and low["unit"] == "ml"
            and mlk and mlk[0]["unit"] == "ml" and deq(mlk[0]["quantity"], 6000) and deq(mlk[0]["shortfall"], 2000)
            and chk and chk[0]["unit"] == "kg" and deq(chk[0]["quantity"], "2.5")
            and o["bad"] == [422, {"error": "unknown unit 'stone'"}] and o["csv"] == [200, want_csv] and o["badx"] == 422
            and o["imp"] == [200, {"imported": 2, "errors": [{"line": 9, "message": "cannot convert kg to ea"}]}]
            and deq(it["CHK-0002"]["quantity"], 3) and it["NEW-1201"]["unit"] == "g" and deq(it["NEW-1201"]["quantity"], 80)
            and deq(it["BOL-0001"]["quantity"], 120)
            and o["stats"]["units"] == {"dz": 1, "ea": 1, "g": 2, "kg": 2, "l": 1, "lb": 1})


# ------------------------------------------------------------------------------------------------- pipelines

@item("pipeline.to_unit", scen('''
    import json
    write("in.csv", "sku,name,category,quantity,unit,reorder_level,unit_cost\\nRCE-1301,Rice,food,2,kg,1,2.00\\n"
                    "BNS-1302,Beans,food,500,g,0,0.01\\nJUC-1303,Juice,food,1.5,l,1,3.00\\nCUP-1304,Cups,supplies,50,,10,0.05\\n")
    write("p.json", json.dumps({"input": {"path": "in.csv"},
                                "steps": [{"step": "where", "field": "category", "equals": "food"},
                                          {"step": "to_unit", "unit": "LB"}],
                                "output": {"path": "out.json"}}))
    OUT["code"] = cli("--store", "st", "run", "p.json")["code"]
    OUT["out"] = json.loads(read("out.json"))
'''))
def _(o):
    out = o["out"]
    by = {x["sku"]: x for x in out}
    def is_(sku, q, u, r, c):
        x = by[sku]
        return deq(x["quantity"], q) and x["unit"] == u and deq(x["reorder_level"], r) and x["unit_cost"] == c
    return (o["code"] == 0 and [x["sku"] for x in out] == ["RCE-1301", "BNS-1302", "JUC-1303"]
            and all(list(x) == ITEM_KEYS for x in out)
            and is_("RCE-1301", "4.409", "lb", "2.205", "0.91") and is_("BNS-1302", "1.102", "lb", 0, "4.54")
            and is_("JUC-1303", "1.5", "l", 1, "3.00"))


@item("pipeline.where_unit_tsv", scen('''
    import json
    write("in.ledger", "[ALM-1401]\\nname = Almonds\\nqty = 3 kg\\nlocation = A2\\n\\n[BRN-1402]\\nname = Bran\\nqty = 900g\\n"
                       "location = A1\\n\\n[CLP-1403]\\nname = Clips\\nqty = 4\\nlocation = A1\\n\\n"
                       "[DAT-1404]\\nname = Dates\\nqty = 2 lb\\nlocation = A3\\n")
    write("p.json", json.dumps({"input": {"path": "in.ledger"},
                                "steps": [{"step": "where", "field": "unit", "in": ["KG", " g"]}],
                                "output": {"format": "tsv", "path": "-"}}))
    r = cli("--store", "st", "run", "p.json")
    OUT["r"] = [r["code"], r["out"]]
'''))
def _(o):
    return o["r"] == [0, "LOCATION\tSKU\tNAME\tQTY\tUNIT\nA1\tBRN-1402\tBran\t900\tg\nA2\tALM-1401\tAlmonds\t3\tkg\n"]


@item("pipeline.report_unit", S('''
    import json
    write("p.json", json.dumps({"input": {"store": True},
                                "steps": [{"step": "where", "field": "category", "equals": "food"}],
                                "output": {"report": "totals", "unit": "g", "path": "tot.json"}}))
    OUT["code"] = cli("--store", "st", "run", "p.json")["code"]
    OUT["tot"] = json.loads(read("tot.json"))
    write("q.json", json.dumps({"input": {"store": True}, "output": {"report": "low-stock", "unit": "ML", "path": "-"}}))
    r = cli("--store", "st", "run", "q.json")
    OUT["low"] = [r["code"], json.loads(r["out"]) if r["code"] == 0 else None]
'''))
def _(o):
    tot, low = o["tot"], o["low"][1]
    mlk = [r for r in low["rows"] if r["sku"] == "MLK-0003"]
    return (o["code"] == 0 and rows_eq(tot["rows"], [{"category": "food", "unit": "g", "items": 4, "quantity": 5360.777, "value": "27.50"}])
            and tot["unit"] == "g" and tot["skipped"] == 1
            and o["low"][0] == 0 and low["unit"] == "ml" and mlk and deq(mlk[0]["reorder_level"], 8000)
            and [r["sku"] for r in low["rows"]] == ["MLK-0003", "CHK-0002", "LNT-0007"])


@item("pipeline.json_to_ledger", scen('''
    import json
    write("in.json", json.dumps([{"sku": "ALM-1501", "name": "Almonds", "quantity": "1.5", "unit": "lb", "unit_cost": "9.00"},
                                 {"sku": "CSH-1502", "name": "Cashews", "quantity": 400, "unit": "G"},
                                 {"sku": "BAG-1503", "name": "Bags", "quantity": 30}]))
    write("p.json", json.dumps({"input": {"path": "in.json"}, "steps": [{"step": "to_unit", "unit": "kg"}],
                                "output": {"path": "out.ledger"}}))
    OUT["code"] = cli("--store", "st", "run", "p.json")["code"]
    OUT["text"] = read("out.ledger")
    cli("--store", "st", "init")
    OUT["imp"] = cli("--store", "st", "import", "out.ledger")["code"]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["code"] == 0 and "qty = 0.68\nunit = kg\n" in o["text"] and "qty = 30\nunit = ea\n" in o["text"]
            and o["imp"] == 0 and units_of(it) == {"ALM-1501": "kg", "CSH-1502": "kg", "BAG-1503": "ea"}
            and deq(it["ALM-1501"]["quantity"], "0.68") and deq(it["ALM-1501"]["unit_cost"], "19.84")
            and deq(it["CSH-1502"]["quantity"], "0.4"))


# ------------------------------------------------------------------------------------------------- kits, packs, stock

K_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
BRD-0100,Bread kit,kits,0,ea,0,K1,0.00
EGG-0101,Eggs,food,2,dz,0,F1,3.60
FLR-0102,Flour,food,2,kg,0,F1,0.90
MLK-0103,Milk,food,1.5,l,0,F2,1.10
PCK-0105,Pancake kit,kits,0,ea,0,K1,0.00
YST-0104,Yeast,food,100,g,0,F2,0.05
"""
K_KITS = """#stockroom-kits v2
kit,component,quantity,unit
BRD-0100,EGG-0101,6,ea
BRD-0100,FLR-0102,450,g
BRD-0100,MLK-0103,300,ml
BRD-0100,YST-0104,7,g
PCK-0105,FLR-0102,2,ea
"""


@item("kits.define_show", scen('''
    write("st/items.csv", K_ITEMS)
    rs = [cli("--store", "st", "kit-define", "BRD-0100", "FLR-0102=500g", "YST-0104=7", "EGG-0101=0.5", "MLK-0103=0.25 L"),
          cli("--store", "st", "kit-define", "PCK-0105", "FLR-0102=2ea"),
          cli("--store", "st", "kit-define", "PCK-0105", "FLR-0102=2stone")]
    OUT["rs"] = [[r["code"], r["err"].strip()] for r in rs]
    OUT["text"] = cli("--store", "st", "kit-show", "BRD-0100")["out"]
    OUT["json"] = cli("--store", "st", "kit-show", "BRD-0100", "--json")["json"]
    OUT["kits_csv"] = read("st/kits.csv")
    OUT["api"] = api("st", "GET", "/kits/BRD-0100")["json"]
''', K_ITEMS=K_ITEMS))
def _(o):
    lines = [l for l in o["text"].splitlines() if l.strip()]
    j = o["json"]
    return (o["rs"][0][0] == 0 and o["rs"][1] == [2, "error: cannot convert ea to kg"]
            and o["rs"][2] == [2, "error: unknown unit 'stone'"]
            and lines == ["BRD-0100 Bread kit: can assemble 4", "  EGG-0101 Eggs: 0.5 dz", "  FLR-0102 Flour: 500 g",
                          "  MLK-0103 Milk: 0.25 l", "  YST-0104 Yeast: 7 g"]
            and j["capacity"] == 4 and [list(x) for x in j["lines"]] == [["component", "name", "quantity", "unit"]] * 4
            and [(x["component"], x["unit"]) for x in j["lines"]] == [("EGG-0101", "dz"), ("FLR-0102", "g"),
                                                                       ("MLK-0103", "l"), ("YST-0104", "g")]
            and o["kits_csv"] == ("#stockroom-kits v2\nkit,component,quantity,unit\nBRD-0100,EGG-0101,0.5,dz\n"
                                  "BRD-0100,FLR-0102,500,g\nBRD-0100,MLK-0103,0.25,l\nBRD-0100,YST-0104,7,g\n")
            and o["api"] == j)


@item("kits.assemble", scen('''
    write("st/items.csv", K_ITEMS)
    write("st/kits.csv", K_KITS)
    OUT["cap0"] = cli("--store", "st", "report", "kits", "--json")["json"]
    r = cli("--store", "st", "assemble", "BRD-0100", "3")
    OUT["asm"] = [r["code"], r["out"].strip()]
    OUT["items"] = items("st")
    OUT["moves"] = moves("st")
    r = api("st", "POST", "/kits/BRD-0100/assemble", {"count": 2})
    OUT["api"] = [r["status"], r["json"]]
    r = cli("--store", "st", "assemble", "PCK-0105", "1")
    OUT["pck"] = [r["code"], r["err"].strip()]
    OUT["cap1"] = cli("--store", "st", "kit-show", "BRD-0100", "--json")["json"]["capacity"]
    r = cli("--store", "st", "validate")
    OUT["val"] = [r["code"], [l for l in r["out"].splitlines() if l.strip()]]
''', K_ITEMS=K_ITEMS, K_KITS=K_KITS))
def _(o):
    it = o["items"]
    want = [("EGG-0101", "-1.5", "dz"), ("FLR-0102", "-1.35", "kg"), ("MLK-0103", "-0.9", "l"), ("YST-0104", "-21", "g"),
            ("BRD-0100", "3", "ea")]
    got = [(m["sku"], m["delta"], m["unit"]) for m in o["moves"]]
    return (o["cap0"]["rows"] == [{"kit": "BRD-0100", "name": "Bread kit", "lines": 4, "capacity": 4},
                                  {"kit": "PCK-0105", "name": "Pancake kit", "lines": 1, "capacity": 0}]
            and o["asm"] == [0, "assembled 3 x BRD-0100: BRD-0100 now 3 ea"]
            and deq(it["EGG-0101"]["quantity"], "0.5") and deq(it["FLR-0102"]["quantity"], "0.65")
            and deq(it["MLK-0103"]["quantity"], "0.6") and deq(it["YST-0104"]["quantity"], 79)
            and deq(it["BRD-0100"]["quantity"], 3)
            and len(got) == 5 and all(g[0] == w[0] and deq(g[1], w[1]) and g[2] == w[2] for g, w in zip(got, want))
            and all(m["reason"] == "assemble" and m["note"] == "kit BRD-0100 x 3" for m in o["moves"])
            and o["api"] == [409, {"error": "insufficient stock for EGG-0101: have 0.5, need 1"}]
            and o["pck"] == [2, "error: cannot convert ea to kg"] and o["cap1"] == 1
            and o["val"] == [1, ["kit PCK-0105 line FLR-0102 unit: ea does not match item unit kg"]])


P_PACKS = """#stockroom-packs v2
sku,supplier,quantity,unit,price
CHK-0002,Bulk Barn,25,lb,30.00
CHK-0002,Pulse Co,10,kg,32.00
CHK-0002,Tiny,500,g,2.00
LNT-0007,Pulse Co,1,l,3.00
MLK-0003,Dairy Co,1000,ml,1.80
MLK-0003,Oat Co,1.5,l,2.50
"""


@item("packs.reorder_mail", scen(S_SETUP + textwrap.dedent('''
    write("st/packs.csv", P_PACKS)
    OUT["mail"] = cli("--store", "st", "reorder-mail")["out"]
    OUT["packs"] = cli("--store", "st", "packs", "CHK-0002")["out"]
    OUT["pj"] = cli("--store", "st", "packs", "MLK-0003", "--json")["json"]
    r = cli("--store", "st", "pack-set", "LNT-0007", "Pulse Co", "2", "3.00", "--unit", "ml")
    OUT["bad"] = [r["code"], r["err"].strip()]
    r = cli("--store", "st", "pack-set", "PEP-0004", "Spice Co", "0.5", "15.00", "--unit", "KG")
    OUT["set"] = [r["code"], r["json"]]
    r = cli("--store", "st", "pack-set", "PEP-0004", "Herb Co", "40", "1.20")
    OUT["set2"] = r["json"]
    OUT["api"] = api("st", "GET", "/items/PEP-0004/packs")["json"]
'''), S_ITEMS=S_ITEMS, S_MOVES=S_MOVES, P_PACKS=P_PACKS))
def _(o):
    mail = ("Reorder list (3 items)\n"
            "CHK-0002 Chickpeas: order 7.5 kg (have 2.5 kg, reorder level 5 kg)\n"
            "  cheapest: 1 x Bulk Barn pack of 25 lb = 30.00\n"
            "LNT-0007 Lentils: order 2.75 kg (have 1.25 kg, reorder level 2 kg)\n"
            "  no pack on file\n"
            "MLK-0003 Oat milk: order 10 l (have 6 l, reorder level 8 l)\n"
            "  cheapest: 7 x Oat Co pack of 1.5 l = 17.50\n")
    packs = ["Bulk Barn  25 lb per pack  30.00", "Pulse Co  10 kg per pack  32.00", "Tiny  500 g per pack  2.00"]
    pj = o["pj"]["packs"]
    return (o["mail"].rstrip("\n") == mail.rstrip("\n")
            and [l for l in o["packs"].splitlines() if l.strip()] == packs
            and [list(p) for p in pj] == [["sku", "supplier", "quantity", "unit", "price"]] * 2
            and [(p["supplier"], p["quantity"], p["unit"]) for p in pj] == [("Dairy Co", 1000, "ml"), ("Oat Co", 1.5, "l")]
            and o["bad"] == [2, "error: cannot convert ml to kg"]
            and o["set"][0] == 0 and o["set"][1] == {"sku": "PEP-0004", "supplier": "Spice Co", "quantity": 0.5, "unit": "kg", "price": "15.00"}
            and o["set2"]["unit"] == "g" and o["set2"]["quantity"] == 40
            and [(p["supplier"], p["unit"]) for p in o["api"]["packs"]] == [("Herb Co", "g"), ("Spice Co", "kg")])


UNDO_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
CFE-0201,Coffee,drinks,2,kg,0,D1,18.00
CUP-0202,Cups,supplies,50,ea,0,D2,0.10
TEA-0203,Tea,drinks,400,g,0,D1,0.04
"""
UNDO_MOVES = """#stockroom-movements v2
id,sku,delta,unit,reason,note
1,CFE-0201,1500,g,receive,initial stock
2,CFE-0201,500,g,receive,
3,CUP-0202,50,ea,receive,
4,TEA-0203,400,g,receive,
"""
OTHER_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
CFE-0201,Coffee,drinks,250,g,0,X1,0.02
CUP-0202,Cups,supplies,1,kg,0,X2,4.00
"""


@item("stock.undo_transfer", scen('''
    write("st/items.csv", UNDO_ITEMS)
    write("st/movements.csv", UNDO_MOVES)
    write("other/items.csv", OTHER_ITEMS)
    r = cli("--store", "st", "undo", "CFE-0201")
    OUT["undo"] = [r["code"], r["out"].strip()]
    r = api("st", "POST", "/items/TEA-0203/undo")
    OUT["undo_api"] = [r["status"], r["json"]]
    r = cli("--store", "st", "transfer", "CFE-0201", "0.5", "--to", "other")
    OUT["t1"] = [r["code"], r["out"].strip()]
    r = cli("--store", "st", "transfer", "CUP-0202", "5", "--to", "other")
    OUT["t2"] = [r["code"], r["err"].strip()]
    r = cli("--store", "st", "transfer", "TEA-0203", "100", "--to", "other")
    OUT["t3"] = [r["code"], r["err"].strip()]
    write("st/movements.csv", read("st/movements.csv") + "")
    cli("--store", "st", "receive", "TEA-0203", "300")
    r = cli("--store", "st", "transfer", "TEA-0203", "0.25", "--unit", "KG", "--to", "other")
    OUT["t4"] = [r["code"], r["out"].strip()]
    OUT["a"] = items("st")
    OUT["b"] = items("other")
    OUT["bm"] = moves("other")
    OUT["other_csv"] = read("other/items.csv").splitlines()[:2]
''', UNDO_ITEMS=UNDO_ITEMS, UNDO_MOVES=UNDO_MOVES, OTHER_ITEMS=OTHER_ITEMS))
def _(o):
    a, b = o["a"], o["b"]
    bm = [(m["sku"], m["delta"], m["unit"], m["reason"], m["note"]) for m in o["bm"]]
    want_bm = [("CFE-0201", "500", "g", "receive", "transfer from st"), ("TEA-0203", "250", "g", "receive", "transfer from st")]
    ua = o["undo_api"]
    return (o["undo"] == [0, "CFE-0201: 1.5 kg (-0.5 kg), undid #2"]
            and ua[0] == 200 and deq(ua[1]["item"]["quantity"], 0) and deq(ua[1]["movement"]["delta"], -400)
            and ua[1]["movement"]["unit"] == "g" and ua[1]["movement"]["note"] == "undo #4"
            and o["t1"] == [0, "CFE-0201: 1 kg here, 750 g at other"]
            and o["t2"] == [2, "error: cannot convert ea to kg"]
            and o["t3"] == [2, "error: insufficient stock for TEA-0203: have 0, need 100"]
            and o["t4"] == [0, "TEA-0203: 50 g here, 250 g at other"]
            and deq(a["CFE-0201"]["quantity"], 1) and deq(a["CUP-0202"]["quantity"], 50)
            and deq(b["CFE-0201"]["quantity"], 750) and b["CFE-0201"]["unit"] == "g" and deq(b["CUP-0202"]["quantity"], 1)
            and b["TEA-0203"]["unit"] == "g" and b["TEA-0203"]["location"] == "D1" and deq(b["TEA-0203"]["quantity"], 250)
            and len(bm) == 2 and all(g[0] == w[0] and deq(g[1], w[1]) and g[2:] == w[2:] for g, w in zip(bm, want_bm))
            and o["other_csv"] == ["#stockroom-items v2", "sku,name,category,quantity,unit,reorder_level,location,unit_cost"])


RC_ITEMS = """#stockroom-items v2
sku,name,category,quantity,unit,reorder_level,location,unit_cost
BOL-0301,Bolt,fasteners,10,ea,0,A1,0.10
EGG-0302,Eggs,food,2,dz,0,F1,3.60
FLR-0303,Flour,food,5,kg,0,F1,0.90
"""


@item("stock.recheck", scen('''
    write("st/items.csv", RC_ITEMS)
    rs = [cli("--store", "st", "issue", "BOL-0301", "0.5"),
          cli("--store", "st", "receive", "EGG-0302", "0.1"),
          cli("--store", "st", "receive", "EGG-0302", "6", "--unit", "ea"),
          cli("--store", "st", "adjust", "FLR-0303", "4.25")]
    OUT["rs"] = [[r["code"], r["err"].strip()] for r in rs]
    r = api("st", "POST", "/items/EGG-0302/issue", {"qty": "1", "unit": "ea"})
    OUT["api"] = [r["status"], r["json"]]
    write("count.csv", "sku,counted\\nBOL-0301,7.5\\nFLR-0303,4\\n")
    r = cli("--store", "st", "count", "count.csv")
    OUT["count"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    write("add.csv", "sku,name,quantity,unit\\nEGG-0302,Eggs,5,ea\\nEGG-0302,Eggs,12,ea\\n")
    r = cli("--store", "st", "import", "add.csv")
    OUT["imp"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
    OUT["n"] = len(moves("st"))
''', RC_ITEMS=RC_ITEMS))
def _(o):
    rs, it = o["rs"], o["items"]
    return (rs[0] == [1, "invalid: BOL-0301 quantity: must be a whole number of ea"]
            and rs[1] == [1, "invalid: EGG-0302 quantity: must be a whole number of ea"]
            and rs[2] == [0, ""] and rs[3] == [0, ""]
            and o["api"] == [422, {"error": "invalid item", "problems": ["EGG-0302 quantity: must be a whole number of ea"]}]
            and o["count"] == [1, ["count.csv:2: quantity: must be a whole number of ea"]]
            and o["imp"] == [1, ["add.csv:2: quantity: must be a whole number of ea"]]
            and deq(it["BOL-0301"]["quantity"], 10) and deq(it["EGG-0302"]["quantity"], "3.5")
            and deq(it["FLR-0303"]["quantity"], 4) and o["n"] == 4)


@item("vendor.acme_cargo", scen('''
    cli("--store", "st", "init")
    write("acme.csv", "Item No,Description,Qty,UOM,Unit Price,Bin\\nNUT-0401,Wing nut,40,C62,0.20,a1\\n"
                      "SND-0402,Sand,25,KGM,0.30,y1\\nOIL-0403,Oil,5,ltr,4.00,y2\\nBAG-0404,Sacks,3,BAG,1.00,y3\\n"
                      "TAP-0405,Tape,6,,1.50,a2\\n")
    r = cli("--store", "st", "import", "acme.csv", "--format", "vendor-acme")
    OUT["acme"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    write("cargo.tsv", "SKU\\tNAME\\tQTY\\tUNIT_CODE\\tCOST\\tCATEGORY\\nEGG-0406\\tEggs\\t4\\tDZN\\t3.60\\tfood\\n"
                       "HAM-0407\\tHam\\t3.5\\tLBR\\t5.00\\tfood\\nSPN-0408\\tSponge\\t12\\tH87\\t0.40\\tcleaning\\n")
    r = cli("--store", "st", "import", "cargo.tsv", "--format", "vendor-cargo")
    OUT["cargo"] = [r["code"], r["err"]]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["acme"] == [1, ["acme.csv:5: unknown unit 'BAG'"]] and o["cargo"] == [0, ""]
            and units_of(it) == {"NUT-0401": "ea", "SND-0402": "kg", "OIL-0403": "l", "TAP-0405": "ea",
                                 "EGG-0406": "dz", "HAM-0407": "lb", "SPN-0408": "ea"}
            and deq(it["SND-0402"]["quantity"], 25) and deq(it["HAM-0407"]["quantity"], "3.5"))


@item("vendor.brightline_delta", scen('''
    cli("--store", "st", "init")
    write("bl.csv", "article;label;amount;measure;price_eur\\nRCE-0501;Rice;1.250,5;GRM;0,01\\nJUC-0502;Juice;2,5;LTR;1,20\\n"
                    "SLT-0503;Salt;12;kgm;0,40\\nCAN-0504;Cans;24;EA;0,50\\nTUB-0505;Tub;1;TUB;3,00\\n")
    r = cli("--store", "st", "import", "bl.csv", "--format", "vendor-brightline")
    OUT["bl"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    write("delta.txt", "code|name|quantity|uom|reorder|location\\nVAN-0506|Vanilla|250|MLT|50|B1\\n"
                       "SAF-0507|Saffron|2|ONZ|1|B2\\nPIN-0508|Pins|100||20|B3\\nOAT-0509|Oats|3|kgs|1|B4\\n")
    r = cli("--store", "st", "import", "delta.txt", "--format", "vendor-delta")
    OUT["dl"] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["bl"] == [1, ["bl.csv:6: unknown unit 'TUB'"]] and o["dl"] == [1, ["delta.txt:5: unknown unit 'kgs'"]]
            and units_of(it) == {"RCE-0501": "g", "JUC-0502": "l", "SLT-0503": "kg", "CAN-0504": "ea",
                                 "VAN-0506": "ml", "SAF-0507": "oz", "PIN-0508": "ea"}
            and deq(it["RCE-0501"]["quantity"], "1250.5") and deq(it["JUC-0502"]["quantity"], "2.5")
            and deq(it["SAF-0507"]["reorder_level"], 1))


@item("sort.quantity", S('''
    import json
    OUT["list"] = [x["sku"] for x in cli("--store", "st", "list", "--sort", "quantity", "--json")["json"]]
    OUT["api"] = [x["sku"] for x in api("st", "GET", "/items?sort=quantity&limit=4")["json"]["items"]]
    write("p.json", json.dumps({"input": {"store": True}, "steps": [{"step": "sort", "by": "quantity", "reverse": True}],
                                "output": {"format": "tsv", "path": "-"}}))
    write("q.json", json.dumps({"input": {"store": True}, "steps": [{"step": "sort", "by": "quantity"}],
                                "output": {"format": "json", "path": "-"}}))
    r = cli("--store", "st", "run", "q.json")
    OUT["step"] = [x["sku"] for x in json.loads(r["out"])] if r["code"] == 0 else None
    write("r.json", json.dumps({"input": {"store": True}, "steps": [{"step": "sort", "by": "quantity", "reverse": True}],
                                "output": {"format": "json", "path": "-"}}))
    r = cli("--store", "st", "run", "r.json")
    OUT["rev"] = [x["sku"] for x in json.loads(r["out"])] if r["code"] == 0 else None
    OUT["csv"] = cli("--store", "st", "history", "PEP-0004", "--csv")["out"]
'''))
def _(o):
    order = ["RIB-0005", "BOL-0001", "PEP-0004", "LNT-0007", "SLT-0006", "CHK-0002", "MLK-0003"]
    return (o["list"] == order and o["api"] == order[:4] and o["step"] == order and o["rev"] == order[::-1]
            and o["csv"].splitlines() == ["id,sku,delta,unit,reason,note", "4,PEP-0004,0.3,kg,receive,", "5,PEP-0004,-50,g,issue,"])


@item("vendor.elm_fjord_granite", scen('''
    cli("--store", "st", "init")
    write("elm.csv", "sku,desc,on_hand,uom_code,cost\\nWAX-0601,Wax,3.5,KGM,4.00\\nCRD-0602,Cord,12,DZN,0.80\\n"
                     "GLV-0603,Gloves,40,,0.50\\nPOT-0604,Pot,2,BOX,5.00\\n")
    write("fjord.csv", "Artikel;Bezeichnung;Menge;Einheit;Preis\\nMEL-0605;Mehl;2,5;kgm;1,20\\nMIL-0606;Milch;1.000;MLT;0,90\\n"
                       "TUE-0607;Tuete;100;H87;0,05\\nFAS-0608;Fass;1;FASS;10,00\\n")
    write("granite.csv", "part,title,stock,price\\nGRV-0609,Gravel,500 KGM,0.05\\nBRK-0610,Brick,40 C62,0.60\\n"
                         "SND-0611,Sand,25,0.10\\nCEM-0612,Cement,3 SACK,8.00\\nLIM-0613,Lime,2 lbr,4.00\\n")
    rs = [cli("--store", "st", "import", f, "--format", "vendor-" + f.split(".")[0]) for f in ("elm.csv", "fjord.csv", "granite.csv")]
    OUT["rs"] = [[r["code"], [l for l in r["err"].splitlines() if l.strip()]] for r in rs]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["rs"] == [[1, ["elm.csv:5: unknown unit 'BOX'"]], [1, ["fjord.csv:5: unknown unit 'FASS'"]],
                        [1, ["granite.csv:5: unknown unit 'SACK'"]]]
            and units_of(it) == {"WAX-0601": "kg", "CRD-0602": "dz", "GLV-0603": "ea", "MEL-0605": "kg", "MIL-0606": "ml",
                                 "TUE-0607": "ea", "GRV-0609": "kg", "BRK-0610": "ea", "SND-0611": "ea", "LIM-0613": "lb"}
            and deq(it["MEL-0605"]["quantity"], "2.5") and deq(it["MIL-0606"]["quantity"], 1000)
            and deq(it["GRV-0609"]["quantity"], 500) and deq(it["WAX-0601"]["quantity"], "3.5"))


@item("vendor.harbor_iris_juniper", scen('''
    cli("--store", "st", "init")
    write("harbor.jsonl", '{"item": "RCE-0701", "name": "Rice", "count": 5, "uom": "KGM", "price": "2.00"}\\n'
                          '{"item": "OIL-0702", "name": "Oil", "count": 750, "uom": "mlt"}\\n'
                          '{"item": "PEA-0703", "name": "Peas", "count": 12}\\n'
                          '{"item": "BAG-0704", "name": "Bags", "count": 1, "uom": "PKG"}\\n')
    rows = ["* Iris feed"] + ["%-8s %-20s %8s %-3s" % r for r in (("PEN-0705", "Pens", "100", ""), ("INK-0706", "Ink", "0.5", "LTR"),
                                                                     ("PAP-0707", "Paper", "2.5", "KGM"), ("TON-0708", "Toner", "1", "CT"))]
    write("iris.txt", "\\n".join(rows) + "\\n")
    write("juniper.csv", "sku,name,packs,pack_size,pack_uom,cost_per_pack\\nBXS-0709,Boxes,4,25,C62,10.00\\n"
                         "FOI-0710,Foil,2,1.5,KGM,6.00\\nTAP-0711,Tape,3,0.5,LTR,2.00\\nWRP-0712,Wrap,1,1,ROLL,3.00\\n")
    rs = [cli("--store", "st", "import", f, "--format", "vendor-" + n)
          for f, n in (("harbor.jsonl", "harbor"), ("iris.txt", "iris"), ("juniper.csv", "juniper"))]
    OUT["rs"] = [[r["code"], [l for l in r["err"].splitlines() if l.strip()]] for r in rs]
    OUT["items"] = items("st")
'''))
def _(o):
    it = o["items"]
    return (o["rs"] == [[1, ["harbor.jsonl:4: unknown unit 'PKG'"]], [1, ["iris.txt:5: unknown unit 'CT'"]],
                        [1, ["juniper.csv:5: unknown unit 'ROLL'"]]]
            and units_of(it) == {"RCE-0701": "kg", "OIL-0702": "ml", "PEA-0703": "ea", "PEN-0705": "ea", "INK-0706": "l",
                                 "PAP-0707": "kg", "BXS-0709": "ea", "FOI-0710": "kg", "TAP-0711": "l"}
            and deq(it["FOI-0710"]["quantity"], 3) and deq(it["FOI-0710"]["unit_cost"], "4.00")
            and deq(it["TAP-0711"]["quantity"], "1.5") and deq(it["INK-0706"]["quantity"], "0.5"))


@item("report.item_rows", S('''
    OUT["loc"] = cli("--store", "st", "report", "by-location", "--json")["json"]
    OUT["loc_g"] = cli("--store", "st", "report", "by-location", "--json", "--unit", "g")["json"]
    OUT["slow"] = cli("--store", "st", "report", "slow-movers", "--json")["json"]
    OUT["slow_oz"] = api("st", "GET", "/reports/slow-movers?unit=OZ")["json"]
    OUT["val"] = cli("--store", "st", "report", "valuation", "--json", "--unit", "nonsense")["code"]
'''))
def _(o):
    def loc(rows):
        return [(r["location"], r["sku"], r["quantity"], r["unit"]) for r in rows]
    def slow(rows):
        return [(r["sku"], r["quantity"], r["unit"], r["value"]) for r in rows]
    lp, lg, sp, so = o["loc"], o["loc_g"], o["slow"], o["slow_oz"]
    want_l = [("A1", "BOL-0001", 120, "ea"), ("C1", "MLK-0003", 6, "l"), ("C3", "CHK-0002", 2.5, "kg"),
              ("C3", "LNT-0007", 1.25, "kg"), ("C3", "PEP-0004", 250, "g"), ("C3", "SLT-0006", 3, "lb")]
    want_g = [("A1", "BOL-0001", 120, "ea"), ("C1", "MLK-0003", 6, "l"), ("C3", "CHK-0002", 2500, "g"),
              ("C3", "LNT-0007", 1250, "g"), ("C3", "PEP-0004", 250, "g"), ("C3", "SLT-0006", 1360.777, "g")]
    want_s = [("BOL-0001", 120, "ea", "14.40"), ("MLK-0003", 6, "l", "11.70"), ("RIB-0005", 1.5, "dz", "7.20"),
              ("SLT-0006", 3, "lb", "6.00"), ("LNT-0007", 1.25, "kg", "3.50")]
    want_o = [("BOL-0001", 120, "ea", "14.40"), ("MLK-0003", 6, "l", "11.70"), ("RIB-0005", 1.5, "dz", "7.20"),
              ("SLT-0006", 48, "oz", "6.00"), ("LNT-0007", 44.092, "oz", "3.50")]
    def same(got, want):
        return len(got) == len(want) and all(g[0] == w[0] and g[1] == w[1] and deq(g[2], w[2]) and g[3] == w[3]
                                             for g, w in zip(got, want))
    def same_s(got, want):
        return len(got) == len(want) and all(g[0] == w[0] and deq(g[1], w[1]) and g[2:] == w[2:] for g, w in zip(got, want))
    return (same(loc(lp["rows"]), want_l) and same(loc(lg["rows"]), want_g) and lp["unit"] is None and lg["unit"] == "g"
            and list(lp["rows"][0]) == ["location", "sku", "name", "quantity", "unit"]
            and same_s(slow(sp["rows"]), want_s) and same_s(slow(so["rows"]), want_o) and so["unit"] == "oz"
            and list(sp["rows"][0]) == ["sku", "name", "quantity", "unit", "value"] and o["val"] == 0)


@item("report.inbound", S('''
    with open("st/movements.csv", "a") as f:
        f.write("8,PEP-0004,200,g,import,feed.csv\\n9,OLD-0099,1,kg,receive,\\n10,OLD-0099,250,g,import,\\n"
                "11,BOL-0001,2,kg,receive,\\n12,OLD-0099,-1,l,issue,\\n")
    OUT["plain"] = cli("--store", "st", "report", "inbound", "--json")["json"]
    OUT["kg"] = api("st", "GET", "/reports/inbound?unit=KG")["json"]
'''))
def _(o):
    def rows(r):
        return [(x["sku"], x["receipts"], x["quantity"], x["unit"]) for x in r["rows"]]
    want_p = [("BOL-0001", 1, 120, "ea"), ("CHK-0002", 1, 3, "kg"), ("LNT-0007", 1, 1.25, "kg"), ("OLD-0099", 2, 1250, "g"),
              ("PEP-0004", 2, 500, "g"), ("SLT-0006", 1, 3, "lb")]
    want_k = [("BOL-0001", 1, 120, "ea"), ("CHK-0002", 1, 3, "kg"), ("LNT-0007", 1, 1.25, "kg"), ("OLD-0099", 2, 1.25, "kg"),
              ("PEP-0004", 2, 0.5, "kg"), ("SLT-0006", 1, 1.361, "kg")]
    def same(got, want):
        return len(got) == len(want) and all(g[0] == w[0] and g[1] == w[1] and deq(g[2], w[2]) and g[3] == w[3]
                                             for g, w in zip(got, want))
    return (same(rows(o["plain"]), want_p) and same(rows(o["kg"]), want_k) and o["kg"]["unit"] == "kg"
            and list(o["plain"]["rows"][0]) == ["sku", "receipts", "quantity", "unit"])


@item("export.text_formats", S('''
    OUT["labels"] = cli("--store", "st", "export", "-", "--format", "labels", "--category", "packing")["out"]
    OUT["labels_all"] = cli("--store", "st", "export", "-", "--format", "labels")["out"]
    OUT["md_code"] = cli("--store", "st", "export", "items.md")["code"]
    OUT["md"] = read("items.md")
    OUT["md_kg"] = cli("--store", "st", "export", "-", "--format", "markdown", "--unit", "kg", "--category", "food")["out"]
'''))
def _(o):
    md = ("| SKU | Name | Qty | Location |\n|---|---|---|---|\n| BOL-0001 | Hex bolt | 120 ea | A1 |\n"
          "| CHK-0002 | Chickpeas | 2.5 kg | C3 |\n| LNT-0007 | Lentils | 1.25 kg | C3 |\n| MLK-0003 | Oat milk | 6 l | C1 |\n"
          "| PEP-0004 | Peppercorn | 250 g | C3 |\n| RIB-0005 | Ribbon | 1.5 dz | - |\n| SLT-0006 | Sea salt | 3 lb | C3 |\n")
    md_kg = ("| SKU | Name | Qty | Location |\n|---|---|---|---|\n| CHK-0002 | Chickpeas | 2.5 kg | C3 |\n"
             "| LNT-0007 | Lentils | 1.25 kg | C3 |\n| MLK-0003 | Oat milk | 6 l | C1 |\n| PEP-0004 | Peppercorn | 0.25 kg | C3 |\n"
             "| SLT-0006 | Sea salt | 1.361 kg | C3 |\n")
    blocks = o["labels_all"].split("\n\n")
    return (o["labels"] == "[ RIB-0005 ] Ribbon\nqty: 1.5 dz   loc: -\n" and len(blocks) == 7
            and blocks[4] == "[ PEP-0004 ] Peppercorn\nqty: 250 g   loc: C3"
            and o["md_code"] == 0 and o["md"] == md and o["md_kg"] == md_kg)


@item("export.shop_snapshot", S('''
    OUT["shop"] = cli("--store", "st", "export", "-", "--format", "shop", "--category", "food")["out"]
    OUT["snap"] = cli("--store", "st", "export", "-", "--format", "snapshot")["out"]
    OUT["api"] = api("st", "GET", "/export?format=snapshot&category=packing")["body"]
'''))
def _(o):
    shop = ("Handle,Title,Variant SKU,Variant Inventory Qty,unit,Variant Price\nchk-0002,Chickpeas,CHK-0002,2.5,kg,3.20\n"
            "lnt-0007,Lentils,LNT-0007,1.25,kg,2.80\nmlk-0003,Oat milk,MLK-0003,6,l,1.95\npep-0004,Peppercorn,PEP-0004,250,g,0.04\n"
            "slt-0006,Sea salt,SLT-0006,3,lb,2.00\n")
    snap = ("sku,name,quantity,unit,reorder_level,value\nBOL-0001,Hex bolt,120,ea,50,14.40\nCHK-0002,Chickpeas,2.5,kg,5,8.00\n"
            "LNT-0007,Lentils,1.25,kg,2,3.50\nMLK-0003,Oat milk,6,l,8,11.70\nPEP-0004,Peppercorn,250,g,100,10.00\n"
            "RIB-0005,Ribbon,1.5,dz,1,7.20\nSLT-0006,Sea salt,3,lb,1,6.00\n")
    return (o["shop"] == shop and o["snap"] == snap
            and o["api"] == "sku,name,quantity,unit,reorder_level,value\nRIB-0005,Ribbon,1.5,dz,1,7.20\n")


@item("api.legacy_v1", S('''
    OUT["get"] = api("st", "GET", "/v1/stock/CHK-0002")["json"]
    calls = [("/v1/stock/PEP-0004/scan", {"delta": -0.05, "unit": "kg"}), ("/v1/stock/PEP-0004/scan", {"delta": 2, "unit": "l"}),
             ("/v1/stock/RIB-0005/scan", {"delta": 1}), ("/v1/stock/RIB-0005/scan", {"delta": "0.1"}),
             ("/v1/stock/BOL-0001/scan", {"delta": -6, "unit": "dz"})]
    OUT["rs"] = [[r["status"], r["json"]] for r in (api("st", "POST", p, b) for p, b in calls)]
    OUT["items"] = items("st")
    OUT["last"] = moves("st")[-1]
'''))
def _(o):
    rs, it = o["rs"], o["items"]
    return (o["get"] == {"sku": "CHK-0002", "qty": 2.5, "unit": "kg", "location": "C3"} and list(o["get"]) == ["sku", "qty", "unit", "location"]
            and rs[0] == [200, {"sku": "PEP-0004", "qty": 200, "unit": "g", "location": "C3"}]
            and rs[1] == [422, {"error": "cannot convert l to g"}]
            and rs[2][0] == 200 and deq(rs[2][1]["qty"], "2.5") and rs[2][1]["unit"] == "dz"
            and rs[3] == [422, {"error": "invalid item", "problems": ["RIB-0005 quantity: must be a whole number of ea"]}]
            and rs[4][0] == 200 and deq(rs[4][1]["qty"], 48)
            and deq(it["PEP-0004"]["quantity"], 200) and deq(it["RIB-0005"]["quantity"], "2.5")
            and o["last"]["note"] == "scanner" and deq(o["last"]["delta"], -72) and o["last"]["unit"] == "ea")


# ------------------------------------------------------------------------------------------------- generated families

with open(os.path.join(os.environ["INIT_CHECK_DIR"], "hidden", "partners.json"), encoding="utf-8") as _f:
    PARTNERS = json.load(_f)
VENDORS = {v["name"]: v for v in PARTNERS["vendors"]}
MARKETS = {m["name"]: m for m in PARTNERS["markets"]}
# the items of fixture S as text: sku, name, category, quantity, unit, reorder, location, cost
S_ROWS = [line.split(",") for line in S_CSV.splitlines()[1:]]
FACT = {"ea": ("count", Decimal(1)), "dz": ("count", Decimal(12)), "g": ("mass", Decimal(1)),
        "kg": ("mass", Decimal(1000)), "lb": ("mass", Decimal("453.59237")), "oz": ("mass", Decimal("28.349523125")),
        "ml": ("volume", Decimal(1)), "l": ("volume", Decimal(1000))}


def conv(q, a, b):
    from decimal import ROUND_HALF_UP
    if a == b:
        return Decimal(q)
    return (Decimal(q) * FACT[a][1] / FACT[b][1]).quantize(Decimal("0.001"), rounding=ROUND_HALF_UP)


def qtext(d):
    s = format(Decimal(d).quantize(Decimal("0.001")), "f").rstrip("0").rstrip(".")
    return s or "0"


def vendor_feed(v):
    code = v["name"][:3].upper()
    skus = ["%s-%04d" % (code, n) for n in (1, 2, 3, 4)]
    dc, d, hdr, style = v["decimal_comma"], v["delimiter"], v["header"], v["style"]

    def num(s):
        return s.replace(".", ",") if dc else s
    rows = [(skus[0], "Alpha", ("5", "0.5") if style == "packs" else "2.5", "KGM", "3.20"),
            (skus[1], "Bravo", ("3", "4") if style == "packs" else "12", "C62", "0.50"),
            (skus[2], "Charlie", ("1", "750") if style == "packs" else "750", "mlt", "0.01"),
            (skus[3], "Delta", ("1", "1") if style == "packs" else "1", "XYZ", "1.00")]
    order = ["sku", "name", "qty"] + (["size"] if style == "packs" else []) + ([] if style == "cell" else ["uom"]) + ["cost"]
    lines = [d.join(hdr[k] for k in order)]
    for sku, name, qty, uom, cost in rows:
        cells = {"sku": v["prefix"] + sku, "name": name, "cost": num(cost), "uom": uom}
        if style == "packs":
            cells["qty"], cells["size"] = num(qty[0]), num(qty[1])
        elif style == "cell":
            cells["qty"] = num(qty) + " " + uom
        else:
            cells["qty"] = num(qty)
        lines.append(d.join(cells[k] for k in order))
    packs = style == "packs"
    want = {skus[0]: ("kg", "2.5", "6.40" if packs else "3.20"), skus[1]: ("ea", "12", "0.13" if packs else "0.50"),
            skus[2]: ("ml", "750", "0.00" if packs else "0.01")}
    return "\n".join(lines) + "\n", want


def vendor_item(names):
    feeds = {n: vendor_feed(VENDORS[n])[0] for n in names}
    snippet = scen('''
        cli("--store", "st", "init")
        OUT["rs"] = {}
        for name in sorted(FEEDS):
            write(name + ".feed", FEEDS[name])
            r = cli("--store", "st", "import", name + ".feed", "--format", "vendor-" + name)
            OUT["rs"][name] = [r["code"], [l for l in r["err"].splitlines() if l.strip()]]
        OUT["items"] = items("st")
    ''', FEEDS=feeds)

    def pred(o):
        it = o["items"]
        want_all = {}
        for n in names:
            if o["rs"].get(n) != [1, ["%s.feed:5: unknown unit 'XYZ'" % n]]:
                return False
            want_all.update(vendor_feed(VENDORS[n])[1])
        if sorted(it) != sorted(want_all):
            return False
        return all(it[s]["unit"] == u and deq(it[s]["quantity"], q) and deq(it[s]["unit_cost"], c)
                   for s, (u, q, c) in want_all.items())
    return snippet, pred


for _name, _vendors in (("vendor.generated_a", ["kestrel", "lark", "maple", "nimbus", "orchid"]),
                        ("vendor.generated_b", ["raven", "sierra", "tundra", "vale", "willow"]),
                        ("vendor.generated_c", ["xenia", "yarrow", "alder", "ember", "gale", "heron"])):
    _snip, _pred = vendor_item(_vendors)
    ITEMS.append((_name, _snip, _pred))


def market_expected(m, unit_after=True):
    import csv as _csv
    import io as _io
    rows = [dict(zip(["sku", "name", "category", "quantity", "unit", "reorder", "location", "price"], r)) for r in S_ROWS]
    if m["kind"] == "text":
        out = []
        for r in rows:
            out.append(m["template"].format(sku=r["sku"], name=r["name"], quantity=r["quantity"] + " " + r["unit"],
                                            price=r["price"], location=r["location"] or "-"))
        return "\n".join(out) + "\n"
    headers, fields = list(m["headers"]), list(m["fields"])
    q = fields.index("quantity")
    headers.insert(q + 1, "UNIT" if m["upper"] else "unit")
    fields.insert(q + 1, "unit")
    buf = _io.StringIO()
    w = _csv.writer(buf, delimiter=m["delimiter"], lineterminator="\n")
    w.writerow(headers)
    for r in rows:
        w.writerow([r[f] for f in fields])
    return buf.getvalue()


def market_item(names):
    snippet = S('''
        OUT["out"] = {}
        for name in NAMES:
            r = cli("--store", "st", "export", "-", "--format", "mkt-" + name)
            OUT["out"][name] = [r["code"], r["out"]]
    ''').replace("S_ITEMS = ", "NAMES = %r\nS_ITEMS = " % (names,), 1)

    def pred(o):
        return all(o["out"].get(n) == [0, market_expected(MARKETS[n])] for n in names)
    return snippet, pred


for _name, _markets in (("export.markets_a", ["bazaar", "fairground", "gallery", "kiosk"]),
                        ("export.markets_b", ["emporium", "ledger-shop", "market-row", "jetty"])):
    _snip, _pred = market_item(_markets)
    ITEMS.append((_name, _snip, _pred))

GROUP_KEYS = {
    "aisle": lambda r: r["location"][:1] or "-",
    "family": lambda r: r["sku"][:3],
    "status": lambda r: "low" if Decimal(r["quantity"]) <= Decimal(r["reorder"]) else "ok",
    "cost_band": lambda r: "under 1" if Decimal(r["price"]) < 1 else ("1-10" if Decimal(r["price"]) < 10 else "10+"),
}


def group_expected(column, key, unit):
    rows = [dict(zip(["sku", "name", "category", "quantity", "unit", "reorder", "location", "price"], r)) for r in S_ROWS]
    groups, skipped = {}, 0
    for r in rows:
        value = (Decimal(r["quantity"]) * Decimal(r["price"])).quantize(Decimal("0.01"))
        if unit is None:
            g, q = (GROUP_KEYS[key](r), r["unit"]), Decimal(r["quantity"])
        elif FACT[r["unit"]][0] == FACT[unit][0]:
            g, q = (GROUP_KEYS[key](r), unit), conv(r["quantity"], r["unit"], unit)
        else:
            skipped += 1
            continue
        e = groups.setdefault(g, [0, Decimal(0), Decimal(0)])
        e[0] += 1
        e[1] += q
        e[2] += value
    out = [{column: g[0], "items": e[0], "quantity": qtext(e[1]), "unit": g[1], "value": "%.2f" % e[2]}
           for g, e in sorted(groups.items())]
    return out, skipped


def group_rows_ok(got, column, key, unit):
    want, skipped = group_expected(column, key, unit)
    if not isinstance(got, dict) or got.get("unit") != unit or got.get("skipped") != skipped:
        return False
    rows = got.get("rows")
    if not isinstance(rows, list) or len(rows) != len(want):
        return False
    for r, w in zip(rows, want):
        if list(r) != [column, "items", "quantity", "unit", "value"]:
            return False
        if r[column] != w[column] or r["items"] != w["items"] or not deq(r["quantity"], w["quantity"]) \
                or r["unit"] != w["unit"] or r["value"] != w["value"]:
            return False
    return True


@item("report.grouped", S('''
    OUT["aisle"] = cli("--store", "st", "report", "by-aisle", "--json")["json"]
    OUT["aisle_g"] = cli("--store", "st", "report", "by-aisle", "--json", "--unit", "g")["json"]
    OUT["family_ea"] = api("st", "GET", "/reports/by-family?unit=ea")["json"]
    OUT["status"] = cli("--store", "st", "report", "by-status", "--json")["json"]
    OUT["band_l"] = cli("--store", "st", "report", "by-cost-band", "--json", "--unit", "L")["json"]
'''))
def _(o):
    return (group_rows_ok(o["aisle"], "aisle", "aisle", None) and group_rows_ok(o["aisle_g"], "aisle", "aisle", "g")
            and group_rows_ok(o["family_ea"], "family", "family", "ea") and group_rows_ok(o["status"], "status", "status", None)
            and group_rows_ok(o["band_l"], "band", "cost_band", "l"))


# ------------------------------------------------------------------------------------------------- regression (1.6)

@item("regress.cli_flow", scen('''
    s = "st"
    rs = [cli("--store", s, "init"),
          cli("--store", s, "add", "BOL-1601", "Carriage bolt", "--category", "fasteners", "--qty", "40",
              "--reorder", "10", "--location", "a3", "--cost", "0.25"),
          cli("--store", s, "add", "WSH-1602", "Washer", "--qty", "100", "--location", "A3"),
          cli("--store", s, "receive", "BOL-1601", "15", "--note", "PO 77"),
          cli("--store", s, "issue", "WSH-1602", "30"),
          cli("--store", s, "issue", "BOL-1601", "100"),
          cli("--store", s, "adjust", "WSH-1602", "68"),
          cli("--store", s, "adjust", "WSH-1602", "68")]
    OUT["codes"] = [r["code"] for r in rs]
    OUT["err5"] = rs[5]["err"]
    OUT["show"] = cli("--store", s, "show", "BOL-1601")["json"]
    OUT["hist"] = cli("--store", s, "history", "WSH-1602", "--json")["json"]
    OUT["val"] = cli("--store", s, "report", "valuation", "--json")["json"]
    OUT["list"] = cli("--store", s, "list", "--json")["json"]
    OUT["missing"] = cli("--store", s, "show", "NOP-0001")
'''))
def _(o):
    sh, h, v = o["show"], o["hist"], o["val"]
    return (o["codes"] == [0, 0, 0, 0, 0, 2, 0, 0]
            and o["err5"] == "error: insufficient stock for BOL-1601: have 55, need 100\n"
            and sh["sku"] == "BOL-1601" and sh["quantity"] == 55 and sh["reorder_level"] == 10 and sh["location"] == "A3"
            and sh["unit_cost"] == "0.25" and sh["value"] == "13.75" and sh["category"] == "fasteners"
            and [m["delta"] for m in h] == [100, -30, -2] and [m["reason"] for m in h] == ["receive", "issue", "adjust"]
            and [m["note"] for m in h] == ["initial stock", "", ""] and [m["id"] for m in h] == [2, 4, 5]
            and v["rows"] == [{"location": "A3", "items": 2, "value": "13.75"}] and v["total"] == "13.75"
            and [x["sku"] for x in o["list"]] == ["BOL-1601", "WSH-1602"]
            and o["missing"]["code"] == 2 and o["missing"]["err"] == "error: unknown sku: NOP-0001\n")


@item("regress.validation", scen('''
    write("st/items.csv", "#stockroom-items v1\\nsku,name,category,quantity,reorder_level,location,unit_cost\\n"
          "AB-12,Short sku,parts,1,0,A1,1.00\\nBLK-0002, ,parts,1,0,A1,1.00\\nCAT-0003,Cat,Parts,1,0,A1,1.00\\n"
          "NEG-0004,Negative,parts,-3,0,A1,1.00\\nLOC-0005,Bad loc,parts,1,0,AA1,1.00\\nOK-00006x,x,parts,1,0,,1.00\\n")
    write("st/movements.csv", "#stockroom-movements v1\\nid,sku,delta,reason,note\\n1,NEG-0004,1,lost,\\n2,NEG-0004,0,adjust,\\n")
    r = cli("--store", "st", "validate")
    OUT["r"] = [r["code"], [l for l in r["out"].splitlines() if l.strip()]]
    r = cli("--store", "st", "add", "abc-12", "Thing")
    OUT["add"] = [r["code"], r["err"]]
    r = cli("--store", "st", "add", "GOOD-0007", "Thing", "--qty", "-1")
    OUT["add2"] = [r["code"], r["err"]]
'''))
def _(o):
    want = {"AB-12 sku: must look like ABC-1234", "BLK-0002 name: must not be blank",
            "CAT-0003 category: must be lower-case and not blank", "NEG-0004 quantity: must not be negative",
            "LOC-0005 location: must look like A1 or B12", "OK-00006X sku: must look like ABC-1234",
            "movement 1 reason: unknown reason 'lost'", "movement 2 delta: must not be zero"}
    return (o["r"][0] == 1 and set(o["r"][1]) == want and len(o["r"][1]) == len(want)
            and o["add"] == [1, "invalid: ABC-12 sku: must look like ABC-1234\n"]
            and o["add2"][0] == 1 and "invalid: GOOD-0007 sku: must look like ABC-1234" in o["add2"][1]
            and "invalid: GOOD-0007 quantity: must not be negative" in o["add2"][1])


@item("regress.import_errors", scen('''
    import json
    cli("--store", "st", "init")
    write("a.csv", "sku,name\\nAAA-0001,Apple\\n")
    write("b.csv", "sku,name,qty\\nBBB-0001,Bee,x\\nbad,Bad,1\\nBBB-0002,Bee two,7\\n\\nBBB-0003,,2\\n")
    write("c.json", json.dumps([{"name": "No sku", "quantity": 1}, {"sku": "CCC-0001", "name": "Cee"},
                                {"sku": "CCC-0002", "name": "Cee two", "qty": "4"}, 5]))
    write("d.ledger", "stray = 1\\n[DDD-0001]\\nname = Dee\\nqty = 3\\ncolour = red\\n\\n[DDD-0002]\\nname = Dee two\\n"
                      "\\n[DDD-0003]\\nname = Dee three\\nqty = 6\\nreorder = 2\\n\\n[ddd-0004]\\nname = Dee four\\nqty 5\\n")
    rs = [cli("--store", "st", "import", f) for f in ("a.csv", "b.csv", "c.json", "d.ledger")]
    OUT["rs"] = [[r["code"], [l for l in r["err"].splitlines() if l.strip()], r["out"].strip()] for r in rs]
    OUT["skus"] = sorted(items("st"))
    OUT["q"] = items("st")["DDD-0003"]["reorder_level"]
'''))
def _(o):
    rs = o["rs"]
    return (rs[0][:2] == [1, ["a.csv:1: missing column(s): quantity"]]
            and rs[1][:2] == [1, ["b.csv:2: bad quantity 'x'", "b.csv:3: bad sku 'bad'", "b.csv:6: name: must not be blank"]]
            and rs[1][2] == "imported 1 item, 3 errors"
            and rs[2][:2] == [1, ["c.json:1: missing sku", "c.json:2: missing quantity", "c.json:4: expected an object"]]
            and rs[3][:2] == [1, ["d.ledger:1: data outside a [SKU] section", "d.ledger:5: unknown key 'colour'",
                                  "d.ledger:7: missing qty", "d.ledger:17: expected key = value"]]
            and o["skus"] == ["BBB-0002", "CCC-0002", "DDD-0003"] and deq(o["q"], 2))


@item("regress.api_errors", scen('''
    write("st/items.csv", V1_ITEMS)
    write("st/movements.csv", V1_MOVES)
    calls = [("GET", "/items/NOP-0000", None), ("GET", "/nothing", None), ("DELETE", "/items", None),
             ("POST", "/items/OIL-0002/issue", "{not json"), ("POST", "/items/OIL-0002/issue", {"qty": 10}),
             ("PATCH", "/items/OIL-0002", {"quantity": 5}), ("GET", "/reports/nope", None),
             ("POST", "/items", {"sku": "NUT-0001", "name": "Dup", "quantity": 1}),
             ("POST", "/items/OIL-0002/receive", {"qty": "abc"}),
             ("GET", "/items?q=nut", None), ("PATCH", "/items/nut-0001", {"location": "c4", "name": "  Hex   nut M8 "})]
    OUT["rs"] = [[r["status"], r["json"]] for r in (api("st", m, p, b) for m, p, b in calls)]
''', V1_ITEMS=V1_ITEMS, V1_MOVES=V1_MOVES))
def _(o):
    rs = o["rs"]
    return ([r[0] for r in rs] == [404, 404, 405, 400, 409, 400, 404, 409, 400, 200, 200]
            and rs[0][1] == {"error": "unknown sku: NOP-0000"} and rs[1][1] == {"error": "no route for GET /nothing"}
            and rs[3][1] == {"error": "body is not valid JSON"}
            and rs[4][1] == {"error": "insufficient stock for OIL-0002: have 3.5, need 10"}
            and rs[5][1] == {"error": "cannot patch field(s): quantity"} and rs[6][1] == {"error": "unknown report: nope"}
            and rs[7][1] == {"error": "duplicate sku: NUT-0001"} and rs[8][1] == {"error": "bad quantity 'abc'"}
            and [x["sku"] for x in rs[9][1]["items"]] == ["NUT-0001"] and rs[9][1]["total"] == 1
            and rs[10][1]["location"] == "C4" and rs[10][1]["name"] == "Hex nut M8")


@item("regress.pipeline_reports", scen('''
    import csv, io, json
    write("st/items.csv", V1_ITEMS.replace("OIL-0002,Gear oil,fluids,3.5,1,B2,7.90", "OIL-0002,Gear oil,fluids,1,1,B2,7.90"))
    write("st/movements.csv", V1_MOVES)
    write("in.csv", "sku,name,category,qty,loc,cost\\nPEN-1701,Pen,office,12,a1,0.50\\nCLP-1702,Clip,office,0,A2,0.02\\n"
                    "MUG-1703,Mug,kitchen,6,B1,3.00\\nINK-1704,Ink,office,3,A1,7.25\\n")
    write("p.json", json.dumps({"input": {"path": "in.csv"},
                                "steps": [{"step": "where", "field": "category", "in": ["office"]},
                                          {"step": "drop_zero"}, {"step": "relocate", "from": "A1", "to": "D9"},
                                          {"step": "sort", "by": "name"}],
                                "output": {"format": "csv", "path": "out.csv"}}))
    r = cli("--store", "st", "run", "p.json")
    OUT["code"] = [r["code"], r["err"].strip()]
    OUT["rows"] = list(csv.DictReader(io.StringIO(read("out.csv"))))
    OUT["tot"] = cli("--store", "st", "report", "totals", "--json")["json"]
    OUT["low"] = cli("--store", "st", "report", "low-stock", "--json")["json"]
    OUT["val"] = cli("--store", "st", "report", "valuation", "--json")["json"]
''', V1_ITEMS=V1_ITEMS, V1_MOVES=V1_MOVES))
def _(o):
    rows = [{k: r[k] for k in ("sku", "name", "category", "quantity", "location", "unit_cost")} for r in o["rows"]]
    tot = [{k: r[k] for k in ("category", "items", "quantity", "value")} for r in o["tot"]["rows"]]
    low = [{k: r[k] for k in ("sku", "location", "quantity", "reorder_level", "shortfall")} for r in o["low"]["rows"]]
    return (o["code"] == [0, "pipeline: 4 in, 2 out, 0 errors"]
            and rows == [{"sku": "INK-1704", "name": "Ink", "category": "office", "quantity": "3", "location": "D9", "unit_cost": "7.25"},
                         {"sku": "PEN-1701", "name": "Pen", "category": "office", "quantity": "12", "location": "D9", "unit_cost": "0.50"}]
            and tot == [{"category": "fasteners", "items": 1, "quantity": 250, "value": "12.50"},
                        {"category": "fluids", "items": 1, "quantity": 1, "value": "7.90"}]
            and low == [{"sku": "OIL-0002", "location": "B2", "quantity": 1, "reorder_level": 1, "shortfall": 0}]
            and o["val"]["rows"] == [{"location": "A1", "items": 1, "value": "12.50"}, {"location": "B2", "items": 1, "value": "7.90"}]
            and o["val"]["total"] == "20.40")




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

    threads = [threading.Thread(target=worker) for _ in range(6)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    for name, _, _ in ITEMS:
        print("%s %s" % ("PASS" if results.get(name) else "FAIL", name))


main()
