def parse(line):
    """Parse a CSV line into exactly 3 trimmed fields.

    - each field is stripped of surrounding whitespace
    - returns None when the resulting column count is not 3
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount column (third column) across rows.

    Blank amount fields are skipped, not treated as zero and not an error.
    Each amount is converted with int(); surrounding whitespace is ignored.
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = row[2]
        if amount is None or not str(amount).strip():
            continue
        result += int(str(amount).strip())
    return result
