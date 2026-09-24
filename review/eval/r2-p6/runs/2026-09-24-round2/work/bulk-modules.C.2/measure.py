class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        if unit not in self._FACTORS:
            raise ValueError(f"unsupported unit: {unit!r}")
        return int(round(value * self._FACTORS[unit]))
