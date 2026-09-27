class Impl:
    """Length conversion helper.

    Supported units: ``m``, ``cm``, ``mm``.  Any other unit is rejected
    with :class:`ValueError`.
    """

    # centimetres per supported unit
    _CM_PER_UNIT = {"m": 100.0, "cm": 1.0, "mm": 0.1}

    def to_cm(self, value, unit):
        try:
            per_unit = self._CM_PER_UNIT[unit]
        except (KeyError, TypeError):
            raise ValueError("illegal unit: {!r}".format(unit))
        return value * per_unit
