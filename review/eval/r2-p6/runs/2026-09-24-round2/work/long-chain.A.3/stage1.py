"""Stage 1: parse the raw orders CSV into a list of typed dict rows.

Semantics
---------
``parse_orders(path)`` reads the CSV file at ``path`` which is expected to have
a header row ``id,qty,price`` followed by one order per line.  It returns a
list of ``{"id": int, "qty": int, "price": int}`` dicts, preserving the file
order.  The header is detected and skipped by field name; blank lines are
ignored.  All three fields are converted to ``int``.
"""

import csv


def parse_orders(path):
    """Read ``path`` and return a list of order dicts with int fields."""
    rows = []
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle)
        header = True
        for record in reader:
            if not record or all(cell.strip() == "" for cell in record):
                continue
            if header:
                header = False
                # Skip the header row if it really is the documented header.
                if [cell.strip().lower() for cell in record] == ["id", "qty", "price"]:
                    continue
            order_id, qty, price = (cell.strip() for cell in record[:3])
            rows.append({"id": int(order_id), "qty": int(qty), "price": int(price)})
    return rows
