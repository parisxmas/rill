#include <stdio.h>
#include <stdlib.h>

#define INF 0x3fffffffffffffffLL

static long long *hd, *hn;
static int hsize;

static void swap(long long *b, int i, int j) { long long x = b[i]; b[i] = b[j]; b[j] = x; }

static void hpush(long long d, long long v) {
  hd[hsize] = d; hn[hsize] = v;
  int i = hsize++;
  while (i > 0 && hd[(i - 1) / 2] > hd[i]) { int p = (i - 1) / 2; swap(hd, i, p); swap(hn, i, p); i = p; }
}

static void hpop(void) {
  hsize--; hd[0] = hd[hsize]; hn[0] = hn[hsize];
  int i = 0;
  for (;;) {
    int l = 2 * i + 1;
    if (l >= hsize) return;
    int c = (l + 1 < hsize && hd[l + 1] < hd[l]) ? l + 1 : l;
    if (hd[i] <= hd[c]) return;
    swap(hd, i, c); swap(hn, i, c); i = c;
  }
}

static long long seed = 12345;
static long long rnd(void) { seed = (seed * 1103515245 + 12345) & 2147483647LL; return seed; }

int main(void) {
  int n = 200000, m = 2000000, s = 0, t = n - 1;
  int *eu = malloc(m * sizeof *eu), *ev = malloc(m * sizeof *ev);
  long long *ew = malloc(m * sizeof *ew);
  for (int i = 0; i < m; i++) { eu[i] = rnd() % n; ev[i] = rnd() % n; ew[i] = rnd() % 100; }

  int *off = calloc(n + 1, sizeof *off);
  for (int i = 0; i < m; i++) off[eu[i] + 1]++;
  for (int i = 1; i <= n; i++) off[i] += off[i - 1];
  int *cur = malloc((n + 1) * sizeof *cur);
  for (int i = 0; i <= n; i++) cur[i] = off[i];
  int *to = malloc(m * sizeof *to);
  long long *wt = malloc(m * sizeof *wt);
  for (int i = 0; i < m; i++) { int p = cur[eu[i]]++; to[p] = ev[i]; wt[p] = ew[i]; }

  long long *dist = malloc(n * sizeof *dist);
  for (int i = 0; i < n; i++) dist[i] = INF;
  dist[s] = 0;
  hd = malloc((m + 1) * sizeof *hd);
  hn = malloc((m + 1) * sizeof *hn);
  hpush(0, s);
  while (hsize > 0) {
    long long d = hd[0];
    int u = (int)hn[0];
    hpop();
    if (u == t) { printf("%lld\n", d); return 0; }
    if (d > dist[u]) continue;
    for (int e = off[u]; e < off[u + 1]; e++) {
      long long nd = d + wt[e];
      if (nd < dist[to[e]]) { dist[to[e]] = nd; hpush(nd, to[e]); }
    }
  }
  printf("-1\n");
  return 0;
}
