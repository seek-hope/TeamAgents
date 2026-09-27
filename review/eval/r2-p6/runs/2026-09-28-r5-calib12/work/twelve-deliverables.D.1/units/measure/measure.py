class Impl:
    def to_cm(self, value, unit):
        if unit == "m":
            return value * 100
        elif unit == "cm":
            return value
        elif unit == "mm":
            return value / 10
        else:
            raise ValueError(f"unsupported unit: {unit!r}")
