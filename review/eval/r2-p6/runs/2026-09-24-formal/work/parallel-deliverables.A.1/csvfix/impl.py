def parse(line):
    """Split a CSV line into fields.

    Each field is trimmed of surrounding whitespace. A line that does not
    contain exactly three columns is invalid and returns ``None``.
    """
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount (third) column, skipping blank amount fields."""
    return sum(int(r[2]) for r in rows if len(r) > 2 and r[2].strip())
