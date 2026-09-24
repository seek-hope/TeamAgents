CACHE = {}
def cached(k, v):
    CACHE[k] = v
    return CACHE
