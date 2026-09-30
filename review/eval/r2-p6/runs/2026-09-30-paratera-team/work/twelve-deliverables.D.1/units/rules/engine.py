def evaluate(rules, facts):
    """Evaluate a rule against ``facts``.

    Supported rule shapes:
      {"all": [key, ...]} -> True iff every listed fact key is truthy
      {"any": [key, ...]} -> True iff at least one listed fact key is truthy
    """
    if "all" in rules:
        return all(bool(facts.get(k)) for k in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(k)) for k in rules["any"])
    return False
