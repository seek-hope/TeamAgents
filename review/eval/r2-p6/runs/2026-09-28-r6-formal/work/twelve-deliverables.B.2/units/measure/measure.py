_FACTORS = {"m": 100.0, "cm": 1.0, "mm": 0.1}


class Impl:
    def to_cm(self, value, unit):
        """把 value（给定单位）换算成厘米；支持 m/cm/mm，非法单位抛 ValueError。"""
        if unit not in _FACTORS:
            raise ValueError("unsupported unit: %r" % (unit,))
        return value * _FACTORS[unit]
