def evaluate(rules, facts):
    """按规则求值。

    - ``{"all": [...]}``：所有列出的键在 facts 中均为真值才为 True。
    - ``{"any": [...]}``：任意一个键为真值即为 True。
    - 两个键可以同时出现，此时取两者的与。
    - 缺失的键视为假。
    """
    result = True
    if "all" in rules:
        result = result and all(bool(facts.get(k, False)) for k in rules["all"])
    if "any" in rules:
        result = result and any(bool(facts.get(k, False)) for k in rules["any"])
    return bool(result)
