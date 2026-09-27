def evaluate(rules, facts):
    """Evaluate a rule mapping against facts.

    Supported rules (real ``bool`` results):
      - ``{"all": [keys]}``: True iff every key maps to a truthy value in facts.
      - ``{"any": [keys]}``: True iff at least one key is truthy in facts.

    Missing keys count as falsy.
    """
    if "all" in rules:
        return all(bool(facts.get(key)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key)) for key in rules["any"])
    return False
