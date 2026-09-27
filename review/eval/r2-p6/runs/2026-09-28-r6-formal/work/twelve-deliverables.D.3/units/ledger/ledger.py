"""一个只关心单个账户的小账本。"""


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        out = []
        for kind, account, cents in rows:
            if account != op:
                continue
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            out.append((kind, account, cents))
        return out


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
