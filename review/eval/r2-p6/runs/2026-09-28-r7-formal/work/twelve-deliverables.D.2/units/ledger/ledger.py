"""一个只关心单个账户的小账本。"""

_KINDS = ("deposit", "withdraw")


class Ledger:
    def apply(self, rows, account):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        selected = []
        for kind, row_account, cents in rows:
            if kind not in _KINDS:
                raise ValueError("unknown kind: %r" % (kind,))
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if row_account == account:
                selected.append((kind, row_account, cents))
        return selected


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
        else:
            raise ValueError("unknown kind: %r" % (kind,))
    return net
