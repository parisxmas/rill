# Chapter 14. Dynamic Programming

Dynamic programming is the family of algorithms that solve a problem by filling
in a table of smaller answers. It is also the family where the functional style
runs hardest into its own constraints, because "fill in a table" is a sentence
about mutation.

This chapter is therefore the most useful one in Part III for calibrating your
expectations. It has one problem that is *better* functionally, and one that is
not, and the difference between them is worth understanding precisely.

---

## 14.1 The one that is better: Fibonacci

The textbook motivation for memoisation:

```rill
# book/code/ch14/fib.rill
fn slow(n) = if n < 2 then n else slow(n - 1) + slow(n - 2)

fn fast(n) = go(n, 0, 1)

fn go(n, a, b) = if n == 0 then a else go(n - 1, b, a + b)

fn main() =
  println(slow(27))
  println(fast(27))
  println(fast(90))
```

```
$ rill run fib.rill
196418
196418
2880067194370816120
```

`slow` is the exponential version — it recomputes `fib(25)` thousands of times,
and it is the standard illustration of why memoisation exists.

`fast` does not memoise. It does something better: it observes that the
recurrence only ever looks back two steps, and carries those two values forward
as accumulators. It is O(n), constant space, tail recursive, and `fib(90)`
returns instantly.

This is the pattern to look for before reaching for a table. A great many
"dynamic programming" problems are really *linear recurrences with a small
window*, and a window that fits in parameters does not need a table at all.
Functional programming pushes you towards noticing that, because a table is
awkward and parameters are not.

> **Where memoisation goes when you do want it**
>
> A memo table is a `Dict` (Chapter 11) threaded through the recursion: each
> call takes the table and returns the answer *and* the updated table. It works,
> it is O(log n) per lookup, and it is noticeably more code than the imperative
> `@lru_cache`. Languages with mutable-but-controlled state (a `ref` in OCaml,
> `unsafePerformIO` in Haskell, `Buf` here) all cheat at this point, and so does
> the next section.

---

## 14.2 The one that is not: edit distance

Levenshtein distance — the minimum number of insertions, deletions and
substitutions to turn one string into another — is genuinely a table problem.
The recurrence looks two ways at once:

```
d[i][j] = min( d[i-1][j-1] + (a[i] != b[j])   substitute
             , d[i-1][j]   + 1                delete
             , d[i][j-1]   + 1 )              insert
```

Every cell depends on three neighbours. There is no window that fits in a
parameter, and building it out of immutable lists means either O(n) indexing or
rebuilding a row per cell.

So this is where `Buf` earns its place in the language:

```rill
# book/code/ch14/edit.rill
fn cost(a, b) = if a == b then 0 else 1

fn min3(a, b, c) = min(a, min(b, c))

fn first_row(row: Buf(Int), j, n) =
  buf_set(row, j, j)
  if j >= n then 0 else first_row(row, j + 1, n)

fn next_row(a, b, prev: Buf(Int), cur: Buf(Int), i, j, n) =
  # substitution from the diagonal, deletion from above, insertion from the left
  d = min3(buf_get(prev, j - 1) + cost(str_get(a, i - 1), str_get(b, j - 1)), buf_get(prev, j) + 1, buf_get(cur, j - 1) + 1)
  buf_set(cur, j, d)
  if j >= n then 0 else next_row(a, b, prev, cur, i, j + 1, n)

fn rows(a, b, prev: Buf(Int), cur: Buf(Int), i, m, n) =
  buf_set(cur, 0, i)
  next_row(a, b, prev, cur, i, 1, n)
  copy_row(cur, prev, 0, n)
  if i >= m then buf_get(prev, n) else rows(a, b, prev, cur, i + 1, m, n)

fn copy_row(from: Buf(Int), to: Buf(Int), j, n) =
  buf_set(to, j, buf_get(from, j))
  if j >= n then 0 else copy_row(from, to, j + 1, n)

fn distance(a, b) =
  m = str_len(a)
  n = str_len(b)
  prev = buf_int(n + 1)
  cur = buf_int(n + 1)
  first_row(prev, 0, n)
  if m == 0 then n else rows(a, b, prev, cur, 1, m, n)

fn main() =
  println(distance("kitten", "sitting"))
  println(distance("flaw", "lawn"))
  println(distance("", "abc"))
  println(distance("same", "same"))
  println(distance("intention", "execution"))
```

```
$ rill run edit.rill
3
2
3
0
5
```

Those are the right answers — `kitten` → `sitting` is 3, `intention` →
`execution` is 5, both textbook values, and they were checked against an
independent implementation while writing this chapter.

### Reading it

**Only two rows exist at a time.** The full table is m×n, but each row depends
only on the one above it, so `prev` and `cur` are enough. That is the standard
space optimisation, and it is the same in every language.

**The loops are still tail recursion.** `next_row` walks a row, `rows` walks the
rows, `copy_row` copies one to the other — three loops, each a function with
its index as a parameter. The control flow is Chapter 7; only the data is
different.

**`buf_set` mutates.** This is the one place the language lets you, and it is
what makes each cell O(1). The buffer is reference counted like a string, so
there is no `free` to forget, and every index is bounds-checked — which is how
the off-by-one in the first draft of this listing announced itself:

```
rill: buffer index 8 out of range, length 8
```

That message is worth the check it costs. The bug was reading the diagonal from
`prev[j]` instead of `prev[j-1]`; in C it would have read whatever was next in
memory and produced a plausible wrong number.

### What it would look like without `Buf`

You can write this with lists. `nth` gives you indexing, so the recurrence
translates directly — and every `nth` is O(n), turning an O(mn) algorithm into
O(mn·n). For two ten-character words nobody notices. For diffing two files it
is the difference between instant and never.

The alternative that stays immutable is to build each row as a list, left to
right, with the partially built row available as an accumulator. That works, is
O(mn), and is genuinely elegant — `cur[j-1]` is the head of the accumulator, so
"insertion from the left" is free. It allocates a cons cell per cell of the
table, where the buffer version allocates two rows for the whole run.

All three are legitimate. The order to try them in is: the elegant list version
first, and the buffer only when a measurement says the allocation matters.

---

## 14.3 The general shape

Across the chapter, three ways to carry the state of a table:

| Method | Lookup | Allocation | When |
|---|---|---|---|
| **Accumulator parameters** | free | none | the recurrence has a small fixed window |
| **A `Dict` threaded through** | O(log n) | per entry | sparse tables, or keys that are not integers |
| **A `Buf`** | O(1) | one array | dense integer-indexed tables, in a hot loop |

And the decision procedure that goes with it:

1. **Can the recurrence be rewritten with a window?** Fibonacci can; edit
   distance cannot. If it can, you are done and you never build a table.
2. **Is the table sparse, or keyed by something other than a small integer?**
   Then a `Dict` is the right structure and the log factor is the price of
   staying immutable.
3. **Is it dense, integer-keyed and hot?** Then it is an array, and Rill lets
   you say so.

The thing to resist is deciding this by ideology in either direction. A
functional programmer who insists on lists for edit distance has written a
quadratic slowdown into their program for no benefit; an imperative programmer
who reaches for an array before checking whether a window exists has written
twenty lines where three would do.

---

## 14.4 Exercises

1. Write the 0/1 knapsack in a `Buf`, then work out whether it has a window
   that would let you avoid the table. (It does not, and understanding why is
   the point.)
2. Write the list-based edit distance described in §14.2 and check it against
   the buffer version on a few hundred random pairs.
3. Extend `distance` to return the *alignment* rather than just the number.
   This needs the whole table rather than two rows — what does that change?
4. Longest common subsequence is the same table shape with a different
   recurrence. Write it, and then write `diff(a, b)` on top of it.
5. Write `coins(amount, denominations)` — the fewest coins making an amount —
   first with a `Dict` memo, then with a `Buf`. Compare the code and the time.

---

Part III ends here. Chapter 15 goes underneath the language: what happens to
memory when there is no collector.
