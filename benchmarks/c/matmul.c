#include <stdio.h>
#include <stdlib.h>
int main(void) {
  long n = 500;
  double *a = malloc(n * n * sizeof(double)), *b = malloc(n * n * sizeof(double)),
         *c = calloc(n * n, sizeof(double));
  for (long i = 0; i < n; i++)
    for (long j = 0; j < n; j++) { a[i*n+j] = (double)((i * j) % 10); b[i*n+j] = (double)((i + j) % 10); }
  for (long i = 0; i < n; i++)
    for (long k = 0; k < n; k++) {
      double aik = a[i*n+k];
      for (long j = 0; j < n; j++) c[i*n+j] += aik * b[k*n+j];
    }
  double s = 0;
  for (long i = 0; i < n * n; i++) s += c[i];
  printf("%lld\n", (long long)s);
  free(a); free(b); free(c);
  return 0;
}
