"""布尔规则求值。

语义边界：
- ``{"all": [...]}``：所有列出的 fact 都为真才为真；空列表为 ``True``（空合取）。
- ``{"any": [...]}``：任一 fact 为真即为真；空列表为 ``False``（空析取）。
- 缺失的 fact 视为 ``False``；结果恒为 ``bool``。
- 同时给出 ``all`` 与 ``any`` 时两者需同时满足；没有任何已知键时返回 ``False``。
"""


def evaluate(rules, facts):
    facts = facts or {}
    results = []
    if "all" in rules:
        results.append(all(bool(facts.get(key, False)) for key in rules["all"]))
    if "any" in rules:
        results.append(any(bool(facts.get(key, False)) for key in rules["any"]))
    if not results:
        return False
    return all(results)
