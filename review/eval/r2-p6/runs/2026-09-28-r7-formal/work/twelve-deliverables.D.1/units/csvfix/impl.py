def parse(line):
    """Split a CSV line into exactly three trimmed fields.

    Returns ``None`` when the line does not contain exactly three columns.
    """
    fields = [part.strip() for part in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third (numeric) column, skipping blank fields.

    Blank cells are ignored (they count as nothing, not as zero); rows with
    fewer than three columns are ignored as well.
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        value = row[2].strip()
        if not value:
            continue
        result += int(value)
    return result
