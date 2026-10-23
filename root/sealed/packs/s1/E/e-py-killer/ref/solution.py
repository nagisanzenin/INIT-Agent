"""Reference solution for e-py-killer. gen/run replaces the P line with the instance's parameters:
lo / hi (the allowed digits lo..hi), inner ("asc" | "desc": digit order inside a combination), outer ("asc" |
"desc": lexicographic order of the returned combinations), use_include (honour `include`) and repeat (how often
one digit may appear in a combination: 1 or 2)."""
P = None  # @@PARAMS@@


def combinations(target, size, exclude, include=()):
    if size < 1 or target < 0:
        raise ValueError("size must be >= 1 and target >= 0")
    digits = [d for d in range(P["lo"], P["hi"] + 1) if d not in exclude]
    found = []

    def walk(i, left, total, acc):
        if total > target:
            return
        if left == 0:
            if total == target:
                found.append(acc)
            return
        if i == len(digits) or (len(digits) - i) * P["repeat"] < left:
            return
        d = digits[i]
        for k in range(min(P["repeat"], left) + 1):
            walk(i + 1, left - k, total + k * d, acc + [d] * k)

    walk(0, size, 0, [])
    if P["use_include"]:
        found = [c for c in found if all(x in c for x in include)]
    if P["inner"] == "desc":
        found = [c[::-1] for c in found]
    return sorted(found, reverse=P["outer"] == "desc")
