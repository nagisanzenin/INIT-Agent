"""Dev fixture: the unmutated exercise's behaviour (classic 0/1 knapsack by dynamic programming), returning
the indices of one optimal set in its natural order and ignoring max_items and groups. It must score
below 1000."""


def best_items(max_weight, items, max_items=None):
    if max_weight < 0:
        raise ValueError("max_weight must not be negative")
    table = [[0] * (max_weight + 1) for _ in range(len(items) + 1)]
    for i, item in enumerate(items, 1):
        for w in range(max_weight + 1):
            table[i][w] = table[i - 1][w]
            if item["weight"] <= w:
                table[i][w] = max(table[i][w], table[i - 1][w - item["weight"]] + item["value"])
    chosen, w = [], max_weight
    for i in range(len(items), 0, -1):
        if table[i][w] != table[i - 1][w]:
            chosen.append(i - 1)
            w -= items[i - 1]["weight"]
    return sorted(chosen)
