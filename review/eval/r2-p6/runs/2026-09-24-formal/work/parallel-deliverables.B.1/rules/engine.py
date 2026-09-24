"""Tiny rule evaluator.

A rule is a mapping with either an ``"all"`` key (every listed fact must be
truthy) or an ``"any"`` key (at least one listed fact must be truthy).  Missing
facts count as falsy.
"""


def evaluate(rules, facts):
    """Evaluate ``rules`` against ``facts`` and return a ``bool``."""
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
