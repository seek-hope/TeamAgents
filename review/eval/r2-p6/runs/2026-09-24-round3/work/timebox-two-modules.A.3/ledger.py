class Ledger:
    def apply(self, rows, account):
        """Return the rows belonging to ``account``, in their original order.

        Raises ``ValueError`` if any row carries a negative amount.
        """
        for _kind, _account, cents in rows:
            if cents < 0:
                raise ValueError("amount must not be negative: %r" % (cents,))
        return [row for row in rows if row[1] == account]


def balance(rows, account):
    """Net amount for ``account``: deposits add, withdrawals subtract."""
    total = 0
    for kind, row_account, cents in rows:
        if row_account != account:
            continue
        if kind == "deposit":
            total += cents
        elif kind == "withdraw":
            total -= cents
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return total
