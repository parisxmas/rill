#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct J J;
typedef struct { char *k; J *v; } Field;

struct J {
  int tag; /* 0 null, 1 bool, 2 int, 3 str, 4 arr, 5 obj */
  union {
    int b;
    long long n;
    char *s;
    struct { J **v; int n; } a;
    struct { Field *v; int n; } o;
  } u;
};

static const char *src;
static int len, pos;

static void die(const char *msg, int i) {
  printf("error at %d: %s\n", i + 1, msg);
  exit(1);
}

static J *mk(int tag) {
  J *j = calloc(1, sizeof *j);
  j->tag = tag;
  return j;
}

typedef struct { char *p; int n, cap; } Buf;

static void bput(Buf *b, char c) {
  if (b->n + 1 >= b->cap) {
    b->cap = b->cap ? b->cap * 2 : 16;
    b->p = realloc(b->p, b->cap);
  }
  b->p[b->n++] = c;
  b->p[b->n] = 0;
}

static void bputs(Buf *b, const char *s) {
  while (*s) bput(b, *s++);
}

static char *bdone(Buf *b) { return b->p ? b->p : calloc(1, 1); }

static void ws(void) {
  while (pos < len && (src[pos] == ' ' || src[pos] == '\t' || src[pos] == '\n' || src[pos] == '\r'))
    pos++;
}

static J *parse(void);

static J *plit(const char *lit, J *v) {
  int n = strlen(lit);
  if (pos + n <= len && memcmp(src + pos, lit, n) == 0) {
    pos += n;
    return v;
  }
  die("unexpected character", pos);
  return NULL;
}

static J *pint(void) {
  int i = pos, a = src[pos] == '-' ? pos + 1 : pos, j = a;
  while (j < len && src[j] >= '0' && src[j] <= '9') j++;
  if (j == a) die("unexpected character", i);
  J *r = mk(2);
  r->u.n = strtoll(src + i, NULL, 10);
  pos = j;
  return r;
}

static char *pstr(void) {
  Buf b = {0};
  for (;;) {
    if (pos >= len) die("unterminated string", pos);
    char c = src[pos];
    if (c == '"') {
      pos++;
      return bdone(&b);
    }
    if (c == '\\') {
      pos++;
      if (pos >= len) die("unterminated string", pos);
      switch (src[pos]) {
      case '"': case '\\': case '/': bput(&b, src[pos]); break;
      case 'n': bput(&b, '\n'); break;
      case 't': bput(&b, '\t'); break;
      case 'r': bput(&b, '\r'); break;
      case 'b': bput(&b, '\b'); break;
      case 'f': bput(&b, '\f'); break;
      default: die("bad escape", pos);
      }
      pos++;
      continue;
    }
    bput(&b, c);
    pos++;
  }
}

static J *parr(void) {
  J *r = mk(4);
  if (pos < len && src[pos] == ']') {
    pos++;
    return r;
  }
  for (;;) {
    J *v = parse();
    r->u.a.v = realloc(r->u.a.v, (r->u.a.n + 1) * sizeof *r->u.a.v);
    r->u.a.v[r->u.a.n++] = v;
    ws();
    if (pos >= len) die("expected ',' or ']'", pos);
    if (src[pos] == ',') {
      pos++;
      ws();
      continue;
    }
    if (src[pos] == ']') {
      pos++;
      return r;
    }
    die("expected ',' or ']'", pos);
  }
}

static J *pobj(void) {
  J *r = mk(5);
  if (pos < len && src[pos] == '}') {
    pos++;
    return r;
  }
  for (;;) {
    if (pos >= len || src[pos] != '"') die("unexpected character", pos);
    pos++;
    char *k = pstr();
    ws();
    if (pos >= len || src[pos] != ':') die("expected ':'", pos);
    pos++;
    J *v = parse();
    r->u.o.v = realloc(r->u.o.v, (r->u.o.n + 1) * sizeof *r->u.o.v);
    r->u.o.v[r->u.o.n].k = k;
    r->u.o.v[r->u.o.n++].v = v;
    ws();
    if (pos >= len) die("expected ',' or '}'", pos);
    if (src[pos] == ',') {
      pos++;
      ws();
      continue;
    }
    if (src[pos] == '}') {
      pos++;
      return r;
    }
    die("expected ',' or '}'", pos);
  }
}

static J *parse(void) {
  ws();
  if (pos >= len) die("unexpected end of input", pos);
  char c = src[pos];
  if (c == 'n') return plit("null", mk(0));
  if (c == 't') {
    J *j = mk(1);
    j->u.b = 1;
    return plit("true", j);
  }
  if (c == 'f') return plit("false", mk(1));
  if (c == '"') {
    pos++;
    J *j = mk(3);
    j->u.s = pstr();
    return j;
  }
  if (c == '[') {
    pos++;
    ws();
    return parr();
  }
  if (c == '{') {
    pos++;
    ws();
    return pobj();
  }
  if (c == '-' || (c >= '0' && c <= '9')) return pint();
  die("unexpected character", pos);
  return NULL;
}

static void quote(Buf *b, const char *s) {
  bput(b, '"');
  for (; *s; s++) {
    switch (*s) {
    case '"': bputs(b, "\\\""); break;
    case '\\': bputs(b, "\\\\"); break;
    case '\n': bputs(b, "\\n"); break;
    case '\t': bputs(b, "\\t"); break;
    case '\r': bputs(b, "\\r"); break;
    case '\b': bputs(b, "\\b"); break;
    case '\f': bputs(b, "\\f"); break;
    default: bput(b, *s);
    }
  }
  bput(b, '"');
}

static void show(Buf *b, J *j) {
  char tmp[32];
  switch (j->tag) {
  case 0: bputs(b, "null"); break;
  case 1: bputs(b, j->u.b ? "true" : "false"); break;
  case 2:
    snprintf(tmp, sizeof tmp, "%lld", j->u.n);
    bputs(b, tmp);
    break;
  case 3: quote(b, j->u.s); break;
  case 4:
    bput(b, '[');
    for (int i = 0; i < j->u.a.n; i++) {
      if (i) bput(b, ',');
      show(b, j->u.a.v[i]);
    }
    bput(b, ']');
    break;
  default:
    bput(b, '{');
    for (int i = 0; i < j->u.o.n; i++) {
      if (i) bput(b, ',');
      quote(b, j->u.o.v[i].k);
      bput(b, ':');
      show(b, j->u.o.v[i].v);
    }
    bput(b, '}');
  }
}

static void drop(J *j) {
  switch (j->tag) {
  case 3: free(j->u.s); break;
  case 4:
    for (int i = 0; i < j->u.a.n; i++) drop(j->u.a.v[i]);
    free(j->u.a.v);
    break;
  case 5:
    for (int i = 0; i < j->u.o.n; i++) { free(j->u.o.v[i].k); drop(j->u.o.v[i].v); }
    free(j->u.o.v);
    break;
  }
  free(j);
}

int main(void) {
  static char line[1 << 26];
  if (!fgets(line, sizeof line, stdin)) line[0] = 0;
  len = strlen(line);
  if (len && line[len - 1] == '\n') line[--len] = 0;
  src = line;
  J *v = parse();
  ws();
  if (pos < len) die("trailing content", pos);
  Buf b = {0};
  show(&b, v);
  printf("%s\n", bdone(&b));
  drop(v);
  free(b.p);
  return 0;
}
