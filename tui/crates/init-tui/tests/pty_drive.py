"""Drive init-tui in TTY mode through a pseudo-terminal (docs/tui.md TUI-12, TUI-13). Stdlib only;
run as `python3 -I pty_drive.py <init-tui> <ROOT_HOME> [args...]`.

It plays a minimal terminal: answers the cursor-position query (CSI 6n) and the primary device
attributes query (CSI c, without kitty keyboard support), applies window sizes with TIOCSWINSZ,
and records every byte the TUI writes. Commands come one per line on stdin, as JSON arrays:
  ["type", "text"]            write bytes to the TUI (keys)
  ["typing", "text", 0.03]    one key at a time, with a pause between keys
  ["wait", "needle", secs]    wait until the output since the last mark contains needle
  ["mark"]                    start a new search window
  ["resize", rows, cols]      TIOCSWINSZ on the terminal (the kernel sends SIGWINCH)
  ["sleep", secs]
  ["exit", secs]              wait for the TUI to exit; then print the summary and stop
Each command answers one line on stdout: "ok" or "fail <why>". The summary is one JSON line.
The only process signalled is our own child, by its pid, after the exit timeout.
"""

import fcntl
import json
import os
import select
import struct
import sys
import termios
import threading
import time


def main():
    exe, rh = sys.argv[1], sys.argv[2]
    extra = sys.argv[3:]
    rows, cols = 24, 80
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    env = {"TERM": "xterm-256color", "HOME": os.path.dirname(rh), "PATH": "/usr/bin:/bin"}
    if "INIT_AGENT_LAUNCHER" in os.environ:  # what scripts/init-agent passes the TUI (v0.5.3)
        env["INIT_AGENT_LAUNCHER"] = os.environ["INIT_AGENT_LAUNCHER"]
    pid = os.fork()
    if pid == 0:
        os.close(master)
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for i in (0, 1, 2):
            os.dup2(slave, i)
        if slave > 2:
            os.close(slave)
        # a shell around the TUI prints the terminal modes before and after it (`stty -g`) and its exit code
        script = 'printf "STTY0:%s\\n" "$(stty -g)"; "$0" "$@"; c=$?; printf "\\nEXIT:%s STTY1:%s\\n" "$c" "$(stty -g)"'
        os.execve("/bin/sh", ["/bin/sh", "-c", script, exe, "--root-home", rh] + extra, env)
    print("pid %d" % pid, flush=True)
    out = bytearray()
    mark = 0
    state = {"cy": 1}

    lock = threading.Lock()
    done = threading.Event()

    def reader():
        # a terminal reads its output continuously: a TUI must never block on its own writes here
        while not done.is_set():
            r, _, _ = select.select([master], [], [], 0.05)
            if not r:
                continue
            try:
                b = os.read(master, 65536)
            except OSError:
                return
            if not b:
                return
            with lock:
                out.extend(b)
            # answer the queries a terminal would
            if b"\x1b[6n" in b:
                os.write(master, ("\x1b[%d;1R" % state["cy"]).encode())
            if b"\x1b[c" in b:
                os.write(master, b"\x1b[?62;22c")

    threading.Thread(target=reader, daemon=True).start()

    def pump(timeout):
        with lock:
            n0 = len(out)
        time.sleep(timeout)
        with lock:
            return len(out) > n0

    def wait_for(needle, secs):
        n = needle.encode()
        end = time.time() + secs
        while time.time() < end:
            with lock:
                if n in out[mark:]:
                    return True
            time.sleep(0.01)
        with lock:
            return n in out[mark:]

    code = None
    for line in sys.stdin:
        cmd = json.loads(line)
        op = cmd[0]
        ok = True
        why = ""
        if op == "type":
            os.write(master, cmd[1].encode())
            pump(0.01)
        elif op == "typing":
            for ch in cmd[1]:
                os.write(master, ch.encode())
                end = time.time() + cmd[2]
                while time.time() < end:
                    pump(max(0.0, end - time.time()))
        elif op == "wait":
            ok = wait_for(cmd[1], cmd[2])
            why = "no %r in %r" % (cmd[1], bytes(out[mark:][-400:]))
        elif op == "mark":
            while pump(0.05):
                pass
            with lock:
                mark = len(out)
        elif op == "resize":
            rows, cols = cmd[1], cmd[2]
            state["cy"] = rows
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            pump(0.05)
        elif op == "sleep":
            end = time.time() + cmd[1]
            while time.time() < end:
                pump(max(0.0, end - time.time()))
        elif op == "exit":
            end = time.time() + cmd[1]
            while time.time() < end:
                pump(0.02)
                p, status = os.waitpid(pid, os.WNOHANG)
                if p == pid:
                    code = os.waitstatus_to_exitcode(status)
                    break
            if code is None:
                os.kill(pid, 9)  # our own child, by its pid
                os.waitpid(pid, 0)
                ok, why = False, "no exit"
            while pump(0.1):
                pass
            done.set()
            with lock:
                text = out.decode("utf-8", "replace")
            s0 = text.split("STTY0:", 1)[1].split("\n", 1)[0].strip() if "STTY0:" in text else "?"
            tail = text.rsplit("EXIT:", 1)[1] if "EXIT:" in text else ""
            tui_exit = int(tail.split(" ", 1)[0]) if tail else None
            s1 = tail.split("STTY1:", 1)[1].split("\n", 1)[0].strip() if "STTY1:" in tail else "!"
            summary = {"exit": tui_exit, "shell_exit": code, "termios_same": s0 == s1, "stty": [s0, s1], "output": text}
            print("ok" if ok else "fail " + why, flush=True)
            print(json.dumps(summary), flush=True)
            return
        print("ok" if ok else "fail " + why, flush=True)
    # stdin closed without "exit": stop our child
    os.kill(pid, 9)
    os.waitpid(pid, 0)


if __name__ == "__main__":
    main()
