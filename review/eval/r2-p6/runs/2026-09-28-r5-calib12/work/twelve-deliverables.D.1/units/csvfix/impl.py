def parse(line):
    """Split on commas, trim surrounding whitespace of every field.

    Returns None when the resulting column count is not 3, otherwise the
    list of 3 trimmed strings.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third (amount) column as int.

    Rows whose third field is blank or absent are skipped, never an error.
    """
    total = 0
    for r in rows:
        if len(r) < 3:
            continue
        value = r[2].strip()
        if not value:
            continue
        total += int(value)
    return total
