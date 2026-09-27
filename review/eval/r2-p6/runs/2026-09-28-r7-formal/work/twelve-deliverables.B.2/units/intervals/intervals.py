"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def _normalize(self, ranges):
        normalized = []
        for r in ranges:
            lo, hi = r[0], r[1]
            if lo > hi:
                lo, hi = hi, lo
            normalized.append((lo, hi))
        normalized.sort()
        return normalized

    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。
        """
        merged = []
        for lo, hi in self._normalize(ranges):
            if merged and lo - merged[-1][1] - 1 <= gap:
                if hi > merged[-1][1]:
                    merged[-1] = (merged[-1][0], hi)
            else:
                merged.append((lo, hi))
        return merged

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。
        """
        hlo, hhi = hole[0], hole[1]
        if hlo > hhi:
            hlo, hhi = hhi, hlo

        out = []
        for lo, hi in self.merge(ranges):
            if hhi < lo or hlo > hi:
                # 完全不相交，原样保留
                out.append((lo, hi))
                continue
            if hlo <= lo and hi <= hhi:
                # 被完全覆盖
                continue
            if lo <= hlo - 1:
                out.append((lo, hlo - 1))
            if hhi + 1 <= hi:
                out.append((hhi + 1, hi))
        return out

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))
