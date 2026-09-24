def parse(text):
    try:
        return int(text)
    except:  # too broad
        return 0
