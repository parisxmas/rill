/* Symbolic differentiation: the same trees, rules and rounds as deriv.rill,
   every node malloc'd and freed. */
#include <stdio.h>
#include <stdlib.h>

typedef enum { NUM, X, ADD, MUL, POW } Kind;
typedef struct Expr {
    Kind kind;
    long n;
    struct Expr *a, *b;
} Expr;

static Expr *mk(Kind k, long n, Expr *a, Expr *b) {
    Expr *e = malloc(sizeof *e);
    e->kind = k; e->n = n; e->a = a; e->b = b;
    return e;
}
static Expr *num(long n) { return mk(NUM, n, NULL, NULL); }
static Expr *var(void) { return mk(X, 0, NULL, NULL); }
static Expr *add(Expr *a, Expr *b) { return mk(ADD, 0, a, b); }
static Expr *mul(Expr *a, Expr *b) { return mk(MUL, 0, a, b); }
static Expr *pw(Expr *a, long n) { return mk(POW, n, a, NULL); }

static Expr *copy(const Expr *e) { return e ? mk(e->kind, e->n, copy(e->a), copy(e->b)) : NULL; }
static void free_expr(Expr *e) { if (!e) return; free_expr(e->a); free_expr(e->b); free(e); }

static Expr *d(const Expr *e) {
    switch (e->kind) {
    case NUM: return num(0);
    case X:   return num(1);
    case ADD: return add(d(e->a), d(e->b));
    case MUL: return add(mul(d(e->a), copy(e->b)), mul(copy(e->a), d(e->b)));
    case POW: return mul(mul(num(e->n), pw(copy(e->a), e->n - 1)), d(e->a));
    }
    return NULL;
}

static int is_num(const Expr *e, long v) { return e->kind == NUM && e->n == v; }

static Expr *simp(Expr *e) {
    Expr *a, *b;
    switch (e->kind) {
    case ADD:
        a = simp(e->a); b = simp(e->b); free(e);
        if (a->kind == NUM && b->kind == NUM) { long v = a->n + b->n; free(a); free(b); return num(v); }
        if (is_num(a, 0)) { free(a); return b; }
        if (is_num(b, 0)) { free(b); return a; }
        return add(a, b);
    case MUL:
        a = simp(e->a); b = simp(e->b); free(e);
        if (a->kind == NUM && b->kind == NUM) { long v = a->n * b->n; free(a); free(b); return num(v); }
        if (is_num(a, 0) || is_num(b, 0)) { free_expr(a); free_expr(b); return num(0); }
        if (is_num(a, 1)) { free(a); return b; }
        if (is_num(b, 1)) { free(b); return a; }
        if (a->kind == NUM && b->kind == MUL && b->a->kind == NUM) {
            Expr *r = mul(num(a->n * b->a->n), b->b);
            free(a); free(b->a); free(b);
            return r;
        }
        if (b->kind == NUM && a->kind == MUL && a->a->kind == NUM) {
            Expr *r = mul(num(a->a->n * b->n), a->b);
            free(b); free(a->a); free(a);
            return r;
        }
        return mul(a, b);
    case POW:
        a = simp(e->a);
        if (e->n == 0) { free_expr(a); free(e); return num(1); }
        if (e->n == 1) { free(e); return a; }
        e->a = a;
        return e;
    default:
        return e;
    }
}

static long eval(const Expr *e, long x) {
    switch (e->kind) {
    case NUM: return e->n;
    case X:   return x;
    case ADD: return eval(e->a, x) + eval(e->b, x);
    case MUL: return eval(e->a, x) * eval(e->b, x);
    case POW: { long p = 1, base = eval(e->a, x); for (long i = 0; i < e->n; i++) p *= base; return p; }
    }
    return 0;
}

static long size(const Expr *e) {
    switch (e->kind) {
    case ADD: case MUL: return 1 + size(e->a) + size(e->b);
    case POW: return 1 + size(e->a);
    default: return 1;
    }
}

static long chain(Expr *f) {
    long acc = 0;
    Expr *e = f;
    while (!is_num(e, 0)) {
        acc += eval(e, 1) + size(e);
        Expr *next = simp(d(e));
        if (e != f) free_expr(e);
        e = next;
    }
    free_expr(e);
    return acc;
}

int main(void) {
    Expr *acc = num(7);
    for (long i = 12; i >= 1; i--) acc = add(mul(num(i), pw(var(), i)), acc);
    Expr *f = add(acc, mul(pw(add(mul(num(2), var()), num(1)), 4), pw(var(), 3)));
    long total = 0;
    for (int r = 0; r < 3000; r++) total += chain(f);
    printf("%ld\n", total);
    free_expr(f);
    return 0;
}
