#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
static int64_t *a;
static void qs(int64_t lo, int64_t hi) {
  if (lo >= hi) return;
  int64_t p = a[(lo + hi) / 2], i = lo, j = hi;
  for (;;) {
    while (a[i] < p) i++;
    while (a[j] > p) j--;
    if (i >= j) break;
    int64_t t = a[i]; a[i] = a[j]; a[j] = t;
    i++; j--;
  }
  qs(lo, j);
  qs(j + 1, hi);
}
int main(void) {
  int64_t n = 5000000, x = 12345;
  a = malloc(n * sizeof *a);
  for (int64_t i = 0; i < n; i++) { x = (x * 1103515245 + 12345) & 2147483647; a[i] = x % 1000000000; }
  qs(0, n - 1);
  int64_t sum = 0;
  for (int64_t i = 0; i < n; i++) sum += a[i] * (i % 7);
  printf("%lld\n", (long long)sum);
  free(a);
  return 0;
}
