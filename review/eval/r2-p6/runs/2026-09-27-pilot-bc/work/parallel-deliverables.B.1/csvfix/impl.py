def parse(line):
    """Split one CSV line into exactly 3 trimmed fields.

    Every field is stripped of surrounding whitespace. Returns ``None``
    when the line does not contain exactly three columns.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount column (third column) of ``rows``.

    Blank amount fields are skipped. Rows with fewer than three columns
    contribute nothing.
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = row[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
