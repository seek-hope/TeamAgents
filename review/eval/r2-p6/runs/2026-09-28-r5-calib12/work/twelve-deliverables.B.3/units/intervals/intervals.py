"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。"""
        if not ranges:
            return []
        ordered = sorted((r[0], r[1]) for r in ranges)
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            prev = merged[-1]
            missing = lo - prev[1] - 1
            if missing <= gap:
                if hi > prev[1]:
                    prev[1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。"""
        hole_lo, hole_hi = hole
        result = []
        for lo, hi in self.merge(ranges):
            if hole_hi < lo or hole_lo > hi:
                result.append((lo, hi))
                continue
            if lo < hole_lo:
                result.append((lo, hole_lo - 1))
            if hi > hole_hi:
                result.append((hole_hi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))
