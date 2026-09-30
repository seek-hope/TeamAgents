def parse(line):
    """Parse a CSV line into exactly three trimmed fields.

    Each field is stripped of surrounding whitespace.  A line whose
    column count is not 3 is invalid and returns None.
    """
    if line is None:
        return None
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields

def total(rows):
    """Sum the amount column (third column) of the given rows.

    Rows with a blank amount field are skipped.
    """
    result = 0
    for row in rows:
        if not row or len(row) < 3:
            continue
        amount = row[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
