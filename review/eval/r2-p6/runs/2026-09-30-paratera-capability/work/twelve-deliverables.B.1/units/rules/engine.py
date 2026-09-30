def evaluate(rules, facts):
    """根据 facts 求值规则。支持 {"all": [...]} 与 {"any": [...]}。
    缺失的键按 False 处理；无法识别的规则返回 False。"""
    if not isinstance(rules, dict):
        return False
    if "all" in rules:
        return all(bool(facts.get(k, False)) for k in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(k, False)) for k in rules["any"])
    return False
