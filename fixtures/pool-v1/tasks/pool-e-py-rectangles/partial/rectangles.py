def rectangles(strings):
    # dev fixture (partial): counts only the empty and single-rectangle cases right
    corners = sum(row.count("+") for row in strings)
    return 0 if corners < 4 else 1
