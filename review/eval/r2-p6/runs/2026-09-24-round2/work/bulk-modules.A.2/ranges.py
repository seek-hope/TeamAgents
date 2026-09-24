class Impl:
    def merge(self, ranges):
        ordered = sorted((int(start), int(end)) for start, end in ranges)
        merged = []
        for start, end in ordered:
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))
        return merged
