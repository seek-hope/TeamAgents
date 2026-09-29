class Impl:
    def to_cm(self, value, unit):
        """把长度换算成厘米：m -> *100，cm -> 原值，mm -> /10；其他单位抛 ValueError。"""
        if unit == "m":
            return value * 100
        if unit == "cm":
            return value
        if unit == "mm":
            return value / 10
        raise ValueError("unsupported unit: %r" % (unit,))
