"""Reference solution for e-py-grep. gen/run replaces the P line with the instance's parameters:
sep (field separator), word (bool: -w supported), width (line-number padding, 0 = none) and
max_count (bool: -mN supported)."""
P = None  # @@PARAMS@@

_WORD = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"


def _parse(flags):
    opts = {"n": False, "l": False, "i": False, "v": False, "x": False, "c": False, "w": False, "m": None}
    for f in flags:
        if f in ("-n", "-l", "-i", "-v", "-x", "-c") or (f == "-w" and P["word"]):
            opts[f[1]] = True
        elif P["max_count"] and len(f) > 2 and f.startswith("-m") and f[2:].isdigit() and int(f[2:]) >= 1:
            opts["m"] = int(f[2:])
        else:
            raise ValueError("unknown flag %r" % (f,))
    return opts


def _whole_word_at(line, pat):
    start = line.find(pat)
    while start != -1:
        end = start + len(pat)
        before = start == 0 or line[start - 1] not in _WORD
        after = end == len(line) or line[end] not in _WORD
        if before and after:
            return True
        start = line.find(pat, start + 1)
    return False


def _matches(line, pattern, o):
    a, b = (line.lower(), pattern.lower()) if o["i"] else (line, pattern)
    if o["x"]:
        hit = a == b
    elif o["w"]:
        hit = _whole_word_at(a, b)
    else:
        hit = b in a
    return hit != o["v"]


def _lines(text):
    parts = text.split("\n")
    if parts and parts[-1] == "":
        parts.pop()
    return parts


def grep(pattern, flags, files):
    o = _parse(flags)
    many = len(files) > 1
    out = []
    for name, text in files.items():
        selected = [(i, ln) for i, ln in enumerate(_lines(text), 1) if _matches(ln, pattern, o)]
        if o["m"] is not None:
            selected = selected[:o["m"]]
        if o["l"]:
            if selected:
                out.append(name)
        elif o["c"]:
            out.append((name + P["sep"] if many else "") + str(len(selected)))
        else:
            for i, ln in selected:
                num = (str(i).zfill(P["width"]) + P["sep"]) if o["n"] else ""
                out.append((name + P["sep"] if many else "") + num + ln)
    return out
