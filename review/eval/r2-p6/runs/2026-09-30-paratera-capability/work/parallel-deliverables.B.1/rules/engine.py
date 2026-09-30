def evaluate(rules, facts):
    """Evaluate a rule set against a mapping of facts.

    Supported rule forms:
      * {"all": [keys...]} -> True when every key is truthy in facts
      * {"any": [keys...]} -> True when at least one key is truthy

    Unknown/absent keys are treated as False.  An unrecognised rule
    shape evaluates to False.
    """
    if "all" in rules:
        return all(bool(facts.get(key, False)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key, False)) for key in rules["any"])
    return False
