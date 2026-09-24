import csv

EXPECTED_COLUMNS = 3


def parse(line):
    """Split one CSV line into exactly EXPECTED_COLUMNS trimmed fields.

    Returns None when the column count is anything other than 3. Quoted
    fields are handled via the csv module so commas inside quotes do not
    split the row, and surrounding whitespace is stripped from every field.
    """
    if line is None:
        return None
    try:
        fields = next(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None
    fields = [field.strip() for field in fields]
    if len(fields) != EXPECTED_COLUMNS:
        return None
    return fields


def total(rows):
    """Sum the third column (the amount) across rows.

    Blank amount fields are skipped instead of being counted; rows with
    fewer than 3 columns are ignored.
    """
    result = 0
    for row in rows:
        if len(row) < EXPECTED_COLUMNS:
            continue
        amount = row[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
