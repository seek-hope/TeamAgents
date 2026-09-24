class Ledger:
    def apply(self, rows, account):
        selected = []
        for kind, row_account, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if row_account == account:
                selected.append((kind, row_account, cents))
        return selected


def balance(rows, account):
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
