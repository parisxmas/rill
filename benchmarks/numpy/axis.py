import numpy as np, time
n = 2000
i, j = np.indices((n, n))
a = (i - j).astype(np.float64)
def work(a):
    t = a.T
    cols = t.sum(axis=0); rows = t.sum(axis=1)
    return (cols * np.arange(n)).sum(), (rows * np.arange(n)).sum()
wc, wr = work(a)
t0 = time.perf_counter()
c, r = work(a)
t1 = time.perf_counter()
print("%g" % c); print("%g" % r); print("%g" % (wc + wr))
print("ms=%.1f" % ((t1 - t0) * 1000))
