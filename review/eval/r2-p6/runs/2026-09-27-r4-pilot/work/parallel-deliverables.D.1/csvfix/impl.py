def parse(line):
    """Split a CSV line on commas, strip whitespace from each field.

    Returns the list of exactly 3 stripped fields, or None if the line does
    not contain exactly 3 columns.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount field (third column, index 2) of each row.

    Rows whose amount field is blank/whitespace-only are skipped. The amount
    is coerced to int before summing.
    """
    total = 0
    for r in rows:
        if len(r) < 3:
            continue
        amount = r[2].strip()
        if not amount:
            continue
        total += int(amount)
    return total
