#include <stdio.h>
#include <stdlib.h>
typedef struct Node { struct Node *l; long v; struct Node *r; } Node;
static Node *build(long d) {
  if (d == 0) return NULL;
  Node *n = malloc(sizeof *n);
  n->l = build(d - 1); n->v = d; n->r = build(d - 1);
  return n;
}
static long check(Node *t) { return t ? 1 + check(t->l) + check(t->r) : 1; }
static void drop(Node *t) { if (t) { drop(t->l); drop(t->r); free(t); } }
int main(void) {
  long acc = 0;
  for (int k = 0; k < 10; k++) { Node *t = build(18); acc += check(t); drop(t); }
  printf("%ld\n", acc);
  return 0;
}
