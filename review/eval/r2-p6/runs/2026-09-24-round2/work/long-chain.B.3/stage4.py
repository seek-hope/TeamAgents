def report(totals):
    lines = [f"{price}:{totals[price]}" for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
