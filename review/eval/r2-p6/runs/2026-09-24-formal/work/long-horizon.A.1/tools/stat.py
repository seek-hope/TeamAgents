"""Simple statistics helpers for the ``tools`` package."""

from __future__ import annotations


def _check_rows(rows):
    if rows is None:
        raise TypeError("rows must be an iterable of sequences, not None")


def _third_field(row):
    """Return the third field of a row, or None when it is absent/empty."""
    try:
        value = row[2]
    except (IndexError, KeyError, TypeError):
        return None
    if not isinstance(value, str):
        return None
    value = value.strip()
    if value == "":
        return None
    return value


def word_counts(rows):
    """Return a mapping of the third field of each row to its count.

    Rows with fewer than three fields, or whose third field is empty (after
    stripping), are ignored.  Returns a ``dict[str, int]``.
    """
    _check_rows(rows)
    counts = {}
    for row in rows:
        word = _third_field(row)
        if word is None:
            continue
        counts[word] = counts.get(word, 0) + 1
    return counts
