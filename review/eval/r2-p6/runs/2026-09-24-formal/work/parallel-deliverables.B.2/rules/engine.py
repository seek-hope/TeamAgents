def evaluate(rules, facts):
    """对 facts 求值 rules。

    支持两种规则：
      {"all": [key, ...]} —— 所有 key 对应 facts 均为真时返回 True；
      {"any": [key, ...]} —— 任一 key 对应 facts 为真时返回 True。
    """
    if "all" in rules:
        return all(bool(facts.get(key)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key)) for key in rules["any"])
    raise ValueError("rules must contain 'all' or 'any'")
