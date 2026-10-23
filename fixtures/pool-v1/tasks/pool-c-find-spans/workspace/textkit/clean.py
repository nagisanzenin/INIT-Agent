def strip_tail(line, chars=" \t"):
    """`line` without trailing characters from `chars`."""
    end = len(line)
    while end > 0 and line[end - 1] in chars:
        end -= 1
    return line[:end]
