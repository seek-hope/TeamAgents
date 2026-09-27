class Impl:
    def merge(self, ranges):
        """合并闭区间，返回按起点排序的元组列表（空输入返回 []）。

        相邻两段只要有交叠（next.lo <= prev.hi）就合并。
        """
        if not ranges:
            return []
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
        result = [ordered[0]]
        for lo, hi in ordered[1:]:
            prev_lo, prev_hi = result[-1]
            if lo <= prev_hi:
                result[-1] = (prev_lo, max(prev_hi, hi))
            else:
                result.append((lo, hi))
        return result
