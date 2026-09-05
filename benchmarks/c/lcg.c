#include <stdio.h>
#include <stdint.h>
int main(void) {
  int64_t seed = 42, acc = 0;
  for (int64_t k = 100000000; k != 0; k--) {
    int64_t next = (seed * 1103515245 + 12345) % 2147483648LL;
    acc += seed % 100;
    seed = next;
  }
  printf("%lld\n", (long long)acc);
  return 0;
}
