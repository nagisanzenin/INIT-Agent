"""Text statistics helpers."""
import re


def words(text):
    """The words of `text`, lower-cased (letters and apostrophes)."""
    return [w.lower() for w in re.findall(r"[A-Za-z']+", text)]


def word_count(text):
    """How many words `text` has."""
    return len(words(text))


def unique_words(text):
    """How many different words `text` has."""
    return len(set(words(text)))


def line_count(text):
    """How many lines `text` has."""
    return len(text.splitlines())


def char_count(text):
    """How many characters `text` has, newlines excluded."""
    return len(text.replace("\n", ""))
