class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        """Convert ``value`` expressed in ``unit`` (m/cm/mm) to centimetres."""
        if unit not in self._FACTORS:
            raise ValueError("unknown unit: %r" % (unit,))
        return value * self._FACTORS[unit]
