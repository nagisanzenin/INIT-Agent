#!/bin/bash
# Qualify a live provider on your own INIT install (v0.5.0 L3): one short scripted session inside a Root-metered budget,
# then a pass/fail table. COSTS MONEY: at most the cap you give (a run is about $0.10-0.40 of real provider spend on
# Sonnet 5.5 or GPT-6.1 Sol; the ledger also books a reservation for the request the pause cuts, see "cost" below).
#
#   scripts/qualify-provider.sh --cap-usd N [--provider P --model M] [--price IN,CACHED,WRITE,OUT] [--out DIR] [--yes]
#
#   --cap-usd N   required, 0 < N <= 10 (USD). The session runs inside `/goal "…" usd=N`: the Root meters every request
#                 against it and stops the goal at N. This script also stops the session (a pause, then /goal stop)
#                 when the Root's settled spend since the start reaches N, and after 16 model requests. Give at least
#                 1 (2 for Opus 5.5 or GPT-6 Astra): a request cut by the pause with no usage yet (OpenAI) is booked at
#                 its reservation, about $0.35 on GPT-6.1 Sol, although the provider bills only what it generated.
#   --provider P --model M   what you expect to be selected; the script refuses if `init-root model use` chose
#                 something else (it never changes your selection)
#   --price …     the provider's list price in USD per 1M tokens (input, cached input, cache write, output), when the
#                 built-in table below is out of date or the model is not in it
#   --out DIR     where the verdict and the logs go (default ~/init-qualify/<provider>-<model>-<UTC time>)
#   --yes         do not ask before spending
#
# Before you run it, from your own shell (never through the agent):
#   1. set a spend limit at the provider (console.anthropic.com / platform.openai.com) as the server-side bound;
#   2. INIT_ROOT_HOME=~/.init-root ~/.init-root/bin/init-root keys set anthropic      (or openai; hidden input)
#   3. ~/.init-root/bin/init-root model use anthropic/claude-sonnet-5-5               (or openai/gpt-6.1-sol)
#   4. no goal of yours active (`/goal pause` or `/goal stop` in the app).
# This script never reads, prints or copies a key: it asks the Root whether one is present (`providers list --json`).
# It talks to the running Root through the installed `init-agent --raw` (the owner's client), as a person typing would.
#
# The session (owner lines; the agent's normal tools; no self-change is asked for):
#   1. streaming: a plain-text reply with no tool, which must arrive as several deltas;
#   2. tool round trip: run `printf %s qualify-<random> | sha256sum` with bash and reply with the first 16 hex digits;
#      the model cannot compute SHA-256 itself, so the right digits prove call → INIT ran it → result → model;
#   3. pause: a long reply, interrupted after its first text (SIGINT to the client = Esc in the app): the app must say
#      "paused" and go idle within 5 s; then 4. resume: "Please continue …" must be answered with no provider error.
# Checks, from the Root's egress ledger (ROOT_HOME/egress/usage/*.jsonl; usage and cost, no content, no keys) and the
# body's session log (INIT_HOME/state/sessions/*.jsonl):
#   cache     the session's second model request reports cache read > 0 (Root ledger);
#   tokens    every completed request's input, cache read, cache write and output are the same in the ledger (the Root's tap) and in
#             the session log (the body's decoder): two parsers of the same provider bytes agree;
#   cost      the ledger's cost of the completed requests is within 5 % of the provider-reported usage priced at the
#             provider's list price (cache writes at the write price). Requests the pause cut are listed, not compared
#             (the Root charges what it saw or, with no usage, the reservation). The Root bills cache writes at the
#             charter's write prices (`providers add` writes the catalog's); a charter row without them bills writes at
#             2× input, which this check then reports as an over-count;
#   budget    the goal was set with usd=N and the spend stayed under N.
# Also compare with the provider console's usage for the run's hour; the table prints the window.
# Output: a table on stdout, DIR/verdict.json, DIR/frames.jsonl (the client's frames), DIR/ledger.jsonl (the run's
# ledger records). Exit 0: every check passed; 1: a check failed; 3: stopped at the cap; 2: refused (nothing spent);
# 64: usage.
set -euo pipefail
umask 077

CAP=""; PROV=""; MODEL=""; PRICE=""; OUT=""; YES=0
usage() { sed -n '2,15p' "$0" >&2; exit 64; }
while [ $# -gt 0 ]; do
  case "$1" in
    --cap-usd) CAP="${2:-}"; shift 2 || usage ;;
    --provider) PROV="${2:-}"; shift 2 || usage ;;
    --model) MODEL="${2:-}"; shift 2 || usage ;;
    --price) PRICE="${2:-}"; shift 2 || usage ;;
    --out) OUT="${2:-}"; shift 2 || usage ;;
    --yes) YES=1; shift ;;
    -h|--help) sed -n '2,50p' "$0"; exit 0 ;;
    *) echo "qualify-provider.sh: unknown argument $1" >&2; usage ;;
  esac
done
refuse() { printf 'qualify-provider.sh: refused: %s\n' "$*" >&2; exit "${RC:-2}"; }

# ---------------------------------------------------------------- the cap (checked before anything else)
[ -n "$CAP" ] || RC=64 refuse "--cap-usd <n> is required (0 < n <= 10 USD): the session runs inside a Root-metered budget of n"
CAP_MUSD="$(/usr/bin/python3 -I -c '
import re, sys
s = sys.argv[1]
if not re.fullmatch(r"[0-9]{1,2}(\.[0-9]{1,6})?", s): sys.exit(1)
whole, _, frac = s.partition(".")
m = int(whole) * 1_000_000 + int((frac + "000000")[:6])
sys.exit(1) if not 0 < m <= 10_000_000 else print(m)' "$CAP")" \
  || RC=64 refuse "--cap-usd $CAP: give a decimal number of USD, more than 0 and at most 10 (for example 1 or 0.50)"
echo "cap: \$$CAP (Root-metered: the goal's usd= budget, and this script stops at the Root's spend since the start)"

# ---------------------------------------------------------------- the install and the selection
RH="${INIT_ROOT_HOME:-$HOME/.init-root}"
ROOT_BIN="$RH/bin/init-root"
[ -x "$ROOT_BIN" ] || refuse "no $ROOT_BIN: is INIT installed? (INIT_ROOT_HOME selects another Root home)"
read -r IH BIN_DIR <<<"$(/usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("init_home",""), d.get("bin_dir",""))' "$RH/install.json")"
[ -d "$IH" ] || refuse "$RH/install.json names no INIT_HOME"
RENV=(env -i HOME="$HOME" INIT_ROOT_HOME="$RH" PATH=/usr/bin:/bin LANG=en_US.UTF-8)
"${RENV[@]}" "$ROOT_BIN" status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("slot")=="running" else 1)' \
  || refuse "the Root is not running with its kernel and body; start INIT first (init-agent, or init-root start)"
SEL="$("${RENV[@]}" "$ROOT_BIN" providers list --json)" || refuse "init-root providers list --json failed"
read -r SP SM KEY TESTED <<<"$(/usr/bin/python3 -I -c '
import json, sys
d = json.loads(sys.argv[1]); s = d.get("selected") or {}
m = next((x for x in d.get("models", []) if x.get("selected")), {})
print(s.get("provider") or "-", s.get("model") or "-", m.get("key") or "-", m.get("tested") or "-")' "$SEL")"
if [ -n "$PROV$MODEL" ] && [ "${PROV:-$SP}/${MODEL:-$SM}" != "$SP/$SM" ]; then
  refuse "the selected model is $SP/$SM, not ${PROV:-$SP}/${MODEL:-$SM}. Select it yourself first: $ROOT_BIN model use ${PROV:-$SP}/${MODEL:-$SM}"
fi
case "$KEY" in present|not_required) ;; *) refuse "no key for $SP (state: $KEY). Enter it yourself, hidden: INIT_ROOT_HOME=$RH $ROOT_BIN keys set $SP" ;; esac

# ---------------------------------------------------------------- the reference price (µUSD per 1M tokens)
# The providers' list prices, standard tier, from reports/INIT v1 providers UX vision.md Table A (read 2026-10-20):
# input, cached input, cache write (5-minute TTL for Anthropic; OpenAI's one write class), output. DeepSeek at its peak
# price, which the Root always bills. This is the reference the ledger is compared with, kept apart from the Root's
# own catalog on purpose. Above 272K input tokens OpenAI reprices the whole request (4/0.20/15 for gpt-6.1-sol): the
# script flags such a request instead of pricing it.
case "$SP/$SM" in
  anthropic/claude-sonnet-5-5) REF="2000000 100000 2500000 10000000" ;;
  anthropic/claude-opus-5-5)   REF="4000000 200000 5000000 20000000" ;;
  openai/gpt-6.1-sol)          REF="2000000 100000 2500000 10000000" ;;
  openai/gpt-6-astra)          REF="10000000 1000000 12500000 50000000" ;;
  deepseek/deepseek-flash)     REF="300000 6000 300000 1200000" ;;
  deepseek/deepseek-v4-pro)    REF="1320000 44000 1320000 3960000" ;;
  *) REF="" ;;
esac
if [ -n "$PRICE" ]; then
  REF="$(/usr/bin/python3 -I -c '
import sys
from decimal import Decimal
p = [int(Decimal(x.strip()) * 1_000_000) for x in sys.argv[1].split(",")]
assert len(p) == 4 and all(x >= 0 for x in p)
print(*p)' "$PRICE")" || RC=64 refuse "--price $PRICE: give IN,CACHED,WRITE,OUT in USD per 1M tokens, for example 2.00,0.10,2.50,10.00"
fi
[ -n "$REF" ] || refuse "no reference price for $SP/$SM: pass --price IN,CACHED,WRITE,OUT (USD per 1M tokens, the provider's list price)"

CLIENT=("$RH/bin/init-root" attach)
for c in "${BIN_DIR:+$BIN_DIR/init-agent}" "$HOME/.local/bin/init-agent"; do
  [ -n "$c" ] && [ -x "$c" ] && { CLIENT=("$c"); break; }
done
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${OUT:-$HOME/init-qualify/$SP-$SM-$STAMP}"
mkdir -p "$OUT"
cat <<EOF
provider: $SP/$SM ($TESTED); key: $KEY (never read here); Root: $RH; INIT_HOME: $IH
client:   ${CLIENT[*]}
price:    $REF µUSD per 1M tokens (input, cached, cache write, output)
results:  $OUT
EOF
if [ $YES = 0 ]; then
  printf 'This spends real money at %s, up to $%s. Continue? [y/N] ' "$SP" "$CAP"
  read -r a </dev/tty || a=""
  case "$a" in y|Y|yes) ;; *) refuse "not confirmed; nothing was sent" ;; esac
fi

# ---------------------------------------------------------------- the session and the checks
cat >"$OUT/qualify.py" <<'PY'
import hashlib, json, os, queue, re, secrets, signal, subprocess, sys, threading, time

root_bin, rh, ih, home, out, cap, prov, model, ref, client = sys.argv[1:11]
cap = int(cap)
P_IN, P_CR, P_CW, P_OUT = (int(x) for x in ref.split())
client = json.loads(client)
MAX_REQUESTS = 16
NONCE = os.environ.get("QUALIFY_NONCE") or secrets.token_hex(8)
DIGEST = hashlib.sha256(f"qualify-{NONCE}".encode()).hexdigest()[:16]
INTENT = ("provider qualification by scripts/qualify-provider.sh: answer the owner's test lines exactly as asked; "
          "make no change to yourself and propose none")
REnv = {"HOME": home, "INIT_ROOT_HOME": rh, "PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8"}
LEDGER = os.path.join(rh, "egress", "usage")
now_ms = lambda: int(time.time() * 1000)


def spent():
    """The Root's settled spend (`egress stats`), as the goal meter counts it: at settlement. Reservations are left
    out: each in-flight request reserves its whole output allowance (Sonnet 5.5: about $0.33), which alone can exceed
    a small cap; the overshoot is at most the cost of the request that crosses the cap."""
    try:
        r = subprocess.run([root_bin, "egress", "stats"], capture_output=True, text=True, timeout=10, env=REnv)
        return json.loads(r.stdout)["money"].get("total_spent_musd") or 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        return None


def ledger_sizes():
    try:
        return {f: os.path.getsize(os.path.join(LEDGER, f)) for f in os.listdir(LEDGER) if f.endswith(".jsonl")}
    except OSError:
        return {}


def ledger_since(sizes):
    recs = []
    for f in sorted(ledger_sizes()):
        with open(os.path.join(LEDGER, f), "rb") as fh:
            fh.seek(sizes.get(f, 0))
            for line in fh.read().splitlines():
                try:
                    recs.append(json.loads(line))
                except ValueError:
                    pass
    return recs


def requests(sizes):
    """This model's live requests since the start, in order: (start, end or None)."""
    recs = ledger_since(sizes)
    ends = {r.get("id"): r for r in recs if r.get("t") == "end"}
    return [(s, ends.get(s.get("id"))) for s in recs
            if s.get("t") == "start" and s.get("lane", "live") == "live" and s.get("provider") == prov and s.get("model") == model]


frames = open(os.path.join(out, "frames.jsonl"), "a")
p = subprocess.Popen(client + ["--raw", "--no-replay"], cwd=home, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, env=REnv, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
spent0, sizes0, t0 = spent(), ledger_sizes(), now_ms()
stopped = None


def send(text):
    frames.write(json.dumps({"ms": now_ms(), "in": text}) + "\n")
    p.stdin.write(text + "\n")
    p.stdin.flush()


def interrupt():
    if p.poll() is None:
        p.send_signal(signal.SIGINT)  # our own child: the client sends the Root `interrupt` (Esc in the app)


def over_budget():
    s = spent()
    if s is not None and spent0 is not None and s - spent0 >= cap:
        return "cap"
    if len(requests(sizes0)) > MAX_REQUESTS:
        return "requests"
    return None


def step(say, limit, pause_after_text=False):
    """One owner line until the turn is idle again. With pause_after_text: SIGINT once the first text arrives."""
    global stopped
    r = {"say": say[:100], "texts": [], "infos": [], "notices": [], "tools": [], "busy": False, "idle": False,
         "t0_ms": now_ms()}
    ts = time.monotonic()
    send(say)
    poll, sig_at = 0.0, None
    while time.monotonic() - ts < limit:
        if time.monotonic() - poll > 1.0:
            poll = time.monotonic()
            why = over_budget()
            if why:
                stopped = why
                interrupt()
                break
        if pause_after_text and sig_at is None and r["busy"] and (r["texts"] or time.monotonic() - r["busy_at"] > 20):
            sig_at = time.monotonic()
            r["interrupted_after_ms"] = int((sig_at - ts) * 1000)
            interrupt()
        try:
            line = q.get(timeout=0.2)
        except queue.Empty:
            continue
        if line is None:
            r["client_eof"] = True
            break
        frames.write(json.dumps({"ms": now_ms(), "out": line}) + "\n")
        try:
            v = json.loads(line)
        except ValueError:
            continue
        t = v.get("t")
        d = v.get("display") if isinstance(v.get("display"), dict) else v
        kind = d.get("kind")
        if t == "display" and kind in ("text", None) and isinstance(d.get("text"), str):
            if not r["texts"]:
                r["first_text_ms"] = int((time.monotonic() - ts) * 1000)
            r["texts"].append(d["text"])
        elif t == "display" and kind == "info" and isinstance(d.get("text"), str):
            r["infos"].append(d["text"])
        elif t == "display" and kind in ("tool_start", "tool_end"):
            r["tools"].append([kind, str(d.get("text") or d.get("summary") or d.get("name") or "")[:200]])
        elif t == "notice":
            n = str(v.get("text", ""))
            r["notices"].append(n[:400])
            if "budget is spent" in n:
                stopped = "goal_budget"
                interrupt()
        elif t == "state":
            if v.get("busy") is True and not r["busy"]:
                r["busy"], r["busy_at"] = True, time.monotonic()
            elif v.get("busy") is False and r["busy"]:
                r["idle"] = True
                break
    if sig_at is not None:
        r["idle_after_interrupt_ms"] = int((time.monotonic() - sig_at) * 1000)
    r["reply"] = "".join(r["texts"])[-1500:]
    r["wall_ms"] = int((time.monotonic() - ts) * 1000)
    r["t1_ms"] = now_ms()
    r.pop("busy_at", None)
    return r


def root_line(say, settle=4.0):
    """A /goal line: the Root answers with a notice (no model request)."""
    send(say)
    notes, ts = [], time.monotonic()
    while time.monotonic() - ts < settle:
        try:
            line = q.get(timeout=0.2)
        except queue.Empty:
            continue
        if line is None:
            break
        frames.write(json.dumps({"ms": now_ms(), "out": line}) + "\n")
        try:
            v = json.loads(line)
        except ValueError:
            continue
        d = v.get("display") if isinstance(v.get("display"), dict) else v
        if isinstance(d.get("text"), str) and (v.get("t") == "notice" or d.get("kind") in ("info", "notice", "warn", None)):
            notes.append(d["text"])
    return notes


steps, goal_id = {}, None
time.sleep(1.0)
status = next((n for n in root_line("/goal") if n.startswith("[root] {")), None)
try:
    g = json.loads(status[len("[root] "):]).get("goal") if status else None
except ValueError:
    g = None
if g and g.get("state") == "active":
    print(f"refused: your goal {g.get('id')} is active; pause or stop it first (/goal pause). Nothing was sent.", file=sys.stderr)
    p.stdin.close()
    sys.exit(2)
cap_usd = f"{cap // 1_000_000}.{cap % 1_000_000:06d}"
notes = root_line(f'/goal "{INTENT}" usd={cap_usd} wall=1h rounds=1')
goal_id = next((m.group(1) for n in notes for m in [re.search(r"goal (G-?\d+) set", n)] if m), None)
if not goal_id:
    print(f"refused: the Root did not confirm the goal (its answer: {notes[-3:]}); nothing was sent to the model", file=sys.stderr)
    p.stdin.close()
    sys.exit(2)
try:
    if goal_id:
        steps["stream"] = step("Reply in plain text and do not use any tool: write the whole numbers from 1 to 40 in "
                               "order, separated by single spaces, and nothing else.", 240)
        if not stopped:
            steps["tool"] = step("Use your bash tool to run exactly this command, unchanged: `printf %s qualify-" + NONCE +
                                 " | { sha256sum 2>/dev/null || shasum -a 256; }`. Then reply with only the first 16 "
                                 "hex digits of its output.", 300)
        if not stopped:
            steps["pause"] = step("Do not use any tool. Write a numbered list of 120 short, different facts about the "
                                  "Rust programming language, one per line.", 240, pause_after_text=True)
        if not stopped:
            time.sleep(1.0)
            steps["resume"] = step("Please continue, but write only three more items of that list, then stop.", 240)
finally:
    if goal_id:
        steps["goal_stop"] = {"notices": root_line("/goal stop")}
    try:
        p.stdin.close()
        p.wait(timeout=5)
    except Exception:
        p.terminate()  # our own child
        p.wait()

# ---------------------------------------------------------------- evidence: the ledger and the session log
deadline = time.monotonic() + 45  # every request the run started must be settled (an `end` record)
while time.monotonic() < deadline and any(e is None for _, e in requests(sizes0)):
    time.sleep(1)
reqs = requests(sizes0)
spent1 = spent()
with open(os.path.join(out, "ledger.jsonl"), "w") as f:
    for s, e in reqs:
        f.write(json.dumps({"start": s, "end": e}, sort_keys=True) + "\n")
t0_iso = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(t0 / 1000 - 2))
msgs = []
sdir = os.path.join(ih, "state", "sessions")
for fn in sorted(os.listdir(sdir)) if os.path.isdir(sdir) else []:
    fp = os.path.join(sdir, fn)
    if not fn.endswith(".jsonl") or os.path.getmtime(fp) * 1000 < t0 - 2000:
        continue
    for line in open(fp, errors="replace"):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if ev.get("type") == "message" and ev.get("role") == "assistant" and ev.get("usage") and str(ev.get("ts", "")) >= t0_iso \
                and ev.get("provider") == prov and ev.get("model") == model:
            msgs.append(ev)
msgs.sort(key=lambda e: e.get("ts", ""))


def ms_of(t):
    """RFC 3339 UTC (the ledger's `at`, the session log's `ts`) → epoch ms."""
    import calendar
    try:
        base, _, frac = str(t).rstrip("Z").partition(".")
        return calendar.timegm(time.strptime(base, "%Y-%m-%dT%H:%M:%S")) * 1000 + int((frac + "000")[:3] or 0)
    except ValueError:
        return 0


# The pause step's requests are left out of the token and cost comparison on both sides: the body logs the cut reply
# as paused, while the Root settles what it saw (it may keep reading the upstream to its end, or book the reservation).
pw = steps.get("pause", {})
in_pause = lambda t: pw.get("t0_ms", 0) <= ms_of(t) <= pw.get("t1_ms", 0)
done = [(s, e) for s, e in reqs if e and e.get("usage") and e.get("finish") and int(e.get("status") or 0) < 400
        and not in_pause(s.get("at"))]
cut = [(s, e) for s, e in reqs if (s, e) not in done]
full = [m for m in msgs if m.get("stop_reason") not in (None, "aborted", "paused", "interrupted") and not in_pause(m.get("ts"))]

checks = {}
def check(name, ok, detail):
    checks[name] = {"pass": bool(ok), "detail": detail}

st = steps.get("stream", {})
check("budget", goal_id and stopped is None and spent1 is not None and spent0 is not None and spent1 - spent0 < cap,
      f"goal {goal_id or 'NOT SET'} usd={cap_usd}; Root spend since start {None if spent1 is None or spent0 is None else spent1 - spent0} "
      f"µUSD (settled); stopped: {stopped or 'no'}")
check("streaming", st.get("idle") and len(st.get("texts", [])) >= 2 and "40" in st.get("reply", ""),
      f"{len(st.get('texts', []))} text deltas, first after {st.get('first_text_ms')} ms, turn {st.get('wall_ms')} ms")
tl = steps.get("tool", {})
ran = [t for t in tl.get("tools", []) if t[0] == "tool_end"]
check("tool_round_trip", tl.get("idle") and ran and DIGEST in tl.get("reply", "").lower(),
      f"{len(ran)} tool call(s) ran; reply {'has' if DIGEST in tl.get('reply', '').lower() else 'LACKS'} the digest {DIGEST}")
pa, re_ = steps.get("pause", {}), steps.get("resume", {})
paused = any(i.startswith("paused") for i in pa.get("infos", []))
errs = [e for _, e in reqs if e and int(e.get("status") or 0) >= 400]
check("pause_resume", paused and pa.get("idle") and pa.get("idle_after_interrupt_ms", 99999) <= 5000
      and re_.get("idle") and re_.get("reply", "").strip() and not errs,
      f"interrupt after {pa.get('interrupted_after_ms')} ms; 'paused' shown: {paused}; idle {pa.get('idle_after_interrupt_ms')} ms "
      f"after; resume reply {len(re_.get('reply', ''))} chars; provider errors: {[e.get('status') for e in errs] or 'none'}")
crs = [int((e or {}).get("usage", {}).get("cache_read") or 0) if e and e.get("usage") else None for _, e in reqs]
check("cache", len(crs) >= 2 and (crs[1] or 0) > 0, f"cache read per request (Root ledger): {crs}")
pairs = list(zip(done, full))
tok_ok = len(done) == len(full) and len(done) >= 3 and all(
    (e["usage"].get("input"), e["usage"].get("cache_read"), e["usage"].get("cache_write", 0) + e["usage"].get("cache_write_1h", 0),
     e["usage"].get("output")) ==
    (m["usage"]["input_tokens"] + m["usage"]["cache_read_tokens"] + m["usage"]["cache_write_tokens"],
     m["usage"]["cache_read_tokens"], m["usage"]["cache_write_tokens"], m["usage"]["output_tokens"]) for (_, e), m in pairs)
check("tokens", tok_ok, f"{len(done)} completed requests in the ledger, {len(full)} in the session log; "
      f"(input, cache read, cache write, output) {'equal' if tok_ok else 'DIFFER'}")


def list_price(u):
    w, r = u["cache_write_tokens"], u["cache_read_tokens"]
    s = u["input_tokens"] * P_IN + w * P_CW + r * P_CR + u["output_tokens"] * P_OUT
    return -(-s // 1_000_000)


ledger_cost = sum(int(e.get("cost_musd") or 0) for (_, e), _ in pairs)
ref_cost = sum(list_price(m["usage"]) for _, m in pairs)
writes = sum(m["usage"]["cache_write_tokens"] for _, m in pairs)
big = [m["usage"]["input_tokens"] + m["usage"]["cache_read_tokens"] + m["usage"]["cache_write_tokens"] for _, m in pairs]
big = [b for b in big if prov == "openai" and b > 272_000]
delta = (ledger_cost - ref_cost) / ref_cost if ref_cost else None
check("cost", tok_ok and delta is not None and abs(delta) <= 0.05 and not big,
      f"ledger {ledger_cost} µUSD vs provider usage at list price {ref_cost} µUSD: "
      f"{'n/a' if delta is None else f'{delta * 100:+.1f} %'}; cache writes {writes} tokens; not compared: {len(cut)} cut "
      f"request(s) the ledger "
      f"charged {sum(int((e or {}).get('cost_musd') or 0) for _, e in cut)} µUSD" + (f"; over 272K input: {big}" if big else ""))
ok = all(c["pass"] for c in checks.values())
verdict = {"v": 1, "provider": prov, "model": model, "cap_musd": cap, "window_utc": [t0_iso, time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime())],
           "reference_price_musd_per_mtok": {"input": P_IN, "cache_read": P_CR, "cache_write": P_CW, "output": P_OUT},
           "requests": len(reqs), "spent_musd": None if spent1 is None or spent0 is None else spent1 - spent0,
           "checks": checks, "steps": {k: {x: y for x, y in v.items() if x != "texts"} for k, v in steps.items()},
           "stopped": stopped, "pass": ok}
json.dump(verdict, open(os.path.join(out, "verdict.json"), "w"), indent=1, sort_keys=True)
print(f"\nqualify {prov}/{model}: {'PASS' if ok else 'FAIL'}  (window {verdict['window_utc'][0]} to {verdict['window_utc'][1]} UTC; "
      f"{len(reqs)} model requests; Root spend {verdict['spent_musd']} µUSD of {cap})")
w = max(len(k) for k in checks)
for k, c in checks.items():
    print(f"  {k:<{w}}  {'PASS' if c['pass'] else 'FAIL'}  {c['detail']}")
print(f"verdict: {os.path.join(out, 'verdict.json')}")
sys.exit(3 if stopped else (0 if ok else 1))
PY
set +e
/usr/bin/python3 -I "$OUT/qualify.py" "$ROOT_BIN" "$RH" "$IH" "$HOME" "$OUT" "$CAP_MUSD" "$SP" "$SM" "$REF" \
  "$(/usr/bin/python3 -I -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${CLIENT[@]}")"
rc=$?
set -e
exit $rc
