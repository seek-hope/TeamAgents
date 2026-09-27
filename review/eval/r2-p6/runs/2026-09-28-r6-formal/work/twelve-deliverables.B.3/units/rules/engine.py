def evaluate(rules, facts):
    """按规则求值。

    - ``{"all": [key, ...]}``：所有 key 在 facts 中为真才为 True。
    - ``{"any": [key, ...]}``：任一 key 在 facts 中为真即为 True。
    - 缺失的 key 视为假；无法识别的规则返回 False。
    """
    if not isinstance(rules, dict):
        return False
    if "all" in rules:
        return all(bool(facts.get(key, False)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key, False)) for key in rules["any"])
    return False
