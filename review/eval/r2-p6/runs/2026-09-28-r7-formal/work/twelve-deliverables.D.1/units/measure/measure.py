class Impl:
    _TO_CM = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        try:
            factor = self._TO_CM[unit]
        except KeyError:
            raise ValueError("illegal unit: %r" % (unit,))
        return value * factor
