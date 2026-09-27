def evaluate(rules, facts):
    """Evaluate a rule mapping against a fact mapping.

    Supported rule forms:
      {"all": [fact, ...]} -> True iff every listed fact is truthy
      {"any": [fact, ...]} -> True iff at least one listed fact is truthy

    Missing facts are treated as falsy. If both keys are present, both
    conditions must hold. Always returns a real ``bool``.
    """
    results = []
    if "all" in rules:
        results.append(all(bool(facts.get(key, False)) for key in rules["all"]))
    if "any" in rules:
        results.append(any(bool(facts.get(key, False)) for key in rules["any"]))
    if not results:
        return False
    return all(results)
