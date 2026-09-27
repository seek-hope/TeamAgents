"""一个只关心单个账户的小账本。"""

_KINDS = ("deposit", "withdraw")


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        kept = []
        for row in rows:
            kind, account, cents = row
            if kind not in _KINDS:
                raise ValueError("unknown kind: %r" % (kind,))
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if account == op:
                kept.append((kind, account, cents))
        return kept


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
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
