import csv

def parse(line):
    """Parse one CSV record: honor quoting, trim each field, require 3 columns.

    Returns a list of exactly 3 trimmed fields, or None when the record does
    not contain exactly 3 columns.
    """
    for row in csv.reader([line]):
        fields = [f.strip() for f in row]
    if len(fields) != 3:
        return None
    return fields

def total(rows):
    """Sum the amount from the third column (index 2) of each row.

    Rows that are None, not exactly 3 fields, or whose amount is empty/blank
    are skipped. A non-empty, non-numeric amount raises ValueError.
    """
    result = 0
    for r in rows:
        if r is None or len(r) != 3:
            continue
        amount = r[2]
        if amount is None or str(amount).strip() == "":
            continue
        result += int(amount)
    return result
