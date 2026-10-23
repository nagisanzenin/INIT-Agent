def find_all(text, sub, start=0, end=None, *, overlapping=False, ignore_case=False):
    """Start indices (in `text`) of the occurrences of `sub` that lie inside text[start:end]."""
    if sub == "":
        raise ValueError("empty pattern")
    hay, needle = (text.lower(), sub.lower()) if ignore_case else (text, sub)
    step = 1 if overlapping else len(needle)
    index = []
    pos = hay.find(needle, start, end)
    while pos != -1:
        index.append(pos)
        pos = hay.find(needle, pos + step, end)
    return index
