def evaluate(rules, facts):
    """对 facts 求值规则。

    - {"all": [k, ...]}：所有列出的键在 facts 中为真时才返回 True。
    - {"any": [k, ...]}：任一列出的键为真即返回 True。
    - 其它形状返回 False。
    返回真正的 bool。
    """
    if "all" in rules:
        return all(bool(facts.get(key)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key)) for key in rules["any"])
    return False
