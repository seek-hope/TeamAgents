def evaluate(rules, facts):
    """Evaluate a rule against ``facts``.

    Supported rule shapes:
      * ``{"all": [key, ...]}`` -> True when every listed fact is truthy
      * ``{"any": [key, ...]}`` -> True when at least one listed fact is truthy

    An empty ``all`` is vacuously True, an empty ``any`` is False, and an
    unknown/unset fact counts as False.
    """
    if "all" in rules:
        return all(bool(facts.get(k, False)) for k in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(k, False)) for k in rules["any"])
    return False
