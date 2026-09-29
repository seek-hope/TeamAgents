"""一个只关心单个账户的小账本。"""


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        result = []
        for kind, account, cents in rows:
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if account == op:
                result.append((kind, account, cents))
        return result


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
    total = 0
    for kind, acc, cents in rows:
        if acc != account:
            continue
        if kind == "deposit":
            total += cents
        elif kind == "withdraw":
            total -= cents
    return total
