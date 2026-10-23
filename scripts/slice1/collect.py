#!/usr/bin/python3 -I
"""Copies the Slice-1 logs, lineage and state of one run into build/slice1-artifacts/<mode>/, redacting
secrets on the way. The key file (when given) is read only to know what to redact; its content is never
printed or written. Also redacted: anything shaped like a provider key (`sk-…`), `Bearer …`/`x-api-key`
values, and the mock key. Prints one JSON summary line: files copied and redaction counts (no content)."""
import json
import os
import re

import sys

SKIP_DIRS = {"bin", "blobs", "kernel.git", "body.git", "trees", "packs", "keys", "cache", "probe", "fleet", "target",
             ".git", "vendor"}
MAX_BYTES = 4 << 20
PATTERNS = [
    re.compile(rb"sk-[A-Za-z0-9_\-]{12,}"),
    re.compile(rb"(?i)(bearer\s+)[A-Za-z0-9_\-\.]{8,}"),
    re.compile(rb"(?i)(x-api-key[\"']?\s*[:=]\s*[\"']?)[A-Za-z0-9_\-\.]{8,}"),
    re.compile(rb"test-slice1-[A-Za-z0-9\-]+"),
]


def redact(b: bytes, secrets: list) -> tuple:
    n = 0
    for s in secrets:
        if s:
            c = b.count(s)
            if c:
                b = b.replace(s, b"[REDACTED:key]")
                n += c
    for p in PATTERNS:
        def sub(m):
            return (m.group(1) if m.lastindex else b"") + b"[REDACTED]"
        b, c = p.subn(sub, b)
        n += c
    return b, n


def main() -> int:
    if len(sys.argv) < 4:
        print("usage: collect.py <dest> <keyfile|-> <src>=<label> …", file=sys.stderr)
        return 64
    dest, keyfile = sys.argv[1], sys.argv[2]
    secrets = []
    if keyfile != "-" and os.path.isfile(keyfile):
        with open(keyfile, "rb") as f:
            k = f.read().strip()
        if k:
            secrets.append(k)
    files, red, skipped = 0, 0, 0
    for spec in sys.argv[3:]:
        src, label = spec.split("=", 1)
        if not os.path.exists(src):
            continue
        walk = [(os.path.dirname(src), [], [os.path.basename(src)])] if os.path.isfile(src) else os.walk(src)
        for root, dirs, names in walk:
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
            for n in names:
                p = os.path.join(root, n)
                if os.path.islink(p) or not os.path.isfile(p):
                    continue
                if os.path.getsize(p) > MAX_BYTES:
                    skipped += 1
                    continue
                with open(p, "rb") as f:
                    b = f.read()
                if b"\0" in b[:4096]:
                    skipped += 1
                    continue
                b, c = redact(b, secrets)
                red += c
                rel = os.path.relpath(p, src) if os.path.isdir(src) else n
                out = os.path.join(dest, label, rel)
                os.makedirs(os.path.dirname(out), exist_ok=True)
                with open(out, "wb") as f:
                    f.write(b)
                files += 1
    # a final scan of everything written: no secret may remain
    leaks = 0
    for root, _, names in os.walk(dest):
        for n in names:
            with open(os.path.join(root, n), "rb") as f:
                b = f.read()
            leaks += sum(b.count(s) for s in secrets if s)
            leaks += sum(len(p.findall(b)) for p in PATTERNS[:1])
    print(json.dumps({"files": files, "redactions": red, "skipped_binary_or_large": skipped, "leaks_after": leaks}))
    return 0 if leaks == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
