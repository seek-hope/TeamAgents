class Impl:
    def to_cm(self, value, unit):
        """把 value（以 unit 为单位）换算成厘米。支持 m/cm/mm，非法单位抛 ValueError。"""
        if unit == "m":
            return value * 100
        if unit == "cm":
            return value
        if unit == "mm":
            return value / 10
        raise ValueError("unsupported unit: %r" % (unit,))
