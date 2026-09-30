def process(paths):
    handles = [open(p) for p in paths]
    return len(handles)
