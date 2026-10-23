"""Dev fixture: the unmutated exercise's behaviour (case-insensitive letter anagrams, a word is not its own
anagram, input order, nothing ignored). It must score below 1000."""


def find_anagrams(word, candidates):
    w = word.lower()
    return [c for c in candidates if c.lower() != w and sorted(c.lower()) == sorted(w)]
