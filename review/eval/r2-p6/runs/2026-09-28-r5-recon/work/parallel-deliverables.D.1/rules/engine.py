def evaluate(rules, facts):
    """Evaluate a rule set against a mapping of named facts.

    Supported rule shapes:
      {"all": [names...]} -> True iff every named fact is truthy in ``facts``.
      {"any": [names...]} -> True iff at least one named fact is truthy.

    Facts that are missing from ``facts`` count as falsy. The return value is
    always a real ``bool`` (callers compare with ``is True`` / ``is False``).
    """
    if not isinstance(rules, dict):
        return False

    facts = facts if isinstance(facts, dict) else {}
    results = []
    for key in ("all", "any"):
        if key in rules:
            names = rules[key] or []
            values = [bool(facts.get(name)) for name in names]
            if key == "all":
                results.append(all(values))
            else:
                results.append(any(values))

    if not results:
        return False
    return bool(all(results))
