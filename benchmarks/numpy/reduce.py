import numpy as np, time
n = 20_000_000
a = (np.arange(n) % 1000) * 0.5
def work(a):
    return a.mean(), a.std(), a.max(), a.argmax()
wm, wsd, wmx, wam = work(a)
t0 = time.perf_counter()
m, sd, mx, am = work(a)
t1 = time.perf_counter()
print("%g" % m); print("%g" % sd); print("%g" % mx); print(am)
print("%g" % (wm + wsd + wmx)); print(wam)
print("ms=%.1f" % ((t1 - t0) * 1000))
