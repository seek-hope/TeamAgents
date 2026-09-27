"""Tiny rule engine: evaluate {"all": [...]} / {"any": [...]} against facts.

A rule is a dict with one (or both) of the keys:

* ``"all"``: every listed fact name must be truthy.
* ``"any"``: at least one listed fact name must be truthy.

Semantics (deliberate, documented edge cases):

* A fact is looked up with ``facts.get(name, False)`` and coerced with
  ``bool()``, so a *missing* fact counts as ``False`` and never raises.
* ``{"all": []}`` -> ``True`` (vacuous truth); ``{"any": []}`` -> ``False``.
* If both keys are present the conditions are combined with AND, i.e. the
  rule holds only when the ``all`` group and the ``any`` group both hold.
* An unknown rule key raises ``ValueError`` (typos are not silently ignored).
* An empty rule dict raises ``ValueError`` (nothing to evaluate).
* The returned value is always a real ``bool``, never a truthy object, so
  ``evaluate(...) is True`` / ``is False`` assertions hold.
"""

_KNOWN_KEYS = ("all", "any")


def _group(rules, key):
    names = rules[key]
    if isinstance(names, str) or not hasattr(names, "__iter__"):
        raise TypeError(
            "rule %r must map to a sequence of fact names, got %r"
            % (key, type(names).__name__)
        )
    return names


def _hit(facts, name):
    """A fact name is satisfied when it is present and truthy."""
    return bool(facts.get(name, False))


def evaluate(rules, facts):
    """Return whether ``facts`` satisfies ``rules``.

    ``rules`` is ``{"all": [...]}``, ``{"any": [...]}`` or both;
    ``facts`` maps fact names to arbitrary values (interpreted as booleans).
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a dict, got %r" % (type(rules).__name__,))
    if not isinstance(facts, dict):
        raise TypeError("facts must be a dict, got %r" % (type(facts).__name__,))

    unknown = [key for key in rules if key not in _KNOWN_KEYS]
    if unknown:
        raise ValueError(
            "unknown rule key(s): %s; expected one of %s"
            % (sorted(map(repr, unknown)), ", ".join(map(repr, _KNOWN_KEYS)))
        )
    if not rules:
        raise ValueError("empty rule: expected 'all' and/or 'any'")

    result = True
    for key in _KNOWN_KEYS:
        if key not in rules:
            continue
        names = _group(rules, key)
        if key == "all":
            result = result and all(_hit(facts, name) for name in names)
        else:
            result = result and any(_hit(facts, name) for name in names)
    return bool(result)
