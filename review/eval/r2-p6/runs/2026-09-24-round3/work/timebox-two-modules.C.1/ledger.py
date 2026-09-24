class Ledger:
    def apply(self, rows, account):
        for kind, acct, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % ((kind, acct, cents),))
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
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return total
