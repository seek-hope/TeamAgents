def evaluate(rules, facts):
    """Evaluate a rule dict against a facts dict.

    ``rules`` is a mapping with either:

    * key ``"all"``: a list of fact names; the result is True iff every
      named fact is truthy in ``facts``.
    * key ``"any"``: a list of fact names; the result is True iff at least
      one named fact is truthy in ``facts``.

    Missing fact names count as falsy. The result is always a real Python
    ``bool`` so that identity checks (``is True`` / ``is False``) hold.
    """
    if "all" in rules:
        return bool(all(facts.get(name) for name in rules["all"]))
    if "any" in rules:
        return bool(any(facts.get(name) for name in rules["any"]))
    raise ValueError("rules must contain either 'all' or 'any'")
