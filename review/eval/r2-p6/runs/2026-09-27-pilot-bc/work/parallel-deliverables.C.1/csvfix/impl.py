import csv


def parse(line):
    """Parse one CSV line into exactly 3 trimmed fields.

    Each field is trimmed of surrounding whitespace.  A field may be quoted
    (so it can legally contain a comma).  Returns ``None`` when the line does
    not contain exactly 3 columns.
    """
    if not isinstance(line, str):
        return None
    try:
        fields = next(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None
    if len(fields) != 3:
        return None
    return [f.strip() for f in fields]


def total(rows):
    """Sum the amount column (third column) of parsed rows.

    Blank amount fields are skipped; rows that are not parseable as 3 columns
    are ignored as well.
    """
    result = 0
    for row in rows or []:
        if not row or len(row) < 3:
            continue
        amount = row[2]
        if amount is None:
            continue
        amount = str(amount).strip()
        if not amount:
            continue
        result += int(amount)
    return result
