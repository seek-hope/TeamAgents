class Impl:
    def merge(self, ranges):
        """Merge closed integer ranges into sorted, non-overlapping tuples.

        Intervals are treated as inclusive on both ends, so two ranges merge
        only when they overlap (``start <= current_end``); merely adjacent
        ranges such as [1, 4] and [5, 7] stay separate.
        """
        if not ranges:
            return []
        merged = []
        for start, end in sorted((min(s, e), max(s, e)) for s, e in ranges):
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))
        return merged
