def rotate_after(text, mark, occurrence=1):
    """Moves the part of `text` up to and including the chosen occurrence of `mark` to the end."""
    if not isinstance(mark, str) or mark == "":
        raise ValueError("mark must be a non-empty string")
    if occurrence == 0:
        raise ValueError("occurrence must not be 0")
    found = []
    pos = text.find(mark)
    while pos != -1:
        found.append(pos)
        pos = text.find(mark, pos + len(mark))
    if abs(occurrence) > len(found):
        return text
    i = found[occurrence - 1] if occurrence > 0 else found[occurrence]
    cut = i + len(mark)
    return text[cut:] + text[:cut]
