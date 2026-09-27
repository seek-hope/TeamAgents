class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        """把 value（给定单位）换算成厘米；非法单位抛 ValueError。"""
        if unit not in self._FACTORS:
            raise ValueError(f"unknown unit: {unit!r}")
        return value * self._FACTORS[unit]
