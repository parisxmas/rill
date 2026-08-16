#include <stdio.h>
int main(void) {
  static long long v[1024];
  for (int i = 0; i < 1024; i++) v[i] = i;
  long long acc = 0;
  for (long long i = 0; i < 100000000LL; i++) acc += v[i & 1023];
  printf("%lld\n", acc);
  return 0;
}
