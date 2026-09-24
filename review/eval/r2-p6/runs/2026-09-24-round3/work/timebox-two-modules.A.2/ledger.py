class Ledger:
    def apply(self, rows, account):
        for kind, acct, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
        return [(kind, acct, cents) for kind, acct, cents in rows if acct == account]


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
