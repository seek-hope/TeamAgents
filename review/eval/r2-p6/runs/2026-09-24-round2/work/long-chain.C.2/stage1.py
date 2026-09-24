"""Stage 1: parse the raw orders CSV into rows of typed records."""

import csv

FIELDS = ("id", "qty", "price")


def parse_orders(path):
    """Read ``path`` (header ``id,qty,price``) and return a list of dicts.

    Each dict has integer values under the keys ``id``, ``qty`` and ``price``.
    Row order in the file is preserved.
    """
    rows = []
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames != list(FIELDS):
            raise ValueError(
                "unexpected header %r, expected %r"
                % (reader.fieldnames, list(FIELDS))
            )
        for record in reader:
            rows.append({name: int(record[name]) for name in FIELDS})
    return rows
