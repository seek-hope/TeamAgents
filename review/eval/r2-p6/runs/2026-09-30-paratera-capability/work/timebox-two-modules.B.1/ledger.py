class Ledger:
    """A tiny single-entry ledger helper."""

    def apply(self, rows, account):
        """Return the rows belonging to ``account``, preserving order.

        Each row is a ``(kind, account, cents)`` tuple.  A negative amount is
        rejected with :class:`ValueError`.
        """
        result = []
        for kind, acct, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if acct == account:
                result.append((kind, acct, cents))
        return result


def balance(rows, account):
    """Net amount for ``account``: deposits add, withdrawals subtract."""
    total = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        if kind == "deposit":
            total += cents
        elif kind == "withdraw":
            total -= cents
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return total
