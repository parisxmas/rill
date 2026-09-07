import numpy as np, time
n = 10_000_000
a = np.arange(n, dtype=np.float64) * 0.0001
b = np.arange(n, dtype=np.float64) * -0.0000001
c = np.arange(n, dtype=np.float64) + 1.0
def work(a, b, c):
    return np.sin(a).sum() + np.exp(b).sum() + np.log(c).sum()
warm = work(a, b, c)
t0 = time.perf_counter()
s = work(a, b, c)
t1 = time.perf_counter()
print("%g" % s); print("%g" % warm)
print("ms=%.1f" % ((t1 - t0) * 1000))
