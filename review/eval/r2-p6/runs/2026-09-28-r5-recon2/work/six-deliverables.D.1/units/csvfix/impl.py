"""CSV fix: parse and total.

``parse`` reads one CSV line, honours quoted fields, trims surrounding
whitespace on each field and only accepts rows with exactly three columns;
malformed rows yield ``None``.

``total`` sums the (third-column) amount of each row, skipping rows whose
amount column is blank.
"""

import csv
import io


def parse(line):
    """Parse one CSV record into exactly three trimmed fields.

    Quoted fields (including embedded commas) are handled via the stdlib
    ``csv`` module.  Surrounding whitespace is stripped from every field.

    Returns a list of three strings, or ``None`` when the record does not
    contain exactly three columns.
    """
    if line is None:
        return None
    if isinstance(line, (list, tuple)):
        fields = list(line)
    else:
        try:
            fields = next(csv.reader(io.StringIO(str(line))))
        except StopIteration:
            return None
    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the amount (third column) of ``rows``.

    Rows whose third column is empty/whitespace are skipped.  Rows with
    fewer than three columns are also skipped (nothing to add).
    """
    result = 0
    for row in rows:
        if row is None or len(row) < 3:
            continue
        amount = str(row[2]).strip()
        if not amount:
            continue
        result += int(amount)
    return result
