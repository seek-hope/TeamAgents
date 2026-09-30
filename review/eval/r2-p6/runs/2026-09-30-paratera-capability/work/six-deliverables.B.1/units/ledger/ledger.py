"""一个只关心单个账户的小账本。

语义边界：
- 行必须是 ``(kind, account, cents)`` 三元组；``cents`` 为负抛 ``ValueError``。
- ``kind`` 只允许 ``"deposit"`` 与 ``"withdraw"``，其它值抛 ``ValueError``。
- ``apply(rows, account)`` 在**校验全部行**后返回属于该账户的行（保持原顺序）。
- ``balance(rows, account)`` 只看该账户：deposit 相加、withdraw 相减；
  该账户没有行时返回 ``0``。
"""


class Ledger:
    def apply(self, rows, account):
        for row in rows:
            kind, _acct, cents = row
            if kind not in ("deposit", "withdraw"):
                raise ValueError("unknown kind: %r" % (kind,))
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
        return [row for row in rows if row[1] == account]


def balance(rows, account):
    net = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        if kind == "deposit":
            net += cents
        elif kind == "withdraw":
            net -= cents
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return net
