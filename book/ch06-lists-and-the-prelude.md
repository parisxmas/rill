# Chapter 6. Lists and the Prelude

Every listing so far has used `Cons` and `Nil` without explanation. This
chapter explains them, which turns out to explain a good deal about the
language: the list is not a built-in type, it is an ordinary sum type declared
in the prelude, and every function over it is ordinary Rill you could have
written.

---

## 6.1 The list is not special

Here is the whole definition, from `crates/rill_syntax/src/prelude.rill`:

```
type List(a)
  Nil
  Cons(head: a, tail: List(a))
```

*A list of `a` is either empty, or a first element followed by a list of `a`.*

That is a **recursive** sum type: `Cons`'s second field is another `List(a)`.
And it is **generic**: `a` is a type parameter, so `List(Int)` and `List(Str)`
are different types made from the same declaration.

```rill
# book/code/ch06/lists.rill
fn main() =
  xs = Cons(1, Cons(2, Cons(3, Nil)))
  println(xs)
  ...
```

```
$ rill run lists.rill
Cons(1, Cons(2, Cons(3, Nil)))
3
6
Cons(3, Cons(2, Cons(1, Nil)))
Cons(1, Cons(2, Cons(3, Cons(4, Nil))))
Cons(1, Cons(2, Cons(3, Cons(4, Cons(5, Nil)))))
Cons(8, Cons(9, Cons(10, Nil)))
Some(2)
None
true
false
```

`Cons(1, Cons(2, Cons(3, Nil)))` is verbose, and Rill 0.2 has no `[1, 2, 3]`
literal. In practice most lists come from `range`, from `split`, or from a
function rather than from being typed out — but this is a genuine ergonomic gap
and worth naming rather than glossing over.

> **Why a linked list and not an array?**
>
> Because of immutability. Adding an element to the front of a linked list
> allocates one cell that points at the existing list, and the existing list is
> untouched — so both the old and the new list are valid, and they share every
> element. An immutable array would have to be copied.
>
> The cost is that indexing is O(n) and there is no cache locality. Lists are
> the right structure for *sequences you walk*, which is most of them; when you
> need indexing or packed memory, Chapter 15's `Buf` is the answer.

---

## 6.2 Reading the prelude

The value of the list being ordinary is that the prelude is readable. Here is
`map`:

```
fn map(l, f) = match l
  Nil -> Nil
  Cons(x, rest) -> Cons(f(x), map(rest, f))
```

Two cases, matching the two cases of the type. An empty list maps to an empty
list; a list with a head maps to *`f` of the head, followed by the map of the
rest*. That is the entire definition and it is the shape of nearly every list
function you will ever write.

```
fn filter(l, p) = match l
  Nil -> Nil
  Cons(x, rest) -> if p(x) then Cons(x, filter(rest, p)) else filter(rest, p)

fn fold(l, acc, f) = match l
  Nil -> acc
  Cons(x, rest) -> fold(rest, f(acc, x), f)

fn len(l) = match l
  Nil -> 0
  Cons(_, rest) -> 1 + len(rest)
```

Learn to write these three from memory. Once the shape is automatic —
*base case for `Nil`, recursive case for `Cons`* — a large class of problems
stops needing any thought.

Note the difference between `fold` and the other two: in `fold` the recursive
call is the **last thing that happens**, while in `map` a `Cons` is built
around the result and in `len` a `1 +` is applied to it. That difference is the
subject of Chapter 7, and it decides which of these run in constant stack.

---

## 6.3 What the prelude gives you

Every program has these in scope. Any definition of your own with the same name
replaces the prelude's.

**Types**: `Option(a)`, `Result(a, e)`, `List(a)`.

**Lists**

| | |
|---|---|
| `map(l, f)` `filter(l, p)` `fold(l, acc, f)` | the three combinators |
| `len(l)` `sum(l)` | aggregate |
| `range(a, b)` | the list `a` through `b` inclusive |
| `reverse(l)` `append(a, b)` | rearrange |
| `take(l, n)` `drop(l, n)` `nth(l, n)` | slice; `nth` returns `Option` |
| `any(l, p)` `all(l, p)` | quantify |

**Option and Result**: `unwrap_or(o, d)`, `opt_map(o, f)`, `res_map(r, f)`.

**Numbers**: `max(a, b)`, `min(a, b)`, `abs(n)`.

**Strings**: `join(l, sep)`, `repeat(s, n)`, `split(s, sep)`, `lines(s)`,
`trim(s)`, `contains(s, p)`, `starts_with(s, p)`.

**IO**: `args()`, `read_file(path) -> Result(Str, Str)`.

That is the whole standard library. It is small, and that is a fair criticism
of a young language — there is no map, no set, no sort, no formatted printing.
Chapters 10 and 11 write a sort and a map, which is one way to turn the
complaint into a lesson.

---

## 6.4 Strings

```rill
# book/code/ch06/strings.rill
fn main() =
  s = "  the quick brown fox  "
  println("[" + trim(s) + "]")
  println(split(trim(s), " "))
  println(join(split(trim(s), " "), "-"))
  println(len(split(trim(s), " ")))
  println(contains(s, "brown"))
  println(starts_with(trim(s), "the"))
  println(repeat("ab", 3))
  println(str_len("fox"))
  println(str_sub("quick", 1, 3))
  println(str_find("quick", "ick"))
```

```
$ rill run strings.rill
[the quick brown fox]
Cons(the, Cons(quick, Cons(brown, Cons(fox, Nil))))
the-quick-brown-fox
4
true
true
ababab
3
uic
2
```

Strings are immutable and reference counted, and `+` joins them. The low-level
operations are byte-indexed:

| | |
|---|---|
| `str_len(s)` | length in bytes |
| `str_get(s, i)` | the byte at `i`, as an `Int` |
| `str_sub(s, start, len)` | a substring |
| `str_find(s, needle)` | index, or -1 |
| `chr(byte)` | a one-byte string |

"Byte-indexed" is the truth about all of these, and it stays the truth: a lexer
wants bytes, a wire protocol wants bytes, and the compiler that compiles the
prelude is one of them. `#s` counts bytes, `s[i]` is a byte, `s[i:j]` cuts
between bytes.

What bytes are not is characters, and the difference shows the moment the text
is not ASCII. `"günaydın"` is ten bytes and eight letters, so `#s` says ten, and
`s[0:2]` cuts the `ü` in half and gives back something that is not text at all.

So the characters are their own layer on top, and they say so in their names:

| | |
|---|---|
| `char_count(s)` | how many characters |
| `char_at(s, i)` | the `i`th character, as a codepoint |
| `char_sub(s, i, n)` | `n` characters from character `i` |
| `char_take(s, n)` / `char_drop(s, n)` | the first `n`, or the rest |
| `chars(s)` | every codepoint, as a `List(Int)` |
| `from_char(c)` / `from_chars(cs)` | back to a string |

```rill
fn main() =
  s = "günaydın"
  println(int_to_str(#s))              # 10 — bytes
  println(int_to_str(char_count(s)))   # 8  — letters
  println(char_take(s, 2))             # gü
  println(s[0:2])                      # g<broken> — the byte cut
```

Note that `from_char` is the one that spells a codepoint properly. `chr` is not
it and is not meant to be: `chr` writes a single low byte, which is right for
building bytes and wrong for every character past 127.

All of it walks the string, because UTF-8 gives no other way to find the `n`th
character — a character's position is the sum of the widths before it. For a
name or a line of a menu that costs nothing. For a loop over a long string, use
`char_decode(s, at)` and `char_wide(b)` together: they step through byte offsets
without counting from the beginning each time.

Malformed input never hangs and never reads past the end. A string cut in the
middle of a character — which is exactly what byte slicing makes — still counts
and still decodes; what comes back for the broken character is the byte itself.

Notice that `split` returns a `List(Str)`, so the list functions apply to it
directly — `len(split(s, " "))` is a word count, and `join` puts it back
together. Text processing in Rill is list processing.

---

## 6.5 Your own recursive types

Nothing about `List` is privileged, so your own structures work identically:

```rill
# book/code/ch06/mylist.rill
type Stack(a)
  Empty
  Push(top: a, rest: Stack(a))

fn depth(s) = match s
  Empty -> 0
  Push(_, rest) -> 1 + depth(rest)

fn peek(s) = match s
  Empty -> None
  Push(x, _) -> Some(x)

fn main() =
  s = Push(3, Push(2, Push(1, Empty)))
  println(s)
  println(depth(s))
  println(peek(s))
  println(peek(Empty))
```

```
$ rill run mylist.rill
Push(3, Push(2, Push(1, Empty)))
3
Some(3)
None
```

`peek` returning `Option` rather than crashing on an empty stack is the
Chapter 5 discipline applied without being asked. Notice also that `Push` and
`Cons` are the same structure with different names — a stack *is* a list, and
the reason to declare your own is to give the operations names that mean
something in your problem, and to stop list functions from applying where they
would not make sense.

Chapter 11 builds a binary tree the same way, which is where recursive types
start to earn their keep.

---

## 6.6 Exercises

1. Write `last(l)` returning `Option(a)`, and `init(l)` returning everything
   but the last element.
2. Write `zip(a, b)` pairing two lists into a `List` of a two-field type you
   declare. What should it do when the lists differ in length?
3. Write `words(s)` that splits on spaces and drops empty strings, so that
   `words("  a  b ")` has two elements.
4. `flatten(list_of_lists)` — write it with `fold` and `append`. Then work out
   why that is O(n²) and what it would take to fix.
5. Read `take` and `drop` in the prelude listing in §6.2 and predict what
   `take(range(1, 5), 100)` does before running it.

---

Chapter 7 answers the question this chapter kept deferring: if `len` calls
itself once per element, what happens on a list of ten million?
