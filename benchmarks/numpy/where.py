import numpy as np, time
n = 10_000_000
a = (np.arange(n) % 10000).astype(np.float64)
b = (np.arange(n) % 3).astype(np.float64)
def work(a, b):
    m = a > 5000
    c = np.where(m, a, b)
    return c.sum(), m.sum()
ws, wc = work(a, b)
t0 = time.perf_counter()
s, cnt = work(a, b)
t1 = time.perf_counter()
print(repr(float(s))); print(cnt); print(repr(float(ws))); print(wc)
print("ms=%.1f" % ((t1 - t0) * 1000))
