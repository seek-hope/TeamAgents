"""Rule evaluation engine.

A rule is a dict with exactly one of the following keys:

* ``{"all": [f1, f2, ...]}`` -- satisfied when every named fact is truthy.
* ``{"any": [f1, f2, ...]}`` -- satisfied when at least one named fact is truthy.

Facts are looked up in the ``facts`` mapping by name; a name that is missing
from ``facts`` counts as false. Results are always real ``bool`` values.

Empty lists follow the standard vacuous semantics: ``all`` of nothing is true,
``any`` of nothing is false.
"""


def _is_true(name, facts):
    """Return the truthiness of fact ``name``; missing facts are false."""
    try:
        value = facts[name]
    except (KeyError, TypeError):
        return False
    return bool(value)


def evaluate(rules, facts):
    """Evaluate ``rules`` against ``facts`` and return a ``bool``.

    ``rules`` must be a mapping containing ``"all"`` or ``"any"`` (if both are
    present, ``"all"`` takes precedence). Anything that cannot be interpreted
    as a rule evaluates to ``False``.
    """
    if not isinstance(rules, dict):
        return False

    if "all" in rules:
        names = rules["all"]
        if not isinstance(names, (list, tuple, set, frozenset)):
            return False
        return all(_is_true(name, facts) for name in names)

    if "any" in rules:
        names = rules["any"]
        if not isinstance(names, (list, tuple, set, frozenset)):
            return False
        return any(_is_true(name, facts) for name in names)

    return False
