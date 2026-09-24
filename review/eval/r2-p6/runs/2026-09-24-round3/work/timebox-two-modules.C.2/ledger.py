"""Simple account ledger.

`Ledger.apply(rows, account)` returns the rows belonging to `account`, keeping
their original order.  Any row whose amount (in cents) is negative raises
`ValueError`.  `balance(rows, account)` nets the rows of `account`:
`deposit` adds, `withdraw` subtracts.
"""

_DEPOSIT = "deposit"
_WITHDRAW = "withdraw"


class Ledger:
    def apply(self, rows, account):
        rows = list(rows)
        for _, _, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
        return [row for row in rows if row[1] == account]


def balance(rows, account):
    total = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        if cents < 0:
            raise ValueError("negative amount: %r" % (cents,))
        if kind == _DEPOSIT:
            total += cents
        elif kind == _WITHDRAW:
            total -= cents
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return total
