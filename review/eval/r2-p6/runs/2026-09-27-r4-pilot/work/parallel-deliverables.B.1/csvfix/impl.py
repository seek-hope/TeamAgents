def parse(line):
    """Split a CSV line into exactly three fields, trimming each field.

    Returns None when the line does not contain exactly three fields.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount held in the third column of each row.

    Rows with fewer than three columns are ignored, and blank amount
    fields (empty or whitespace only) are skipped.
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
