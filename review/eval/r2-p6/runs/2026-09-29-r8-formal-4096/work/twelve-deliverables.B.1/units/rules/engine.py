def evaluate(rules, facts):
    """实现 all/any 两种规则：facts 中查不到的名字视为 False。
    两者同时存在时按 AND 组合；每一条都要满足才返回 True。"""
    if "all" in rules:
        if not all(facts.get(name, False) for name in rules["all"]):
            return False
    if "any" in rules:
        if not any(facts.get(name, False) for name in rules["any"]):
            return False
    return True
