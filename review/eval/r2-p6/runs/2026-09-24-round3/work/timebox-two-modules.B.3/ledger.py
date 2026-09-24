class Ledger:
    def apply(self, rows, account):
        """Return the rows belonging to ``account``, preserving order.

        Any row with a negative amount (in cents) raises ``ValueError``.
        """
        for kind, row_account, cents in rows:
            if cents < 0:
                raise ValueError("amount must not be negative")

        return [row for row in rows if row[1] == account]


def balance(rows, account):
    """Net balance for ``account``: deposits add, withdrawals subtract."""
    total = 0
    for kind, row_account, cents in rows:
        if row_account != account:
            continue
        if kind == "deposit":
            total += cents
        elif kind == "withdraw":
            total -= cents
        else:
            raise ValueError("unknown row kind: %r" % (kind,))
    return total
