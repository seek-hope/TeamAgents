def parse(line):
    """Split a CSV line into fields, trimming whitespace.

    Fields are trimmed; if the resulting column count is not exactly 3
    the line is invalid and None is returned.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount column (third column) of parsed rows.

    Blank amounts are skipped. Fields are validated with parse-like
    semantics: only rows that contain exactly 3 fields are considered.
    """
    result = 0
    for r in rows:
        if len(r) != 3:
            continue
        amount = r[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
