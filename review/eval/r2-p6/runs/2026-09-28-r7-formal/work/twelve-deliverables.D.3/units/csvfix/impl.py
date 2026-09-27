COLUMNS = 3
AMOUNT_INDEX = 2


def parse(line):
    """Split a CSV line on commas, strip surrounding whitespace from each
    field, and reject lines whose column count is not exactly 3."""
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != COLUMNS:
        return None
    return fields


def total(rows):
    """Sum the third column (amount) as int, skipping rows whose third
    column is missing or blank."""
    total_amount = 0
    for row in rows:
        if len(row) <= AMOUNT_INDEX:
            continue
        amount = row[AMOUNT_INDEX].strip()
        if not amount:
            continue
        total_amount += int(amount)
    return total_amount
