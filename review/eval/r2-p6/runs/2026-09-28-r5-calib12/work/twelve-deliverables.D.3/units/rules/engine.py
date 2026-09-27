def _lookup(facts, key):
    """Return the truthiness of ``key`` in ``facts``.

    Missing keys count as False instead of raising, so a rule can safely
    reference facts that were not supplied.
    """
    if isinstance(facts, dict):
        return bool(facts.get(key, False))
    raise TypeError("facts must be a mapping")


def evaluate(rules, facts):
    """Evaluate a rule against a set of facts.

    Supported rule shapes (a dict with exactly one of the keys below):

    * ``{"all": [key, ...]}`` -> True when every referenced fact is truthy
      (an empty list is vacuously True).
    * ``{"any": [key, ...]}`` -> True when at least one referenced fact is
      truthy (an empty list is False).

    Facts is a mapping of name -> value; values are interpreted by truthiness.

    Returns a real ``bool`` so callers can use identity checks.
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a mapping")

    if "all" in rules:
        return all(_lookup(facts, key) for key in rules["all"])
    if "any" in rules:
        return any(_lookup(facts, key) for key in rules["any"])

    raise ValueError("rule must contain either 'all' or 'any'")
