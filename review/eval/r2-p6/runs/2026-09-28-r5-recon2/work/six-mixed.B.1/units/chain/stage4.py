def report(totals):
    lines = ["%d:%d" % (price, totals[price]) for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
