def evaluate(rules, facts):
    """求值规则：``{"all": [...]}`` 需要全部为真，``{"any": [...]}`` 需要至少一个为真。

    缺失的 fact 视为 False。两种键同时出现时以 ``all`` 优先。
    """
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
