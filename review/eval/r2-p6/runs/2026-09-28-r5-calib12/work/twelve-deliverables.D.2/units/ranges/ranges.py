class Impl:
    def merge(self, ranges):
        """Merge overlapping/adjacent closed integer intervals.

        Returns a list of tuples sorted by start; [] for empty input.
        """
        result = []
        for start, end in ranges:
            if start > end:
                start, end = end, start
            result.append((start, end))
        result.sort()
        merged = []
        for start, end in result:
            if merged and start <= merged[-1][1]:
                if end > merged[-1][1]:
                    merged[-1] = (merged[-1][0], end)
            else:
                merged.append((start, end))
        return merged
