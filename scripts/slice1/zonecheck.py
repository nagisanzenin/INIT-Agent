#!/usr/bin/python3 -I
"""Read-only zone evidence for the running instances in `init-root status --json` (stdin):
`sandbox_check(pid, NULL, 0)` (1 = the process runs under a Seatbelt profile) and the ps line. No
signal is sent to anything. (The path form of sandbox_check is variadic, which ctypes cannot call
correctly on arm64; the canary/decoy denials are WP21's Z1/Z5 tests.)"""
import ctypes
import json
import subprocess
import sys

lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
check = lib.sandbox_check
check.restype = ctypes.c_int
st = json.load(sys.stdin)
out = []
for i in st.get("instances") or []:
    pid = int(i["pid"])
    sandboxed = check(pid, None, 0)
    ps = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "pid=,pgid=,comm="], capture_output=True, text=True).stdout.strip()
    out.append({"kind": i["kind"], "pid": pid, "sandboxed": sandboxed, "ps": ps})
print(json.dumps(out, sort_keys=True))
sys.exit(0 if out and all(o["sandboxed"] == 1 for o in out) else 1)
