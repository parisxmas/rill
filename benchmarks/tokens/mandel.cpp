#include <cstdio>

int main() {
  int w = 2000, h = 2000;
  long long total = 0;
  for (int y = 0; y < h; y++) {
    double ci = (static_cast<double>(y) - h / 2.0) * 4.0 / h;
    for (int x = 0; x < w; x++) {
      double cr = (static_cast<double>(x) - w / 2.0) * 4.0 / w;
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
  std::printf("%lld\n", total);
}
