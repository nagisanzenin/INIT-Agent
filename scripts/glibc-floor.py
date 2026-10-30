#!/usr/bin/env python3
"""The lowest glibc that every ELF file under the given paths can run on (v0.5.4; get.sh's GLIBC_MIN).

    python3 -I scripts/glibc-floor.py <file or directory> ...

Reads each ELF file's version needs (the SHT_GNU_verneed section, what `objdump -p` lists under "Version
References") and prints the highest GLIBC_x.y any of them names, then one line per file with its own highest. A
binary built against a newer glibc fails to start on an older one ("version `GLIBC_2.34' not found"), so get.sh
refuses a Linux below this number before it downloads anything. Stdlib only, 64-bit and 32-bit, either byte order;
reads nothing but the files given.
"""
import os
import re
import struct
import sys

SHT_GNU_VERNEED = 0x6FFFFFFE


def needs(path):
    """The GLIBC_x.y names in the version needs of the ELF file at `path` ([] when it is not ELF or names none)."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"\x7fELF" or len(data) < 64:
        return []
    is64, end = data[4] == 2, "<" if data[5] == 1 else ">"
    if is64:
        shoff, = struct.unpack_from(end + "Q", data, 0x28)
        shentsize, shnum, shstrndx = struct.unpack_from(end + "HHH", data, 0x3A)
    else:
        shoff, = struct.unpack_from(end + "I", data, 0x20)
        shentsize, shnum, shstrndx = struct.unpack_from(end + "HHH", data, 0x2E)
    sections = []
    for i in range(shnum):
        o = shoff + i * shentsize
        if is64:
            _, typ, _, _, off, size, link, info = struct.unpack_from(end + "IIQQQQII", data, o)
        else:
            _, typ, _, _, off, size, link, info = struct.unpack_from(end + "IIIIIIII", data, o)
        sections.append((typ, off, size, link, info))
    out = []
    for typ, off, size, link, info in sections:
        if typ != SHT_GNU_VERNEED:
            continue
        stroff = sections[link][1]  # the string table the entries' names index (.dynstr)
        name = lambda n: data[stroff + n:data.index(b"\0", stroff + n)].decode("ascii", "replace")
        o = off
        for _ in range(info):  # sh_info: the number of Verneed entries
            _, cnt, _, aux, nxt = struct.unpack_from(end + "HHIII", data, o)
            a = o + aux
            for _ in range(cnt):
                _, _, _, vname, anext = struct.unpack_from(end + "IHHII", data, a)
                out.append(name(vname))
                a += anext
            if not nxt:
                break
            o += nxt
    return out


def key(v):
    return tuple(int(x) for x in v.split("_", 1)[1].split("."))


def main(paths):
    files = []
    for p in paths:
        if os.path.isdir(p):
            for d, _, names in os.walk(p):
                files += [os.path.join(d, n) for n in sorted(names)]
        else:
            files.append(p)
    per = {}
    for f in files:
        if os.path.isfile(f) and not os.path.islink(f):
            g = [v for v in needs(f) if re.fullmatch(r"GLIBC_[0-9]+(\.[0-9]+)+", v)]
            if g:
                per[f] = max(g, key=key)
    if not per:
        sys.exit("glibc-floor.py: no ELF file under %s names a GLIBC version" % " ".join(paths))
    print(max(per.values(), key=key).split("_", 1)[1])
    for f, v in sorted(per.items()):
        print("  %s %s" % (v, f))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__.strip())
    main(sys.argv[1:])
