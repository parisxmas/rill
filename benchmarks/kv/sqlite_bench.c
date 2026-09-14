/* The same work as kv_bench.rill on SQLite through its C API — prepared
   statements, no interpreter in the way. WAL mode, synchronous=FULL and
   fullfsync=ON, which is the durability lib/db/kv.rill gives (a commit is on
   the platter), and an 8 MB page cache, which is lib/db/kv.rill's. */
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e3 + t.tv_nsec / 1e6; }
static long nxt(long x) { return (x * 1103515245 + 12345) & 2147483647; }
static void run(sqlite3 *db, const char *sql) { char *err = 0; if (sqlite3_exec(db, sql, 0, 0, &err) != SQLITE_OK) { fprintf(stderr, "%s: %s\n", sql, err); exit(1); } }

int main(int argc, char **argv) {
    long n = atol(argv[1]);
    const char *path = "/tmp/sqlite_bench.db";
    remove(path); remove("/tmp/sqlite_bench.db-wal"); remove("/tmp/sqlite_bench.db-shm");
    sqlite3 *db;
    sqlite3_open(path, &db);
    run(db, "pragma journal_mode=wal");
    run(db, "pragma synchronous=full");
    run(db, "pragma fullfsync=on");
    run(db, "pragma cache_size=-8192");
    run(db, "create table kv(k text primary key, v text) without rowid");
    sqlite3_stmt *put, *get, *scan;
    sqlite3_prepare_v2(db, "insert or replace into kv values(?, ?)", -1, &put, 0);
    sqlite3_prepare_v2(db, "select v from kv where k = ?", -1, &get, 0);
    sqlite3_prepare_v2(db, "select k from kv where k >= 'key:1' and k <= 'key:2' order by k", -1, &scan, 0);
    char k[64], v[64];

    double t0 = now();
    run(db, "begin");
    long x = 42;
    for (long i = 0; i < n; i++) {
        x = nxt(x);
        snprintf(k, sizeof k, "key:%ld", x); snprintf(v, sizeof v, "value-%ld", x);
        sqlite3_bind_text(put, 1, k, -1, SQLITE_TRANSIENT); sqlite3_bind_text(put, 2, v, -1, SQLITE_TRANSIENT);
        sqlite3_step(put); sqlite3_reset(put);
    }
    run(db, "commit");
    printf("put %ld + commit: %.0f ms\n", n, now() - t0);

    double t1 = now(); long hits = 0; x = 42;
    for (long i = 0; i < n; i++) {
        x = nxt(x);
        snprintf(k, sizeof k, "key:%ld", x);
        sqlite3_bind_text(get, 1, k, -1, SQLITE_TRANSIENT);
        if (sqlite3_step(get) == SQLITE_ROW) hits++;
        sqlite3_reset(get);
    }
    printf("get %ld (%ld hits): %.0f ms\n", n, hits, now() - t1);

    double t2 = now(); long c = 0;
    while (sqlite3_step(scan) == SQLITE_ROW) c++;
    sqlite3_reset(scan);
    printf("range scan %ld rows: %.0f ms\n", c, now() - t2);

    double t3 = now();
    for (int i = 0; i < 1000; i++) {
        run(db, "begin");
        snprintf(k, sizeof k, "tx%d", i);
        sqlite3_bind_text(put, 1, k, -1, SQLITE_TRANSIENT); sqlite3_bind_text(put, 2, "1", -1, SQLITE_STATIC);
        sqlite3_step(put); sqlite3_reset(put);
        run(db, "commit");
    }
    printf("1000 single-put commits: %.0f ms\n", now() - t3);
    sqlite3_close(db);
    return 0;
}
