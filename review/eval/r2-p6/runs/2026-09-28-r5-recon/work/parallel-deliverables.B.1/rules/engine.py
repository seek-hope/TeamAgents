def _truthy(value):
    return bool(value)


def evaluate(rules, facts):
    """Evaluate a rule against facts.

    Supported rule shapes:
      {"all": [fact, ...]} -> True when every named fact is truthy
      {"any": [fact, ...]} -> True when at least one named fact is truthy
    """
    if not isinstance(rules, dict) or len(rules) != 1:
        raise ValueError("rule must be a single-key dict with 'all' or 'any'")

    (op, names), = rules.items()
    names = list(names or [])

    if op == "all":
        return all(_truthy(facts.get(name, False)) for name in names)
    if op == "any":
        return any(_truthy(facts.get(name, False)) for name in names)
    raise ValueError("unsupported rule operator: %r" % (op,))
