"""CSV helpers.

Expected semantics (see test_csvfix.py):

* ``parse(line)`` strips surrounding whitespace from every field and returns
  ``None`` when the line does not have exactly three columns.
* ``total(rows)`` uses the *third* column as the amount, skips rows whose
  amount is empty/blank, and raises ``ValueError`` on a non-numeric amount
  (i.e. bad amounts are reported, not silently treated as 0).
"""

import csv

COLUMNS = 3


def parse(line):
    """Split ``line`` into exactly ``COLUMNS`` trimmed fields.

    Returns ``None`` when the line cannot be split into three columns.
    """
    if line is None:
        return None

    try:
        fields = next(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None

    fields = [field.strip() for field in fields]
    if len(fields) != COLUMNS:
        return None
    return fields


def total(rows):
    """Sum the amount (third column) of every row.

    Rows with fewer than three columns or with a blank amount are skipped.
    A non-blank, non-numeric amount raises ``ValueError``.
    """
    result = 0
    for row in rows:
        if row is None or len(row) <= 2:
            continue

        amount = row[2]
        amount = amount.strip() if isinstance(amount, str) else amount
        if amount is None or amount == "":
            continue

        result += int(amount)
    return result
