def parse(line):
    """Split a CSV line into exactly 3 trimmed fields.

    Returns None when the line does not have exactly 3 columns.
    """
    if line is None:
        return None
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column (the amount), skipping blank amount fields."""
    result = 0
    for row in rows:
        if row is None or len(row) < 3:
            continue
        amount = row[2].strip() if isinstance(row[2], str) else row[2]
        if amount == "" or amount is None:
            continue
        result += int(amount)
    return result
