"""CSV normalization helpers for the ``tools`` package."""

from __future__ import annotations

import os


def normalize(path):
    """Read a headerless comma-separated UTF-8 file and normalize its rows.

    Semantics:
      * Every field has leading/trailing whitespace stripped.
      * Blank lines (empty or whitespace-only) are ignored.
      * Lines whose first non-whitespace character is ``#`` are treated as
        full-line comments and ignored.  (A ``#`` appearing later in a line is
        ordinary data.)
      * Row order and field order are preserved.
      * A missing file yields ``[]``.

    Returns a ``list[list[str]]``.
    """
    if not os.path.isfile(path):
        return []

    rows = []
    with open(path, "r", encoding="utf-8") as fh:
        for raw_line in fh:
            line = raw_line.rstrip("\n").rstrip("\r")
            if line.strip() == "":
                continue
            if line.lstrip().startswith("#"):
                continue
            fields = [field.strip() for field in line.split(",")]
            rows.append(fields)
    return rows
