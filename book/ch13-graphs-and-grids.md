# Chapter 13. Graphs and Grids

Trees are the friendly case: the type enforces the shape, and every node has
one parent. A graph has neither property. It has cycles, so a naive recursion
runs forever; it has no root, so there is no obvious place to start; and it has
no shape a sum type can capture.

This is where the functional style stops being obviously better and starts
requiring judgement. This chapter shows both answers — the list-based one that
is short and slow, and the array-based one that is longer and fast — and is
explicit about when to reach for which.

---

## 13.1 A graph as a list of edges

The simplest representation needs no new machinery at all:

```rill
# book/code/ch13/graph.rill
type Edge
  Edge(from: Int, to: Int)

fn edge_from(e) = match e
  Edge(a, _) -> a

fn edge_to(e) = match e
  Edge(_, b) -> b

fn neighbours(edges, n) = map(filter(edges, \e -> edge_from(e) == n), edge_to)
```

`neighbours` is a `filter` followed by a `map`: keep the edges leaving `n`,
then take where each one goes. Two combinators, one line, and no data structure
beyond a list.

### Breadth-first search

```rill
fn seen(l, n) = any(l, \v -> v == n)

fn bfs(edges, frontier, visited) = match frontier
  Nil -> reverse(visited)
  Cons(n, rest) -> bfs_on(edges, n, rest, visited)

fn bfs_on(edges, n, rest, visited) =
  fresh = filter(neighbours(edges, n), \m -> !seen(visited, m) && !seen(rest, m))
  bfs(edges, append(rest, fresh), append(reverse(fresh), visited))

fn reachable(edges, start) = bfs(edges, Cons(start, Nil), Cons(start, Nil))

fn main() =
  g = Cons(Edge(1, 2), Cons(Edge(1, 3), Cons(Edge(2, 4), Cons(Edge(3, 4), Cons(Edge(4, 5), Cons(Edge(6, 7), Nil))))))
  println(neighbours(g, 1))
  println(reachable(g, 1))
  println(reachable(g, 6))
  println(reachable(g, 5))
```

```
$ rill run graph.rill
[2 3]
[1 2 3 4 5]
[6 7]
[5]
```

The classic BFS has a mutable queue and a mutable visited-set. Here both are
parameters:

- **`frontier`** is the queue. Taking from the front is `Cons(n, rest)`;
  adding to the back is `append(rest, fresh)`.
- **`visited`** is the set. Membership is `any`, and marking is `Cons`.

The termination argument is the same as the imperative one and easier to see: a
node enters `frontier` only if it is in neither `visited` nor `frontier`, so
each node is enqueued at most once and the frontier must eventually empty. The
cycle `1→2→4` and `1→3→4` does not loop, because 4 is already in `visited` the
second time it is offered.

`reachable(g, 6)` gives `[6 7]`, correctly finding the disconnected component,
and `reachable(g, 5)` gives `[5]` because these edges are directed.

### Why this is slow

Every operation in the listing is linear:

| Operation | This version | What it should be |
|---|---|---|
| membership test (`seen`) | O(V) | O(1) with a hash set |
| enqueue (`append`) | O(V) | O(1) with a real queue |
| `neighbours` | O(E) — scans every edge | O(deg) with an adjacency list |

Multiply those together and BFS over the whole graph is roughly O(V²·E) rather
than O(V+E). For a graph with twenty nodes nobody will notice. For a road
network it is unusable.

Two of the three are fixable within the functional style: an adjacency
`Dict` from Chapter 11 makes `neighbours` a lookup, and the same `Dict` makes
`seen` O(log V). The queue is fixable with the standard two-list trick — a
front list and a reversed back list, giving amortised O(1) at both ends, which
is one of the classic results in Okasaki's book.

The third fix, and the honest one for a grid, is to stop using lists.

---

## 13.2 The same problem on a grid, with buffers

A maze is a graph whose structure is implicit: cells are numbered by position,
and the neighbours of `(x, y)` are the four cells around it. Nothing needs to
be stored, and the natural representation is an array.

Rill's `Buf` (Chapter 15 covers it properly) is a counted block of elements of
one C width — a real array, bounds-checked, reference counted, and **mutable**.
It is the deliberate hole in the language's immutability, and this is exactly
the kind of code it exists for.

```rill
# book/code/ch13/flood.rill
fn clear(d: Buf(Int), i) =
  buf_set(d, i, 0 - 1)
  if i + 1 >= buf_len(d) then 0 else clear(d, i + 1)

fn flood(g: Buf(U8), d: Buf(Int), q: Buf(Int), from) =
  clear(d, 0)
  buf_set(d, from, 0)
  buf_set(q, 0, from)
  walk(g, d, q, 0, 1)

fn walk(g: Buf(U8), d: Buf(Int), q: Buf(Int), head, tail) =
  cell = buf_get(q, head)
  after = side(g, d, q, cell % gw(), cell / gw(), buf_get(d, cell), 0, tail)
  if head + 1 >= after then 0 else walk(g, d, q, head + 1, after)

fn side(g: Buf(U8), d: Buf(Int), q: Buf(Int), x, y, far, k, tail) =
  nx = x + dx_of(k)
  ny = y + dy_of(k)
  next = if fresh(g, d, nx, ny) then push(d, q, ny * gw() + nx, far + 1, tail) else tail
  if k >= 3 then next else side(g, d, q, x, y, far, k + 1, next)

fn fresh(g: Buf(U8), d: Buf(Int), x, y) =
  open_at(g, x, y) && buf_get(d, y * gw() + x) < 0

fn push(d: Buf(Int), q: Buf(Int), cell, far, tail) =
  buf_set(d, cell, far)
  buf_set(q, tail, cell)
  tail + 1
```

```
$ rill run flood.rill
 ## ## ## ## ## ## ## ## ## ## ## ## ## ## ##
 ##  0  1  2  3  4 ## 18 19 20 21 22 23 24 ##
 ##  1 ## ## ##  5 ## 17 ## ## ## ## ## 25 ##
 ##  2 ##  8  7  6 ## 16 ## 18 19 20 ## 26 ##
 ##  3 ##  9 ## ## ## 15 ## 17 ## 21 ## 25 ##
 ##  4 ## 10 11 12 13 14 ## 16 ## 22 23 24 ##
 ##  5 ## ## ## ## ## ## ## 15 ## ## ## 25 ##
 ##  6  7  8  9 10 11 12 13 14 ## ## ## 26 ##
 ## ## ## ## ## ## ## ## ## ## ## ## ## ## ##
```

Each number is that cell's distance from the corner. This is a **flood fill**,
and it is BFS with three arrays instead of three lists:

- **`d`** holds the distance to every cell, or -1 for "not reached". It is the
  visited-set and the answer at once.
- **`q`** is the queue: a flat array of cell indices with `head` and `tail` as
  cursors. Every cell is pushed at most once, so `q` never needs more than one
  slot per cell and no reallocation is possible.
- **`g`** is the maze, one byte per cell.

The loops are still tail recursion — `walk` over the queue, `side` over the
four directions — so the control flow is the same shape as everything else in
this book. What changed is the data.

**Complexity: O(V + E), which for a grid is O(cells).** Each cell is visited
once and its four neighbours are examined once. There is no membership scan,
because `d[cell] < 0` *is* the membership test, in one indexed read.

### Reading the picture

The output repays a look. Follow the numbers from `0` at the top left: they
run down the left wall, turn right, wind through the middle, and come back up
the right-hand side to 26. The distances are corridor distances, not straight
lines — cell `(13,1)` shows 24 despite being twelve cells away as the crow
flies, because the only route to it goes all the way around.

That is what makes a flood fill useful for something other than distances: read
the numbers around you and step to the smallest, and you are following the
shortest path home, round every corner and out of every dead end, without ever
storing a path. The Art Vandal maze — a separate project written in Rill —
does exactly this to make a creature chase the player through a maze, and its
`floodmap.rill` runs the same flood with no window attached so its output can
be checked — which is how the numbers above were verified against an
independent breadth-first search.

---

## 13.3 Choosing between them

| | Lists (§13.1) | Buffers (§13.2) |
|---|---|---|
| Lines of code | ~15 | ~40 |
| Complexity | O(V²·E) as written | O(V + E) |
| Memory | allocates per step | three fixed arrays |
| Immutable | yes | no — `buf_set` mutates |
| Reads like the definition | yes | not really |
| Works for arbitrary graphs | yes | grids and dense integer-keyed graphs |

The rule that falls out of this chapter, and it generalises past graphs:

> **Use values until the shape of the problem is an array. Then use an array.**

The list version is the one to write first, because it is short enough to be
obviously correct and it is where you discover whether the algorithm is right
at all. The buffer version is what you write when the profile says the graph is
big and this is the hot loop — and by then you have a correct version to check
the fast one against.

That last point is not rhetorical. When the flood fill in the maze demo was
first written it silently produced nothing, and the way it was fixed was to run
the same flood outside the game and compare its distances against a reference
BFS, cell by cell. Having two implementations of the same thing is not
duplication; it is the only cheap way to know that the fast one is right.

---

## 13.4 What is missing, and what to reach for

Rill 0.2 has no priority queue, so **Dijkstra's algorithm** and **A\*** are not
one-liners. On a grid with unit costs you do not need them — BFS gives the
shortest path, which is why the flood fill above is enough for a maze. With
weights you need a heap, and a functional leftist heap is about thirty lines
and is the other famous chapter of Okasaki.

Two more standard graph algorithms are worth knowing fit comfortably here:

**Depth-first search** is the same code with the frontier used as a stack
instead of a queue — `append(fresh, rest)` instead of `append(rest, fresh)`.
One expression, and you have DFS.

**Topological sort** is DFS with the node appended to the answer *after* its
children, which is a two-line change and is the natural way to schedule
anything with dependencies.

**Connected components** is `reachable` in a loop over the nodes not yet seen,
which the exercises ask for.

---

## 13.5 Exercises

1. Turn `bfs` into DFS by changing one expression, and check that the
   traversal order changes but the reachable set does not.
2. Make the graph undirected by adding both directions in `neighbours`, then
   compute the connected components of a graph with three of them.
3. Replace `seen` and `neighbours` with the `Dict` from Chapter 11 and measure
   the difference on a graph of a thousand nodes.
4. In `flood.rill`, write `path_from(d, x, y)` that returns the list of cells
   from `(x, y)` back to the start by always stepping to a neighbour whose
   distance is one less.
5. Add a second flood from a different start, and produce the set of cells that
   are strictly closer to the first one — a Voronoi partition of the maze.
6. Prove the queue in §13.2 can never overflow. What is the exact bound, and
   which line of `push` guarantees it?

---

That is the last algorithm chapter. What follows is the machinery underneath —
how memory is managed with no collector, and how the concurrency works.
