class Impl:
    _FACTORS_TO_CM = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        """把 value（以 unit 为单位的长度）换成厘米。

        支持 "m" / "cm" / "mm"；非法单位抛 ValueError。
        """
        if unit not in self._FACTORS_TO_CM:
            raise ValueError("unsupported unit: %r" % (unit,))
        return value * self._FACTORS_TO_CM[unit]
