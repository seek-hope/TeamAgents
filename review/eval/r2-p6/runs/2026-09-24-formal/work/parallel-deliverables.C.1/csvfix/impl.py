def parse(line):
    """Split a CSV line into exactly 3 trimmed fields.

    Each field is stripped of surrounding whitespace.  If the line does not
    contain exactly three columns, ``None`` is returned.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount in the third column, skipping blank amounts."""
    return sum(int(r[2]) for r in rows if len(r) > 2 and r[2].strip())
