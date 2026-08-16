#include <algorithm>
#include <cstdio>
#include <limits>
#include <utility>
#include <vector>

std::vector<long long> hd;
std::vector<int> hn;
int hs = 0;

void push(long long d, int v) {
  hd[hs] = d;
  hn[hs] = v;
  int i = hs++;
  while (i > 0 && hd[(i - 1) / 2] > hd[i]) {
    int p = (i - 1) / 2;
    std::swap(hd[i], hd[p]);
    std::swap(hn[i], hn[p]);
    i = p;
  }
}

void pop() {
  hs--;
  hd[0] = hd[hs];
  hn[0] = hn[hs];
  int i = 0;
  for (;;) {
    int l = 2 * i + 1;
    if (l >= hs) return;
    int c = (l + 1 < hs && hd[l + 1] < hd[l]) ? l + 1 : l;
    if (hd[i] <= hd[c]) return;
    std::swap(hd[i], hd[c]);
    std::swap(hn[i], hn[c]);
    i = c;
  }
}

int main() {
  int n = 200000, m = 2000000, s = 0, t = n - 1;
  long long seed = 12345;
  auto rnd = [&] { seed = (seed * 1103515245 + 12345) & 2147483647LL; return seed; };
  std::vector<int> eu(m), ev(m);
  std::vector<long long> ew(m);
  for (int i = 0; i < m; i++) {
    eu[i] = static_cast<int>(rnd() % n);
    ev[i] = static_cast<int>(rnd() % n);
    ew[i] = rnd() % 100;
  }

  std::vector<int> off(n + 2, 0);
  for (int i = 0; i < m; i++) off[eu[i] + 1]++;
  for (int i = 1; i <= n; i++) off[i] += off[i - 1];
  std::vector<int> cur = off;
  std::vector<int> to(m);
  std::vector<long long> wt(m);
  for (int i = 0; i < m; i++) {
    int p = cur[eu[i]]++;
    to[p] = ev[i];
    wt[p] = ew[i];
  }

  std::vector<long long> dist(std::max(n, 1), std::numeric_limits<long long>::max());
  dist[s] = 0;
  hd.resize(m + 1);
  hn.resize(m + 1);
  push(0, s);
  while (hs > 0) {
    long long d = hd[0];
    int u = hn[0];
    pop();
    if (u == t) {
      std::printf("%lld\n", d);
      return 0;
    }
    if (d > dist[u]) continue;
    for (int e = off[u]; e < off[u + 1]; e++) {
      long long nd = d + wt[e];
      if (nd < dist[to[e]]) {
        dist[to[e]] = nd;
        push(nd, to[e]);
      }
    }
  }
  std::printf("-1\n");
}
