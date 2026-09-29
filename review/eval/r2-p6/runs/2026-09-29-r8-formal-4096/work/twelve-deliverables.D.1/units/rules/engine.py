def evaluate(rules, facts):
    """Evaluate a single all/any rule against ``facts``.

    ``{"all": [keys...]}`` -> True iff every listed key maps to a truthy
    value in ``facts``.  ``{"any": [keys...]}`` -> True iff at least one
    listed key is truthy.  Empty lists follow Python's builtins:
    ``all([])`` is True and ``any([])`` is False.

    Missing keys are treated as falsy.
    """
    if "all" in rules:
        return all(facts.get(key) for key in rules["all"])
    if "any" in rules:
        return any(facts.get(key) for key in rules["any"])
    raise ValueError("rule must contain 'all' or 'any'")
