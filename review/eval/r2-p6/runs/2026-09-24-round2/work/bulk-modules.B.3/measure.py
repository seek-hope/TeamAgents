class Impl:
    # centimetres per unit
    _FACTORS = {
        "m": 100,
        "cm": 1,
        "mm": 0.1,
    }

    def to_cm(self, value, unit):
        """Convert ``value`` expressed in ``unit`` to whole centimetres.

        Supported units: ``m``, ``cm``, ``mm``.  Anything else raises
        ``ValueError``.
        """
        try:
            factor = self._FACTORS[unit]
        except (KeyError, TypeError):
            raise ValueError(f"unsupported unit: {unit!r}")
        return int(round(value * factor))
