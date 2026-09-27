"""极小的规则求值器。

规则是单键字典，支持两种形式：
- {"all": [fact_key, ...]}：所有 fact 都为真才为 True（空列表为 True）。
- {"any": [fact_key, ...]}：任一 fact 为真即为 True（空列表为 False）。

未在 facts 中出现的键视为假。其他键（未知操作）抛 ValueError。
"""


def evaluate(rules, facts):
    if not isinstance(rules, dict) or len(rules) != 1:
        raise ValueError("rule must be a single-key dict: %r" % (rules,))
    (op, keys), = rules.items()
    if op == "all":
        return all(bool(facts.get(k, False)) for k in keys)
    if op == "any":
        return any(bool(facts.get(k, False)) for k in keys)
    raise ValueError("unknown rule operator: %r" % (op,))
