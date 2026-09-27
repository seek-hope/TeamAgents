class Impl:
    def to_cm(self, value, unit):
        """把 value 从 unit（m / cm / mm）换算成厘米；非法单位抛 ValueError。"""
        if unit == "m":
            return value * 100
        if unit == "cm":
            return value
        if unit == "mm":
            return value / 10
        raise ValueError(f"unsupported unit: {unit!r}")
