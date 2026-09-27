def evaluate(rules, facts):
    """规则是一个 dict：

    - ``{"all": [k, ...]}``：所有键在 facts 中为真才为真；
    - ``{"any": [k, ...]}``：任意一个键在 facts 中为真即为真。

    返回 Python 的 True/False。
    """
    def holds(key):
        return bool(facts.get(key, False))

    if "all" in rules:
        return bool(all(holds(k) for k in rules["all"]))
    if "any" in rules:
        return bool(any(holds(k) for k in rules["any"]))
    return False
