"""GBAL proof driver (tests/real/gbal_proof.sh): one `init --raw --no-replay` client per phase, owner lines from a
plan, every frame recorded with its wall-clock ms.

usage: python3 -I gbal_drive.py <init> <pw_home> <rh> <ih> <ws> <stream.jsonl> <stop_musd> <plan.json>
plan: [{"say": "...", "kind": "turn"|"root", "timeout": s}]
Watchdog: settled + reserved spend (init-root egress stats) >= stop_musd, or free disk < GBAL_DISK_STOP_GIB (15.2) -> "/stop",
exit 3 (spend) or 4 (disk). Prints one JSON summary line per step. Never reads or prints a key.
"""
import json, os, queue, subprocess, sys, threading, time

init, home, rh, ih, ws, out, cap, plan = sys.argv[1:9]
cap = int(cap)
plan = json.load(open(plan))
FLOOR_KB = int(float(os.environ.get("GBAL_DISK_STOP_GIB", "15.2")) * 1024 * 1024)


def spent():
    """Settled plus reserved (in-flight) spend, from the Root (`init-root egress stats`); the published
    INIT_HOME/root/ceilings.json (written on a timer) when the Root does not answer."""
    try:
        r = subprocess.run([os.path.join(rh, "bin", "init-root"), "egress", "stats"], capture_output=True, text=True, timeout=10,
                           env={"HOME": home, "INIT_ROOT_HOME": rh, "PATH": "/usr/bin:/bin"})
        m = json.loads(r.stdout)["money"]
        return (m.get("total_spent_musd") or 0) + (m.get("total_reserved_musd") or 0)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        pass
    try:
        c = json.load(open(os.path.join(ih, "root", "ceilings.json")))
        return c.get("money", {}).get("total_spent_musd") or 0
    except (OSError, ValueError):
        return -1


def free_kb():
    st = os.statvfs("/")
    return st.f_bavail * st.f_frsize // 1024


env = {"HOME": home, "PATH": "/usr/bin:/bin", "INIT_ROOT_HOME": rh, "LANG": "en_US.UTF-8"}
p = subprocess.Popen([init, "--raw", "--no-replay"], cwd=ws, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, env=env, text=True, bufsize=1)
q = queue.Queue()
threading.Thread(target=lambda: ([q.put(l.rstrip("\n")) for l in p.stdout], q.put(None)), daemon=True).start()
f = open(out, "a")


def rec(k, x):
    f.write(json.dumps({"ms": int(time.time() * 1000), "k": k, "f": x}) + "\n")
    f.flush()


def send(text):
    rec("in", text)
    p.stdin.write(text + "\n")
    p.stdin.flush()


time.sleep(1.0)
code = 0
for step in plan:
    say, kind, limit = step["say"], step.get("kind", "turn"), step.get("timeout", 600)
    t0, s0 = time.monotonic(), spent()
    send(say)
    busy = idle = False
    # A line queued while the body restarts (a rollback) can run with no busy:true state reaching this client: then
    # the turn counts as done 10 s after a turn_end with the kernel idle and nothing more on the stream.
    ended = idle_state = False
    last_out = time.monotonic()
    texts, notices, tools = [], [], []
    stopped = None
    last_poll = 0.0
    while time.monotonic() - t0 < limit:
        if time.monotonic() - last_poll > 2.0:
            last_poll = time.monotonic()
            if spent() >= cap:
                stopped = "spend_cap"
            elif free_kb() < FLOOR_KB:
                stopped = "disk_floor"
            if stopped:
                send("/stop")
                code = 3 if stopped == "spend_cap" else 4
                break
        try:
            line = q.get(timeout=0.25)
        except queue.Empty:
            if kind == "root" and time.monotonic() - t0 > step.get("settle", 4):
                break
            if kind == "turn" and not busy and ended and idle_state and time.monotonic() - last_out > 10:
                idle = True
                break
            continue
        if line is None:
            stopped = "client_eof"
            code = 5
            break
        try:
            v = json.loads(line)
        except ValueError:
            rec("raw", line)
            continue
        rec("out", v)
        last_out = time.monotonic()
        t = v.get("t")
        d = v.get("display") if isinstance(v.get("display"), dict) else v
        if t == "display" and d.get("kind") in ("text", None) and isinstance(d.get("text"), str):
            texts.append(d["text"])
        elif t == "display" and d.get("kind") in ("tool_start", "tool_end"):
            tools.append((d.get("kind"), str(d.get("text") or d.get("summary") or "")[:160]))
        elif t == "display" and d.get("kind") == "turn_end":
            ended = True
        elif t == "notice":
            notices.append(str(v.get("text", ""))[:300])
        elif t == "state":
            idle_state = v.get("busy") is False
            if v.get("busy") is True:
                busy = True
            elif v.get("busy") is False and busy:
                idle = True
                break
    summary = {"say": say[:80], "kind": kind, "wall_s": round(time.monotonic() - t0, 1), "busy": busy, "idle": idle,
               "spent_musd": spent(), "step_musd": spent() - s0, "reply": "".join(texts)[-600:],
               "notices": notices[-6:], "tools": [x[1] for x in tools if x[0] == "tool_end"][-12:], "stopped": stopped}
    print(json.dumps(summary, ensure_ascii=False), flush=True)
    if stopped:
        break
    time.sleep(step.get("after", 2))
try:
    p.stdin.close()
    p.wait(timeout=5)
except Exception:
    p.terminate()  # our own child
    p.wait()
sys.exit(code)
