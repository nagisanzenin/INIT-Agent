#!/usr/bin/python3 -I
"""REAL-M7 watchdog: the disk and memory guard that runs beside a real line (orchestrator condition (b), 2026-10-21).

D's first G4 was stopped when free disk fell from 7.2 GB to 0.24 GB. The cause was swap, not run roots: one sandboxed
python3.9 inside the sealed slice grew to 43.9 GB of compressed memory, macOS grew its swap files on the same APFS
volume until `vm_swap_create_file` failed (ENOSPC), and jetsam killed it ("low swap"). Free disk fell more than 3.7 GB
inside one 30 s poll. So this guard polls every <= 5 s and acts on two readings:

  - disk: free < --min-free-mb (6000) -> SIGSTOP the line's process tree (everything below the Root), freeze spend
    (`checks_m7.py freeze`: every pool's ceiling := spent, applied live), then SIGKILL the tree, stop the Root by PID
    (TERM, 5 s, KILL) and the extra --pid processes (the line's driver). Exit 3, after writing --trip-file.
  - memory (optional, --mem-cap-mb > 0): a workload process below the Root whose physical footprint (resident +
    compressed, proc_pid_rusage) exceeds the cap is SIGKILLed alone, the way jetsam did but before swap grows. The
    Root and the kernel are never memory-killed (logged only; the disk guard backs them up).

The tree is the descendants of --root-pid by ppid, re-read every poll. A process that was seen once stays tracked
after its parent exits (orphans reparent to launchd), keyed by pid and start time, so a reused pid is never signalled.
Never signalled: pid <= 1, this process, its parent (m7.sh), --deny pids, and any process whose command line names
the owner's live homes (~/.init, ~/.init-root).

  m7watch.py run --root-pid P [--pid D]... --rh RH --home HOME --log FILE [--trip-file F] [--min-free-mb 6000]
                 [--mem-cap-mb 0] [--poll-s 5] [--freeze-cmd ARGV_JSON]
  m7watch.py selftest      pure decisions + a live process-tree test ($0, no disk fill)
  m7watch.py fill-test --dir SCRATCH [--fill-mb 300]   the $0 guard test: a fake line tree, a real fill of
                 --fill-mb in SCRATCH, a threshold just above the filled level; checks the trip order and that every
                 tracked pid (an orphan and a memory hog included) is gone; deletes the fill by exact path.
"""
import argparse
import ctypes
import json
import os
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
NEVER_MEM_KILL = ("init-root", "init-kernel")
OWNER_MARKS = ("/.init-root", "/.init/")


# ------------------------------------------------------------------------------------------------ readings


def free_mb(path="/"):
    s = os.statvfs(path)
    return s.f_bavail * s.f_frsize // (1 << 20)


def swap_used_mb():
    try:
        out = subprocess.run(["/usr/sbin/sysctl", "-n", "vm.swapusage"], capture_output=True, text=True, timeout=5).stdout
        for part in out.split("  "):
            part = part.strip()
            if part.startswith("used = "):
                return int(float(part[7:].rstrip("M")))
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return None


class _RU0(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(n, ctypes.c_uint64) for n in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins", "wired_size", "resident_size",
        "phys_footprint", "proc_start_abstime", "proc_exit_abstime")]


try:
    _LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib")
except OSError:
    _LIBPROC = None


def rusage(pid):
    """(phys_footprint bytes, start abstime) of one of our own processes, or None (gone, or not ours)."""
    if _LIBPROC is None:
        return None
    ru = _RU0()
    if _LIBPROC.proc_pid_rusage(ctypes.c_int(pid), ctypes.c_int(0), ctypes.byref(ru)) != 0:
        return None
    if ru.proc_exit_abstime:  # a zombie: exited, not yet reaped by its parent
        return None
    return ru.phys_footprint, ru.proc_start_abstime


def ps_table():
    """{pid: (ppid, comm, args)} for every process (one `ps` call)."""
    out = subprocess.run(["/bin/ps", "-axww", "-o", "pid=,ppid=,ucomm=,args="], capture_output=True, text=True, timeout=10).stdout
    t = {}
    for ln in out.splitlines():
        f = ln.split(None, 3)
        if len(f) >= 3 and f[0].isdigit() and f[1].isdigit():
            t[int(f[0])] = (int(f[1]), f[2], f[3] if len(f) > 3 else "")
    return t


# ------------------------------------------------------------------------------------------------ decisions (pure)


def descendants(table, roots):
    """The pids below `roots` (roots excluded), by ppid."""
    kids = {}
    for pid, (ppid, _, _) in table.items():
        kids.setdefault(ppid, []).append(pid)
    out, stack = set(), list(roots)
    while stack:
        for c in kids.get(stack.pop(), []):
            if c not in out and c not in roots:
                out.add(c)
                stack.append(c)
    return out


def signallable(pid, args, me, parent, deny):
    return pid > 1 and pid not in (me, parent) and pid not in deny and not any(m in (args or "") for m in OWNER_MARKS)


def disk_trip(free, min_free):
    return free is not None and free < min_free


def mem_victims(procs, cap_mb):
    """procs: {pid: (comm, footprint_mb)} below the Root -> [(pid, comm, mb)] over the cap, largest first; the Root and
    the kernel are never victims (they are logged by the caller)."""
    if cap_mb <= 0:
        return []
    v = [(p, c, mb) for p, (c, mb) in procs.items() if mb > cap_mb and c not in NEVER_MEM_KILL]
    return sorted(v, key=lambda x: -x[2])


# ------------------------------------------------------------------------------------------------ the watchdog


class Watch:
    def __init__(self, a):
        self.a = a
        self.me, self.parent = os.getpid(), os.getppid()
        self.deny = set(a.deny or [])
        self.roots = [p for p in a.root_pid if p > 1]
        self.extra = [p for p in (a.pid or []) if p > 1]
        self.tracked = {}  # pid -> (start abstime, comm)
        self.start = {p: (rusage(p) or (0, None))[1] for p in self.roots + self.extra}
        self.logf = open(a.log, "a", buffering=1)

    def log(self, **kw):
        kw["t"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        self.logf.write(json.dumps(kw, sort_keys=True) + "\n")

    def alive(self, pid, st0):
        r = rusage(pid)
        return r is not None and (st0 is None or r[1] == st0)

    def refresh(self):
        table = ps_table()
        for pid in descendants(table, set(self.roots)):
            ppid, comm, args = table[pid]
            if not signallable(pid, args, self.me, self.parent, self.deny):
                continue
            r = rusage(pid)
            if r is None:
                continue
            old = self.tracked.get(pid)
            if old is None or old[0] != r[1]:
                self.tracked[pid] = (r[1], comm)
        for pid in list(self.tracked):
            if not self.alive(pid, self.tracked[pid][0]):
                del self.tracked[pid]
        return table

    def sig(self, pid, s, st0):
        if self.alive(pid, st0):
            try:
                os.kill(pid, s)
                return True
            except ProcessLookupError:
                return False
            except PermissionError:
                return False
        return False

    def trip(self, why, free):
        self.refresh()
        tree = dict(self.tracked)
        stopped = sum(self.sig(p, signal.SIGSTOP, st) for p, (st, _) in tree.items())
        self.log(ev="trip", why=why, free_mb=free, stopped=stopped, tracked=len(tree))
        fz = {"rc": None}
        try:
            r = subprocess.run(self.a.freeze_argv, capture_output=True, text=True, timeout=45)
            fz = {"rc": r.returncode, "out": r.stdout.strip()[-300:], "err": r.stderr.strip()[-200:]}
        except (OSError, subprocess.SubprocessError) as e:
            fz = {"rc": None, "err": str(e)[:200]}
        self.log(ev="freeze", **fz)
        self.refresh()
        tree.update(self.tracked)
        for p, (st, _) in tree.items():
            self.sig(p, signal.SIGKILL, st)
        for p in self.roots + self.extra:
            self.sig(p, signal.SIGTERM, self.start.get(p))
        end = time.monotonic() + 5
        while time.monotonic() < end and any(self.alive(p, self.start.get(p)) for p in self.roots + self.extra):
            time.sleep(0.1)
        for p in self.roots + self.extra:
            self.sig(p, signal.SIGKILL, self.start.get(p))
        time.sleep(0.3)
        left = [p for p, (st, _) in tree.items() if self.alive(p, st)] + [p for p in self.roots + self.extra if self.alive(p, self.start.get(p))]
        self.log(ev="stopped", killed=len(tree), left=left, free_mb=free_mb())
        if self.a.trip_file:
            with open(self.a.trip_file, "w") as f:
                f.write(json.dumps({"why": why, "free_mb": free, "freeze": fz, "left": left}) + "\n")
        return 3

    def run(self):
        self.log(ev="start", roots=self.roots, extra=self.extra, min_free_mb=self.a.min_free_mb, mem_cap_mb=self.a.mem_cap_mb,
                 poll_s=self.a.poll_s, free_mb=free_mb(), swap_used_mb=swap_used_mb())
        last_hb = 0.0
        peak = 0
        while any(self.alive(p, self.start.get(p)) for p in self.roots):
            free = free_mb()
            if disk_trip(free, self.a.min_free_mb):
                return self.trip(f"disk_below_{self.a.min_free_mb}MB", free)
            self.refresh()
            procs = {}
            for p, (st, comm) in self.tracked.items():
                r = rusage(p)
                if r and r[1] == st:
                    procs[p] = (comm, r[0] // (1 << 20))
            big = max((mb for _, mb in procs.values()), default=0)
            peak = max(peak, big)
            for p, comm, mb in mem_victims(procs, self.a.mem_cap_mb):
                ok = self.sig(p, signal.SIGKILL, self.tracked[p][0])
                self.log(ev="mem_kill", pid=p, comm=comm, footprint_mb=mb, cap_mb=self.a.mem_cap_mb, killed=ok)
            for p, (comm, mb) in procs.items():
                if self.a.mem_cap_mb > 0 and mb > self.a.mem_cap_mb and comm in NEVER_MEM_KILL:
                    self.log(ev="mem_over_cap_not_killed", pid=p, comm=comm, footprint_mb=mb)
            if time.monotonic() - last_hb >= 60:
                last_hb = time.monotonic()
                self.log(ev="hb", free_mb=free, swap_used_mb=swap_used_mb(), procs=len(procs), max_footprint_mb=big, peak_mb=peak)
            time.sleep(self.a.poll_s)
        self.log(ev="end", why="root_exited", free_mb=free_mb(), peak_mb=peak)
        return 0


# ------------------------------------------------------------------------------------------------ tests ($0)

TREE = r"""
import os, sys, time
role = sys.argv[1]
if role == "root":
    # child A forks an orphan-to-be B, then exits after 3 s; C is a plain sleeper; H allocates and touches pages
    for r in ("a", "c", "h"):
        if os.fork() == 0:
            os.execv(sys.executable, [sys.executable, "-I", __file__, r] + sys.argv[2:])
    while True:
        time.sleep(1)
elif role == "a":
    if os.fork() == 0:
        os.execv(sys.executable, [sys.executable, "-I", __file__, "b"])
    time.sleep(3)
    os._exit(0)
elif role == "h":
    mb = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    blob = b"\x01" * (mb << 20)
    while True:
        time.sleep(1)
else:
    while True:
        time.sleep(1)
"""


def _spawn_tree(d, hog_mb):
    script = os.path.join(d, "m7watch_tree.py")
    with open(script, "w") as f:
        f.write(TREE)
    return subprocess.Popen([sys.executable, "-I", script, "root", str(hog_mb)], start_new_session=True), script


def selftest():
    fails = []

    def ok(c, name):
        print(("ok   " if c else "FAIL ") + name)
        if not c:
            fails.append(name)

    t = {10: (1, "init-root", "x"), 11: (10, "init-kernel", ""), 12: (11, "init-body", ""), 13: (12, "python3.9", ""),
         14: (10, "init-body", "/rh/sealed/run/abc/bin/init-body"), 20: (1, "other", ""), 21: (20, "python3.9", "")}
    ok(descendants(t, {10}) == {11, 12, 13, 14}, "descendants by ppid, roots excluded, strangers excluded")
    ok(descendants(t, {99}) == set(), "no tree for a gone root")
    ok(not signallable(1, "", 5, 6, set()) and not signallable(5, "", 5, 6, set()) and not signallable(6, "", 5, 6, set()),
       "never pid 1, self or parent")
    ok(not signallable(40, "/Users/x/.init-root/bin/init-root start", 5, 6, set()), "never the owner's ~/.init-root")
    ok(not signallable(41, "/Users/x/.init/bin/init-body", 5, 6, set()), "never the owner's ~/.init")
    ok(not signallable(31845, "", 5, 6, {31845}), "deny list")
    ok(signallable(42, "/Users/x/Documents/INIT Agent/target/m7-homes/r/rh/bin/init-root", 5, 6, set()), "the test home is ours")
    ok(disk_trip(5999, 6000) and not disk_trip(6000, 6000) and not disk_trip(None, 6000), "disk trip strictly below")
    procs = {12: ("init-body", 300), 13: ("python3.9", 9000), 11: ("init-kernel", 9000), 15: ("rustc", 5000)}
    ok(mem_victims(procs, 4096) == [(13, "python3.9", 9000), (15, "rustc", 5000)], "memory victims: workloads only, largest first")
    ok(mem_victims(procs, 0) == [], "memory cap off")
    ok(rusage(os.getpid()) is not None and rusage(os.getpid())[0] > 0, "proc_pid_rusage reads our footprint")
    ok(free_mb() > 0, "statvfs free")
    # live: the tree is tracked, the orphan stays tracked after its parent exits
    d = os.environ.get("TMPDIR") or "/tmp"
    p, script = _spawn_tree(d, 0)
    try:
        time.sleep(1.0)
        a = argparse.Namespace(root_pid=[p.pid], pid=[], deny=[], log=os.devnull, trip_file=None, min_free_mb=0, mem_cap_mb=0,
                               poll_s=0.2, freeze_argv=["/usr/bin/true"])
        w = Watch(a)
        w.refresh()
        n1 = len(w.tracked)
        time.sleep(3.5)
        tbl = w.refresh()
        orphan = [q for q in w.tracked if tbl.get(q, (0,))[0] == 1]
        ok(n1 == 4, f"tracked a, b, c, h ({n1})")
        ok(len(w.tracked) == 3 and len(orphan) == 1, f"a exited; its child b is an orphan and still tracked ({len(w.tracked)}, {orphan})")
    finally:
        for q in list(getattr(w, "tracked", {})):
            try:
                os.kill(q, signal.SIGKILL)
            except OSError:
                pass
        p.kill()
        p.wait()
        os.remove(script)
    print(json.dumps({"pass": not fails, "fails": fails}))
    return 0 if not fails else 1


def fill_test(d, fill_mb, hog_mb, mem_cap_mb):
    """The $0 guard test: a fake line tree, a real fill, a threshold just above the filled level."""
    os.makedirs(d, exist_ok=True)
    fill = os.path.join(d, "m7watch-fill.bin")
    log = os.path.join(d, "m7watch-test.jsonl")
    trip = os.path.join(d, "m7watch-test.trip")
    marker = os.path.join(d, "m7watch-freeze.marker")
    for f in (log, trip, marker):
        if os.path.exists(f):
            os.remove(f)
    f0 = free_mb()
    if f0 < 5300 + fill_mb + 200:
        print(json.dumps({"pass": False, "why": f"refused: free {f0} MB leaves < 5.3 GB after a {fill_mb} MB fill"}))
        return 2
    p, script = _spawn_tree(d, hog_mb)
    drv = subprocess.Popen(["/bin/sleep", "600"])
    # the freeze stand-in records that the tree was stopped (not killed) when spend was frozen
    freeze = [sys.executable, "-I", "-c", "import sys,json; open(sys.argv[1],'w').write('frozen'); print(json.dumps({'rc':0}))", marker]
    w = None
    res = {}
    try:
        time.sleep(1.0)
        thr = f0 - fill_mb // 2
        w = subprocess.Popen([sys.executable, "-I", os.path.abspath(__file__), "run", "--root-pid", str(p.pid), "--pid", str(drv.pid),
                              "--rh", d, "--home", d, "--log", log, "--trip-file", trip, "--min-free-mb", str(thr),
                              "--mem-cap-mb", str(mem_cap_mb), "--poll-s", "1", "--freeze-cmd", json.dumps(freeze)])
        time.sleep(5.0)  # a runs 3 s: b becomes an orphan; the hog is killed on the memory cap
        t0 = time.monotonic()
        with open(fill, "wb") as fh:
            chunk = os.urandom(1 << 20)
            for _ in range(fill_mb):
                fh.write(chunk)
            fh.flush()
            os.fsync(fh.fileno())
        t_fill = time.monotonic()
        rc = w.wait(timeout=60)
        t_trip = time.monotonic()
        evs = [json.loads(l) for l in open(log)]
        kinds = [e["ev"] for e in evs]
        tracked_seen = max([e.get("tracked", 0) for e in evs if e["ev"] == "trip"] or [0])
        res = {"rc": rc, "threshold_mb": thr, "free_before_mb": f0, "events": kinds,
               "trip_s_after_fill": round(t_trip - t_fill, 1), "fill_s": round(t_fill - t0, 1),
               "mem_kill": [e for e in evs if e["ev"] == "mem_kill"], "trip": next((e for e in evs if e["ev"] == "trip"), None),
               "stopped": next((e for e in evs if e["ev"] == "stopped"), None), "freeze_ran": os.path.exists(marker),
               "trip_file": os.path.exists(trip), "tracked_at_trip": tracked_seen}
        root_gone = p.poll() is not None or p.wait(timeout=5) is not None
        drv_gone = drv.poll() is not None or drv.wait(timeout=5) is not None
        st = res["stopped"] or {}
        order = kinds.index("trip") < kinds.index("freeze") < kinds.index("stopped") if all(k in kinds for k in ("trip", "freeze", "stopped")) else False
        res["pass"] = bool(rc == 3 and order and res["freeze_ran"] and res["trip_file"] and root_gone and drv_gone and st.get("left") == []
                           and (mem_cap_mb <= 0 or any(e.get("killed") for e in res["mem_kill"])) and tracked_seen >= 2)
        res["root_gone"], res["driver_gone"] = root_gone, drv_gone
    finally:
        for q in (p, drv, w):
            if q is not None and q.poll() is None:
                q.kill()
        if os.path.exists(fill):
            os.remove(fill)  # exact path, created above
        if os.path.exists(script):
            os.remove(script)
        res["fill_removed"] = not os.path.exists(fill)
        res["free_after_mb"] = free_mb()
    print(json.dumps(res, sort_keys=True))
    return 0 if res.get("pass") else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=("run", "selftest", "fill-test"))
    ap.add_argument("--root-pid", type=int, action="append", default=[])
    ap.add_argument("--pid", type=int, action="append", default=[])
    ap.add_argument("--deny", type=int, action="append", default=[])
    ap.add_argument("--rh")
    ap.add_argument("--home")
    ap.add_argument("--log", default=os.devnull)
    ap.add_argument("--trip-file")
    ap.add_argument("--min-free-mb", type=int, default=6000)
    ap.add_argument("--mem-cap-mb", type=int, default=0)
    ap.add_argument("--poll-s", type=float, default=5.0)
    ap.add_argument("--freeze-cmd", help="JSON argv; default: checks_m7.py freeze --rh RH --home HOME")
    ap.add_argument("--dir")
    ap.add_argument("--fill-mb", type=int, default=300)
    ap.add_argument("--hog-mb", type=int, default=300)
    a = ap.parse_args()
    if a.cmd == "selftest":
        return selftest()
    if a.cmd == "fill-test":
        return fill_test(a.dir, a.fill_mb, a.hog_mb, 200)
    if a.poll_s > 5:
        ap.error("--poll-s must be <= 5")
    if not a.root_pid:
        ap.error("--root-pid is required")
    a.freeze_argv = json.loads(a.freeze_cmd) if a.freeze_cmd else [
        "/usr/bin/python3", "-I", os.path.join(HERE, "checks_m7.py"), "freeze", "--rh", a.rh, "--home", a.home]
    return Watch(a).run()


if __name__ == "__main__":
    sys.exit(main())
