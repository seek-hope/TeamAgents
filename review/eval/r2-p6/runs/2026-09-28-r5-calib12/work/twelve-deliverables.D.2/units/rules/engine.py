def evaluate(rules, facts):
    """Evaluate a rule against facts.

    Semantics:
      {"all": [keys]} -> True iff every key is truthy in facts (vacuously True when empty)
      {"any": [keys]} -> True iff at least one key is truthy in facts (False when empty)
    A missing key counts as falsy.
    """
    if "all" in rules:
        return all(bool(facts.get(k)) for k in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(k)) for k in rules["any"])
    raise ValueError("unknown rule: %r" % (rules,))
