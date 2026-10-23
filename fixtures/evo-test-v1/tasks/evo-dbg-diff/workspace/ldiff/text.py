"""The line model shared by both commands (SPEC.md section 1).

A file is a list of Line values. A line's text never holds the newline character; `eol` says whether a newline
followed the line in the file. Only the last line of a file read from disk can lack its newline, so the list of
lines describes the file content exactly."""
import collections
import os
import sys

Line = collections.namedtuple("Line", "text eol")

STDIN = "-"


class TextError(Exception):
    """A file could not be read, decoded or written. The message names the file."""


def split_lines(data):
    """Splits decoded file content into lines. An empty content has no lines."""
    if data == "":
        return []
    lines = []
    for chunk in data.splitlines(True):
        if chunk.endswith("\n"):
            lines.append(Line(chunk[:-1], True))
        else:
            lines.append(Line(chunk, False))
    return lines


def join_lines(lines):
    """The file content of a list of lines: every line but the last is followed by a newline, the last one only
    when it has one."""
    out = []
    last = len(lines) - 1
    for index, line in enumerate(lines):
        out.append(line.text)
        if line.eol or index < last:
            out.append("\n")
    return "".join(out)


def display_name(path):
    return "standard input" if path == STDIN else path


def read_bytes(path):
    """The raw content of a file, or of standard input for `-`."""
    if path == STDIN:
        return sys.stdin.buffer.read()
    try:
        with open(path, "rb") as f:
            return f.read()
    except FileNotFoundError:
        raise TextError("%s: No such file or directory" % path)
    except IsADirectoryError:
        raise TextError("%s: Is a directory" % path)
    except OSError as e:
        raise TextError("%s: %s" % (path, e.strerror or "cannot be read"))


def read_text(path):
    """The decoded content of a file (UTF-8)."""
    data = read_bytes(path)
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        raise TextError("%s: not a UTF-8 text file" % display_name(path))


def read_lines(path, missing_is_empty=False):
    """The lines of a file; with missing_is_empty, a file that does not exist has no lines."""
    if missing_is_empty and path != STDIN and not os.path.lexists(path):
        return []
    return split_lines(read_text(path))


def write_bytes(path, data):
    try:
        with open(path, "wb") as f:
            f.write(data)
    except OSError as e:
        raise TextError("%s: %s" % (path, e.strerror or "cannot be written"))


def write_text(path, text):
    write_bytes(path, text.encode("utf-8"))


def write_lines(path, lines):
    write_text(path, join_lines(lines))
