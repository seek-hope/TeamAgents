"""一个只关心单个账户的小账本。"""


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]。

        校验每一行：kind 只能是 "deposit"/"withdraw"，金额为负时抛 ValueError。
        返回属于账户 ``op`` 的行（保持原顺序）。
        """
        out = []
        for kind, account, cents in rows:
            if kind not in ("deposit", "withdraw"):
                raise ValueError("bad kind: %r" % (kind,))
            if cents < 0:
                raise ValueError("negative amount: %r" % (cents,))
            if account == op:
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
            raise ValueError("bad kind: %r" % (kind,))
    return total
