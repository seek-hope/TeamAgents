def report(totals):
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
