class Impl:
    _FACTORS = {"m": 100.0, "cm": 1.0, "mm": 0.1}

    def to_cm(self, value, unit):
        """把 value（以 unit 为单位）换算成厘米。

        支持 m / cm / mm；非法单位抛 ValueError。
        """
        if unit not in self._FACTORS:
            raise ValueError("unsupported unit: %r" % (unit,))
        return float(value) * self._FACTORS[unit]
