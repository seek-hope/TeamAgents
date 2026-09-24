"""Statistics helpers for normalized rows."""
from __future__ import annotations

from collections import Counter

__all__ = ["word_counts"]


def word_counts(rows: list[list[str]]) -> dict[str, int]:
    """Count occurrences of the third field across *rows*.

    The third field (index ``2``) is used as the key.  Rows that lack a
    third field, and third fields that are empty (after normalization),
    are ignored.  The result is a plain ``dict[str, int]``.
    """
    counter: Counter[str] = Counter()
    for row in rows:
        if len(row) < 3:
            continue
        key = row[2]
        if key == "":
            continue
        counter[key] += 1
    return dict(counter)
