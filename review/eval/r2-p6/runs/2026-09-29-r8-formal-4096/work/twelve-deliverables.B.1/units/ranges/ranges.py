class Impl:
    def merge(self, ranges):
        """合并闭区间：只有真正重叠（next_lo <= prev_hi）才合并，
        仅相邻的整数区间保持分开。返回按起点排序的元组列表；空输入返回 []。"""
        if not ranges:
            return []
        ordered = sorted((lo, hi) for lo, hi in ranges)
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            if lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1][1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]
