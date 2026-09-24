"""Behavioural reproduction of the one suspected defect in each src/mXX.py.

Run:  python3 verify_defects.py
"""
import gc
import importlib.util
import os
import sys
import tempfile
import warnings

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, "src", name + ".py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


print("== m01 (off_by_one) ==")
m01 = load("m01")
print("  total([1,2,3,4]) =", m01.total([1, 2, 3, 4]), "(expected 10)")

print("== m02 (mutable_default) ==")
m02 = load("m02")
print("  push(1) =", m02.push(1), " push(2) =", m02.push(2), "(default list is shared)")

print("== m03 (shadowed_builtin) ==")
m03 = load("m03")
print("  mean([1,2,3,4]) =", m03.mean([1, 2, 3, 4]))
import dis
names = [i.argval for i in dis.get_instructions(m03.mean) if i.opname == "STORE_FAST"]
print("  local var names:", names, "-> 'sum' shadows builtins.sum")

print("== m04 (wrong_except) ==")
m04 = load("m04")
print("  parse('abc') =", m04.parse("abc"))
print("  parse(None)  =", m04.parse(None), "(TypeError silently swallowed by bare except)")

print("== m05 (integer_division) ==")
m05 = load("m05")
print("  ratio(7,2) =", m05.ratio(7, 2), "(expected 3.5)")

print("== m06 (unclosed_resource) ==")
m06 = load("m06")
with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
    fh.write("hello")
    tmp = fh.name
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    data = m06.read(tmp)
    gc.collect()
print("  read() =", data, "| ResourceWarning(s):", [str(x.message) for x in w])
os.unlink(tmp)

print("== m07 (silent_truncation) ==")
m07 = load("m07")
xs = list(range(10))
print("  head(range(10), 2) =", m07.head(xs, 2), "(n ignored for n<=3, silently returns all)")
print("  head(range(10), 5) =", m07.head(xs, 5))

print("== m08 (mutable_shared_state) ==")
m08 = load("m08")
m08.cached("a", 1)
m08.cached("b", 2)
print("  CACHE after two independent calls:", m08.cached("a", 3), "(caller receives/mutates module-global dict)")

print("== m09 (missing_validation) ==")
m09 = load("m09")
print("  withdraw(100, 150) =", m09.withdraw(100, 150), "(negative balance accepted)")
print("  withdraw(100, -50) =", m09.withdraw(100, -50), "(negative amount accepted)")

print("== m10 (wrong_default) ==")
m10 = load("m10")
print("  connect('db') =", m10.connect("db"), "(tls defaults to False)")

print("== m11 (sort_instability) ==")
m11 = load("m11")
rows_a = [("x", 1), ("y", 1), ("z", 1)]
rows_b = [("z", 1), ("y", 1), ("x", 1)]
print("  ranked(A) =", m11.ranked(rows_a))
print("  ranked(B) =", m11.ranked(rows_b), "(equal keys -> order depends on input, no tiebreaker)")

print("== m12 (resource_leak) ==")
m12 = load("m12")
paths = []
for i in range(5):
    fh = tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False)
    fh.write("x")
    fh.close()
    paths.append(fh.name)
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    n = m12.process(paths)
    gc.collect()
def open_fd_targets():
    out = []
    for fd in os.listdir("/proc/self/fd"):
        try:
            out.append(os.readlink("/proc/self/fd/" + fd))
        except OSError:
            pass
    return out


targets = open_fd_targets()
print("  process(5 paths) =", n,
      "| still-open fds for those paths:", sum(1 for p in paths if p in targets),
      "| ResourceWarning(s):", [str(x.message) for x in w])
for p in paths:
    os.unlink(p)
