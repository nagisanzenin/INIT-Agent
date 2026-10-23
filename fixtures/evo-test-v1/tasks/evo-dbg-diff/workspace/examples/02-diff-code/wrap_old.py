import os
import sys

WIDTH = 72


def wrap(text, width=WIDTH):
    """Splits text into lines of at most width characters."""
    words = text.split()
    lines, line = [], ""
    for word in words:
        if len(line) + len(word) >= width:
            lines.append(line)
            line = ""
        line = (line + " " + word).strip()
    if line:
        lines.append(line)
    return lines


def main(argv):
    with open(argv[1]) as f:
        text = f.read()
    for line in wrap(text):
        print(line)
    return 0
