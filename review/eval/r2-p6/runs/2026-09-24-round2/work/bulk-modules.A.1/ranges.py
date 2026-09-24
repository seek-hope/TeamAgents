class Impl:
    def merge(self, ranges):
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: r[0])
        result = []
        for start, end in ordered:
            if result and start <= result[-1][1]:
                prev_start, prev_end = result[-1]
                result[-1] = (prev_start, max(prev_end, end))
            else:
                result.append((start, end))
        return result
