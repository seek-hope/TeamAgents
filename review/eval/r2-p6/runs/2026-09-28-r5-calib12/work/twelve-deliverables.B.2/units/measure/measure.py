class Impl:
    def to_cm(self, value, unit):
        """把长度换算成厘米。

        语义边界：
          - 支持的单位只有 ``"m"``（*100）、``"cm"``（*1）、``"mm"``（/10）；
          - 其他任何单位（包括 None、大小写不同的写法）抛 ``ValueError``；
          - 返回值是数值（米/毫米换算时可能是浮点数）。
        """
        if unit == "m":
            return value * 100
        if unit == "cm":
            return value
        if unit == "mm":
            return value / 10
        raise ValueError("unsupported unit: %r" % (unit,))
