def parse(line):
    """Split ``line`` on commas, trim whitespace from every field.

    Return ``None`` when the number of columns is not exactly 3.
    """
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the integer value of the 3rd column across ``rows``.

    Rows with fewer than 3 columns are ignored, and rows whose 3rd field is
    blank are skipped. Returns an ``int`` (0 for an empty/valueless input).
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        field = row[2].strip()
        if not field:
            continue
        result += int(field)
    return result
