#include <cstdio>
#include <vector>

int main() {
  std::vector<long long> v(1024);
  for (int i = 0; i < 1024; i++) v[i] = i;
  long long acc = 0;
  for (long long i = 0; i < 100000000LL; i++) acc += v[i & 1023];
  std::printf("%lld\n", acc);
}
