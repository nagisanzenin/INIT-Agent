"""Dev fixture: the unmutated exercise's behaviour (each listed entry becomes a lower-case key with its
score). It must score below 1000."""


def transform(legacy_data):
    return {letter.lower(): score for score, letters in legacy_data.items() for letter in letters}
