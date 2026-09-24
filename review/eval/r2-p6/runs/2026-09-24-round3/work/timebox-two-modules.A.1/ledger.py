class Ledger:
    def apply(self, rows, account):
        result = []
        for row in rows:
            kind, acct, cents = row
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if acct == account:
                result.append((kind, acct, cents))
        return result


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
