import csv
import io

N_COLS = 3


def parse(line):
    """Parse one CSV line into exactly 3 trimmed fields.

    - Quoted fields are supported (standard CSV quoting rules).
    - Leading/trailing whitespace around each field is stripped.
    - Returns None when the line does not have exactly 3 columns
      (or when the line cannot be parsed as CSV at all).
    """
    if line is None:
        return None
    try:
        row = next(csv.reader(io.StringIO(line)))
    except (csv.Error, StopIteration):
        return None
    fields = [f.strip() for f in row]
    if len(fields) != N_COLS:
        return None
    return fields


def total(rows):
    """Sum the amount column (third field) of parsed rows.

    Blank amount fields are skipped (not counted as 0 and not an error).
    Rows that are missing the amount column are skipped as well.
    """
    acc = 0
    for r in rows:
        if not r or len(r) < N_COLS:
            continue
        value = r[2]
        if value is None:
            continue
        if isinstance(value, str):
            value = value.strip()
            if not value:
                continue
        acc += int(value)
    return acc
