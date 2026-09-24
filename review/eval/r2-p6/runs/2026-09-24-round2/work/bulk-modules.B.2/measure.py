class Impl:
    def to_cm(self, value, unit):
        factors = {"m": 100, "cm": 1, "mm": 0.1}
        if unit not in factors:
            raise ValueError(f"unsupported unit: {unit!r}")
        return round(value * factors[unit])
