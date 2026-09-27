class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        """把 value（unit 单位）换算成厘米；unit 只支持 m/cm/mm，否则抛 ValueError。"""
        try:
            factor = self._FACTORS[unit]
        except (KeyError, TypeError):
            raise ValueError("unknown unit: %r" % (unit,))
        return value * factor
