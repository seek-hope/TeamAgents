"""Length unit conversion helpers."""

# Factor: multiply a value in the given unit by this to get centimeters.
_TO_CM = {
    "m": 100.0,
    "cm": 1.0,
    "mm": 0.1,
}


class Impl:
    def to_cm(self, value, unit):
        """Convert ``value`` expressed in ``unit`` (m, cm or mm) to centimeters.

        Raises ValueError for unsupported units.
        """
        try:
            factor = _TO_CM[unit]
        except (KeyError, TypeError):
            raise ValueError("unsupported unit: {!r}".format(unit))
        return value * factor
