def evaluate(rules, facts):
    """Evaluate a rule against a set of facts.

    A rule is a single-key dict:
      {"all": [keys]} -> True when every key is truthy in facts.
      {"any": [keys]} -> True when at least one key is truthy in facts.
    Unknown rule shapes evaluate to False.
    """
    if "all" in rules:
        return all(facts.get(key, False) for key in rules["all"])
    if "any" in rules:
        return any(facts.get(key, False) for key in rules["any"])
    return False
