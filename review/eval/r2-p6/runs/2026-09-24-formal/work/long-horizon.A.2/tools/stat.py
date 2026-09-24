"""Aggregation helpers over normalized rows."""


def word_counts(rows):
    """Return a mapping of third field -> number of occurrences.

    Fields that are empty, or rows that do not have a third field at all,
    are ignored.
    """
    counts = {}
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2]
        if word == "":
            continue
        counts[word] = counts.get(word, 0) + 1
    return counts
