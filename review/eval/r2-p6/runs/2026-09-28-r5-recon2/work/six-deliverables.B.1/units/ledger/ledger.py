"""一个只关心单个账户的小账本。"""

VALID_KINDS = ("deposit", "withdraw")


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户(op)**的行（保持原顺序）。

        - kind 只有 "deposit" 与 "withdraw"，其它 kind 抛 ValueError。
        - 金额为负时抛 ValueError（校验所有行，而不只是目标账户的行）。
        """
        for row in rows:
            kind, _account, cents = row
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if kind not in VALID_KINDS:
                raise ValueError("unknown kind: %r" % (kind,))
        return [row for row in rows if row[1] == op]


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
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
