def evaluate(rules, facts):
    """Evaluate a rule against facts.

    Supported rules:
      {"all": ["a", "b"]} -> True when every named fact is truthy.
      {"any": ["a", "b"]} -> True when at least one named fact is truthy.
    Unknown rules evaluate to False.
    """
    if "all" in rules:
        return all(facts.get(name, False) for name in rules["all"])
    if "any" in rules:
        return any(facts.get(name, False) for name in rules["any"])
    return False
