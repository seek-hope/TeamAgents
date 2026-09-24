"""Tiny rule engine.

``evaluate(rules, facts)`` supports two combinators:

* ``{"all": [...]}`` -> ``True`` only when every referenced fact is truthy.
* ``{"any": [...]}`` -> ``True`` when at least one referenced fact is truthy.

Entries inside the lists are either fact names (looked up in ``facts``) or
nested rule dicts, so combinators may be nested. Unknown/missing facts are
treated as ``False``.
"""


def _holds(entry, facts):
    """Return whether a single rule entry holds given ``facts``."""
    if isinstance(entry, dict):
        return evaluate(entry, facts)
    return bool(facts.get(entry, False))


def evaluate(rules, facts):
    if not isinstance(rules, dict):
        # Bare fact name used as a rule.
        return _holds(rules, facts)

    if "all" in rules:
        return all(_holds(entry, facts) for entry in rules["all"])
    if "any" in rules:
        return any(_holds(entry, facts) for entry in rules["any"])

    return False
