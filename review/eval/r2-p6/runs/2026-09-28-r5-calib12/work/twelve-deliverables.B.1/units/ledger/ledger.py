"""一个只关心单个账户的小账本。"""


class Ledger:
    def apply(self, rows, op):
        """筛选出 account == op 的行（保持原顺序）；金额为负时抛 ValueError。"""
        selected = []
        for kind, account, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if account == op:
                selected.append((kind, account, cents))
        return selected


def balance(rows, account):
    """该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
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
