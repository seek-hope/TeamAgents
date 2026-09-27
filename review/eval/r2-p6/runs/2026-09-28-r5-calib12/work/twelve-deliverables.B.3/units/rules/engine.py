def evaluate(rules, facts):
    """实现 {"all": [...]} 与 {"any": [...]} 两种规则。

    - all：列出的键全部为真才为 True（空列表为 True）。
    - any：列出的键任一为真即为 True（空列表为 False）。
    - 同时给出两者时两个条件都要满足；没有任何已知键时返回 False。
    """
    handled = False
    if "all" in rules:
        handled = True
        if not all(bool(facts.get(key, False)) for key in rules["all"]):
            return False
    if "any" in rules:
        handled = True
        if not any(bool(facts.get(key, False)) for key in rules["any"]):
            return False
    return handled
