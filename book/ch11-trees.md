# Chapter 11. Trees

A tree is where recursive types stop being a teaching example. Lists are
recursive but they are also just sequences, and every language has those.
A tree is where the shape of the data and the shape of the code line up so
exactly that writing the second is mostly a matter of reading the first.

This chapter also answers the question Chapter 6 left open — what do you do
when you need a map and the standard library has none.

---

## 11.1 The type

```rill
type Tree(a)
  Leaf
  Node(left: Tree(a), value: a, right: Tree(a))
```

*A tree is empty, or a node with a left subtree, a value, and a right subtree.*

Compare that with the equivalent in a language without sum types:

```java
class Node<T> {
    Node<T> left;      // may be null
    T value;
    Node<T> right;     // may be null
}
```

The Java version has no way to say "empty tree" except `null`, so every method
starts by checking for it and every field access is a potential
`NullPointerException`. `Leaf` is a value, costs no allocation (Chapter 15
explains why nullary constructors are singletons), and the compiler makes you
handle it.

---

## 11.2 A search tree, whole

```rill
# book/code/ch11/tree.rill
fn insert(t, x) = match t
  Leaf -> Node(Leaf, x, Leaf)
  Node(l, v, r) -> if x < v then Node(insert(l, x), v, r) else if x > v then Node(l, v, insert(r, x)) else t

fn member(t, x) = match t
  Leaf -> false
  Node(l, v, r) -> if x == v then true else if x < v then member(l, x) else member(r, x)

fn to_list(t) = match t
  Leaf -> Nil
  Node(l, v, r) -> append(to_list(l), Cons(v, to_list(r)))

fn size(t) = match t
  Leaf -> 0
  Node(l, _, r) -> 1 + size(l) + size(r)

fn height(t) = match t
  Leaf -> 0
  Node(l, _, r) -> 1 + max(height(l), height(r))

fn from_list(l) = fold(l, Leaf, insert)

fn main() =
  t = from_list(Cons(5, Cons(3, Cons(8, Cons(1, Cons(9, Cons(3, Nil)))))))
  println(to_list(t))
  println(size(t))
  println(height(t))
  println(member(t, 8))
  println(member(t, 4))
  println(to_list(from_list(range(1, 7))))
  println(height(from_list(range(1, 7))))
```

```
$ rill run tree.rill
Cons(1, Cons(3, Cons(5, Cons(8, Cons(9, Nil)))))
5
3
true
false
Cons(1, Cons(2, Cons(3, Cons(4, Cons(5, Cons(6, Cons(7, Nil)))))))
7
```

Six functions, and every one of them has the same two-case shape: what to do
for `Leaf`, what to do for `Node`. That is not a style — it is the type
dictating the code. When the data has two cases the function has two cases, and
the compiler checks you wrote both.

Three things in the output are worth reading closely.

**`to_list` gives `1 3 5 8 9` — sorted.** An in-order walk of a search tree
produces its contents in order, which means `to_list(from_list(xs))` is a sort.
It is O(n log n) if the tree is balanced, and it does not survive the next
observation.

**`size` is 5, but six numbers went in.** The duplicate `3` hit the `else t`
branch of `insert` and was dropped. This tree is a *set*, and that was a design
decision made in one word.

**`height(from_list(range(1, 7)))` is 7.** Inserting 1, 2, 3, 4, 5, 6, 7 in
order gives a tree where every node has an empty left subtree — a linked list
wearing a tree costume. Every operation is O(n), and this is the classic
weakness of an unbalanced search tree, in every language.

Fixing it means rebalancing, and the standard answer in this style is a
**red-black tree**, whose functional insertion is a famous half-page of
pattern matching by Chris Okasaki. It is beyond this chapter, but it is worth
knowing that the functional version is dramatically shorter than the imperative
one — rotations become patterns rather than pointer surgery. Okasaki's *Purely
Functional Data Structures* is the book, and it is the natural sequel to this
chapter.

---

## 11.3 Where the immutability pays

```rill
# book/code/ch11/sharing.rill
fn main() =
  a = fold(Cons(5, Cons(3, Cons(8, Nil))), Leaf, insert)
  b = insert(a, 7)
  println(to_list(a))
  println(to_list(b))
```

```
$ rill run sharing.rill
Cons(3, Cons(5, Cons(8, Nil)))
Cons(3, Cons(5, Cons(7, Cons(8, Nil))))
```

`b` has the 7; `a` does not. Both are valid, usable trees, and `insert` did not
modify anything.

Now the part that answers Chapter 1's objection about copying. Inserting 7 into
this tree did **not** copy it. Look at what `insert` builds:

```
Node(l, v, insert(r, x))
      ^  ^
      └──┴── the untouched left subtree and value, reused as-is
```

Only the nodes **on the path from the root to the new leaf** are rebuilt.
Everything hanging off that path is shared between `a` and `b` — the same
memory, pointed at by both. For a balanced tree of a million nodes, inserting
one element allocates about twenty nodes, not a million.

This is what "persistent data structure" means, and it is only safe because
nothing can change. If `a` could be modified, `b` sharing its subtrees would be
a catastrophic aliasing bug — the exact bug in Chapter 1's `add_tag` example.
Immutability is what converts sharing from a hazard into the default.

The practical consequences are large: you can keep the old version for undo,
hand a snapshot to another strand with no lock and no copy, or keep every
historical version of a structure at the cost of the deltas.

---

## 11.4 A map, since the prelude has none

Rill 0.2 has no dictionary type. Here is one, as a search tree of key-value
entries:

```rill
# book/code/ch11/map.rill
type Entry(k, v)
  Entry(key: k, value: v)

type Dict(k, v)
  Empty
  Branch(left: Dict(k, v), item: Entry(k, v), right: Dict(k, v))

fn put(d, k, v) = match d
  Empty -> Branch(Empty, Entry(k, v), Empty)
  Branch(l, e, r) -> match e
    Entry(ek, _) -> if k < ek then Branch(put(l, k, v), e, r) else if k > ek then Branch(l, e, put(r, k, v)) else Branch(l, Entry(k, v), r)

fn get(d, k) = match d
  Empty -> None
  Branch(l, e, r) -> match e
    Entry(ek, ev) -> if k == ek then Some(ev) else if k < ek then get(l, k) else get(r, k)

fn count_words(d, l) = match l
  Nil -> d
  Cons(w, rest) -> count_words(put(d, w, unwrap_or(get(d, w), 0) + 1), rest)

fn main() =
  words = split("the cat sat on the mat the end", " ")
  d = count_words(Empty, words)
  println(get(d, "the"))
  println(get(d, "cat"))
  println(get(d, "dog"))
```

```
$ rill run map.rill
Some(3)
Some(1)
None
```

Four things this listing demonstrates at once.

**Two type parameters.** `Dict(k, v)` is generic in both, and `Dict(Str, Int)`
comes out of the use with no annotation anywhere.

**`get` returns `Option`.** A missing key is `None`, not an exception and not a
zero that might have been a real value. `get(d, "dog")` is `None` and the
caller must say what that means.

**`unwrap_or(get(d, w), 0) + 1`** is the word-count idiom: look up, default to
zero, add one, put back. In Python that is `counts[w] = counts.get(w, 0) + 1`
— the same sentence, and the difference is that `put` returns a new dictionary
rather than modifying one.

**`count_words` threads the dictionary through the recursion.** This is the
functional answer to "a loop that updates a variable": the variable becomes a
parameter, and each step passes the updated version to the next. It is tail
recursive, so it is a loop.

### What this costs

A tree map is O(log n) for lookup where a hash map is O(1), and the constant
factor is worse because it chases pointers. It is also unbalanced here, so
inserting sorted keys degrades it to a list — inserting words in alphabetical
order would be O(n) per operation.

For counting the words in a sentence, none of that matters. For a hot lookup
table with a million entries, it matters a great deal, and the answer is a hash
table over a `Buf` (Chapter 15) or a balanced tree. Knowing that you are making
this trade is the point; making it silently is the problem.

---

## 11.5 Trees are everywhere once you look

The shape in this chapter — a recursive sum type, and functions that match its
cases — is not really about search trees. It is the shape of:

- **an expression** (`Add(Mul(Num(2), Num(3)), Num(4))`), which Chapter 12
  parses and evaluates;
- **a document** (a heading contains paragraphs contain spans);
- **a file system** (a directory contains files and directories);
- **a decision** (a game tree, a behaviour tree);
- **JSON**, which is a five-case sum type and nothing else.

In every one of them the same two rules apply: the type has a case per shape,
and each function has an arm per case. Once that reflex is trained, a
surprising number of "hard" programs turn out to be a type declaration and four
short functions.

---

## 11.6 Exercises

1. Write `remove(t, x)`. The hard case is a node with two children — the usual
   answer is to replace it with the smallest value in its right subtree.
2. Write `fold_tree(t, init, f)` that folds the values in order, and redefine
   `size`, `sum` and `to_list` in terms of it.
3. `to_list` uses `append`, which makes it O(n²) on a degenerate tree. Rewrite
   it with an accumulator so it is linear.
4. Add `keys(d)` and `values(d)` to the dictionary, and `merge(a, b)` where
   `b`'s entries win.
5. Write `depth_of(t, x)` returning `Option(Int)` — how far down a value is, or
   `None` if it is not there.
6. Model JSON as a type: null, boolean, number, string, array of values, object
   of key-value pairs. Write `render(json) -> Str`. (`lib/json.rill`
   in this repository is a full parser and printer, if you want to compare.)

---

Chapter 12 takes the tree that a program's text turns into, and builds it from
characters.
