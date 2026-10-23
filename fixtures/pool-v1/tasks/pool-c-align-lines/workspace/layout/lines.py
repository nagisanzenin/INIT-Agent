def center_lines(text, width):
    """Centers every line of `text` in `width` columns."""
    result = ""
    lines = text.split('\n')
    for l in lines:
        result += l.center(width)
        result += '\n'

    # Remove the very last empty line
    result = result[:-1]
    return result
