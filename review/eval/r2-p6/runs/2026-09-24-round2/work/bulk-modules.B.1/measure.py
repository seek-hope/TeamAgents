class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        try:
            factor = self._FACTORS[unit.lower()]
        except (AttributeError, KeyError):
            raise ValueError(f"unsupported unit: {unit!r}")
        return int(round(value * factor))
