"""Reading and normalizing header-less CSV files under ``data/``."""

from __future__ import annotations

import csv
import os


def normalize(path):
    """Read a header-less comma-separated UTF-8 CSV file.

    Each field is stripped of leading/trailing whitespace.  Blank lines and
    whole-line comments (lines whose first non-whitespace character is ``#``)
    are ignored.  Row order is preserved.

    Returns a ``list[list[str]]``.  A missing file yields an empty list.
    """
    if not os.path.isfile(path):
        return []

    rows = []
    with open(path, "r", encoding="utf-8", newline="") as fh:
        for line in fh:
            stripped = line.strip()
            if not stripped:  # blank / whitespace-only line
                continue
            if stripped.startswith("#"):  # whole-line comment
                continue
            fields = next(csv.reader([line]))
            rows.append([field.strip() for field in fields])
    return rows
