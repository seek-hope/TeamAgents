class Impl:
    def merge(self, ranges):
        """合并闭区间，返回按起点排序的元组列表。

        只有真正重叠或端点接触（next.lo <= cur.hi）的区间才合并；
        相邻但不相交（如 (1,4) 与 (5,7)）保持分离。
        """
        normalized = []
        for r in ranges:
            lo, hi = r[0], r[1]
            if lo > hi:
                lo, hi = hi, lo
            normalized.append((lo, hi))
        normalized.sort()

        merged = []
        for lo, hi in normalized:
            if merged and lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1] = (merged[-1][0], hi)
            else:
                merged.append((lo, hi))
        return merged
