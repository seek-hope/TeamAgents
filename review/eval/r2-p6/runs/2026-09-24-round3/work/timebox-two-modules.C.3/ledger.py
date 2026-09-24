class Ledger:
    def apply(self, rows, account):
        for kind, acct, cents in rows:
            if cents < 0:
                raise ValueError("amount must be non-negative: %r" % (cents,))
        return [row for row in rows if row[1] == account]


def balance(rows, account):
    total = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        if kind == "deposit":
            total += cents
        elif kind == "withdraw":
            total -= cents
    return total
