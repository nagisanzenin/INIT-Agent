def rotate_after(text, letter):
    """Moves the part of `text` up to and including the first `letter` to the end."""
    if letter in text:
        start = text.index(letter)
        return text[start + 1:] + text[:start + 1]
    return text
