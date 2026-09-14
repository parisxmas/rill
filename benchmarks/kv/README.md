# lib/db/kv.rill against SQLite

`lib/db/kv.rill` is a key–value store on disk — a B+ tree of 4 KB pages, an
8 MB page cache, a write-ahead log with a checksum per frame — written in
Rill on three calls added to the runtime for it: `file_pread_into`,
`file_pwrite` and `file_lock`. This is it against SQLite 3.53.3 through the
C API, on one workload, with the same durability (WAL mode, `synchronous=FULL`
and `fullfsync=ON`, so that a commit is on the platter) and the same 8 MB
cache. Apple M4, 2026-09-13. Reproduce with `benchmarks/kv/run.sh`.

| step | SQLite ms | Rill ms | Rill / SQLite |
|---|---:|---:|---:|
| 200 000 random keys put, one commit | 160 | 100–140 | **0.6–0.9** |
| the 200 000 read back, one at a time | 207 | 64 | **0.31** |
| range scan, 103 553 rows | 3 | 1 | **0.33** |
| 1 000 one-key transactions, each committed | 4092 | 4095 | 1.00 |

The put row moves between runs: the tree's pages are written to the log at
commit and the cost of that write is the operating system's, not the
program's. The last row is the drive: a durable commit on this machine is
one `F_FULLFSYNC`, four milliseconds, and neither program can do it faster.
Without `fullfsync` — which is SQLite's default, and which on a Mac means a
commit that the drive may still lose — SQLite does the thousand in 23 ms;
the store does not offer that setting.

## What is being compared

SQLite is doing more: it parses SQL once per statement, keeps a schema, types
its columns, and holds a lock protocol that lets other processes in. The
store does none of that, and its wins on reads are what not doing it buys —
a `get` is a descent through the cache and a binary search on each page, with
the key compared in place and one string made for the answer.

What is the same is the shape underneath: a page cache in front of a tree,
a log the transaction's pages go to first, a checksum on every frame (the
store uses SQLite's own, two 32-bit sums over the words), a sync of the log
at commit, and a checkpoint that copies the logged pages into the tree
afterwards. A crash between any two writes loses the transaction that was
not committed and never a page; `key_value_store_under_stress_and_recovery`
stages one.

## What the store does not do

Merge nodes that empty on delete, reuse freed pages, or let two processes
in: the exclusive lock at open is what says so. Keys are at most 255 bytes
and values 1500. The source is 648 lines, `lib/db/kv.rill`.

## How it got here

The first measurement of the put row was 2198 ms against 160. The cache was
direct-mapped, 256 slots, a page's slot its number modulo 256 — so a leaf
arriving evicted the root or an inner node as readily as anything else, and
every eviction of a dirty page was a frame written to the log through a
byte-at-a-time CRC and two `pwrite`s. Three changes: a clock cache found
through a map, which never evicts what every operation touches (2198 → 638);
one write per frame, through the frame buffer (638 → 400); and SQLite's word
checksum in place of the CRC (→ 338). Then the cache at 8 MB, which is what
SQLite was given (→ 100–140).
