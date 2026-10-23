"""Reference solution for e-py-etl. gen/run replaces the P line with the instance's parameters:
case ("lower" | "upper": the output letters), combine ("sum" | "max" | "min"), str_keys (bool: scores may
be digit strings), bonus (letters whose final score is multiplied), mult (the multiplier) and as_list
(bool: return sorted (letter, score) tuples instead of a dict)."""
P = None  # @@PARAMS@@

_ASCII = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"


def transform(legacy_data):
    acc = {}
    for key, entries in legacy_data.items():
        score = int(key) if P["str_keys"] and isinstance(key, str) else key
        for entry in entries:
            for ch in entry:
                if ch not in _ASCII:
                    continue
                letter = ch.lower() if P["case"] == "lower" else ch.upper()
                if letter not in acc:
                    acc[letter] = score
                elif P["combine"] == "sum":
                    acc[letter] += score
                elif P["combine"] == "max":
                    acc[letter] = max(acc[letter], score)
                else:
                    acc[letter] = min(acc[letter], score)
    for letter in acc:
        if letter.lower() in P["bonus"]:
            acc[letter] *= P["mult"]
    if P["as_list"]:
        return sorted(acc.items(), key=lambda kv: (-kv[1], kv[0]))
    return acc
