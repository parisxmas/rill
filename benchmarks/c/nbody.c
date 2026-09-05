#include <stdio.h>
#include <math.h>
#define PI 3.141592653589793
#define SM (4 * PI * PI)
#define DPY 365.24
typedef struct { double x, y, z, vx, vy, vz, m; } Body;
static Body bodies[5] = {
  {0, 0, 0, 0, 0, 0, SM},
  {4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
   1.66007664274403694e-03 * DPY, 7.69901118419740425e-03 * DPY, -6.90460016972063023e-05 * DPY, 9.54791938424326609e-04 * SM},
  {8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
   -2.76742510726862411e-03 * DPY, 4.99852801234917238e-03 * DPY, 2.30417297573763929e-05 * DPY, 2.85885980666130812e-04 * SM},
  {1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
   2.96460137564761618e-03 * DPY, 2.37847173959480950e-03 * DPY, -2.96589568540237556e-05 * DPY, 4.36624404335156298e-05 * SM},
  {1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
   2.68067772490389322e-03 * DPY, 1.62824170038242295e-03 * DPY, -9.51592254519715870e-05 * DPY, 5.15138902046611451e-05 * SM},
};
/* Every body sums the pull of every other (25 pairs, not 10): the same
   arithmetic in the same order as the two Rill versions, so the answers agree. */
static void advance(double dt) {
  for (int i = 0; i < 5; i++) {
    Body *b = &bodies[i];
    double vx = b->vx, vy = b->vy, vz = b->vz;
    for (int j = 0; j < 5; j++) {
      Body *o = &bodies[j];
      double dx = b->x - o->x, dy = b->y - o->y, dz = b->z - o->z;
      double d2 = dx * dx + dy * dy + dz * dz;
      if (d2 == 0.0) continue;
      double mag = dt / (d2 * sqrt(d2));
      vx -= dx * o->m * mag; vy -= dy * o->m * mag; vz -= dz * o->m * mag;
    }
    b->vx = vx; b->vy = vy; b->vz = vz;
  }
  for (int i = 0; i < 5; i++) {
    Body *b = &bodies[i];
    b->x += dt * b->vx; b->y += dt * b->vy; b->z += dt * b->vz;
  }
}
static double energy(void) {
  double e = 0;
  for (int i = 0; i < 5; i++) {
    Body *b = &bodies[i];
    e += 0.5 * b->m * (b->vx * b->vx + b->vy * b->vy + b->vz * b->vz);
    for (int j = 0; j < 5; j++) {
      Body *o = &bodies[j];
      double dx = b->x - o->x, dy = b->y - o->y, dz = b->z - o->z;
      double d2 = dx * dx + dy * dy + dz * dz;
      if (d2 == 0.0) continue;
      e -= 0.5 * b->m * o->m / sqrt(d2);
    }
  }
  return e;
}
int main(void) {
  double px = 0, py = 0, pz = 0;
  for (int i = 0; i < 5; i++) { px += bodies[i].vx * bodies[i].m; py += bodies[i].vy * bodies[i].m; pz += bodies[i].vz * bodies[i].m; }
  bodies[0].vx = -px / SM; bodies[0].vy = -py / SM; bodies[0].vz = -pz / SM;
  printf("%lld\n", (long long)(energy() * 1e9));
  for (int i = 0; i < 1000000; i++) advance(0.01);
  printf("%lld\n", (long long)(energy() * 1e9));
  return 0;
}
