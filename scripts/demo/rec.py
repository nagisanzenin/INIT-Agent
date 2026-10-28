"""Record a terminal program into an asciinema v2 cast with a caption row (TUI-OBSERVE recording kit,
scripts/demo/record-mock.sh). Stdlib only; run as `python3 -I rec.py [options] -- PROGRAM [ARGS...]`.

The recorder is the program's terminal: it runs PROGRAM in a pseudo-terminal one row shorter than the
recording, keeps the last row for the caption (DECSTBM: the program scrolls rows 1..R-1 only) and
repaints the caption after every chunk the program writes, so every frame carries it. It answers the
queries a terminal would: the cursor position (CSI 6n), primary device attributes (CSI c) and the
background colour (OSC 11, the brand night #0D0E11).

  --out CAST        the cast (asciinema v2: a header line, then [t, "o", data] events and "m" markers)
  --cols C --rows R the recording's size (the program gets C x R-1)
  --caption TEXT    the caption row (it must say "mock model" for a mock run, brand §09 rule 4)
  --ctl FIFO        commands, one per line: `key NAME` (ctrl-o, ctrl-d, ctrl-c, enter, esc, q, or one
                    character), `type TEXT`, `mark LABEL`, `end` (Ctrl-D, then wait 5 s for the exit)
  --env K=V         an environment variable for PROGRAM (repeatable; the base is TERM, COLORTERM, LANG, PATH)

Exit: PROGRAM's exit code. The only process signalled is our own child, by its pid, when it does not
exit within 5 s of `end`.
"""

import argparse
import codecs
import fcntl
import json
import os
import select
import struct
import sys
import termios
import time

KEYS = {"ctrl-o": "\x0f", "ctrl-d": "\x04", "ctrl-c": "\x03", "enter": "\r", "esc": "\x1b", "tab": "\t"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--cols", type=int, default=100)
    ap.add_argument("--rows", type=int, default=32)
    ap.add_argument("--caption", required=True)
    ap.add_argument("--ctl")
    ap.add_argument("--env", action="append", default=[])
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
    if not cmd:
        ap.error("no program")
    cols, rows, inner = a.cols, a.rows, a.rows - 1
    env = {"TERM": "xterm-256color", "COLORTERM": "truecolor", "LANG": "en_US.UTF-8", "PATH": "/usr/bin:/bin"}
    for kv in a.env:
        k, _, v = kv.partition("=")
        env[k] = v

    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", inner, cols, 0, 0))
    pid = os.fork()
    if pid == 0:
        os.close(master)
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for i in (0, 1, 2):
            os.dup2(slave, i)
        if slave > 2:
            os.close(slave)
        os.execve(cmd[0], cmd, env)
    os.close(slave)

    # the caption: bone on the hairline grey, never the Root's amber (brand: amber is only the Root)
    text = (" " + a.caption)[:cols].ljust(cols)
    caption = "\x1b7\x1b[%d;1H\x1b[0;38;2;232;228;216;48;2;36;39;45m%s\x1b[0m\x1b8" % (rows, text)
    t0 = time.monotonic()
    out = open(a.out, "w", encoding="utf-8")
    out.write(json.dumps({"version": 2, "width": cols, "height": rows, "timestamp": int(time.time()), "title": a.caption,
                          "env": {"TERM": env["TERM"]}}) + "\n")

    def event(kind, data):
        out.write(json.dumps([round(time.monotonic() - t0, 6), kind, data], ensure_ascii=False) + "\n")
        out.flush()

    # the program scrolls rows 1..R-1; the caption row stays
    event("o", "\x1b[%d;1r\x1b[H\x1b[2J" % inner + caption)
    ctl = os.open(a.ctl, os.O_RDWR | os.O_NONBLOCK) if a.ctl else None
    dec = codecs.getincrementaldecoder("utf-8")(errors="replace")
    pending, ctlbuf, end_at, code, killed = b"", b"", None, None, False

    def send(s):
        os.write(master, s.encode())

    while True:
        fds = [master] + ([ctl] if ctl is not None else [])
        r, _, _ = select.select(fds, [], [], 0.05)
        if master in r:
            try:
                b = os.read(master, 65536)
            except OSError:
                b = b""
            if b:
                q = pending + b
                if b"\x1b[6n" in q:
                    send("\x1b[%d;1R" % inner)
                if b"\x1b]11;?" in q:
                    send("\x1b]11;rgb:0d0d/0e0e/1111\x1b\\")
                if b"\x1b[c" in q or b"\x1b[0c" in q:
                    send("\x1b[?62;22c")
                pending = q[-8:]
                event("o", dec.decode(b) + caption)
        if ctl is not None and ctl in r:
            try:
                ctlbuf += os.read(ctl, 4096)
            except BlockingIOError:
                pass
            while b"\n" in ctlbuf:
                line, ctlbuf = ctlbuf.split(b"\n", 1)
                op, _, arg = line.decode("utf-8", "replace").partition(" ")
                if op == "key":
                    send(KEYS.get(arg, arg))
                elif op == "type":
                    for ch in arg:
                        send(ch)
                        time.sleep(0.03)
                elif op == "mark":
                    event("m", arg)
                elif op == "end":
                    # Ctrl-D leaves an empty box; Ctrl-C twice leaves anyway (the agent keeps running)
                    send(KEYS["ctrl-d"])
                    time.sleep(0.5)
                    send(KEYS["ctrl-c"] + KEYS["ctrl-c"])
                    end_at = time.monotonic() + 5
        p, status = os.waitpid(pid, os.WNOHANG)
        if p == pid:
            code = os.waitstatus_to_exitcode(status)
            break
        # never block in waitpid: an exiting program waits for the terminal to read its last output
        if end_at is not None and time.monotonic() > end_at:
            os.kill(pid, 9 if killed else 15)  # our own child, by its pid
            end_at = None if killed else time.monotonic() + 5
            killed = True
    # drain what is left, then put the terminal back (no scroll region)
    while True:
        r, _, _ = select.select([master], [], [], 0.1)
        if not r:
            break
        try:
            b = os.read(master, 65536)
        except OSError:
            break
        if not b:
            break
        event("o", dec.decode(b) + caption)
    # the scroll region reset goes out with the caption, so even the last event carries it (REL-050: the check counts it)
    event("o", "\x1b[r" + caption)
    out.close()
    sys.exit(code if code is not None else 1)


if __name__ == "__main__":
    main()
