def find_all(text, sub):
    """Start indices of the non-overlapping occurrences of `sub` in `text`, left to right."""
    index = []
    starting = 0
    while starting != -1:
        starting = text.find(sub, starting)
        if starting != -1:
            index.append(starting)
            starting += len(sub)
    return index
