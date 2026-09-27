class Impl:
    def merge(self, ranges):
        """Merge closed integer ranges into a start-sorted list of tuples.

        Each input range is a ``[start, end]`` pair with inclusive endpoints.
        Ranges that overlap share a single tuple; ranges separated by a real
        gap are kept apart. Nested and unsorted inputs are handled.
        """
        merged = []
        for start, end in sorted((min(a, b), max(a, b)) for a, b in ranges):
            if merged and start <= merged[-1][1]:
                if end > merged[-1][1]:
                    merged[-1][1] = end
            else:
                merged.append([start, end])
        return [(start, end) for start, end in merged]
