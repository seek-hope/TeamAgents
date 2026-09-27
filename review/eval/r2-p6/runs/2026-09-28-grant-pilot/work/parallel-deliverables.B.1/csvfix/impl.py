def parse(line):
    """Split a CSV line into 3 fields, trimming whitespace around each field.

    Returns ``None`` when the line does not have exactly 3 columns.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount column (third field), skipping blank amounts."""
    total_value = 0
    for r in rows:
        if len(r) <= 2:
            continue
        amount = r[2].strip()
        if not amount:
            continue
        total_value += int(amount)
    return total_value
