def parse(line):
    """Split a CSV line into fields, trimming whitespace per field.

    Returns a list of exactly 3 fields, or None when the line does not
    contain exactly 3 columns.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the integer value of the THIRD column across rows.

    Blank third-column fields count as 0 (skipped, no error).
    """
    result = 0
    for r in rows:
        if len(r) > 2 and r[2].strip():
            result += int(r[2])
    return result
