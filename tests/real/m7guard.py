#!/usr/bin/python3 -I
"""REAL-M7 standalone money guard (D G4 retry 3, 2026-10-21): takes over from a line driver whose in-process guard
carried a stale line cap, with the orchestrator's program rule: $3.00 real by balance from $9.56 (floor $6.56).

Same meters and the same Guard as checks_m7.py: the priced real estimate since the line's start (30 s), the free balance
(5 min), the Root lineage tripwires. On a stop: freeze spend (`checks_m7.py freeze`) and stop the Root by PID (TERM,
10 s, KILL). Exits 0 when the gate's g4 record lands (or a G0-G3 reject, a rollback or a demotion), 3 after a stop.

  m7guard.py --ih IH --rh RH --home HOME --root-pid P --since ISO --seq0 N --bal0-musd B --floor-musd F --cap-musd C --log FILE
"""
import argparse
import calendar
import json
import os
import signal
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import checks as S  # noqa: E402
import checks_m7 as M  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    for k in ("--ih", "--rh", "--home", "--since", "--log"):
        ap.add_argument(k, required=True)
    for k in ("--root-pid", "--seq0", "--bal0-musd", "--floor-musd", "--cap-musd"):
        ap.add_argument(k, type=int, required=True)
    a = ap.parse_args()
    t0 = calendar.timegm(time.strptime(a.since, "%Y-%m-%dT%H:%M:%SZ"))
    g = M.Guard(a.cap_musd, a.bal0_musd, floor=a.floor_musd)
    logf = open(a.log, "a", buffering=1)

    def log(**kw):
        kw["t"] = M.iso(time.time())
        logf.write(json.dumps(kw, sort_keys=True) + "\n")

    log(ev="start", cap_musd=a.cap_musd, eff_cap_musd=g.eff_cap(), floor_musd=a.floor_musd, bal0_musd=a.bal0_musd, since=a.since, seq0=a.seq0)
    last_bal = 0.0
    while True:
        lin = [r for r in S.lineage(a.rh) if int(r.get("seq") or 0) >= a.seq0]
        u = M.priced_usage(a.rh, t0)
        bal = "skip"
        if time.monotonic() - last_bal >= M.BAL_S:
            last_bal = time.monotonic()
            bal = M.read_balance(a.ih, "real")
        trip = M.tripwire_in(lin)
        stop, act = g.tick(u["real_musd"], bal, trip)
        log(ev="poll", real_est_musd=u["real_musd"], root_musd=u["root_musd"], requests=u["requests"],
            balance_musd=None if bal == "skip" else bal, stop=stop)
        if act == "freeze":
            r = subprocess.run(["/usr/bin/python3", "-I", os.path.join(HERE, "checks_m7.py"), "freeze", "--rh", a.rh, "--home", a.home],
                               capture_output=True, text=True, timeout=60)
            log(ev="freeze", why=stop, rc=r.returncode, out=r.stdout.strip()[-300:])
            try:
                os.kill(a.root_pid, signal.SIGTERM)
                for _ in range(40):
                    time.sleep(0.25)
                    os.kill(a.root_pid, 0)
                os.kill(a.root_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            log(ev="stopped", root_pid=a.root_pid)
            return 3
        kinds = [(r.get("kind"), (r.get("data") or {}).get("gate_kind"), (r.get("data") or {}).get("verdict")) for r in lin]
        if any(k == "gate" and gk == "g4" for k, gk, _ in kinds) or any(k in ("rollback", "demotion") for k, _, _ in kinds) \
                or any(k == "gate" and gk == "promote" and v != "accept" for k, gk, v in kinds):
            time.sleep(15)
            log(ev="end", kinds=[k for k, _, _ in kinds][-12:])
            return 0
        time.sleep(M.POLL_S)


if __name__ == "__main__":
    sys.exit(main())
