#include <stdio.h>
int main(void) {
  long n = 5000000, s = 0;
  for (long x = 0; x < n; x++) if (x % 3 == 0) s += x * x % 1000;
  printf("%ld\n", s);
  return 0;
}
