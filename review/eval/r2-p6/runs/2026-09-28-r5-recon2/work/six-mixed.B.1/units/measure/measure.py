class Impl:
    _TO_CM = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        if unit not in self._TO_CM:
            raise ValueError("invalid unit: %r" % (unit,))
        return int(round(value * self._TO_CM[unit]))
