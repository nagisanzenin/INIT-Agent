#!/usr/bin/python3 -I
"""Slice-1 owner driver: runs `init-root attach --raw --no-replay`, sends one owner line, and collects
the frames until the body goes idle again (`state{busy:false}` after `state{busy:true}`; a `turn_end`
display follows every model turn, so it does not end the task), or until a text frame matches --expect
when --until-text is given, or the timeout passes. Every frame goes to --out as one JSON line. Exit 0 when --expect matched the joined
display text, 1 when it did not, 2 on a timeout, 3 when the spend cap stopped the task.

The attach process is this script's own child; it is stopped by its handle (stdin closed, then
terminate/kill of that pid only)."""
import argparse
import json
import os
import queue
import re
import signal
import socket
import subprocess
import sys
import threading
import time


def day_spent(sock_path: str):
    """GET /_root/ceilings over the Root's egress socket; `money.day_spent_musd`, or None."""
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(3)
        s.connect(sock_path)
        s.sendall(b"GET /_root/ceilings HTTP/1.1\r\nhost: root\r\nconnection: close\r\n\r\n")
        buf = b""
        while True:
            b = s.recv(65536)
            if not b:
                break
            buf += b
        s.close()
        body = buf.split(b"\r\n\r\n", 1)[1]
        if b"transfer-encoding: chunked" in buf.split(b"\r\n\r\n", 1)[0].lower():
            out, rest = b"", body
            while rest:
                n, _, rest = rest.partition(b"\r\n")
                k = int(n.split(b";")[0], 16)
                if k == 0:
                    break
                out, rest = out + rest[:k], rest[k + 2:]
            body = out
        return int(json.loads(body)["money"]["day_spent_musd"])
    except Exception:
        return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="init-root binary")
    ap.add_argument("--rh", required=True, help="ROOT_HOME")
    ap.add_argument("--home", required=True, help="HOME for the attach client")
    ap.add_argument("--line", required=True, help="the owner line to send")
    ap.add_argument("--expect", required=True, help="regex over the joined display text")
    ap.add_argument("--timeout", type=float, default=120.0)
    ap.add_argument("--out", required=True)
    ap.add_argument("--until-text", action="store_true", help="stop at the first matching text frame")
    ap.add_argument("--spend-sock", help="ROOT_HOME/run/egress.sock: poll /_root/ceilings while waiting")
    ap.add_argument("--spend-max-musd", type=int, default=0, help="interrupt and /stop above this day spend")
    a = ap.parse_args()

    env = {"HOME": a.home, "INIT_ROOT_HOME": a.rh, "PATH": "/usr/bin:/bin"}
    p = subprocess.Popen([a.bin, "attach", "--raw", "--no-replay"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, env=env, text=True, bufsize=1)
    q: "queue.Queue[str | None]" = queue.Queue()

    def reader():
        for line in p.stdout:
            q.put(line.rstrip("\n"))
        q.put(None)

    threading.Thread(target=reader, daemon=True).start()
    t0 = time.monotonic()
    time.sleep(0.3)  # let the attach hello reach the Root before the line
    p.stdin.write(a.line + "\n")
    p.stdin.flush()
    texts, rc, saw_end, busy_seen, turns = [], 2, False, False, 0
    spent, last_poll, over, poll_fail = None, float("-inf"), False, 0  # monotonic() may start near 0
    pat = re.compile(a.expect)
    with open(a.out, "a") as out:
        while time.monotonic() - t0 < a.timeout:
            if a.spend_sock and a.spend_max_musd and time.monotonic() - last_poll >= 0.5:
                last_poll = time.monotonic()
                got = day_spent(a.spend_sock)
                poll_fail += got is None
                spent = got if got is not None else spent
                if spent is not None and spent > a.spend_max_musd:
                    over = True
                    out.write(json.dumps({"ms": int((time.monotonic() - t0) * 1000), "harness": "spend cap",
                                          "day_spent_musd": spent}) + "\n")
                    p.send_signal(signal.SIGINT)  # our own attach child: it sends `interrupt`
                    p.stdin.write("/stop\n")
                    p.stdin.flush()
                    rc = 3
                    break
            try:
                line = q.get(timeout=0.5)
            except queue.Empty:
                continue
            if line is None:
                break
            ms = int((time.monotonic() - t0) * 1000)
            try:
                v = json.loads(line)
            except ValueError:
                v = {"unparsed": line}
            out.write(json.dumps({"ms": ms, "frame": v}, sort_keys=True) + "\n")
            out.flush()
            f = v.get("frame") if isinstance(v.get("frame"), dict) else v
            kind = f.get("kind") or (f.get("display") or {}).get("kind")
            text = f.get("text") if isinstance(f.get("text"), str) else (f.get("display") or {}).get("text", "")
            if v.get("t") == "display" or f.get("t") == "display":
                if kind in ("text", None):
                    texts.append(text or "")
                if kind == "turn_end":
                    turns += 1
            if v.get("t") == "state":
                if v.get("busy") is True:
                    busy_seen = True
                elif v.get("busy") is False and busy_seen:
                    saw_end = True
            if a.until_text and pat.search("".join(texts)):
                rc = 0
                break
            if saw_end:
                rc = 0 if pat.search("".join(texts)) else 1
                break
    joined = "".join(texts)
    print(json.dumps({"rc": rc, "spend_cap_hit": over, "day_spent_musd_seen": spent, "spend_poll_failures": poll_fail, "idle_after_task": saw_end, "model_turns": turns, "wall_ms": int((time.monotonic() - t0) * 1000),
                      "reply": joined[-400:]}, sort_keys=True))
    try:
        p.stdin.close()
    except OSError:
        pass
    try:
        p.wait(timeout=3)
    except subprocess.TimeoutExpired:
        p.terminate()  # our own child, by its handle
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
    return rc


if __name__ == "__main__":
    sys.exit(main())
