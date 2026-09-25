import numpy as np, time
n = 10_000_000
a = np.arange(n, dtype=np.float64)
b = np.arange(n // 2, dtype=np.float64) * 2.0
ix = (np.arange(n // 2, dtype=np.int64) * 7919 % (n - 1))
def work(a, b, ix):
    j = np.concatenate([a, b])
    t = np.tile(b, 2)
    f = np.flip(a)
    r = np.roll(a, 1234567)
    k = np.take(a, ix)
    return j.sum() + t.sum() + f.sum() + r.sum() + k.sum()
warm = work(a, b, ix)
t0 = time.perf_counter()
s = work(a, b, ix)
t1 = time.perf_counter()
print(repr(float(s))); print(repr(float(warm)))
print("ms=%.1f" % ((t1 - t0) * 1000))
