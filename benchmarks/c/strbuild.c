#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(void) {
  long n = 5000000, cap = 1 << 20, len = 0;
  char *b = malloc(cap);
  for (long i = 0; i < n; i++) {
    if (len + 24 > cap) { cap *= 2; b = realloc(b, cap); }
    char tmp[24]; int k = 0; long v = i;
    do { tmp[k++] = '0' + v % 10; v /= 10; } while (v);
    while (k) b[len++] = tmp[--k];
    b[len++] = '\n';
  }
  long nl = 0;
  for (long i = 0; i < len; i++) nl += b[i] == '\n';
  printf("%ld\n%ld\n", len, nl);
  free(b);
  return 0;
}
