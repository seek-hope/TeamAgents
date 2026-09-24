def parse(line):
    """Split a CSV line into exactly 3 trimmed fields.

    Returns None when the line does not have exactly 3 columns.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amounts (third column), skipping rows with a blank amount."""
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = row[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
