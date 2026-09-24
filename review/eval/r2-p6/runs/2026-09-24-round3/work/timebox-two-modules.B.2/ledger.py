class Ledger:
    @staticmethod
    def apply(rows, account):
        """Return the rows belonging to ``account`` (order preserved).

        A ``ValueError`` is raised if any row carries a negative amount.
        """
        for row in rows:
            if row[2] < 0:
                raise ValueError("amount must not be negative: %r" % (row,))
        return [tuple(row) for row in rows if row[1] == account]


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
    return total
