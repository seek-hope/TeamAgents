"""一个只关心单个账户的小账本。"""


def _check(kind, cents):
    if kind not in ("deposit", "withdraw"):
        raise ValueError("unknown kind: %r" % (kind,))
    if cents < 0:
        raise ValueError("amount must not be negative: %r" % (cents,))


class Ledger:
    def apply(self, rows, op):
        """rows 是 [(kind, account, cents), ...]，返回**该账户**的行（保持原顺序）。

        语义边界：
          - kind 只有 "deposit" 与 "withdraw"，其他值抛 ``ValueError``；
          - 金额为负时抛 ``ValueError``（整批输入都校验，不只是目标账户的行）；
          - 返回值是仅属于 ``op`` 账户的行，保持输入顺序。
        """
        for kind, _account, cents in rows:
            _check(kind, cents)
        return [(kind, account, cents)
                for kind, account, cents in rows if account == op]


def balance(rows, account):
    """返回该账户的净额：deposit 相加、withdraw 相减；没有该账户的行时返回 0。

    语义边界：
      - 先按账户过滤，忽略其他账户的行；
      - kind 只有 "deposit" 与 "withdraw"，其他值抛 ``ValueError``。
    """
    net = 0
    for kind, acct, cents in rows:
        if acct != account:
            continue
        _check(kind, cents)
        if kind == "deposit":
            net += cents
        else:
            net -= cents
    return net
