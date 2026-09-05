#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
/* open addressing, linear probing, grown by doubling from 16 at 3/4 full —
   the same policy Rill's Map(k, v) follows. */
typedef struct { int64_t *k, *v; uint8_t *used; int64_t cap, len; } Map;
static uint64_t hsh(int64_t k) { uint64_t x = (uint64_t)k * 0x9E3779B97F4A7C15ull; return x ^ (x >> 29); }
static void init(Map *m, int64_t cap) {
  m->k = malloc(cap * 8); m->v = malloc(cap * 8); m->used = calloc(cap, 1); m->cap = cap; m->len = 0;
}
static void put(Map *m, int64_t k, int64_t v);
static void grow(Map *m) {
  Map n; init(&n, m->cap * 2);
  for (int64_t i = 0; i < m->cap; i++) if (m->used[i]) put(&n, m->k[i], m->v[i]);
  free(m->k); free(m->v); free(m->used); *m = n;
}
static void put(Map *m, int64_t k, int64_t v) {
  if ((m->len + 1) * 4 > m->cap * 3) grow(m);
  uint64_t i = hsh(k) & (m->cap - 1);
  while (m->used[i] && m->k[i] != k) i = (i + 1) & (m->cap - 1);
  if (!m->used[i]) { m->used[i] = 1; m->k[i] = k; m->len++; }
  m->v[i] = v;
}
static int64_t get_or(Map *m, int64_t k, int64_t d) {
  uint64_t i = hsh(k) & (m->cap - 1);
  while (m->used[i]) { if (m->k[i] == k) return m->v[i]; i = (i + 1) & (m->cap - 1); }
  return d;
}
int main(void) {
  Map m; init(&m, 16);
  int64_t n = 2000000, x = 12345;
  for (int64_t i = 0; i < n; i++) { x = (x * 1103515245 + 12345) & 2147483647; put(&m, x, i); }
  /* the same stream again, every other key nudged so half the lookups miss */
  int64_t sum = 0, y = 12345;
  for (int64_t i = 0; i < n; i++) { y = (y * 1103515245 + 12345) & 2147483647; sum += get_or(&m, i % 2 == 0 ? y : y + 1, 0); }
  printf("%lld\n%lld\n", (long long)m.len, (long long)sum);
  return 0;
}
