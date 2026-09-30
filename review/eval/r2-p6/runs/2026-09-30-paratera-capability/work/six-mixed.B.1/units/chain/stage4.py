def report(totals):
    return "".join("{}:{}\n".format(price, totals[price]) for price in sorted(totals))
