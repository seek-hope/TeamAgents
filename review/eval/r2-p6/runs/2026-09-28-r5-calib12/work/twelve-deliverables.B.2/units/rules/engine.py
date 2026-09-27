"""布尔规则的求值引擎。"""


def evaluate(rules, facts):
    """求值 ``{"all": [...]}`` 或 ``{"any": [...]}`` 形式的规则。

    语义边界：
      - ``all``：列表中每个事实名都为真才为真（空列表为真，空真）；
      - ``any``：列表中任一个事实名为真即为真（空列表为假）；
      - 事实从 ``facts`` 字典按名取值，缺失的名字视为假；
      - 同一规则里同时出现 ``all`` 与 ``any`` 时按 ``all`` 处理；
      - 既没有 ``all`` 也没有 ``any`` 时返回 ``False``。
    """
    if not isinstance(rules, dict):
        return False
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
