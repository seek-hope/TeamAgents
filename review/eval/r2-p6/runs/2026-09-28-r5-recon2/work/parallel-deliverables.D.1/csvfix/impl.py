def parse(line):
    """Split a CSV line, trimming whitespace from each field.

    Returns the list of trimmed fields only when the line contains exactly
    three comma-separated fields; otherwise returns None.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column (amount) of ``rows`` as an integer.

    Rows with fewer than three fields and rows whose amount field is empty
    (after trimming) are skipped.
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = str(row[2]).strip()
        if not amount:
            continue
        result += int(amount)
    return result
