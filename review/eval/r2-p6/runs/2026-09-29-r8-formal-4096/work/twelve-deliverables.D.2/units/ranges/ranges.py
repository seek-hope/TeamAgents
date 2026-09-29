class Impl:
    """Merge closed integer/real intervals."""

    def merge(self, ranges):
        """Merge overlapping closed intervals.

        Ranges are given as pairs ``[start, end]`` with ``start <= end``.
        Two closed intervals overlap (or touch) when the next interval's
        start is less than or equal to the current interval's end, so
        ``[1, 3]`` and ``[2, 4]`` merge into ``(1, 4)``.

        Returns a list of ``(start, end)`` tuples sorted by start.
        Empty (or falsy) input returns ``[]``.
        """
        if not ranges:
            return []

        ordered = sorted((pair[0], pair[1]) for pair in ranges)

        merged = []
        cur_start, cur_end = ordered[0]
        for start, end in ordered[1:]:
            if start <= cur_end:
                # Overlapping or touching closed intervals: extend.
                if end > cur_end:
                    cur_end = end
            else:
                merged.append((cur_start, cur_end))
                cur_start, cur_end = start, end
        merged.append((cur_start, cur_end))
        return merged
