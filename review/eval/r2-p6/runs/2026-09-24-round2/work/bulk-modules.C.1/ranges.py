class Impl:
    """Merge closed integer intervals."""

    def merge(self, ranges):
        """Sort intervals by start and merge overlapping ones.

        Intervals are closed, ``[start, end]``, so intervals that share an
        endpoint (``next.start <= current.end``) are merged as well.  Merely
        consecutive integers (e.g. ``[1, 4]`` and ``[5, 7]``) are kept
        separate.  Returns a list of ``(start, end)`` tuples.
        """
        normalized = [
            (start, end) if start <= end else (end, start) for start, end in ranges
        ]
        merged = []
        for start, end in sorted(normalized):
            if merged and start <= merged[-1][1]:
                last_start, last_end = merged[-1]
                merged[-1] = (last_start, max(last_end, end))
            else:
                merged.append((start, end))
        return merged
