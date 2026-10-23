"""Text statistics helpers."""


def words(text):
    """The words of `text`."""
    return text.split()


def word_count(text):
    """How many words `text` has."""
    return len(words(text))


def line_count(text):
    """How many lines `text` has."""
    return len(text.splitlines())


def char_count(text):
    """How many characters `text` has, newlines excluded."""
    return len(text.replace("\n", ""))
