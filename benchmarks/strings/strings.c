#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>
static const char *W[7] = {"she ", "sells ", "sea\n", "shells ", "aaa", "\n", "ashes "};
static double now(void){ struct timespec ts; clock_gettime(CLOCK_MONOTONIC,&ts); return ts.tv_sec*1e3+ts.tv_nsec/1e6; }
static char *text(long n, long *len){
    size_t cap = 64u<<20, l = 0; char *b = malloc(cap); long x = 12345;
    for (long i = 0; i < n; i++){
        long y = (x*1103515245 + 12345) & 2147483647; x = y;
        const char *w = W[y/65536 % 7]; size_t m = strlen(w);
        if (l + m > cap){ cap *= 2; b = realloc(b, cap); }
        memcpy(b + l, w, m); l += m;
    }
    *len = l; return b;
}
static long count(const char *s, long n, const char *p){
    long m = strlen(p), c = 0; const char *e = s + n, *q = s;
    while ((q = memmem(q, e - q, p, m))) { c++; q += m; }
    return c;
}
static long count_byte(const char *s, long n, char c){ long k = 0; for (long i = 0; i < n; i++) k += s[i] == c; return k; }
static char *upper(const char *s, long n){ char *o = malloc(n); for (long i = 0; i < n; i++){ unsigned char c = s[i]; o[i] = (c >= 'a' && c <= 'z') ? c - 32 : c; } return o; }
static char *replace(const char *s, long n, const char *old, const char *nw, long *outlen){
    long m = strlen(old), k = strlen(nw); size_t cap = n + 16, l = 0; char *o = malloc(cap);
    const char *e = s + n, *from = s, *q;
    while ((q = memmem(from, e - from, old, m))){
        size_t need = l + (q - from) + k; if (need > cap){ cap = need * 2; o = realloc(o, cap); }
        memcpy(o + l, from, q - from); l += q - from; memcpy(o + l, nw, k); l += k; from = q + m;
    }
    if (l + (e - from) > cap){ o = realloc(o, l + (e - from)); }
    memcpy(o + l, from, e - from); l += e - from; *outlen = l; return o;
}
static int64_t hash(const char *s, long n){ uint64_t h = 14695981039346656037ull; for (long i = 0; i < n; i++){ h ^= (unsigned char)s[i]; h *= 1099511628211ull; } return (int64_t)h; }
static long last_index_of(const char *s, long n, const char *p){
    long m = strlen(p), best = -1; const char *e = s + n, *q = s;
    while ((q = memmem(q, e - q, p, m))) { best = q - s; q += 1; }
    return best;
}
static long edit_distance(const char *a, long n, const char *b, long m){
    long *prev = malloc((m+1)*sizeof(long)), *cur = malloc((m+1)*sizeof(long));
    for (long j = 0; j <= m; j++) prev[j] = j;
    for (long i = 0; i < n; i++){
        cur[0] = i + 1;
        for (long j = 0; j < m; j++){
            long cost = a[i] == b[j] ? 0 : 1;
            long x = prev[j+1] < cur[j] ? prev[j+1] : cur[j];
            x += 1; long y = prev[j] + cost; cur[j+1] = x < y ? x : y;
        }
        long *t = prev; prev = cur; cur = t;
    }
    return prev[m];
}
static int is_space(unsigned char c){ return c == 32 || c == 9 || c == 10 || c == 13; }
typedef struct { char **v; long n; } words_t;
static long words(const char *s, long n){
    /* the same shape as the Rill: every word copied out into its own string, kept in a list */
    long cap = 1<<20, k = 0; char **v = malloc(cap * sizeof(char*)); long i = 0;
    while (i < n){
        while (i < n && is_space(s[i])) i++;
        if (i >= n) break;
        long a = i; while (i < n && !is_space(s[i])) i++;
        if (k == cap){ cap *= 2; v = realloc(v, cap * sizeof(char*)); }
        char *w = malloc(i - a + 1); memcpy(w, s + a, i - a); w[i-a] = 0; v[k++] = w;
    }
    for (long j = 0; j < k; j++) free(v[j]); free(v);
    return k;
}
int main(void){
    long n; char *t = text(10000000, &n); printf("bytes %ld\n", n);
    double t0 = now(); long c1 = count(t, n, "shells"); printf("count %ld %.0f ms\n", c1, now()-t0);
    t0 = now(); long c2 = count_byte(t, n, '\n'); printf("count_byte %ld %.0f ms\n", c2, now()-t0);
    t0 = now(); char *u = upper(t, n); printf("upper %d %.0f ms\n", (unsigned char)u[5], now()-t0);
    t0 = now(); long rl; char *r = replace(t, n, "sells", "buys", &rl); printf("replace %ld %.0f ms\n", rl, now()-t0);
    t0 = now(); int64_t h = hash(t, n); printf("hash %lld %.0f ms\n", (long long)h, now()-t0);
    t0 = now(); long li = last_index_of(t, n, "ashes aaa"); printf("last_index_of %ld %.0f ms\n", li, now()-t0);
    t0 = now(); long d = edit_distance(t, 4000, t + 1000, 4000); printf("edit_distance %ld %.0f ms\n", d, now()-t0);
    t0 = now(); long w = words(t, 8000000); printf("words %ld %.0f ms\n", w, now()-t0);
    free(u); free(r); free(t); return 0;
}
