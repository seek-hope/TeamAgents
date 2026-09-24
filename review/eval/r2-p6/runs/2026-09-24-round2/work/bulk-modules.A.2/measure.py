class Impl:
    _FACTORS = {
        "m": 100,
        "cm": 1,
        "mm": 0.1,
    }

    def to_cm(self, value, unit):
        if not isinstance(unit, str):
            raise ValueError(f"invalid unit: {unit!r}")
        factor = self._FACTORS.get(unit.strip().lower())
        if factor is None:
            raise ValueError(f"invalid unit: {unit!r}")
        return int(round(value * factor))
