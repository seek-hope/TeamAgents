def parse(line):
    """Split a CSV line into exactly 3 trimmed fields.

    Returns None when the line does not contain exactly 3 columns.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the (third-column) amount of each row, skipping blank amounts."""
    result = 0
    for r in rows:
        if len(r) < 3:
            continue
        amount = r[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
