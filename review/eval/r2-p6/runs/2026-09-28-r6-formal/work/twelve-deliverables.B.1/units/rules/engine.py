def evaluate(rules, facts):
    """规则求值。

    - {"all": [k, ...]}：所有 k 在 facts 中为真才返回 True（空列表为 True）。
    - {"any": [k, ...]}：任一 k 为真即返回 True（空列表为 False）。
    - 缺少某个 key 视为 False；无法识别的规则返回 False。
    """
    if "all" in rules:
        return all(bool(facts.get(key, False)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key, False)) for key in rules["any"])
    return False
