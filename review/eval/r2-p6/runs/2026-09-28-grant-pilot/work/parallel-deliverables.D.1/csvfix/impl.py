def parse(line):
    """Trim each field and require exactly 3 columns.

    Returns the list of trimmed fields, or None when the column count != 3.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the 3rd column (amount), skipping rows with a blank amount."""
    result = 0
    for row in rows:
        if len(row) > 2:
            amount = row[2].strip()
            if amount:
                result += int(amount)
    return result
