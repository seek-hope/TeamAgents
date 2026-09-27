"""小型规则求值器，支持 all / any 两种聚合。"""


def _truthy_list(keys, facts):
    return [bool(facts.get(key)) for key in keys]


def evaluate(rules, facts):
    """对 ``facts`` 求值 ``rules``。

    - ``{"all": [k, ...]}``：所有列出的 key 都为真时结果为 True（空列表为 True）。
    - ``{"any": [k, ...]}``：至少一个 key 为真时结果为 True（空列表为 False）。
    - 同时给出 all 与 any 时两者都要满足（逻辑与）。
    - 缺失的 key 视为假。
    - 既没有 all 也没有 any 时返回 False。
    """
    checks = []
    if "all" in rules:
        checks.append(all(_truthy_list(rules["all"], facts)))
    if "any" in rules:
        checks.append(any(_truthy_list(rules["any"], facts)))
    if not checks:
        return False
    return all(checks)
