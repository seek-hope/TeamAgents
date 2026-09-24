class Impl:
    """Length conversion helper (all results are whole centimetres)."""

    _FACTORS = {"m": 100.0, "cm": 1.0, "mm": 0.1}

    def to_cm(self, value, unit):
        """Convert ``value`` expressed in ``unit`` to an integer number of cm.

        Supported units: ``m``, ``cm``, ``mm``. Any other unit raises
        ``ValueError``.
        """
        try:
            factor = self._FACTORS[unit]
        except (KeyError, TypeError):
            raise ValueError(f"unsupported unit: {unit!r}")
        return int(round(value * factor))
