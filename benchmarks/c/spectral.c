#include <stdio.h>
#include <stdlib.h>
#include <math.h>
static double A(long i, long j) { return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1); }
static void Au(long n, double *u, double *v) {
  for (long i = 0; i < n; i++) { double s = 0; for (long j = 0; j < n; j++) s += A(i, j) * u[j]; v[i] = s; }
}
static void Atu(long n, double *u, double *v) {
  for (long i = 0; i < n; i++) { double s = 0; for (long j = 0; j < n; j++) s += A(j, i) * u[j]; v[i] = s; }
}
static void AtAu(long n, double *u, double *v, double *t) { Au(n, u, t); Atu(n, t, v); }
int main(void) {
  long n = 2000;
  double *u = malloc(n * 8), *v = malloc(n * 8), *t = malloc(n * 8);
  for (long i = 0; i < n; i++) u[i] = 1;
  for (int k = 0; k < 10; k++) { AtAu(n, u, v, t); AtAu(n, v, u, t); }
  double vBv = 0, vv = 0;
  for (long i = 0; i < n; i++) { vBv += u[i] * v[i]; vv += v[i] * v[i]; }
  printf("%lld\n", (long long)(sqrt(vBv / vv) * 1e9));
  return 0;
}
