"""Dev fixture: the unmutated exercise's behaviour with the new interface's validation: ten green bottles
hanging on the wall, one falls per verse.
The validator checks that this scores below 1000 on every instance."""

NUMBERS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"]


def _bottles(n):
    return "%s green bottle%s" % (NUMBERS[n], "" if n == 1 else "s")


def recite(start, take=1):
    if not 1 <= start <= 10 or take < 1 or take > start:
        raise ValueError("bad verse range")
    lines = []
    for n in range(start, start - take, -1):
        first = _bottles(n).capitalize() + " hanging on the wall,"
        lines += [first, first, "And if one green bottle should accidentally fall,",
                  "There'll be %s hanging on the wall." % _bottles(n - 1), ""]
    return lines[:-1]
