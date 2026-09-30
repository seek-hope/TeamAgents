class Impl:
    def merge(self, ranges):
        """Merge overlapping or adjacent closed intervals.

        Returns a list of (start, end) tuples sorted by start.
        """
        if not ranges:
            return []

        ordered = sorted((r[0], r[1]) for r in ranges)

        merged = [ordered[0]]
        for start, end in ordered[1:]:
            last_start, last_end = merged[-1]
            if start <= last_end:
                # overlapping or adjacent -> extend
                if end > last_end:
                    merged[-1] = (last_start, end)
            else:
                merged.append((start, end))

        return merged
