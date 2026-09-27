"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 排序后，若相邻两段之间缺失的整数个数
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回升序列表；空输入返回 []。
        """
        if not ranges:
            return []
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: r[0])
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            prev = merged[-1]
            if lo - prev[1] - 1 <= gap:
                if hi > prev[1]:
                    prev[1] = hi
            else:
                merged.append([lo, hi])
        return [tuple(seg) for seg in merged]

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole，返回剩下的闭区间（升序）。"""
        hlo, hhi = hole
        result = []
        for lo, hi in self.merge(ranges):
            if hhi < lo or hlo > hi:
                result.append((lo, hi))
                continue
            if lo < hlo:
                result.append((lo, hlo - 1))
            if hi > hhi:
                result.append((hhi + 1, hi))
        result.sort()
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))
