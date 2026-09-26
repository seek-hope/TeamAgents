def evaluate(rules, facts):
    """Evaluate a rule tree against a set of facts.

    Supported forms:
      * ``{"all": [...]}`` - True when every listed fact is truthy.
      * ``{"any": [...]}`` - True when at least one listed fact is truthy.

    An empty ``all`` is vacuously True; an empty ``any`` is False.  Unknown
    rule shapes evaluate to False.
    """
    if not isinstance(rules, dict):
        return False

    if "all" in rules:
        return all(bool(facts.get(name)) for name in rules["all"])

    if "any" in rules:
        return any(bool(facts.get(name)) for name in rules["any"])

    return False
