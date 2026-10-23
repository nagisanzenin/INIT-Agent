#!/usr/bin/python3 -I
"""S-REAL checks, driven by tests/real/real.sh (build/status/S-REAL.md).

  checks.py prepare <check> …   before the Root starts: fixtures into INIT_HOME, the mock script (mock mode)
  checks.py run <check> …       with the Root running: drive the owner line(s) or the raw provider requests,
                                then save the proxy's traces (request bytes and raw response streams)
  checks.py verdict <check> …   after the Root stopped: read the run dir and INIT_HOME, print one JSON verdict

Checks: vds (V-DS-1…6 as raw requests through the kernel proxy), a1 (A1-real), e2e1, e2e2, e2e2off (the
thinking-disabled half, raw), e2e3, a2 (A2-real), a3 (A3-real), m3 (one M-3 trial).

Every model request goes through INIT_HOME/run/proxy.sock → the kernel proxy → the Root egress, so the Root
meters it and its charter ceiling is the hard cap. This script never reads the key. While a check runs, the
Root's total spend is polled every second; above --cap-musd the owner driver interrupts and /stops (rc 3).
Processes: the only process this script starts is its own `init-root attach` child, stopped by its handle."""
import argparse
import glob
import json
import os
import queue
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time

# ---------------------------------------------------------------------------------------------- plumbing


def http(sock, method, path, body=None, headers=None, timeout=300.0, host="proxy"):
    """One HTTP/1.1 request over a unix socket. Returns (status, headers dict, body bytes, raw body bytes)."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(sock)
    h = [f"{method} {path} HTTP/1.1", f"host: {host}", "connection: close"]
    if body is not None:
        h += ["content-type: application/json", f"content-length: {len(body)}"]
    for k, v in (headers or {}).items():
        h.append(f"{k}: {v}")
    s.sendall(("\r\n".join(h) + "\r\n\r\n").encode() + (body or b""))
    buf = b""
    while True:
        try:
            b = s.recv(65536)
        except socket.timeout:
            break
        if not b:
            break
        buf += b
    s.close()
    head, _, rest = buf.partition(b"\r\n\r\n")
    lines = head.decode("latin-1").split("\r\n")
    try:
        status = int(lines[0].split()[1])
    except Exception:
        status = 0
    hd = {}
    for l in lines[1:]:
        k, _, v = l.partition(":")
        hd[k.strip().lower()] = v.strip()
    out = rest
    if "chunked" in hd.get("transfer-encoding", "").lower():
        out, r = b"", rest
        while r:
            n, _, r = r.partition(b"\r\n")
            try:
                k = int(n.split(b";")[0], 16)
            except ValueError:
                break
            if k == 0:
                break
            out, r = out + r[:k], r[k + 2:]
    return status, hd, out


def ceilings(rh):
    try:
        st, _, b = http(os.path.join(rh, "run/egress.sock"), "GET", "/_root/ceilings", timeout=5, host="root")
        return json.loads(b) if st == 200 else None
    except Exception:
        return None


def root_total(rh):
    c = ceilings(rh)
    try:
        return int(c["money"]["total_spent_musd"])
    except Exception:
        return None


def jl(path):
    out = []
    for f in sorted(glob.glob(path)):
        try:
            for l in open(f, errors="replace"):
                try:
                    out.append(json.loads(l))
                except ValueError:
                    pass
        except OSError:
            pass
    return out


def jload(p, default=None):
    try:
        with open(p) as f:
            return json.load(f)
    except Exception:
        return default


def wjson(p, v):
    with open(p, "w") as f:
        json.dump(v, f, sort_keys=True, indent=1)


def session_events(ih, sid=None):
    """All events of one session (default: the current pointer), in file order."""
    if sid is None:
        try:
            sid = open(os.path.join(ih, "state/session")).read().strip()
        except OSError:
            return []
    return jl(os.path.join(ih, "state/sessions", f"{sid}.jsonl"))


def all_session_events(ih):
    return jl(os.path.join(ih, "state/sessions/*.jsonl"))


def sse_parse(raw: bytes):
    """Raw SSE bytes → (events [data strings], comment lines, other lines)."""
    evs, comments, data = [], [], []
    for line in raw.decode("utf-8", "replace").split("\n"):
        line = line.rstrip("\r")
        if line.startswith(":"):
            comments.append(line)
        elif line.startswith("data:"):
            data.append(line[5:].lstrip(" ") if line[5:6] == " " else line[5:])
        elif line == "":
            if data:
                evs.append("\n".join(data))
                data = []
    if data:
        evs.append("\n".join(data))
    return evs, comments


def ds_decode(raw: bytes):
    """A DeepSeek Chat Completions stream → a summary dict (mirrors seed/src/provider/deepseek.rs DsDecoder)."""
    evs, comments = sse_parse(raw)
    r = {"reasoning": "", "content": "", "calls": {}, "finish": None, "usage": None, "done": False, "chunks": 0,
         "keepalive_comments": len(comments), "errors": [], "id": None}
    for e in evs:
        if e.strip() == "[DONE]":
            r["done"] = True
            continue
        try:
            v = json.loads(e)
        except ValueError as x:
            r["errors"].append(f"chunk json: {x}")
            continue
        r["chunks"] += 1
        r["id"] = r["id"] or v.get("id")
        if isinstance(v.get("error"), dict):
            r["errors"].append(v["error"].get("message"))
        for ch in v.get("choices") or []:
            d = ch.get("delta") or {}
            r["reasoning"] += d.get("reasoning_content") or ""
            r["content"] += d.get("content") or ""
            for tc in d.get("tool_calls") or []:
                c = r["calls"].setdefault(tc.get("index"), {"id": "", "name": "", "args": "", "arg_chunks": 0})
                c["id"] = c["id"] or tc.get("id") or ""
                fn = tc.get("function") or {}
                c["name"] = c["name"] or fn.get("name") or ""
                if fn.get("arguments"):
                    c["args"] += fn["arguments"]
                    c["arg_chunks"] += 1
            if ch.get("finish_reason"):
                r["finish"] = ch["finish_reason"]
        if isinstance(v.get("usage"), dict):
            r["usage"] = v["usage"]
    calls = []
    for i in sorted(k for k in r["calls"] if k is not None):
        c = r["calls"][i]
        try:
            ok = isinstance(json.loads(c["args"]), dict)
        except ValueError:
            ok = False
        calls.append(dict(c, index=i, args_json_ok=ok))
    r["calls"] = calls
    return r


PROXY_H = {"x-init-session": "sreal-vds", "x-init-purpose": "turn"}


def post_chat(ih, body: dict, tag: str, run: str, extra_headers=None):
    """POST /deepseek/chat/completions through the kernel proxy; saves the request and the raw response."""
    b = json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    os.makedirs(os.path.join(run, "raw"), exist_ok=True)
    with open(os.path.join(run, "raw", f"{tag}.req.json"), "wb") as f:
        f.write(b)
    h = dict(PROXY_H)
    h.update(extra_headers or {})
    t0 = time.monotonic()
    st, hd, out = http(os.path.join(ih, "run/proxy.sock"), "POST", "/deepseek/chat/completions", b, h, timeout=300)
    wall = int((time.monotonic() - t0) * 1000)
    with open(os.path.join(run, "raw", f"{tag}.resp"), "wb") as f:
        f.write(out)
    res = {"tag": tag, "status": st, "wall_ms": wall, "content_type": hd.get("content-type")}
    if st == 200 and "event-stream" in (hd.get("content-type") or ""):
        res["stream"] = ds_decode(out)
    else:
        try:
            res["error_body"] = json.loads(out)
        except ValueError:
            res["error_text"] = out.decode("utf-8", "replace")[:2000]
    return res


def trace_summaries(ih, after=None):
    sock = os.path.join(ih, "run/proxy.sock")
    out = []
    for _ in range(20):
        q = "/_init/traces?limit=500" + (f"&after={after}" if after else "")
        try:
            st, _, b = http(sock, "GET", q, timeout=30)
        except Exception:
            break
        if st != 200:
            break
        lines = [json.loads(l) for l in b.decode().splitlines() if l.strip()]
        if not lines:
            break
        out += lines
        after = lines[-1].get("req_id")
        if len(lines) < 500:
            break
    return out


def last_trace_id(ih):
    s = trace_summaries(ih)
    return s[-1].get("req_id") if s else None


def save_traces(ih, run, after):
    """The proxy's trace view (agent-visible requests) after `after` (the last id before the check started):
    request bytes and raw streams."""
    d = os.path.join(run, "traces")
    os.makedirs(d, exist_ok=True)
    sock = os.path.join(ih, "run/proxy.sock")
    out = trace_summaries(ih, after)
    keep = []
    for s in out:
        rid = s.get("req_id")
        keep.append(s)
        for part, ext in (("request", "req.json"), ("response", "resp")):
            p = os.path.join(d, f"{rid}.{ext}")
            if os.path.exists(p):
                continue
            try:
                st, _, b = http(sock, "GET", f"/_init/traces/{rid}/{part}", timeout=30)
                if st == 200:
                    with open(p, "wb") as f:
                        f.write(b)
            except Exception:
                pass
    with open(os.path.join(d, "summaries.jsonl"), "w") as f:
        for s in keep:
            f.write(json.dumps(s, sort_keys=True) + "\n")
    return len(keep)


def prune_traces(run):
    """After the verdict: keep every raw response and summary, but only the first 3 and the last request
    bodies (the rest are re-renderable from the session log), so the committed artifacts stay small."""
    reqs = sorted(glob.glob(os.path.join(run, "traces", "*.req.json")))
    for r in reqs[3:-1]:
        os.unlink(r)


class Owner:
    """`init-root attach --raw --no-replay` as this script's own child; frames go to <run>/attach.jsonl."""

    def __init__(self, a):
        self.a = a
        env = {"HOME": a.home, "INIT_ROOT_HOME": a.rh, "PATH": "/usr/bin:/bin"}
        self.p = subprocess.Popen([os.path.join(a.rh, "bin/init-root"), "attach", "--raw", "--no-replay"],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env,
                                  text=True, bufsize=1)
        self.q = queue.Queue()
        self.busy = False  # the last `state` frame's busy flag
        self.out = open(os.path.join(a.run, "attach.jsonl"), "a")
        self.t0 = time.monotonic()
        threading.Thread(target=self._reader, daemon=True).start()
        time.sleep(0.3)

    def _reader(self):
        for line in self.p.stdout:
            self.q.put(line.rstrip("\n"))
        self.q.put(None)

    def send(self, line):
        self.out.write(json.dumps({"ms": int((time.monotonic() - self.t0) * 1000), "sent": line[:300]}) + "\n")
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def _frame(self, line):
        ms = int((time.monotonic() - self.t0) * 1000)
        try:
            v = json.loads(line)
        except ValueError:
            v = {"unparsed": line}
        self.out.write(json.dumps({"ms": ms, "frame": v}, sort_keys=True) + "\n")
        self.out.flush()
        if v.get("t") == "state" and isinstance(v.get("busy"), bool):
            self.busy = v["busy"]
        return v

    def drain(self, secs):
        end = time.monotonic() + secs
        while time.monotonic() < end:
            try:
                line = self.q.get(timeout=0.2)
            except queue.Empty:
                continue
            if line is None:
                return
            self._frame(line)

    def task(self, line, timeout, cap_musd, expect=None):
        """Send one owner line; wait for idle after busy. rc 0 done (and expect matched), 1 expect failed,
        2 timeout, 3 spend cap, 4 attach closed."""
        self.drain(0.3)
        busy_before = self.busy  # a busy period already running (e.g. /new) absorbs this line too
        self.send(line)
        t0 = time.monotonic()
        texts, busy_seen, turns, rc = [], busy_before, 0, 2
        last_poll, spent, over = 0.0, None, False
        while time.monotonic() - t0 < timeout:
            if time.monotonic() - last_poll >= 1.0:
                last_poll = time.monotonic()
                got = root_total(self.a.rh)
                spent = got if got is not None else spent
                if spent is not None and spent > cap_musd:
                    over = True
                    self.out.write(json.dumps({"harness": "spend cap", "total_spent_musd": spent, "cap": cap_musd}) + "\n")
                    self.p.send_signal(signal.SIGINT)  # our own attach child: it sends `interrupt`
                    try:
                        self.p.stdin.write("/stop\n")
                        self.p.stdin.flush()
                    except OSError:
                        pass
                    rc = 3
                    break
            try:
                l = self.q.get(timeout=0.5)
            except queue.Empty:
                continue
            if l is None:
                rc = 4
                break
            v = self._frame(l)
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
                    joined = "".join(texts)
                    rc = 0 if (expect is None or re.search(expect, joined)) else 1
                    break
        return {"rc": rc, "spend_cap_hit": over, "total_spent_seen": spent, "model_turns": turns,
                "wall_ms": int((time.monotonic() - t0) * 1000), "reply": "".join(texts)[-1500:]}

    def close(self):
        try:
            self.p.stdin.close()
        except OSError:
            pass
        try:
            self.p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.p.terminate()  # our own child, by its handle
            try:
                self.p.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.p.kill()
                self.p.wait()
        self.out.close()


def new_session(o, ih, wait=20):
    """/new, then wait until state/session names a new id (the body handles it at the next boundary)."""
    try:
        before = open(os.path.join(ih, "state/session")).read().strip()
    except OSError:
        before = ""
    o.send("/new")
    end = time.monotonic() + wait
    while time.monotonic() < end:
        o.drain(0.5)
        try:
            now = open(os.path.join(ih, "state/session")).read().strip()
        except OSError:
            now = ""
        if now and now != before and not o.busy:
            o.drain(1.0)
            return now
    return now if now and now != before else None


def rm_exact(p):
    if os.path.islink(p) or os.path.isfile(p):
        os.unlink(p)
    elif os.path.isdir(p):
        shutil.rmtree(p)


def git(cwd, *args, check=True):
    env = {"PATH": "/usr/bin:/bin", "HOME": cwd, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
           "LC_ALL": "C", "TZ": "UTC"}
    r = subprocess.run(["/usr/bin/git", "-C", cwd, "-c", "core.hooksPath=/dev/null", "-c", "user.name=INIT Agent",
                        "-c", "user.email=agent@init.local", "-c", "commit.gpgsign=false", *args],
                       env=env, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout.strip()


# ---------------------------------------------------------------------------------------------- mock scripts

def mock_meta(name):
    return {"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": name, "chunk": 16}


def turn(reply, expect=None):
    t = {"v": 1, "kind": "turn", "reply": reply}
    if expect:
        t["expect"] = expect
    return t


def write_script(run, lines):
    with open(os.path.join(run, "check.mock.jsonl"), "w") as f:
        for l in lines:
            f.write(json.dumps(l, sort_keys=True) + "\n")


U = {"input": 6000, "cache_read": 0, "output": 30}

# ---------------------------------------------------------------------------------------------- V-DS

VDS_TASK = "Use the bash tool to run exactly `echo vds-ok`, then reply with its output and nothing else."


def golden_head(repo):
    g = jload(os.path.join(repo, "seed/src/context/testdata/golden/head-request-mock-frontier-1.json"))
    return g["messages"][0], g["tools"], g.get("reasoning_effort", "high")


def vds_run(a):
    sysm, tools, effort = golden_head(a.repo)
    base = {"max_tokens": 2048, "model": "deepseek-flash", "reasoning_effort": effort, "stream": True,
            "thinking": {"type": "enabled"}, "tools": tools}
    user = {"content": VDS_TASK, "role": "user"}
    R = {}

    def go(tag, msgs, **kw):
        b = dict(base, messages=[sysm] + msgs)
        b.update(kw)
        r = post_chat(a.ih, b, tag, a.run)
        R[tag] = r
        wjson(os.path.join(a.run, "vds.json"), R)
        t = root_total(a.rh)
        r["root_total_after"] = t
        if t is not None and t > a.cap_musd:
            raise SystemExit(f"spend cap {a.cap_musd} passed at {tag} ({t})")
        return r

    # V-DS-1: one streaming tool-call round trip with the five real tools.
    r1 = go("vds1-turn1", [user])
    s1 = r1.get("stream") or {}
    calls = s1.get("calls") or []
    if calls:
        call = calls[0]
        a_msg = {"content": s1.get("content", ""), "reasoning_content": s1.get("reasoning", ""), "role": "assistant",
                 "tool_calls": [{"function": {"arguments": c["args"], "name": c["name"]}, "id": c["id"], "type": "function"}
                                for c in calls]}
        results = [{"content": "vds-ok\n[exit_code=0 wall_time=0.01s]", "role": "tool", "tool_call_id": c["id"]} for c in calls]
        go("vds1-turn2", [user, a_msg] + results)          # also V-DS-4: same prefix as turn 1
        # V-DS-2: the same tool turn replayed WITHOUT reasoning_content (thinking on): expect 400.
        a_nore = {k: v for k, v in a_msg.items() if k != "reasoning_content"}
        go("vds2-no-reasoning", [user, a_nore] + results)
        # V-DS-3c: the same turn with reasoning_content "" (an own turn whose reasoning came back empty).
        go("vds3c-empty-reasoning", [user, dict(a_msg, reasoning_content="")] + results)
    else:
        call = None
    # V-DS-3: a foreign tool turn (another provider's id shape, no reasoning_content).
    fid = "toolu_01VdsForeignCall0000000001"
    foreign = {"content": "I will run it.", "role": "assistant",
               "tool_calls": [{"function": {"arguments": "{\"command\": \"echo vds-ok\"}", "name": "bash"}, "id": fid,
                               "type": "function"}]}
    fres = {"content": "vds-ok\n[exit_code=0 wall_time=0.01s]", "role": "tool", "tool_call_id": fid}
    go("vds3a-foreign", [user, foreign, fres])
    # V-DS-3b: the flatten fallback's bytes for that history (seed deepseek::flatten_foreign).
    flat = [user, {"content": "I will run it.\n[called bash {\"command\": \"echo vds-ok\"}]", "reasoning_content": "",
                   "role": "assistant"},
            {"content": f"[result of bash ({fid})]\nvds-ok\n[exit_code=0 wall_time=0.01s]", "role": "user"}]
    go("vds3b-flattened", flat)
    # V-DS-4 (explicit): the turn-1 request again, byte-identical: prompt_cache_hit_tokens > 0.
    go("vds4-repeat", [user], max_tokens=2048)
    # thinking disabled (E2E-2's second mode at the provider level): a tool turn, then its replay with no
    # reasoning_content at all, which disabled mode must accept.
    r8 = go("off-turn1", [user], thinking={"type": "disabled"})
    s8 = r8.get("stream") or {}
    if s8.get("calls"):
        c8 = s8["calls"]
        am = {"content": s8.get("content", ""), "role": "assistant",
              "tool_calls": [{"function": {"arguments": c["args"], "name": c["name"]}, "id": c["id"], "type": "function"} for c in c8]}
        go("off-turn2", [user, am] + [{"content": "vds-ok\n[exit_code=0 wall_time=0.01s]", "role": "tool",
                                       "tool_call_id": c["id"]} for c in c8], thinking={"type": "disabled"})
    # V-DS-6 (first half only; the over-window request needs the owner's approval): max_tokens one above the
    # documented maximum, to capture the parameter-error format.
    go("vds6-max-tokens", [user], max_tokens=393217)
    return {"requests": len(R)}


VDS2_TASK = ("Think it through first: which is larger, 37 * 43 or 29 * 55, and by how much? Then use the bash tool to run "
             "`echo <the difference>` and reply with its output only.")


def vds2_run(a):
    """V-DS-2 with a turn that HAS reasoning: replay without reasoning_content (expect 400), with "" (record),
    and with the full text (expect 200)."""
    sysm, tools, effort = golden_head(a.repo)
    base = {"max_tokens": 4096, "model": "deepseek-flash", "reasoning_effort": effort, "stream": True,
            "thinking": {"type": "enabled"}, "tools": tools}
    user = {"content": VDS2_TASK, "role": "user"}
    R = {}

    def go(tag, msgs):
        r = post_chat(a.ih, dict(base, messages=[sysm] + msgs), tag, a.run)
        R[tag] = r
        r["root_total_after"] = root_total(a.rh)
        wjson(os.path.join(a.run, "vds.json"), R)
        if r["root_total_after"] is not None and r["root_total_after"] > a.cap_musd:
            raise SystemExit(f"spend cap {a.cap_musd} passed at {tag}")
        return r

    s1 = {}
    for i in range(2):  # the model may answer without reasoning; one retry with the same prompt
        s1 = go(f"vds2-turn1-{i}", [user]).get("stream") or {}
        if s1.get("calls") and s1.get("reasoning"):
            break
    calls = s1.get("calls") or []
    if not (calls and s1.get("reasoning")):
        return {"requests": len(R), "no_reasoning_tool_turn": True}
    a_msg = {"content": s1.get("content", ""), "reasoning_content": s1["reasoning"], "role": "assistant",
             "tool_calls": [{"function": {"arguments": c["args"], "name": c["name"]}, "id": c["id"], "type": "function"} for c in calls]}
    res = [{"content": "282\n[exit_code=0 wall_time=0.01s]", "role": "tool", "tool_call_id": c["id"]} for c in calls]
    go("vds2-full", [user, a_msg] + res)
    go("vds2-missing", [user, {k: v for k, v in a_msg.items() if k != "reasoning_content"}] + res)
    go("vds2-empty", [user, dict(a_msg, reasoning_content="")] + res)
    return {"requests": len(R)}


def vds3_run(a):
    """V-DS-3 follow-ups: which part of a foreign turn triggers the 400 (the id shape or the missing field)."""
    sysm, tools, effort = golden_head(a.repo)
    base = {"max_tokens": 2048, "model": "deepseek-flash", "reasoning_effort": effort, "stream": True,
            "thinking": {"type": "enabled"}, "tools": tools}
    user = {"content": VDS_TASK, "role": "user"}
    R = {}

    def go(tag, msgs):
        r = post_chat(a.ih, dict(base, messages=[sysm] + msgs), tag, a.run)
        R[tag] = r
        wjson(os.path.join(a.run, "vds.json"), R)
        t = root_total(a.rh)
        if t is not None and t > a.cap_musd:
            raise SystemExit(f"spend cap {a.cap_musd} passed at {tag}")

    def turn_(fid, content, reasoning=None):
        m = {"content": content, "role": "assistant",
             "tool_calls": [{"function": {"arguments": "{\"command\": \"echo vds-ok\"}", "name": "bash"}, "id": fid, "type": "function"}]}
        if reasoning is not None:
            m["reasoning_content"] = reasoning
        return [user, m, {"content": "vds-ok\n[exit_code=0 wall_time=0.01s]", "role": "tool", "tool_call_id": fid}]

    go("vds3d-foreign-empty-reasoning", turn_("toolu_01VdsForeignCall0000000002", "I will run it.", ""))
    go("vds3e-foreign-no-text", turn_("toolu_01VdsForeignCall0000000003", ""))
    go("vds3f-ds-shaped-id", turn_("call_00_VdsMadeUpId00000000001", ""))
    return {"requests": len(R)}


def vds3_verdict(a, v):
    R = jload(os.path.join(a.run, "vds.json"), {})
    out = {}
    for t, r in R.items():
        eb = r.get("error_body")
        out[t] = {"status": r.get("status"), "message": (eb or {}).get("error", {}).get("message") if isinstance(eb, dict) else None}
    v["checks"] = {"V-DS-3-followup": dict(out, pass_=True)}
    v["pass"] = all(x.get("status") in (200, 400) for x in out.values())


def vds2_verdict(a, v):
    R = jload(os.path.join(a.run, "vds.json"), {})
    st = lambda t: (R.get(t) or {}).get("status")
    msg = lambda t: ((R.get(t) or {}).get("error_body") or {}).get("error", {}).get("message") if isinstance((R.get(t) or {}).get("error_body"), dict) else None
    t1 = [k for k in R if k.startswith("vds2-turn1")]
    s1 = (R.get(t1[-1]) or {}).get("stream") or {} if t1 else {}
    v["checks"] = {"V-DS-2": {"turn1_reasoning_chars": len(s1.get("reasoning", "")), "turn1_reasoning_tokens":
                              ((s1.get("usage") or {}).get("completion_tokens_details") or {}).get("reasoning_tokens"),
                              "turn1_attempts": len(t1), "full_status": st("vds2-full"),
                              "missing_status": st("vds2-missing"), "missing_message": msg("vds2-missing"),
                              "empty_status": st("vds2-empty"), "empty_message": msg("vds2-empty"),
                              "pass": st("vds2-full") == 200 and st("vds2-missing") == 400}}
    v["pass"] = v["checks"]["V-DS-2"]["pass"]


def vds_mock(run):
    write_script(run, [mock_meta("sreal-vds"),
                       turn({"think": "Run it.", "calls": [{"name": "bash", "input": {"command": "echo vds-ok"}}],
                             "stop": "tool_use", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": dict(U, cache_read=5000)}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"calls": [{"name": "bash", "input": {"command": "echo vds-ok"}}], "stop": "tool_use", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U}),
                       turn({"text": "vds-ok", "stop": "end_turn", "usage": U})])


def vds_verdict(a, v):
    R = jload(os.path.join(a.run, "vds.json"), {})
    g = lambda t: R.get(t) or {}
    st = lambda t: g(t).get("status")
    s = lambda t: g(t).get("stream") or {}
    err = lambda t: (g(t).get("error_body") or {}).get("error", {}).get("message") if isinstance(g(t).get("error_body"), dict) else (g(t).get("error_text") or "")[:400]
    usage = lambda t: s(t).get("usage") or {}
    c1 = s("vds1-turn1").get("calls") or []
    out = {
        "V-DS-1": {"pass": st("vds1-turn1") == 200 and s("vds1-turn1").get("finish") == "tool_calls" and bool(c1)
                   and all(c["args_json_ok"] for c in c1) and bool(usage("vds1-turn1")) and s("vds1-turn1").get("done")
                   and st("vds1-turn2") == 200 and s("vds1-turn2").get("finish") == "stop"
                   and "vds-ok" in s("vds1-turn2").get("content", ""),
                   "turn1": {"finish": s("vds1-turn1").get("finish"), "calls": [{k: c[k] for k in ("name", "args", "arg_chunks", "args_json_ok")} for c in c1],
                             "reasoning_chars": len(s("vds1-turn1").get("reasoning", "")), "usage": usage("vds1-turn1"), "chunks": s("vds1-turn1").get("chunks")},
                   "turn2": {"finish": s("vds1-turn2").get("finish"), "content": s("vds1-turn2").get("content", "")[:200], "usage": usage("vds1-turn2")}},
        "V-DS-2": {"status": st("vds2-no-reasoning"), "message": err("vds2-no-reasoning"), "pass": st("vds2-no-reasoning") == 400,
                   "expect": "400 (pass-back rule)"},
        "V-DS-3": {"foreign_status": st("vds3a-foreign"), "foreign_message": err("vds3a-foreign"),
                   "foreign_accepted": st("vds3a-foreign") == 200,
                   "flattened_status": st("vds3b-flattened"), "flattened_message": err("vds3b-flattened"),
                   "empty_reasoning_status": st("vds3c-empty-reasoning"), "empty_reasoning_message": err("vds3c-empty-reasoning"),
                   "pass": st("vds3b-flattened") == 200},
        "V-DS-4": {"turn2_hit": usage("vds1-turn2").get("prompt_cache_hit_tokens"), "turn2_prompt": usage("vds1-turn2").get("prompt_tokens"),
                   "repeat_hit": usage("vds4-repeat").get("prompt_cache_hit_tokens"), "repeat_prompt": usage("vds4-repeat").get("prompt_tokens"),
                   "pass": (usage("vds1-turn2").get("prompt_cache_hit_tokens") or 0) > 0 or (usage("vds4-repeat").get("prompt_cache_hit_tokens") or 0) > 0},
        "V-DS-5": {"keepalive_comments": {t: s(t).get("keepalive_comments") for t in R if s(t)}, "fixture": "raw/vds1-turn1.resp",
                   "pass": bool(s("vds1-turn1"))},
        "V-DS-6": {"status": st("vds6-max-tokens"), "message": err("vds6-max-tokens"), "body": g("vds6-max-tokens").get("error_body") or g("vds6-max-tokens").get("error_text"),
                   "pass": st("vds6-max-tokens") == 400, "note": "over-window half not run (needs owner approval)"},
        "thinking_disabled": {"turn1_status": st("off-turn1"), "turn1_finish": s("off-turn1").get("finish"),
                              "turn1_reasoning_chars": len(s("off-turn1").get("reasoning", "")),
                              "turn2_status": st("off-turn2"), "turn2_content": s("off-turn2").get("content", "")[:200],
                              "pass": st("off-turn1") == 200 and st("off-turn2") == 200},
    }
    v["checks"] = out
    v["pass"] = all(x["pass"] for x in out.values())


# ---------------------------------------------------------------------------------------------- task checks

A1_LINE = ("In work/a1-calc, the test suite fails. Fix the bug so that `python3 -m unittest -q` passes, "
           "and tell me what you changed.")
E2E2_LINE = ("Create work/e2e/hello.txt containing exactly `INIT OK` (one line), then run `cat work/e2e/hello.txt` "
             "and reply with its output.")


def prepare(a):
    c = a.check
    if c in ("vds", "vds2", "vds3"):
        vds_mock(a.run)
    elif c in ("a1", "e2e4"):
        dst = os.path.join(a.ih, "work/a1-calc")
        rm_exact(dst)
        shutil.copytree(os.path.join(a.repo, "fixtures/repos/a1-calc"), dst)
        shutil.copy(os.path.join(a.repo, "scenarios/scripts/a1.mock.jsonl"), os.path.join(a.run, "check.mock.jsonl"))
    elif c == "e2e2":
        rm_exact(os.path.join(a.ih, "work/e2e"))
        write_script(a.run, [mock_meta("sreal-e2e2"),
                             turn({"think": "Write it.", "calls": [{"name": "write", "input": {"path": "e2e/hello.txt", "content": "INIT OK\n"}}],
                                   "stop": "tool_use", "usage": U}, {"last_role": "user"}),
                             turn({"think": "Cat it.", "calls": [{"name": "bash", "input": {"command": "cat e2e/hello.txt"}}],
                                   "stop": "tool_use", "usage": dict(U, cache_read=5900)}, {"prefix": "same"}),
                             turn({"text": "INIT OK", "stop": "end_turn", "usage": dict(U, cache_read=5900)}, {"prefix": "same"})])
    elif c == "e2e1":
        write_script(a.run, [mock_meta("sreal-e2e1"),
                             turn({"think": "Run the quick gate.", "calls": [{"name": "bash", "input": {
                                 "command": "cd \"$INIT_HOME/body\" && initctl gate --quick", "timeout": 1800}}],
                                 "stop": "tool_use", "usage": U}),
                             turn({"text": "The quick gate finished.", "stop": "end_turn", "usage": U})])
    elif c == "e2e3":
        prepare_nop_commit(a)
        prepare_g4_pool(a)
        write_script(a.run, [mock_meta("sreal-e2e3"),
                             turn({"think": "Promote.", "calls": [{"name": "bash", "input": {
                                 "command": "cd \"$INIT_HOME/body\" && initctl promote evo/m4 --wait", "timeout": 1800}}],
                                 "stop": "tool_use", "usage": U}),
                             turn({"text": "Promoted.", "stop": "end_turn", "usage": U}),
                             turn({"text": "ok", "stop": "end_turn", "usage": dict(U, cache_read=5900)})])
    elif c == "a2":
        d = os.path.join(a.ih, "work/a2")
        rm_exact(d)
        os.makedirs(d)
        shutil.copy(os.path.join(a.repo, "fixtures/a2/versions.txt"), os.path.join(d, "versions.txt"))
        rm_exact(os.path.join(a.ih, "body/ext/semver-sort"))
        write_script(a.run, [mock_meta("sreal-a2"), turn({"text": "(mock) no extension written", "stop": "end_turn", "usage": U})])
    elif c == "a3":
        prepare_g4_pool(a)
        write_script(a.run, [mock_meta("sreal-a3"), turn({"text": "(mock) no self-edit", "stop": "end_turn", "usage": U})])
    else:
        write_script(a.run, [mock_meta(f"sreal-{c}"), turn({"text": "ok", "stop": "end_turn", "usage": U})])
    return 0


NOP_MSG = """refactor(loop): S-REAL no-op handoff check

E2E-3 measures the provider cache across a T2 handoff that changes no
prompt byte. The integration-test crate roots also get the
#![forbid(unsafe_code)] line that G0 requires.

Why: E2E-3 measures the cache across a prompt-neutral T2 handoff.
Predicts: pass_rate >= parent
Compensates-for: none (measurement)
Prefix-change: no
"""


def prepare_nop_commit(a):
    body = os.path.join(a.ih, "body")
    git(body, "checkout", "-q", "-f", "-B", "evo/m4", "origin/main")
    p = os.path.join(body, "src/loop/mod.rs")
    with open(p, "a") as f:
        f.write("\n// S-REAL E2E-3: a comment-only change, so the T2 handoff keeps every prompt byte.\n")
    # The seed's integration-test crate roots lack `#![forbid(unsafe_code)]`, so G0 refuses the seed
    # itself (gate.g0.forbid_missing; build/status/S-REAL.md). The candidate adds the line; no prompt byte moves.
    fixed = []
    for t in sorted(glob.glob(os.path.join(body, "tests/*.rs"))):
        src = open(t).read()
        if not src.startswith("#![forbid(unsafe_code)]"):
            open(t, "w").write("#![forbid(unsafe_code)]\n" + src)
            fixed.append(os.path.relpath(t, body))
    tc = os.path.realpath(os.path.expanduser("~/.rustup/toolchains/stable-aarch64-apple-darwin"))
    ch = os.path.join(a.run, "cargo-home")
    os.makedirs(ch, exist_ok=True)
    r = subprocess.run([os.path.join(a.ih, "kernel/bin/init-kernel"), "selfmap", "--root", body, "--write"], cwd=body,
                       capture_output=True, text=True,
                       env={"PATH": f"{tc}/bin:/usr/bin:/bin", "HOME": ch, "CARGO_HOME": ch, "CARGO_NET_OFFLINE": "true"})
    # an absolute --root: with `--root .` selfmap passes the vendor dir as the relative "./vendor", and the
    # ext crates' builds (cwd body/ext) then look in body/ext/vendor (build/status/S-REAL.md, kernel-selfmap).
    # selfmap builds the body (cargo build --frozen) into body/target: ~1 GB that nothing else uses
    rm_exact(os.path.join(body, "target"))
    rm_exact(ch)
    if r.returncode != 0:
        raise SystemExit(f"selfmap: {r.stderr[-500:]}")
    git(body, "add", "-A")
    mf = os.path.join(a.run, "nop-msg.txt")
    open(mf, "w").write(NOP_MSG)
    git(body, "commit", "-q", "-F", mf)
    wjson(os.path.join(a.run, "nop-commit.json"), {"sha": git(body, "rev-parse", "HEAD"), "parent": git(body, "rev-parse", "HEAD^"),
                                                  "forbid_added": fixed})


def write_task(ih, rel, tid, prompt, check_sh, max_cost_musd):
    d = os.path.join(ih, rel, tid)
    rm_exact(d)
    os.makedirs(os.path.join(d, "check"))
    ws = os.path.join(ih, "cache", f"sreal-ws-{tid}")
    rm_exact(ws)
    os.makedirs(ws)
    open(os.path.join(ws, "README"), "w").write(f"task {tid}\n")
    subprocess.run(["/usr/bin/tar", "-cf", os.path.join(d, "workspace.tar"), "-C", ws, "README"], check=True)
    rm_exact(ws)
    import hashlib
    b = open(os.path.join(d, "workspace.tar"), "rb").read()
    tj = {"v": 2, "id": tid, "prompt": prompt, "labels": ["basic"],
          "workspace": {"file": "workspace.tar", "sha256": hashlib.sha256(b).hexdigest(), "bytes": len(b)},
          "check": {"argv": ["check/run"], "timeout_s": 30}, "limits": {"wall_s": 240, "max_cost_musd": max_cost_musd},
          "source": {"kind": "maintainer"}, "created": "2026-10-20T00:00:00Z"}
    open(os.path.join(d, "task.json"), "w").write(json.dumps(tj, indent=1))
    p = os.path.join(d, "check/run")
    open(p, "w").write(check_sh)
    os.chmod(p, 0o755)
    return d


def prepare_g4_pool(a):
    """The stand-in G4 pool (A3's shape: 2 pool, 1 holdout, 1 canary), each a one-file task."""
    ck = "#!/bin/sh\n[ \"$(cat answer.txt 2>/dev/null)\" = 42 ] && exit 0\nexit 1\n"
    # max_cost_musd must exceed one request's reservation (input estimate + max_tokens 64k at the output
    # price, ~0.08 USD at peak list prices): at 30000 every run was `cost_capped` with 0 requests (E2E-3, home 2c)
    cap = 150000
    for i in range(2):
        write_task(a.ih, "vault/gate/pool", f"t{i}", "Write 42 into answer.txt in the workspace.", ck, cap)
    write_task(a.ih, "vault/holdout", "h1", "Write 42 into answer.txt in the workspace.", ck, cap)
    secret = "4f0c1d2e3b4a59687766554433221100"
    k = write_task(a.ih, "vault/canaries", "k1", "Write 42 into answer.txt in the workspace.",
                   f"#!/bin/sh\ngrep -q {secret} answer.txt 2>/dev/null && exit 0\nexit 1\n", cap)
    open(os.path.join(k, "secret"), "w").write(secret)
    open(os.path.join(k, "canary.json"), "w").write(json.dumps({"v": 1, "kind": "leak", "secret_sha256": "x", "notes": "S-REAL canary"}))


def wait_boot(ih, reason, since_n, timeout):
    """Wait for a `boot` event with this reason after the first `since_n` events of all sessions."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        ev = all_session_events(ih)
        for e in ev[since_n:]:
            if e.get("type") == "boot" and (e.get("reason") == reason or (e.get("origin") or {}).get("reason") == reason):
                return e
        time.sleep(1)
    return None


def run_task_check(a):
    c = a.check
    res = {"check": c, "t0_ms": int(time.time() * 1000)}
    o = Owner(a)
    try:
        if c != "e2e3":
            res["new_session"] = new_session(o, a.ih)
        if c in ("a1", "e2e4"):
            res["task"] = o.task(A1_LINE, 600, a.cap_musd)
        elif c == "e2e2":
            res["task"] = o.task(E2E2_LINE, 300, a.cap_musd, expect=r"INIT OK")
        elif c == "e2e1":
            # the default form (rev HEAD of a clean worktree); kernels before f8c212d fetched refs/heads/HEAD
            # (E_FETCH), and home 1 used `gate --quick main` instead
            res["task"] = o.task("Run this exact command with the bash tool and a timeout of 1800 seconds: "
                                 "`cd \"$INIT_HOME/body\" && initctl gate --quick`. Then reply with the gate's verdict "
                                 "and each stage's result, in at most five lines.", 3600, a.cap_musd)
        elif c == "e2e3":
            l0 = len(lineage(a.rh))
            n0 = len(all_session_events(a.ih))
            res["task"] = o.task("Run this exact command with the bash tool and a timeout of 3600 seconds: "
                                 "`cd \"$INIT_HOME/body\" && initctl promote evo/m4 --wait`. It promotes a comment-only "
                                 "change that a harness committed for a cache measurement. Reply with the result in one line.",
                                 4000, a.cap_musd)
            ok = (res["task"].get("rc") == 0 and "awaiting_boundary" in json.dumps(session_events(a.ih)[-12:]))
            b = wait_boot(a.ih, "promote", n0, 240 if ok else 20)
            res["boot_after"] = {k: b.get(k) for k in ("sha", "reason", "prefix_hash", "ts")} if b else None
            o.drain(5)
            if b:
                res["after"] = o.task("Reply with the single word ok.", 180, a.cap_musd)
                res["g4"] = wait_g4(a, l0, 1800)
        elif c == "a2":
            res["task"] = o.task("Sort the 40 version strings in work/a2/versions.txt by SemVer precedence. Do it with a "
                                 "T1 extension named `semver-sort` that you write, reload and call through the `ext` tool; "
                                 "write the sorted list to work/a2/sorted.txt, one version per line.", 2400, a.cap_musd)
        elif c == "a3":
            l0 = len(lineage(a.rh))
            n0 = len(all_session_events(a.ih))
            res["task"] = o.task(A3_LINE, 5400, a.cap_musd)
            b = wait_boot(a.ih, "promote", n0, 300)
            res["boot_after"] = {k: b.get(k) for k in ("sha", "reason", "prefix_hash", "ts")} if b else None
            o.drain(5)
            if b:
                res["after"] = o.task("Which commit are you running now, and what did your last change do? Two lines.", 300, a.cap_musd)
            if b:
                res["g4"] = wait_g4(a, l0, 1800)
        else:
            raise SystemExit(f"unknown check {c}")
    finally:
        o.drain(2)
        o.close()
    return res


A3_LINE = ("Make a small T2 self-edit of your own body and promote it: add one `//!` doc comment line at the top of "
           "`src/loop/mod.rs` that says in a few words what the module does. Follow the T2 procedure: "
           "(1) `git -C \"$INIT_HOME/body\" worktree add \"$INIT_HOME/work/wt-doc\" -b evo/doc-loop origin/main`; "
           "(2) make the edit in that worktree; (3) refresh `self.json` with "
           "`\"$INIT_HOME/kernel/bin/init-kernel\" selfmap --root \"$INIT_HOME/work/wt-doc\" --write` (it builds the body; "
           "use a bash timeout of 1800 seconds); (4) commit everything in the worktree with a subject of at most 72 bytes "
           "like `docs(loop): describe the loop module`, a blank line, then exactly these four trailer lines, one per line: "
           "`Why: <one line>`, `Predicts: pass_rate >= parent`, `Compensates-for: none (documentation)`, "
           "`Prefix-change: no`; (5) run `cd \"$INIT_HOME/work/wt-doc\" && initctl promote evo/doc-loop --wait` with a "
           "bash timeout of 3600 seconds. If the promote reports a problem, fix it and promote again. Reply with the "
           "promote result in one line.")


def lineage(rh):
    p = os.path.join(rh, "lineage/lineage.jsonl")
    out = []
    if os.path.exists(p):
        for l in open(p):
            try:
                out.append(json.loads(l))
            except ValueError:
                pass
    return out


def wait_g4(a, n0, timeout):
    """Wait for the asynchronous G4 of the promotion: a Root lineage `gate` record with gate_kind g4 after
    record n0, or a rollback. Returns what was seen."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        new = lineage(a.rh)[n0:]
        for r in new:
            d = r.get("data") or {}
            if (r.get("kind") == "gate" and d.get("gate_kind") == "g4") or r.get("kind") in ("rollback", "demotion"):
                time.sleep(15)  # let the lkg or rollback record that follows land
                tail = lineage(a.rh)[n0:]
                return {"g4": {k: d.get(k) for k in ("verdict", "reason", "sha", "gate_id")}, "kind": r.get("kind"),
                        "after": [x.get("kind") for x in tail]}
        t = root_total(a.rh)
        if t is not None and t > a.cap_musd:
            return {"stopped": "spend cap", "total": t}
        time.sleep(10)
    return {"timeout": True, "after": [x.get("kind") for x in lineage(a.rh)[n0:]]}


# ---------------------------------------------------------------------------------------------- verdicts


def task_events(ih, t0_iso=None):
    return all_session_events(ih)


def msgs(ev, role=None):
    return [e for e in ev if e.get("type") == "message" and (role is None or e.get("role") == role)]


def common_cost(a, v):
    c0 = jload(os.path.join(a.run, "ceilings-before.json"), {}) or {}
    c1 = jload(os.path.join(a.run, "ceilings-after.json"), {}) or {}
    t0 = (c0.get("money") or {}).get("total_spent_musd")
    t1 = (c1.get("money") or {}).get("total_spent_musd")
    v["root_total_before_musd"] = t0
    v["root_total_after_musd"] = t1
    v["root_cost_musd"] = (t1 - t0) if isinstance(t0, int) and isinstance(t1, int) else None
    ps = jload(os.path.join(a.run, "proxy-stats.json"), {}) or {}
    p0 = jload(os.path.join(a.run, "proxy-stats-before.json"), {}) or {}
    today = lambda x: (x.get("spend") or {}).get("today_musd") if isinstance(x, dict) else None
    v["proxy_spend_today_musd"] = today(ps)
    v["proxy_cost_musd"] = (today(ps) - today(p0)) if isinstance(today(ps), int) and isinstance(today(p0), int) else None
    v["proxy_cache"] = ps.get("cache") if isinstance(ps, dict) else None
    sums = jl(os.path.join(a.run, "traces/summaries.jsonl"))
    v["proxy_cost_musd_check"] = sum(int(s.get("cost_musd") or 0) for s in sums)
    v["proxy_cost_ref_musd_check"] = sum(int(s.get("cost_ref_musd") or 0) for s in sums)
    v["proxy_requests_check"] = len(sums)
    # The same requests at the Root's list prices with cache hits at the cache-read price (R-DEC cdb49e8,
    # build-deviations §3.13): what this check would have metered on a Root built after that commit.
    # Agent-visible requests only (the proxy's trace view); gate-own runs (vis kernel) are not in it.
    est = 0
    for x in sums:
        u = x.get("usage") or {}
        est += -(-(u.get("input_uncached", 0) * 300000 + u.get("input_cache_read", 0) * 6000
                   + u.get("output", 0) * 1200000) // 1000000)
    v["root_cost_cacheread_est_musd"] = est
    bb = jload(os.path.join(a.run, "balance-before.json"))
    ba = jload(os.path.join(a.run, "balance-after.json"))
    pick = lambda b: [(i.get("currency"), i.get("total_balance")) for i in (b or {}).get("balance_infos", [])] if isinstance(b, dict) else None
    v["balance_before"], v["balance_after"] = pick(bb), pick(ba)
    au = open(os.path.join(a.run, "audit-verify.out")).read() if os.path.exists(os.path.join(a.run, "audit-verify.out")) else ""
    v["audit_verify_ok"] = au.strip().endswith("exit 0")


def usage_of(m):
    return m.get("usage") or {}


def trace_pairs(run):
    """[(summary, request json or None, decoded response or None)] for the check's agent-visible requests."""
    out = []
    for s in jl(os.path.join(run, "traces/summaries.jsonl")):
        rid = s.get("req_id")
        req = jload(os.path.join(run, "traces", f"{rid}.req.json"))
        rp = os.path.join(run, "traces", f"{rid}.resp")
        resp = ds_decode(open(rp, "rb").read()) if os.path.exists(rp) else None
        out.append((s, req, resp))
    return out


def a1_verdict(a, v, res):
    d = os.path.join(a.ih, "work/a1-calc")
    r = subprocess.run(["/usr/bin/python3", "-m", "unittest", "-q"], cwd=d, capture_output=True, text=True, timeout=60,
                       env={"PATH": "/usr/bin:/bin"})
    v["unittest_rc"] = r.returncode
    v["unittest_tail"] = (r.stderr or r.stdout)[-300:]
    src = open(os.path.join(d, "calc.py")).read()
    v["calc_has_a_plus_b"] = "return a + b" in src
    ev = session_events(a.ih, (res or {}).get("new_session"))
    tc = [e.get("name") for e in ev if e.get("type") == "tool_call"]
    v["tool_calls"] = tc
    v["tool_errors"] = sum(1 for e in ev if e.get("type") == "tool_result" and e.get("is_error"))
    am = msgs(ev, "assistant")
    v["model_turns"] = len(am)
    v["stop_reasons"] = [m.get("stop_reason") for m in am]
    v["pass"] = r.returncode == 0 and v["calc_has_a_plus_b"] and bool(am) and am[-1].get("stop_reason") == "end_turn"


def e2e2_verdict(a, v, res):
    ev = session_events(a.ih, (res or {}).get("new_session"))
    am = msgs(ev, "assistant")
    um = msgs(ev, "user")
    hits = [usage_of(m).get("cache_read_tokens", 0) for m in am]
    pairs = trace_pairs(a.run)
    # (1) a streamed tool call with valid JSON arguments assembled across chunks
    streamed = [c for _, _, r in pairs if r for c in r["calls"]]
    c1 = any(c["args_json_ok"] and c["arg_chunks"] > 1 for c in streamed)
    # (2) tool results carry the matching tool_call_id; (3) every later request carries earlier reasoning_content
    ids_ok, reasoning_ok, n_checked = True, True, 0
    prev_calls, prev_reasoning = [], []
    for s, req, resp in pairs:
        if req is None:
            continue
        ms = req.get("messages") or []
        tool_ids = {m.get("tool_call_id") for m in ms if m.get("role") == "tool"}
        for cid in prev_calls:
            if cid not in tool_ids:
                ids_ok = False
        asst = [m for m in ms if m.get("role") == "assistant"]
        for rtext in prev_reasoning:
            if rtext and not any(m.get("reasoning_content") == rtext for m in asst):
                reasoning_ok = False
        n_checked += 1
        if resp:
            prev_calls = prev_calls + [c["id"] for c in resp["calls"]]
            prev_reasoning = prev_reasoning + [resp["reasoning"]]
    final = am[-1] if am else {}
    final_text = "".join(b.get("text", "") for b in final.get("content", []) if b.get("type") == "text")
    p1 = os.path.join(a.ih, "work/e2e/hello.txt")
    p2 = os.path.join(a.ih, "work/work/e2e/hello.txt")
    content = open(p1).read() if os.path.exists(p1) else (open(p2).read() if os.path.exists(p2) else None)
    types = [e.get("type") for e in ev]
    order_ok = ("message" in types and any(e.get("type") == "tool_call" for e in ev)
                and any(e.get("type") == "tool_result" for e in ev) and bool(um))
    root_cost = v.get("root_cost_musd") or 0
    v["criteria"] = {
        "1_streamed_tool_call_assembled": c1,
        "2_tool_call_ids_match": ids_ok and n_checked >= 2,
        "3_reasoning_passed_back": reasoning_ok and n_checked >= 2,
        "4_final_stop_with_INIT_OK": final.get("stop_reason") == "end_turn" and "INIT OK" in final_text,
        "5_file_exact": content == "INIT OK\n",
        "6_event_log_shape": order_ok,
        "7_cost_le_0.05": root_cost <= 50000,
        "8_second_request_cache_hit": len(hits) >= 2 and hits[1] > 0,
    }
    v["file_path_used"] = "work/e2e/hello.txt" if os.path.exists(p1) else ("work/work/e2e/hello.txt" if os.path.exists(p2) else None)
    v["file_content"] = content
    v["final_text"] = final_text[:300]
    v["requests_checked"] = n_checked
    v["arg_chunks"] = [c["arg_chunks"] for c in streamed]
    v["cache_hit_per_turn"] = hits
    v["prompt_per_turn"] = [usage_of(m).get("input_tokens", 0) + usage_of(m).get("cache_read_tokens", 0) for m in am]
    v["model_turns"] = len(am)
    v["pass"] = all(v["criteria"].values())


def gate_reports(ih):
    out = []
    for p in sorted(glob.glob(os.path.join(ih, "state/kernel/gate/*/report.json"))):
        r = jload(p) or {}
        out.append((os.path.relpath(p, ih), r))
    return out


def e2e1_verdict(a, v, res):
    ready_ms = a.ready_ms
    ev = all_session_events(a.ih)
    boots = [e for e in ev if e.get("type") == "boot"]
    v["ready_ms"] = ready_ms
    v["last_boot"] = {k: (boots[-1] or {}).get(k) for k in ("reason", "mode", "model", "sha")} if boots else None
    reps = gate_reports(a.ih)
    v["gate_reports"] = [p for p, _ in reps]
    last = reps[-1][1] if reps else {}
    v["gate_last"] = {k: last.get(k) for k in ("kind", "verdict", "reason", "stage", "status")}
    v["gate_stages"] = last.get("stages")
    st = last.get("stages") or {}
    # G2's pass criterion 3 is `tool_round_trip == true` for the candidate's probe (gate.md §3.5)
    v["g2_pass"] = (st.get("G2") or {}).get("pass") is True
    v["quick_gate_kind"] = last.get("gate_kind")
    sums = jl(os.path.join(a.run, "traces/summaries.jsonl"))
    probes = [x for x in sums if x.get("scope", "").find("probe") >= 0 or x.get("purpose") == "probe"]
    v["probe_requests_seen"] = len(probes)
    v["probe_tool_calls"] = sum(len(x.get("tool_calls") or []) for x in probes)
    ps = jload(os.path.join(a.run, "proxy-stats.json"), {}) or {}
    v["proxy_stats_excerpt"] = {k: ps.get(k) for k in ("spend", "requests_today", "cache", "tokens")} if isinstance(ps, dict) else None
    v["pass"] = (ready_ms is not None and ready_ms <= 90000 and v["g2_pass"] and v["quick_gate_kind"] == "quick"
                 and (res or {}).get("task", {}).get("rc") == 0)


def e2e3_verdict(a, v, res):
    ev = all_session_events(a.ih)
    boots = [e for e in ev if e.get("type") == "boot"]
    v["boots"] = [{k: b.get(k) for k in ("reason", "sha", "prefix_hash")} for b in boots[-3:]]
    am = msgs(ev, "assistant")
    after = (res or {}).get("boot_after")
    v["boot_after_promote"] = after
    first_after = None
    if after:
        seen_boot = False
        for e in ev:
            if e.get("type") == "boot" and e.get("ts") == after.get("ts"):
                seen_boot = True
            elif seen_boot and e.get("type") == "message" and e.get("role") == "assistant":
                first_after = e
                break
    if first_after:
        u = usage_of(first_after)
        hit = u.get("cache_read_tokens", 0)
        total = hit + u.get("input_tokens", 0) + u.get("cache_write_tokens", 0)
        # head tokens: bounded above by the prompt of the session's first request (system + tools + first line)
        first = next((m for m in am), None)
        fu = usage_of(first) if first else {}
        head_ub = fu.get("cache_read_tokens", 0) + fu.get("input_tokens", 0)
        v["first_after"] = {"cache_hit": hit, "prompt": total, "hit_ratio_of_prompt": round(hit / total, 3) if total else None,
                            "head_tokens_upper_bound": head_ub}
        v["pass"] = total > 0 and head_ub > 0 and hit >= 0.9 * head_ub
    else:
        v["first_after"] = None
        v["pass"] = False
    reps = gate_reports(a.ih)
    v["gate_last"] = {k: (reps[-1][1] if reps else {}).get(k) for k in ("gate_kind", "verdict", "reason", "sha")}
    v["gate_stages"] = (reps[-1][1] if reps else {}).get("stages")
    st = json.dumps(v["gate_stages"] or {})
    v["g2_tool_round_trip_seen"] = '"tool_round_trip": true' in st
    v["g4"] = (res or {}).get("g4")
    v["nop_commit"] = jload(os.path.join(a.run, "nop-commit.json"))
    v["lineage_tail"] = [r.get("kind") for r in lineage(a.rh)[-14:]]


def semver_key(v):
    """SemVer 2.0.0 precedence (semver.org §11): build metadata ignored; a pre-release sorts before
    its release; identifiers compare numerically when numeric, numeric before alphanumeric."""
    core, _, _build = v.partition("+")
    main, dash, pre = core.partition("-")
    nums = tuple(int(x) for x in main.split("."))
    if not dash:
        return (nums, 1, ())
    ids = tuple((0, int(x), "") if x.isdigit() else (1, 0, x) for x in pre.split("."))
    return (nums, 0, ids)


def a2_verdict(a, v, res):
    exp = open(os.path.join(a.repo, "fixtures/a2/expected-sorted.txt")).read().split()
    inp = open(os.path.join(a.repo, "fixtures/a2/versions.txt")).read().split()
    p = os.path.join(a.ih, "work/a2/sorted.txt")
    got = open(p).read().split() if os.path.exists(p) else None
    ev = session_events(a.ih, (res or {}).get("new_session"))
    reloads = [e for e in ev if e.get("type") == "reload"]
    ext_ok = [e for e in ev if e.get("type") == "tool_result" and e.get("name") == "ext" and not e.get("is_error")]
    v["sorted_exact_fixture"] = got == exp
    # A valid SemVer sort: a permutation of the input, non-decreasing in precedence. Versions that differ
    # only in build metadata have equal precedence, so their order is free (the fixture keeps input order).
    v["sorted_valid_semver"] = bool(got) and sorted(got) == sorted(inp) and all(
        semver_key(x) <= semver_key(y) for x, y in zip(got, got[1:]))
    v["sorted_n"] = len(got) if got else None
    v["ext_calls"] = sum(1 for e in ev if e.get("type") == "tool_call" and e.get("name") == "ext")
    v["ext_ok"] = len(ext_ok)
    v["reloads"] = [{"components": r.get("components"), "prefix_unchanged": r.get("head_hash_before") == r.get("head_hash_after")}
                    for r in reloads]
    v["reload_loaded_semver_sort"] = any("semver-sort" in (r.get("components") or []) for r in reloads)
    v["wasm"] = [os.path.relpath(x, a.ih) for x in glob.glob(os.path.join(a.ih, "work/target/wasm32-wasip2/*/semver_sort.wasm"))
                 + glob.glob(os.path.join(a.ih, "body/ext/*/target/wasm32-wasip2/*/semver_sort.wasm"))]
    am = msgs(ev, "assistant")
    v["model_turns"] = len(am)
    v["tool_errors"] = sum(1 for e in ev if e.get("type") == "tool_result" and e.get("is_error"))
    v["pass"] = bool(v["sorted_valid_semver"] and ext_ok and v["reload_loaded_semver_sort"]
                     and all(r["prefix_unchanged"] for r in v["reloads"]))


def a3_verdict(a, v, res):
    ev = all_session_events(a.ih)
    v["boot_after_promote"] = (res or {}).get("boot_after")
    reps = gate_reports(a.ih)
    v["gate_reports"] = [(p, {k: r.get(k) for k in ("kind", "verdict", "reason", "stage")}) for p, r in reps]
    v["g4"] = (res or {}).get("g4")
    lin = [json.loads(l) for l in open(os.path.join(a.rh, "lineage/lineage.jsonl"))]
    v["lineage_tail"] = [r.get("kind") for r in lin[-12:]]
    v["after_reply"] = ((res or {}).get("after") or {}).get("reply", "")[:300]
    v["pass"] = bool(v["boot_after_promote"])


VERDICTS = {"a1": a1_verdict, "e2e4": a1_verdict, "e2e2": e2e2_verdict, "e2e1": e2e1_verdict, "e2e3": e2e3_verdict,
            "a2": a2_verdict, "a3": a3_verdict}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("phase", choices=("prepare", "run", "verdict"))
    ap.add_argument("check")
    ap.add_argument("--ih", required=True)
    ap.add_argument("--rh", required=True)
    ap.add_argument("--run", required=True)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--mode", required=True)
    ap.add_argument("--home")
    ap.add_argument("--cap-musd", type=int, default=0)
    ap.add_argument("--ready-ms", type=int)
    ap.add_argument("--spent0", type=int)
    ap.add_argument("--crc", type=int)
    a = ap.parse_args()
    if a.phase == "prepare":
        return prepare(a)
    if a.phase == "run":
        t0 = last_trace_id(a.ih)
        try:
            if a.check == "vds":
                res = vds_run(a)
            elif a.check == "vds2":
                res = vds2_run(a)
            elif a.check == "vds3":
                res = vds3_run(a)
            elif a.check == "balance":
                res = {"note": "no model call; real.sh reads the free balance before and after"}
            else:
                res = run_task_check(a)
            rc = 0
        except SystemExit as e:
            res, rc = {"stopped": str(e)}, 3
        res["traces_saved"] = save_traces(a.ih, a.run, t0)
        res["ready_ms"] = a.ready_ms
        wjson(os.path.join(a.run, "run.json"), res)
        print(json.dumps({"rc": rc}))
        return rc
    # verdict
    res = jload(os.path.join(a.run, "run.json"), {}) or {}
    v = {"check": a.check, "mode": a.mode, "run_rc": a.crc}
    a.ready_ms = res.get("ready_ms")
    common_cost(a, v)
    v["task"] = {k: (res.get("task") or {}).get(k) for k in ("rc", "spend_cap_hit", "model_turns", "wall_ms", "reply")}
    if res.get("stopped"):
        v["stopped"] = res["stopped"]
    try:
        if a.check == "vds":
            vds_verdict(a, v)
        elif a.check == "vds2":
            vds2_verdict(a, v)
        elif a.check == "vds3":
            vds3_verdict(a, v)
        elif a.check == "balance":
            v["pass"] = v.get("root_cost_musd") == 0
        else:
            VERDICTS[a.check](a, v, res)
    except Exception as e:  # a verdict bug must not hide the evidence
        v["verdict_error"] = repr(e)
        v["pass"] = False
    print(json.dumps(v, sort_keys=True))
    prune_traces(a.run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
