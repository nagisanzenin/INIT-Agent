# dev fixture (partial): overlapping and the empty-pattern check, but no window and no ignore_case
def find_all(text, sub, start=0, end=None, *, overlapping=False, ignore_case=False):
    if sub == "":
        raise ValueError("empty pattern")
    index = []
    pos = text.find(sub)
    while pos != -1:
        index.append(pos)
        pos = text.find(sub, pos + (1 if overlapping else len(sub)))
    return index
