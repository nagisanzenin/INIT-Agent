"""CSV reading and writing (SPEC.md sections 2 and 4)."""


class CsvError(Exception):
    pass


def parse_csv(text):
    """The records of a CSV text, each a list of field strings."""
    rows, row, buf = [], [], []
    at_start = True
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if at_start and c == '"':
            j = i + 1
            while True:
                if j >= n:
                    raise CsvError("unterminated quoted field")
                if text[j] == '"':
                    if text.startswith('"', j + 1):
                        buf.append('"')
                        j += 2
                        continue
                    break
                buf.append(text[j])
                j += 1
            i = j + 1
            at_start = False
        elif c == ",":
            row.append("".join(buf))
            buf = []
            at_start = True
            i += 1
        elif c == "\n" or text.startswith("\r\n", i):
            row.append("".join(buf))
            rows.append(row)
            row, buf = [], []
            at_start = True
            i += 1 if c == "\n" else 2
        else:
            buf.append(c)
            at_start = False
            i += 1
    if row or buf or not at_start:
        row.append("".join(buf))
        rows.append(row)
    return rows


def needs_quotes(field):
    return any(ch in field for ch in ',"\n\r') or field.startswith(" ") or field.endswith(" ")


def format_field(field):
    if needs_quotes(field):
        return '"' + field.replace('"', '""') + '"'
    return field


def format_row(fields):
    return ",".join(format_field(f) for f in fields) + "\n"
