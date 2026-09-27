def evaluate(rules, facts):
    """Evaluate a rule dict against a fact mapping.

    Supported rule shapes:
      {"all": [keys...]} -> True iff every named key is present and truthy.
      {"any": [keys...]} -> True iff at least one named key is present and truthy.

    Boundaries:
      - Empty "all"  -> True  (vacuously satisfied).
      - Empty "any"  -> False (no satisfying key).
      - Missing keys count as falsy/absent.
      - If a rule carries both "all" and "any", both conditions must hold.
      - A rule naming neither "all" nor "any" is False.

    Always returns a real Python bool (safe to compare with ``is``).
    """
    if not isinstance(rules, dict):
        return False

    facts = facts if facts is not None else {}

    if "all" in rules:
        for key in rules["all"] or []:
            if not facts.get(key):
                return False

    if "any" in rules:
        if not any(facts.get(key) for key in rules["any"] or []):
            return False

    return ("all" in rules) or ("any" in rules)
