def _entry_true(name, facts):
    """An entry is True iff facts.get(name) is truthy."""
    return bool(facts.get(name))


def evaluate(rules, facts):
    """Evaluate a rule set against facts.

    Supported rules:
      {"all": [name, ...]}  -> True iff every entry is truthy in facts
      {"any": [name, ...]}  -> True iff at least one entry is truthy in facts
    """
    if not isinstance(rules, dict):
        return False
    if facts is None:
        facts = {}

    if "all" in rules:
        names = rules["all"] or []
        return all(_entry_true(name, facts) for name in names)

    if "any" in rules:
        names = rules["any"] or []
        return any(_entry_true(name, facts) for name in names)

    return False
