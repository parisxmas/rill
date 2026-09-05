#include <stdio.h>
int main(void) {
  int n = 10, perm[16], perm1[16], count[16];
  int maxflips = 0, checksum = 0, permcount = 0, r = n;
  for (int i = 0; i < n; i++) perm1[i] = i;
  for (;;) {
    while (r != 1) { count[r - 1] = r; r--; }
    for (int i = 0; i < n; i++) perm[i] = perm1[i];
    int flips = 0, k = perm[0];
    while (k != 0) {
      for (int i = 0, j = k; i < j; i++, j--) { int t = perm[i]; perm[i] = perm[j]; perm[j] = t; }
      flips++;
      k = perm[0];
    }
    if (flips > maxflips) maxflips = flips;
    checksum += permcount % 2 == 0 ? flips : -flips;
    for (;;) {
      if (r == n) { printf("%d\n%d\n", checksum, maxflips); return 0; }
      int p0 = perm1[0];
      for (int i = 0; i < r; i++) perm1[i] = perm1[i + 1];
      perm1[r] = p0;
      count[r]--;
      if (count[r] > 0) break;
      r++;
    }
    permcount++;
  }
}
