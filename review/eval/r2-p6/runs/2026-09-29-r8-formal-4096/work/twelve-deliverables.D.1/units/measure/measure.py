class Impl:
    def to_cm(self, value, unit):
        if unit == "m":
            return value * 100
        if unit == "cm":
            return value
        if unit == "mm":
            return value / 10
        raise ValueError("unsupported unit: %r" % (unit,))
