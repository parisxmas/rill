import numpy as np, time
n = 5_000_000
a = (np.arange(n, dtype=np.int64) * 7919 % 1000003).astype(np.float64) - 500000.0
b = np.where(np.arange(n) % 5 == 0, np.nan, (np.arange(n) % 1000).astype(np.float64) - 500.0)
def work(a, b):
    return (np.median(a), np.percentile(a, 25) + np.percentile(a, 75),
            np.nansum(b) + np.nanmean(b) + np.nanmax(b))
wm, wp, wn = work(a, b)
t0 = time.perf_counter()
m, p, nn = work(a, b)
t1 = time.perf_counter()
print(repr(float(m))); print(repr(float(p))); print(repr(float(nn))); print(repr(float((wm + wp + wn))))
print("ms=%.1f" % ((t1 - t0) * 1000))
