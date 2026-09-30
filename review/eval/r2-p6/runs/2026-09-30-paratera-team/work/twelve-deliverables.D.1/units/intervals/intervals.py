"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。"""
        if not ranges:
            return []
        normalized = []
        for lo, hi in ranges:
            if lo > hi:
                lo, hi = hi, lo
            normalized.append((lo, hi))
        normalized.sort()
        merged = []
        for lo, hi in normalized:
            if merged and lo - merged[-1][1] - 1 <= gap:
                prev_lo, prev_hi = merged[-1]
                if hi > prev_hi:
                    merged[-1] = (prev_lo, hi)
            else:
                merged.append((lo, hi))
        return merged

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。"""
        merged = self.merge(ranges)
        if not merged:
            return []
        hlo, hhi = hole
        if hlo > hhi:
            hlo, hhi = hhi, hlo
        result = []
        for lo, hi in merged:
            if hi < hlo or lo > hhi:
                # 与 hole 完全不相交，原样保留
                result.append((lo, hi))
                continue
            if lo < hlo:
                result.append((lo, hlo - 1))
            if hi > hhi:
                result.append((hhi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))
