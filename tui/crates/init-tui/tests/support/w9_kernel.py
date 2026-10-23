#!/usr/bin/python3 -I
"""WIRE-009 e2e support: a fake `init-kernel supervise --under-root` in Python (std only).

The real Root (test mode) runs this file as INIT_HOME/kernel/bin/init-kernel inside the macOS `kernel`
profile. It speaks the control link of docs/root/root.md §4.1 (ICJ lines on fd 3, SCM_RIGHTS
datagrams on fd 4) like the Root's own Rust fake kernel (root/.../tests/support/fakekernel.rs), and
adds what the ADR-009 e2e needs:

- a body: `/bin/sh -c BODY` under the real `body` profile, so the Root's grant (Z_WRITE = the
  workspace) and its `INIT_WORKSPACE`/`INIT_ACCESS` env are what the body gets. It prints
  `ws=<INIT_WORKSPACE> access=<INIT_ACCESS>` at start, and for each stdin line `W <path> <base64>`
  writes `$INIT_WORKSPACE/<path>` and prints `wrote <path>` (or `write failed: <path>`);
- an agent turn: an owner line `!tool <text>` makes one chat request to the mock model through the
  Root's egress socket (the boot block's `paths.egress_sock`) and hands each `write` tool call the
  model returns to the body;
- ADR-009 `access{restart:"idle"}`: the body is restarted (stdin EOF, then a new spawn) so it gets
  the new grant, as the real kernel does at its next idle point.

Every step is logged as a JSON line to $INIT_HOME/state/kernel/w9k.jsonl. It never signals a
process: a body ends on stdin EOF, and this kernel exits on EOF of fd 3 or after `stop`.
"""

import array
import base64
import json
import os
import socket
import sys
import threading
import time

HOME = os.environ.get("INIT_HOME", "")
if not HOME.startswith("/"):
    sys.exit(69)
os.makedirs(HOME + "/state/kernel", exist_ok=True)
LOG = open(HOME + "/state/kernel/w9k.jsonl", "a")
LOG_LOCK = threading.Lock()

BODY = r"""
printf 'ws=%s access=%s\n' "$INIT_WORKSPACE" "$INIT_ACCESS"
while IFS=' ' read -r op name data; do
  case "$op" in
    W)
      if [ -n "$INIT_WORKSPACE" ] && printf '%s' "$data" | /usr/bin/base64 -D > "$INIT_WORKSPACE/$name" 2>/dev/null; then
        echo "wrote $name"
      else
        echo "write failed: $name"
      fi ;;
    *) echo "$op $name $data" ;;
  esac
done
"""


def icj(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def log(v):
    with LOG_LOCK:
        LOG.write(icj(v) + "\n")
        LOG.flush()


ctl = socket.socket(fileno=3)
fdc = socket.socket(fileno=4)
W_LOCK = threading.Lock()
PENDING = {}  # id -> [threading.Event, response]
NEXT = [1]
STASH = {}  # datagram tag id -> fds
INBOX = []  # Root requests and events, in order
INBOX_CV = threading.Condition()


def send(v):
    with W_LOCK:
        ctl.sendall((icj(v) + "\n").encode())


def event(ev, data):
    send({"data": data, "ev": ev, "v": 1})


SEQ = [0]


def display(text, kind="text"):
    event("owner.out", {"frame": {"boot_seq": 1, "kind": kind, "t": "display", "text": text}})


def call(cmd, args, timeout=30):
    i = "k-%d" % NEXT[0]
    NEXT[0] += 1
    slot = [threading.Event(), None]
    PENDING[i] = slot
    send({"args": args, "cmd": cmd, "id": i, "v": 1})
    if not slot[0].wait(timeout):
        log({"ev": "timeout", "cmd": cmd})
        sys.exit(69)
    return i, slot[1]


def take_fds(i, n):
    if i in STASH:
        return STASH.pop(i)
    while True:
        msg, anc, _flags, _addr = fdc.recvmsg(4096, socket.CMSG_SPACE(16 * 4))
        fds = []
        for level, typ, data in anc:
            if level == socket.SOL_SOCKET and typ == socket.SCM_RIGHTS:
                a = array.array("i")
                a.frombytes(data[: len(data) - (len(data) % a.itemsize)])
                fds.extend(a.tolist())
        tag = json.loads(msg.decode())
        if tag.get("id") == i:
            if len(fds) != n:
                log({"ev": "fd_count", "want": n, "got": len(fds)})
                sys.exit(69)
            return fds
        STASH[tag.get("id")] = fds


def reader():
    f = ctl.makefile("rb")
    for raw in f:
        if not raw.strip():
            continue
        v = json.loads(raw)
        if "cmd" in v or "ev" in v:
            with INBOX_CV:
                INBOX.append(v)
                INBOX_CV.notify()
        else:
            slot = PENDING.pop(v.get("id"), None)
            if slot:
                slot[1] = v
                slot[0].set()
    with INBOX_CV:
        INBOX.append(None)
        INBOX_CV.notify()


threading.Thread(target=reader, daemon=True).start()

_, r = call("hello", {"kernel_version": "0.0.0-w9k", "proto": 1})
log({"ev": "boot", "ok": r.get("ok"), "access": (r.get("result") or {}).get("access")})
if r.get("ok") is not True:
    sys.exit(69)
BOOT = r["result"]
EGRESS = BOOT["paths"]["egress_sock"]
_, r = call("instances", {})
_, r = call("ready", {})
log({"ev": "ready", "ok": r.get("ok")})


def hb():
    every = max(1, int(BOOT["limits"]["hb_interval_s"]))
    data = {"busy": False, "mode": BOOT["boot"]["mode"], "sha12": "000000000000", "slot": "running"}
    while True:
        try:
            event("hb", data)
        except OSError:
            return
        time.sleep(every)


threading.Thread(target=hb, daemon=True).start()


def spawn_args():
    p = {}
    for k, sub in [("BIN", "/bin"), ("BODY", "/body"), ("CACHE", "/cache"), ("INIT_HOME", ""), ("KERNEL", "/kernel"),
                   ("KERNEL_SOCK", "/run/kernel.sock"), ("LINEAGE", "/lineage"), ("LINK_BIN", "/bin"), ("LINK_KERNEL", "/kernel"),
                   ("LINK_LINEAGE", "/lineage"), ("PROXY_SOCK", "/run/proxy.sock"), ("PUB", "/root"), ("RUN", "/run"),
                   ("STATE", "/state"), ("STATE_KERNEL", "/state/kernel"), ("VAULT", "/vault"), ("WORK", "/work")]:
        p[k] = HOME + sub
    for k, var in [("TOOLCHAIN", "INIT_TEST_TOOLCHAIN"), ("DEVDIR", "INIT_TEST_DEVDIR"), ("HOME_REAL", "INIT_TEST_HOME_REAL")]:
        p[k] = os.environ.get(var, "")
    return {"args": ["-c", BODY], "cwd": HOME + "/work", "env": {"PATH": "/usr/bin:/bin"}, "kind": "body", "lane": "live",
            "params": p, "profile": "body", "program": "/bin/sh"}


class Body:
    def __init__(self):
        self.stdin = None
        self.instance = None
        self.done = threading.Event()

    def start(self):
        i, r = call("spawn", spawn_args())
        log({"ev": "spawn", "ok": r.get("ok"), "result": r.get("result"), "error": r.get("error")})
        if r.get("ok") is not True:
            return False
        self.instance = r["result"]["instance"]
        fin, fout, ferr = take_fds(i, 3)
        self.stdin = os.fdopen(fin, "wb", buffering=0)
        out, err = os.fdopen(fout, "rb"), os.fdopen(ferr, "rb")

        def pump():
            for line in out:
                t = line.decode(errors="replace").rstrip("\n")
                log({"ev": "body", "line": t})
                display("body: %s\n" % t)
            self.done.set()

        threading.Thread(target=pump, daemon=True).start()
        threading.Thread(target=lambda: err.read(), daemon=True).start()
        return True

    def send(self, line):
        try:
            self.stdin.write((line + "\n").encode())
            return True
        except (OSError, AttributeError):
            return False

    def stop(self):
        if self.stdin is not None:
            try:
                self.stdin.close()
            except OSError:
                pass
            self.stdin = None
        self.done.wait(10)


body = Body()
body.start()
RESTART = [None]  # the instance being replaced: the new body spawns once the Root reports its exit


def chat(text):
    """One streamed chat request through the Root's egress; returns the tool calls [(name, args)]."""
    req = {"max_tokens": 200, "messages": [{"content": text, "role": "user"}], "model": "deepseek-flash", "stream": True,
           "tools": [{"function": {"description": "Write a file.", "name": "write",
                                   "parameters": {"properties": {"content": {"type": "string"}, "path": {"type": "string"}},
                                                  "required": ["path", "content"], "type": "object"}}, "type": "function"}]}
    b = icj(req).encode()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(30)
    s.connect(EGRESS)
    head = ("POST /deepseek/chat/completions HTTP/1.1\r\nhost: egress\r\nconnection: close\r\ncontent-type: application/json\r\n"
            "x-init-session: w9\r\nx-init-purpose: turn\r\ncontent-length: %d\r\n\r\n" % len(b))
    s.sendall(head.encode() + b)
    raw = b""
    while True:
        c = s.recv(65536)
        if not c:
            break
        raw += c
    s.close()
    hdr, _, body_bytes = raw.partition(b"\r\n\r\n")
    status = hdr.split(b"\r\n", 1)[0].decode(errors="replace")
    if b"transfer-encoding: chunked" in hdr.lower():
        out, rest = b"", body_bytes
        while rest:
            size_line, _, rest = rest.partition(b"\r\n")
            n = int(size_line.split(b";")[0] or b"0", 16)
            if n == 0:
                break
            out, rest = out + rest[:n], rest[n + 2:]
        body_bytes = out
    calls = {}
    for line in body_bytes.decode(errors="replace").splitlines():
        if not line.startswith("data: ") or line == "data: [DONE]":
            continue
        try:
            d = json.loads(line[6:])
        except ValueError:
            continue
        for ch in d.get("choices") or []:
            for tc in (ch.get("delta") or {}).get("tool_calls") or []:
                c = calls.setdefault(tc.get("index", 0), {"name": "", "arguments": ""})
                fn = tc.get("function") or {}
                c["name"] += fn.get("name") or ""
                c["arguments"] += fn.get("arguments") or ""
    log({"ev": "chat", "status": status, "calls": len(calls)})
    return status, [(c["name"], c["arguments"]) for _, c in sorted(calls.items())]


def turn(text):
    event("owner.out", {"frame": {"busy": True, "level": BOOT["ladder"]["level"], "members": 0, "mode": "normal", "queued": 0,
                                  "sha12": "000000000000", "slot": "running", "t": "state"}})
    status, calls = chat(text)
    if not status.startswith("HTTP/1.1 200"):
        display("model error: %s\n" % status, "error")
    for name, args in calls:
        try:
            a = json.loads(args)
        except ValueError:
            a = {}
        if name == "write" and isinstance(a.get("path"), str) and isinstance(a.get("content"), str):
            body.send("W %s %s" % (a["path"], base64.b64encode(a["content"].encode()).decode()))
    display("tokens: in 1, out 1", "turn_end")
    event("owner.out", {"frame": {"busy": False, "level": BOOT["ladder"]["level"], "members": 0, "mode": "normal", "queued": 0,
                                  "sha12": "000000000000", "slot": "running", "t": "state"}})


while True:
    with INBOX_CV:
        while not INBOX:
            INBOX_CV.wait()
        v = INBOX.pop(0)
    if v is None:
        log({"ev": "eof"})
        body.stop()
        sys.exit(69)
    if "ev" in v:
        log({"ev": "event", "name": v["ev"], "data": v.get("data")})
        data = v.get("data") or {}
        if v["ev"] == "access" and data.get("restart") == "idle" and RESTART[0] is None:
            RESTART[0] = body.instance or ""
            body.stop()  # idle: no turn runs here between lines; stdin EOF ends the body
        if RESTART[0] is not None and (RESTART[0] == "" or (v["ev"] == "exited" and data.get("instance") == RESTART[0])):
            RESTART[0] = None
            body = Body()
            body.start()  # within parallel_bodies = 1: the old one is gone
            log({"ev": "restarted"})
        continue
    cmd, args, rid = v.get("cmd"), v.get("args") or {}, v.get("id")
    if cmd == "owner.in":
        frame = args.get("frame") or {}
        log({"ev": "owner.in", "qid": args.get("qid"), "frame": frame})
        send({"id": rid, "ok": True, "result": {}, "v": 1})
        if frame.get("t") == "line":
            text = frame.get("text", "")
            SEQ[0] += 1
            event("owner.out", {"frame": {"seq": SEQ[0], "t": "echo", "text": text}})
            if text.startswith("!tool"):
                turn(text)
            elif not body.send(text):
                display("w9k: %s\n" % text)
    elif cmd == "stop":
        log({"ev": "stop"})
        send({"id": rid, "ok": True, "result": {}, "v": 1})
        body.stop()
        sys.exit(0)
    elif cmd == "prepare_swap":
        send({"id": rid, "ok": True, "result": {}, "v": 1})
        sys.exit(75)
    else:
        log({"ev": "unknown_cmd", "cmd": cmd})
        send({"error": {"code": "root.unknown_cmd", "details": [], "message": "w9 fake kernel"}, "id": rid, "ok": False, "v": 1})
