import csv
import io


def parse(line):
    """Parse one CSV record.

    Quoted fields (RFC4180 style) may contain commas; every field is stripped
    of surrounding whitespace.  Exactly 3 columns are required, otherwise
    ``None`` is returned (no exception).
    """
    if not line:
        return None
    try:
        fields = next(csv.reader(io.StringIO(line)))
    except StopIteration:
        return None
    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column (the amount) of ``rows`` as integers.

    Blank (or ``None``) amounts are skipped; a non-integer amount raises
    ``ValueError``; rows with fewer than 3 columns are skipped.
    """
    total = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = row[2]
        if amount is None or amount == "":
            continue
        total += int(amount)
    return total
