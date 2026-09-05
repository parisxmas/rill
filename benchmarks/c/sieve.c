#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
int main(void) {
  int64_t n = 50000000;
  uint8_t *f = calloc(n + 1, 1);
  for (int64_t i = 2; i * i <= n; i++)
    if (!f[i]) for (int64_t j = i * i; j <= n; j += i) f[j] = 1;
  int64_t c = 0;
  for (int64_t i = 2; i <= n; i++) c += !f[i];
  printf("%lld\n", (long long)c);
  free(f);
  return 0;
}
