class Impl:
    # Number of centimeters in one unit of each supported unit.
    _CM_PER_UNIT = {
        "m": 100,
        "cm": 1,
        "mm": 0.1,
    }

    def to_cm(self, value, unit):
        """Convert ``value`` expressed in ``unit`` to an integer number of cm.

        Supported units are 'm', 'cm' and 'mm'. ``value`` may be an int or a
        float. Any other unit raises ``ValueError``.
        """
        try:
            factor = self._CM_PER_UNIT[unit]
        except KeyError:
            raise ValueError("unsupported unit: {!r}".format(unit)) from None
        # Round to guard against binary floating point representation noise
        # (e.g. 20 * 0.1 == 2.0000000000000004) and return an exact int.
        return int(round(value * factor))
