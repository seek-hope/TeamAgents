"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。"""
        normalized = [tuple(r) for r in ranges]
        if not normalized:
            return []
        normalized.sort(key=lambda r: (r[0], r[1]))
        merged = [list(normalized[0])]
        for lo, hi in normalized[1:]:
            prev_lo, prev_hi = merged[-1]
            missing = lo - prev_hi - 1
            if missing <= gap:
                if hi > prev_hi:
                    merged[-1][1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。"""
        h_lo, h_hi = hole
        result = []
        for lo, hi in self.merge(ranges):
            if hi < h_lo or lo > h_hi:
                # 与 hole 不相交
                result.append((lo, hi))
                continue
            if lo < h_lo:
                result.append((lo, h_lo - 1))
            if hi > h_hi:
                result.append((h_hi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))
