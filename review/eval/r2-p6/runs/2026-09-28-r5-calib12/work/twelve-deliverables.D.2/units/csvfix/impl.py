def parse(line):
    """Split ``line`` on commas into exactly three trimmed fields.

    Each field is stripped of surrounding whitespace.  When the line does not
    consist of exactly three fields (too few or too many), ``None`` is
    returned instead of a partial/oversized row.
    """
    fields = line.split(",")
    if len(fields) != 3:
        return None
    return [field.strip() for field in fields]


def total(rows):
    """Sum the amount column of ``rows``.

    Rows whose amount is blank or missing (row shorter than the amount
    position) contribute nothing.  Amounts are converted to ``int``.
    """
    amount_index = 2
    total_amount = 0
    for row in rows:
        if row is None or len(row) <= amount_index:
            continue  # amount missing
        amount = row[amount_index]
        if amount is None:
            continue
        amount = str(amount).strip()
        if not amount:
            continue  # blank amount
        total_amount += int(amount)
    return total_amount
