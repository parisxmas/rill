# Chapter 10. Sorting

Sorting is the standard place to compare programming styles, because everyone
knows the algorithms and the differences are stark. It is also the standard
place for functional programming to be oversold — the famous two-line
quicksort is *not* quicksort — so this chapter is careful about what is being
claimed.

Rill has no sort in its prelude. By the end of this chapter you will have
written three.

---

## 10.1 Insertion sort

Start with the one that is genuinely nicer functionally:

```rill
# book/code/ch10/sorts.rill
fn insert(x, l) = match l
  Nil -> Cons(x, Nil)
  Cons(y, rest) -> if x <= y then Cons(x, Cons(y, rest)) else Cons(y, insert(x, rest))

fn insertion_sort(l) = fold(l, Nil, \acc, x -> insert(x, acc))
```

```
$ rill run sorts.rill
Cons(1, Cons(2, Cons(3, Cons(5, Cons(7, Cons(8, Cons(9, Nil)))))))
```

`insert` puts one element into an already-sorted list: if it belongs before the
head, it goes at the front; otherwise it goes somewhere in the tail. Two lines,
and both are the definition read aloud.

`insertion_sort` is then a **fold** — start with the empty list and insert each
element. The entire sort is one line, because the recursive work lives in
`insert` where it belongs.

Beside the imperative version:

```python
def insertion_sort(a):
    for i in range(1, len(a)):
        key = a[i]
        j = i - 1
        while j >= 0 and a[j] > key:
            a[j + 1] = a[j]
            j -= 1
        a[j + 1] = key
```

Every index in that function is an opportunity to be wrong by one, and the
shifting loop has to run backwards for a reason that takes a paragraph to
explain. The functional version has no indices at all. This is the clearest
win in the chapter, and it is not a coincidence: insertion sort is naturally
expressed as "build a sorted result", and the array version is only shifting
things because it insists on using no extra memory.

Both are O(n²). Neither should sort a large list.

---

## 10.2 The quicksort that is not quicksort

This is the program that appears in every functional-language tutorial:

```rill
fn quicksort(l) = match l
  Nil -> Nil
  Cons(p, rest) -> append(quicksort(filter(rest, \x -> x < p)), Cons(p, quicksort(filter(rest, \x -> x >= p))))
```

```
Cons(1, Cons(2, Cons(3, Cons(5, Cons(7, Cons(8, Cons(9, Nil)))))))
```

It is beautiful, it is correct, and it is important to be honest about it:
**this is not quicksort.**

Real quicksort is defined by in-place partitioning. Its entire reason for
existing is that it sorts with O(log n) extra memory and excellent cache
behaviour, which is why it beats merge sort in practice despite the same
average complexity. The program above:

- allocates a new list at every level, so it uses O(n log n) extra memory, not
  O(log n);
- walks `rest` **twice** at every level, once per `filter`;
- calls `append`, which walks the left result again;
- and has no cache locality at all, because a linked list is scattered.

What it is, precisely, is *a description of the divide-and-conquer idea behind
quicksort*, and it is excellent for that. Just do not put it in a benchmark and
claim a language win. The definitive discussion is a paper by Ben Lynn titled
"Quicksort is not quicksort", and everyone who has ever written this listing in
a tutorial should read it.

It also inherits real quicksort's worst case: an already-sorted input picks the
smallest element as pivot every time and degrades to O(n²). The duplicate test
in the listing (`quicksort` of `2 2 1 2`) checks the other classic bug, which is
that `<` and `>=` must partition *all* the remaining elements between them.

---

## 10.3 Merge sort, which is the one to use

Merge sort is the algorithm that suits immutable linked lists, for a reason
worth stating: it never needs to look at an element more than once per level,
and merging two sorted lists is a natural list operation.

```rill
fn merge(a, b) = match a
  Nil -> b
  Cons(x, xs) -> match b
    Nil -> a
    Cons(y, ys) -> if x <= y then Cons(x, merge(xs, b)) else Cons(y, merge(a, ys))

fn halve(l, n) = Cons(take(l, n / 2), Cons(drop(l, n / 2), Nil))

fn merge_sort(l) =
  n = len(l)
  if n <= 1 then l else merge_on(halve(l, n))

fn merge_on(parts) = match parts
  Cons(a, Cons(b, _)) -> merge(merge_sort(a), merge_sort(b))
  _ -> Nil
```

```
Cons(1, Cons(2, Cons(3, Cons(5, Cons(7, Cons(8, Cons(9, Nil)))))))
Nil
Cons(1, Nil)
```

`merge` is the heart of it, and it is the same nested-match shape as everything
else in this book: two lists, four cases, take whichever head is smaller.

`halve` returns two lists in a list, because Rill 0.2 has no tuples. That is an
ergonomic wart and worth naming as one — in OCaml this would be
`(take l (n/2), drop l (n/2))` and the caller would destructure it with
`let (a, b) = ...`. Here it costs a helper function, `merge_on`, whose only job
is to take the two-element list apart. The alternative is declaring a
`type Pair(a, b)  Pair(a, b)`, which is what a real program should do.

**Complexity.** O(n log n) comparisons, and unlike the quicksort listing it is
guaranteed rather than average — merge sort has no bad input. It allocates
O(n log n) cells in total, but only O(n) are live at once because the earlier
levels die as soon as they are merged, and Chapter 15 explains why "die" means
"freed immediately" here.

**Stability.** `if x <= y` takes from the left list on a tie, which makes this
sort stable. Change it to `<` and it stops being stable. That one character is
the entire difference, and it is worth knowing which one you wrote.

Beside the imperative merge sort — which needs an auxiliary array, index
arithmetic for three pointers, and a copy-back step — the functional version is
the clearer program by some distance. This is the fair comparison to make, and
it is the one that tutorials should be making instead of the quicksort one.

---

## 10.4 Sorting by something other than `<`

Real sorting is rarely on the values themselves. Here is a comparator-driven
version, with a `Show` impl so the output is readable:

```rill
# book/code/ch10/sortby.rill
type Person
  Person(name: Str, age: Int)

impl Show(Person)
  fn show(p) = match p
    Person(n, a) -> n + "(" + int_to_str(a) + ")"

impl Show(List(a))
  fn show(l) = "[" + join(map(l, \x -> show(x)), " ") + "]"

fn sort_by(l, before) = match l
  Nil -> Nil
  Cons(p, rest) -> append(sort_by(filter(rest, \x -> before(x, p)), before), Cons(p, sort_by(filter(rest, \x -> !before(x, p)), before)))

fn age_of(p) = match p
  Person(_, a) -> a

fn name_of(p) = match p
  Person(n, _) -> n

fn main() =
  people = Cons(Person("Ada", 36), Cons(Person("Bob", 24), Cons(Person("Cyd", 31), Nil)))
  println(sort_by(people, \a, b -> age_of(a) < age_of(b)))
  println(sort_by(people, \a, b -> name_of(a) > name_of(b)))
```

```
$ rill run sortby.rill
[Bob(24) Cyd(31) Ada(36)]
[Cyd(31) Bob(24) Ada(36)]
```

`sort_by` takes the ordering as a **function**, and the two calls in `main`
pass two different lambdas — one ascending by age, one descending by name.
This is Chapter 4's higher-order functions doing the job they exist for. No
interface to implement, no comparator class, no `Comparable`; a two-argument
function returning a `Bool`.

The two `Show` impls are Chapter 8 earning its place: six lines, and the output
went from `Cons(Person(Ada, 36), ...)` to `[Bob(24) Cyd(31) Ada(36)]`.

> **Or use `Ord` instead**
>
> If a type has one natural ordering, implement `Ord` for it (Chapter 8) and
> every sort written against `<` accepts it directly. Use a comparator argument
> when the ordering is a property of the *call*, not of the type — which is why
> the same list is sorted two ways above.

---

## 10.5 The comparison, honestly

| | Rill (linked lists, immutable) | C/Rust/Java (arrays, in place) |
|---|---|---|
| Insertion sort | clearly nicer — no indices | index arithmetic, shifting loop |
| Merge sort | clearly nicer — `merge` is four cases | auxiliary array, three cursors, copy-back |
| Quicksort | not the real algorithm | this is where the array wins outright |
| Extra memory | O(n) live, O(n log n) allocated | O(log n) for quicksort |
| Cache behaviour | poor — pointers everywhere | excellent — contiguous |
| Sorting by a key | pass a lambda | pass a comparator, or an interface |
| Getting it wrong | usually a type error | usually an off-by-one, at run time |

The summary is that functional sorting is **easier to write correctly and
harder to make fast**. For a few thousand elements, which is what most programs
actually sort, the difference does not matter and the clearer program wins. For
a million elements in a hot loop, you want contiguous memory and in-place
partitioning, and in Rill that means a `Buf` and Chapter 15.

The mistake to avoid is deciding this in the abstract. Write `merge_sort`,
measure it, and only reach for the array if the measurement asks you to.

---

## 10.6 Exercises

1. `merge_sort` calls `len`, `take` and `drop`, walking the list three times
   just to split it. Write a `split_alternating` that produces both halves in
   one pass by dealing elements left, right, left, right.
2. Make the quicksort listing partition in a single pass instead of two
   `filter`s. You will need to return two lists — declare a `Pair` type.
3. Write `is_sorted(l)` and use it to check all three sorts on a few hundred
   random-ish inputs (generate them with a linear congruential generator —
   Chapter 13 has one).
4. Add a third sort: `bottom_up_merge_sort`, which starts with a list of
   one-element lists and repeatedly merges neighbouring pairs. No recursion on
   halves, and it is the version real libraries use for linked lists.
5. Which of the three sorts in §10.1–10.3 are stable? Prove it by sorting a
   list of `Person` by age where two people share an age.

---

Chapter 11 moves from sequences to trees, where recursive types stop being a
teaching device and start being the obviously right tool.
