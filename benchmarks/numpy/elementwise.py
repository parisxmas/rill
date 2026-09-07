import numpy as np, time
n = 10_000_000
a = np.arange(n, dtype=np.float64)
def work(a):
    b = a * 2.0 + 1.0
    c = np.sqrt(a) * b + a
    return c.sum()
warm = work(a)
t0 = time.perf_counter()
s = work(a)
t1 = time.perf_counter()
print("%g" % s); print("%g" % warm)
print("ms=%.1f" % ((t1 - t0) * 1000))
