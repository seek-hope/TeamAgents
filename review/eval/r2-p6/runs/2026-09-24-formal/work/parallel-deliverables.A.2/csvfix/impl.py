def parse(line):
    """Split a CSV line into exactly 3 trimmed fields.

    Returns None when the number of columns is not 3.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column (amount) of the rows, skipping blank amounts."""
    total = 0
    for row in rows:
        if len(row) > 2 and row[2]:
            total += int(row[2])
    return total
