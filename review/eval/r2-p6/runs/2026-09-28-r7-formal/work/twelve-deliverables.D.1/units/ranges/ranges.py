class Impl:
    def merge(self, ranges):
        """Merge closed integer intervals.

        Returns a list of (start, end) tuples sorted by start, with
        overlapping intervals combined.  Returns [] for empty input.
        """
        items = sorted((int(a), int(b)) for a, b in ranges)
        merged = []
        for start, end in items:
            if merged and start <= merged[-1][1]:
                last_start, last_end = merged[-1]
                if end > last_end:
                    merged[-1] = (last_start, end)
            else:
                merged.append((start, end))
        return merged
