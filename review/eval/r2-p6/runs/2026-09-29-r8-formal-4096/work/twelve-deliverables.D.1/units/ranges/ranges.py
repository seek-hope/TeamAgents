class Impl:
    def merge(self, ranges):
        """Merge overlapping or touching closed intervals.

        Accepts an iterable of [start, end] (or (start, end)) pairs and
        returns a list of (start, end) tuples sorted by start. Intervals
        that overlap, touch, or are nested are combined. Empty input
        yields an empty list.
        """
        if ranges is None:
            return []

        intervals = sorted((start, end) for start, end in ranges)

        merged = []
        for start, end in intervals:
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))

        return merged
