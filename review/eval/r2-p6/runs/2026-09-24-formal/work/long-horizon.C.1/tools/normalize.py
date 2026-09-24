"""Read headerless comma-separated CSV files under ``data/``.

Semantics
---------
* UTF-8 text, comma-separated, no header row.
* Every field is stripped of leading/trailing whitespace (``str.strip()``).
* Blank lines (empty after stripping) are ignored.
* Whole-line comments, i.e. lines whose first non-whitespace character is
  ``#``, are ignored.  ``#`` is *not* special inside a data line.
* Row order is preserved.
* A missing file yields ``[]`` (``FileNotFoundError`` is swallowed).

Returns ``list[list[str]]``.
"""
from __future__ import annotations

import csv
import os

__all__ = ["normalize"]


def normalize(path: str) -> list[list[str]]:
    """Return the normalized rows of the CSV file at *path*."""
    try:
        with open(path, "r", encoding="utf-8", newline="") as fh:
            lines = fh.readlines()
    except FileNotFoundError:
        return []

    rows: list[list[str]] = []
    for raw in lines:
        if not raw.strip():
            continue  # blank line
        if raw.lstrip().startswith("#"):
            continue  # whole-line comment
        # ``csv.reader`` needs an iterable of lines; a single-element list
        # keeps each physical line as exactly one record.
        for record in csv.reader([raw]):
            rows.append([field.strip() for field in record])
    return rows
