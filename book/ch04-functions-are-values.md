# Chapter 4. Functions Are Values

In Chapter 1 this was an abstract claim. Here it becomes the thing you actually
type. Functions in Rill are values like integers are values: they can be passed
to functions, returned from functions, stored in data, and made up on the spot.

---

## 4.1 Passing a function by name

```rill
# book/code/ch04/values.rill
fn double(x) = x * 2

fn apply_twice(f, x) = f(f(x))

fn multiplier(k) = \x -> x * k

fn main() =
  println(apply_twice(double, 5))
  triple = multiplier(3)
  println(triple(7))
  println(apply_twice(triple, 2))
  println(apply_twice(\s -> s + "!", "hi"))
```

```
$ rill run values.rill
20
21
18
hi!!
```

Line by line:

**`apply_twice(double, 5)`** passes `double` without calling it. `double` is
`double(x)` called; `double` on its own is the function itself. This is the
same distinction as Python's `f` versus `f()`.

**`\x -> x * k`** is a lambda: backslash, parameters, arrow, body. Multiple
parameters are separated by commas (`\a, b -> a + b`).

**`multiplier(3)` returns a function that has captured `3`.** That is a
closure. `triple` is a value carrying a 3 around inside it, and it outlives the
call to `multiplier` that created it.

**The last line shows `apply_twice` working on strings.** The same
`apply_twice` — nothing was overloaded and nothing was declared generic. §4.4
explains why.

---

## 4.2 Pipelines

Nested calls read inside-out, which is the wrong direction for a sequence of
steps. The `|>` operator turns them around:

```
x |> f(a)      means      f(x, a)
```

The value on the left becomes the *first* argument of the call on the right.

```rill
# book/code/ch04/pipeline.rill
fn square(x) = x * x

fn even(x) = x % 2 == 0

fn main() =
  println(range(1, 10) |> filter(even) |> map(square) |> sum)
  println(sum(map(filter(range(1, 10), even), square)))
  println(range(1, 5) |> map(\x -> x * 10))
```

```
$ rill run pipeline.rill
220
220
Cons(10, Cons(20, Cons(30, Cons(40, Cons(50, Nil)))))
```

The first two lines are the same computation and produce the same answer. The
first reads left to right in the order the work happens: take one to ten, keep
the even ones, square them, add them up. The second reads outside-in, and you
have to find the innermost call before you can start.

This is not a small stylistic matter. Pipelines are how functional code stays
readable as it grows, and `|>` is why `map(l, f)` takes the list first — every
list function in the prelude is arranged so that the data comes first and the
pipeline works.

The third line shows what a list looks like when printed raw:
`Cons(10, Cons(20, ...))`. Chapter 6 explains why, and Chapter 8 shows how to
make it print as `[10 20 30 40 50]` instead.

> **Compared to other languages**
>
> `|>` comes from F# and OCaml; Elixir has it too. Where a language lacks it,
> the same job is done by method chaining — `xs.filter(even).map(square)` in
> Rust, Scala or JavaScript. The advantage of an operator over methods is that
> any function works in a pipeline, including one you wrote five minutes ago,
> without it having to be a method on the type.

---

## 4.3 Where closures come from

A closure is a function plus the variables it captured. `multiplier` above is
the toy version; here is the shape you actually use it in.

Suppose you want to keep the elements of a list above some threshold. The
threshold is not known when you write the code — it arrives at run time:

```
fn above(xs, limit) = filter(xs, \x -> x > limit)
```

The lambda mentions `limit`, which is not one of its parameters. It comes from
the enclosing function, and it is captured by value when the lambda is made.
Every call to `above` produces a different predicate, each carrying its own
`limit`.

That is the whole mechanism, and it is behind most uses of higher-order
functions: a comparison that knows which field to sort on, a callback that
knows which connection it belongs to, a validator that knows the rules it was
configured with.

---

## 4.4 Generics happen without asking

None of the functions in §4.1 were declared generic, and `apply_twice` worked
on integers and strings. This is Hindley–Milner type inference, and it is the
feature that lets a statically typed language feel like a dynamically typed one.

```rill
# book/code/ch04/generic.rill
fn id(x) = x

fn twice(f, x) = f(f(x))

fn biggest(a, b) = if a > b then a else b

fn main() =
  println(id(42))
  println(id("forty-two"))
  println(twice(\n -> n + 1, 0))
  println(twice(\s -> s + "-", "x"))
  println(biggest(3, 9))
  println(biggest("apple", "pear"))
  println(biggest(1.5, 0.5))
```

```
$ rill run generic.rill
42
forty-two
2
x--
9
pear
1.5
```

The compiler works out the most general type each function could have:

- `id(x) = x` — the result is the argument, so `id : a -> a` for any `a`.
- `twice(f, x) = f(f(x))` — `f` is applied to `x` and then to its own result,
  so `f`'s input and output are the same type: `twice : (a -> a, a) -> a`.
- `biggest(a, b) = if a > b then a else b` — this one is more interesting. The
  body uses `>`, so whatever `a` is must be **ordered**. The inferred type is
  "for any ordered `a`, `(a, a) -> a`", and that requirement travels with the
  function.

Try `biggest` at a type with no ordering and the compiler names both the
function and the type. This is the same mechanism as Rust's trait bounds or
Haskell's type classes, with the difference that you did not write the bound —
it was inferred from the body.

### Monomorphization

Generic in Rill does not mean boxed. The compiler emits **one specialized copy
per concrete type**: `id` at `Int` and `id` at `Str` are two separate machine-
code functions, each with no indirection, no type tag, and nothing to unbox.

The trade is compile time and binary size against run time. Chapter 15 returns
to it; the practical effect is that `map(xs, \x -> x * 2)` over a list of
integers compiles to the loop you would have written by hand.

### Two limits worth knowing now

**A function used before its definition is pinned to that use.** If line 3
calls `helper(1)` and `helper` is defined on line 40, `helper` is fixed at
`Int` even if it would otherwise have been generic. Define first, or accept the
narrower type.

**There is no polymorphic recursion.** A generic function cannot call itself at
a different type than it was called at. This rules out a small number of exotic
data structures and is not something you are likely to trip over by accident.

---

## 4.5 Leaving an argument for later

Writing `\x -> add(2, x)` to pass "add two" to something is a lot of notation
for very little thought. An underscore where the argument would go says the same
thing:

```rill
ikiEkle = add(2, _)        # \x -> add(2, x)
yariya  = bol(_, 2)        # \x -> bol(x, 2)
map(l, add(10, _))
```

Any position, not only the ones at the end — `bol(_, 2)` divides *by* two, which
a curried language cannot express without writing the lambda out. Two
underscores make a function of two arguments, in the order they were left:
`uc(_, 5, _)` takes the first and third.

It works for anything that is called, which includes constructors and the
builtins:

```rill
map(Cons(4, Nil), Daire(_))          # a constructor as a function
map(l, int_to_str(_))                # a builtin, which is not a value on its own
```

Note what this deliberately does *not* do. Calling a function with too few
arguments is still an error:

```rill
f = add(2)     # error: `add` takes 2 argument(s), but 1 given
```

In a language where that produced a function, every miscounted argument list
becomes a value of the wrong type, and the complaint arrives somewhere else
entirely — often several functions away. The underscore is how you say you meant
it, and leaving it out is how the compiler knows you did not.

One thing you cannot do yet is call the result immediately: `add(2, _)(5)` does
not parse, because calling what a call returned is not a shape the parser has.
Bind it to a name first.

---

## 4.6 The three combinators

Almost every loop you would write in another language is one of these:

| | Type, informally | What it produces |
|---|---|---|
| `map(list, f)` | `(List(a), a -> b) -> List(b)` | same length, each element transformed |
| `filter(list, p)` | `(List(a), a -> Bool) -> List(a)` | the elements satisfying `p`, in order |
| `fold(list, init, f)` | `(List(a), b, (b, a) -> b) -> b` | one value, accumulated left to right |

`map` and `filter` are the easy two. `fold` is the one that takes a moment,
and it is worth spending that moment because the other two are special cases
of it.

Fold walks the list carrying an accumulator. At each element it calls
`f(accumulator, element)` and the result becomes the new accumulator:

```
fold(Cons(1, Cons(2, Cons(3, Nil))), 0, \a, x -> a + x)

  a = 0,  x = 1   →   1
  a = 1,  x = 2   →   3
  a = 3,  x = 3   →   6
                      6
```

That is `sum`, and in the prelude that is literally how `sum` is defined:

```
fn sum(l) = fold(l, 0, \a, x -> a + x)
```

Change the operator and the seed and you get a different aggregate: `max` of a
list, the length, a string joined together, a count of matches. Once you see
that a fold is "a loop with one variable in it", you stop writing loops.

---

## 4.7 Exercises

1. Write `compose(f, g)` returning a function that applies `g` then `f`. Use it
   to build "double then increment" and "increment then double", and show they
   differ.
2. Write `count_if(xs, p)` two ways — once with `filter` and `len`, once with a
   single `fold`.
3. `product` is to `*` what `sum` is to `+`. Write it as a fold. What seed?
4. Predict the type of `fn pair_up(f, a, b) = f(a, b)` before checking. What
   does the compiler infer for `fn odd_one(f, x) = f(x) + 1`?

---

Chapter 5 introduces the other half of the language: describing your own data,
and taking it apart with pattern matching.
