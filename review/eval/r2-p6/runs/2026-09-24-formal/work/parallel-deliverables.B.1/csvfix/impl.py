"""Small CSV helpers.

Semantics required by ``test_csvfix.py``:

* ``parse`` splits one line into fields, trims whitespace from every field and
  returns ``None`` when the line does not have exactly 3 columns.
* ``total`` sums the third column of the rows and skips rows whose amount
  field is blank (or that have no third column).
"""


def parse(line):
    """Split ``line`` into 3 trimmed fields, or return ``None``.

    Double quotes are honoured so a comma inside a quoted field does not start
    a new column; ``""`` inside a quoted field is an escaped quote.
    """
    fields = []
    buf = []
    in_quotes = False
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_quotes:
            if ch == '"':
                if i + 1 < n and line[i + 1] == '"':
                    buf.append('"')
                    i += 2
                    continue
                in_quotes = False
            else:
                buf.append(ch)
        elif ch == '"':
            in_quotes = True
        elif ch == ",":
            fields.append("".join(buf).strip())
            buf = []
        else:
            buf.append(ch)
        i += 1
    fields.append("".join(buf).strip())

    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column (amount) of ``rows``.

    Rows without a third column and rows with a blank amount are skipped;
    a non-numeric amount raises ``ValueError`` (via ``int``).
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        amount = row[2].strip()
        if not amount:
            continue
        result += int(amount)
    return result
