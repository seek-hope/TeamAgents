class Impl:
    def merge(self, ranges):
        """合并有交叠的闭区间，返回按起点升序的元组列表；空输入返回 []。"""
        ordered = sorted(tuple(r) for r in ranges)
        merged = []
        for lo, hi in ordered:
            if merged and lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1] = (merged[-1][0], hi)
            else:
                merged.append((lo, hi))
        return merged
