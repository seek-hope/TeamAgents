def parse(line):
    """Split on comma, strip whitespace from each field.

    Return None when the number of columns is not exactly 3.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount column (index 2), skipping blank amount fields."""
    running = 0
    for row in rows:
        if len(row) <= 2:
            continue
        amount = row[2].strip() if isinstance(row[2], str) else row[2]
        if amount == "":
            continue
        running += int(amount)
    return running
