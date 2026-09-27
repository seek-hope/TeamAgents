class Impl:
    _TO_CM = {"m": 100.0, "cm": 1.0, "mm": 0.1}

    def to_cm(self, value, unit):
        """把 value（单位 unit）换算成厘米；非法单位抛 ValueError。"""
        try:
            factor = self._TO_CM[unit]
        except KeyError:
            raise ValueError("unknown unit: %r" % (unit,))
        result = value * factor
        if result == int(result):
            return int(result)
        return result
