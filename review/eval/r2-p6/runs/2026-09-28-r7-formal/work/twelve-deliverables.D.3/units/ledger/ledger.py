"""一个只关心单个账户的小账本。"""


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        for row in rows:
            kind, _account, cents = row
            if cents < 0:
                raise ValueError("amount must not be negative: %r" % (cents,))
        return [row for row in rows if row[1] == op]


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
    net = 0
    for kind, row_account, cents in rows:
        if row_account != account:
            continue
        if kind == "deposit":
            net += cents
        elif kind == "withdraw":
            net -= cents
    return net
