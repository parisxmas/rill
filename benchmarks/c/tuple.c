#include <stdio.h>
typedef struct { long a, b; } Pair;
static Pair step(long a, long b) { Pair p = { b, (a + b) % 1000000007 }; return p; }
int main(void) {
  Pair s = { 1, 1 };
  for (long i = 0; i < 100000000; i++) s = step(s.a, s.b);
  printf("%ld\n", s.a);
  return 0;
}
