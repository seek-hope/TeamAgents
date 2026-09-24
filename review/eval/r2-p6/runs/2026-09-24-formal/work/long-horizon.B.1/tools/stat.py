"""Statistics over normalized rows."""

from __future__ import annotations

from collections import Counter


def word_counts(rows):
    """Return a ``{third_field: count}`` mapping.

    Fields that are empty (or missing because a row has fewer than three
    fields) are ignored.
    """
    counter = Counter()
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2]
        if word == "":
            continue
        counter[word] += 1
    return dict(counter)
