class Impl:
    _FACTORS = {"m": 100, "cm": 1, "mm": 0.1}

    def to_cm(self, value, unit):
        try:
            factor = self._FACTORS[unit]
        except KeyError:
            raise ValueError("unknown unit: %r" % (unit,))
        return value * factor


if __name__ == "__main__":
    _I = Impl()
    _VAL, _UNIT = 0, "m"
    print(_I.to_cm(_VAL, _UNIT))
