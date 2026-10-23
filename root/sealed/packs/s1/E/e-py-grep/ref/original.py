"""Dev fixture: the unmutated exercise's behaviour (flags -n -l -i -v -x, ':' separators), returning the
output lines as a list and ignoring flags it does not know. It must score below 1000."""


def grep(pattern, flags, files):
    many = len(files) > 1
    out = []
    for name, text in files.items():
        lines = text.split("\n")
        if lines and lines[-1] == "":
            lines.pop()
        for i, line in enumerate(lines, 1):
            a, b = (line.lower(), pattern.lower()) if "-i" in flags else (line, pattern)
            hit = (a == b) if "-x" in flags else (b in a)
            if hit == ("-v" in flags):
                continue
            if "-l" in flags:
                out.append(name)
                break
            prefix = (name + ":" if many else "") + (str(i) + ":" if "-n" in flags else "")
            out.append(prefix + line)
    return out
