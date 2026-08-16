#include <stdio.h>

int main(void) {
  int w = 2000, h = 2000;
  long long total = 0;
  for (int y = 0; y < h; y++) {
    double ci = ((double)y - (double)h / 2.0) * 4.0 / (double)h;
    for (int x = 0; x < w; x++) {
      double cr = ((double)x - (double)w / 2.0) * 4.0 / (double)w;
      double zr = 0.0, zi = 0.0;
      int i = 0;
      while (i < 255 && zr * zr + zi * zi <= 4.0) {
        double t = zr * zr - zi * zi + cr;
        zi = 2.0 * zr * zi + ci;
        zr = t;
        i++;
      }
      total += i;
    }
  }
  printf("%lld\n", total);
  return 0;
}
