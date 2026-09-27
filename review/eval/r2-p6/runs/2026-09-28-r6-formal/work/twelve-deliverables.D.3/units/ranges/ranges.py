class Impl:
    def merge(self, ranges, *args, **kwargs):
        """Merge overlapping (or directly adjacent) closed integer intervals.

        Returns a list of tuples ``(start, end)`` sorted by start.
        """
        intervals = sorted((r[0], r[1]) for r in ranges)
        merged = []
        for start, end in intervals:
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))
        return merged
