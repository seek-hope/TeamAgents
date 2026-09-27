"""一个只关心单个账户的小账本。"""

_KINDS = ("deposit", "withdraw")


def _validate(rows):
    """校验行：kind 只能是 deposit/withdraw，金额不能为负。"""
    for kind, _account, cents in rows:
        if kind not in _KINDS:
            raise ValueError("unknown kind: %r" % (kind,))
        if cents < 0:
            raise ValueError("negative amount: %r" % (cents,))


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。
        kind 只有 "deposit" 与 "withdraw"；金额为负时抛 ValueError。"""
        _validate(rows)
        return [row for row in rows if row[1] == op]


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。"""
    _validate(rows)
    total = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        if kind == "deposit":
            total += cents
        else:  # kind == "withdraw"，已由 _validate 保证
            total -= cents
    return total
