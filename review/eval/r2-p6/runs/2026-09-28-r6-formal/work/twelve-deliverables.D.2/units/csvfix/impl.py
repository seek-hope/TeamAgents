"""CSV helpers used by the csvfix unit.

Semantics
---------
``parse(line)`` splits a single CSV line on commas, strips surrounding
whitespace from every field and returns the resulting list only when the
line yields **exactly three** fields.  Anything else (too few / too many
fields) is not a valid record and ``None`` is returned.

``total(rows)`` sums the numeric value of the third field of each row
(the amount column).  Blank fields and fields that are not valid numbers
("bad" values) are skipped instead of raising, and malformed rows with
fewer than three fields are ignored as well.
"""


def parse(line):
    """Split ``line`` into exactly 3 trimmed fields, else return None.

    Legacy note: the old implementation split without trimming, did not
    handle quoted fields and never validated the column count.
    """
    if not isinstance(line, str):
        return None
    fields = [field.strip() for field in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """Sum the third column of ``rows``, skipping blank/bad values.

    Legacy note: the old implementation assumed every non-empty third
    field was an integer and raised on anything else.
    """
    result = 0
    for row in rows or []:
        if row is None or len(row) < 3:
            continue
        raw = row[2]
        if raw is None:
            continue
        text = str(raw).strip()
        if not text:
            continue
        try:
            result += int(text)
        except ValueError:
            try:
                result += float(text)
            except ValueError:
                continue
    return result
