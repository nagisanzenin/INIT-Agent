"""Dev fixture: the unmutated exercise's behaviour (unlimited coins of every value in `coins`, fewest coins,
ascending order). It must score below 1000."""


def find_fewest_coins(coins, target):
    if target < 0:
        raise ValueError("target can't be negative")
    values = sorted(coins)
    best = [None] * (target + 1)
    best[0] = []
    for amount in range(1, target + 1):
        for c in values:
            if c <= amount and best[amount - c] is not None:
                cand = best[amount - c] + [c]
                if best[amount] is None or len(cand) < len(best[amount]):
                    best[amount] = cand
    if best[target] is None:
        raise ValueError("can't make target with given coins")
    return sorted(best[target])
