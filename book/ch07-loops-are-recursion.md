# Chapter 7. Loops Are Recursion

Rill has no `for` and no `while`. This is the feature that most reliably makes
newcomers uneasy, and the uneasiness is about performance: *surely a million
iterations of a recursive function will exhaust the stack?*

This chapter answers that concretely, with measurements, and it ends up
teaching something more interesting than the answer.

---

## 7.1 The transformation

A loop has three parts: variables that change, a condition that ends it, and a
body. A recursive function has all three, in different places:

```
# a loop, imperatively                # the same loop in Rill
i = 10                                fn go(i, acc) =
acc = 0                                 if i == 0 then acc
while i != 0:                           else go(i - 1, acc + i)
    acc = acc + i
    i = i - 1
```

- The **loop variables** become parameters.
- The **exit condition** becomes the `if`.
- **Advancing** becomes the arguments of the recursive call.

Nothing is assigned. Each call has its own `i` and its own `acc`, one step
further along than the last.

```rill
# book/code/ch07/loop.rill
fn count_up(i, n) =
  if i > n then 0 else count_on(i, n)

fn count_on(i, n) =
  println(i)
  count_up(i + 1, n)

fn sum_to(n) = go(n, 0)

fn go(i, acc) = if i == 0 then acc else go(i - 1, acc + i)

fn main() =
  count_up(1, 3)
  println(sum_to(10))
  println(sum_to(10000000))
```

```
$ rill run loop.rill
1
2
3
55
50000005000000
```

Ten million iterations, and the program returns. Nothing overflowed.

> **Why `count_up` is split into two functions**
>
> `count_on` does two things — prints, then loops — so it needs a block. The
> `if` that decides whether to continue lives in `count_up`. Splitting a loop
> body from its condition this way is a common shape in Rill, and it is the
> price of not having a statement form for `if`.

---

## 7.2 Why it does not overflow

Look at where the recursive call sits in `go`:

```
fn go(i, acc) = if i == 0 then acc else go(i - 1, acc + i)
                                       ^^^^^^^^^^^^^^^^^^
                                       the last thing that happens
```

When `go` calls itself, there is nothing left to do afterwards. The result of
the inner call *is* the result of the outer one. So the outer frame holds
nothing worth keeping — no pending addition, no saved local — and the compiler
reuses it instead of pushing a new one.

That is a **tail call**, and eliminating it turns the recursion into a jump.
The machine code for `go` is a loop: compare, subtract, add, jump back.

In Rill this is a guarantee, not an optimization. A call in tail position
compiles to a jump, so a tail-recursive function runs in constant stack for any
number of iterations. The same guarantee exists in Scheme, ML, OCaml, Haskell
and Erlang. It does *not* exist in Java, Python or JavaScript, which is why
recursion has a bad reputation among programmers who learned there.

Here is the check, at twenty million iterations:

```
$ /usr/bin/time -l ./tail20
200000010000000
             1310720  maximum resident set size
```

1.3 MB. Twenty million iterations of a function calling itself, and the process
uses about as much memory as a hello-world.

---

## 7.3 Making a loop out of a non-tail function

Not every recursion is in tail position. Compare:

```
fn sum_up(n) = if n == 0 then 0 else n + sum_up(n - 1)
                                     ^^^^^^^^^^^^^^^^
                                     an addition is still pending
```

Here the recursive call is *not* the last thing: after it returns, `n` must
still be added. So the frame has to survive, and this is a genuine stack
consumer.

The fix is the **accumulator**: carry the running answer forward as an argument
so nothing is left pending.

```
fn sum_up(n) = go(n, 0)
fn go(n, acc) = if n == 0 then acc else go(n - 1, acc + n)
```

This is the single most useful transformation to have automatic. Whenever you
write a recursion and find an operation waiting on the result, ask whether it
can be pushed into an extra parameter. Usually it can. `reverse` in the prelude
is exactly this trick:

```
fn rev_onto(l, acc) = match l
  Nil -> acc
  Cons(x, rest) -> rev_onto(rest, Cons(x, acc))

fn reverse(l) = rev_onto(l, Nil)
```

The accumulator is a list being built backwards, which is why reversing is the
natural thing to fall out of it.

---

## 7.4 An honest measurement, and a surprise

Having explained that `sum_up` is not tail recursive and therefore consumes
stack, here is what actually happens when you run it at twenty million:

```
$ /usr/bin/time -l ./deep20
200000010000000
             1294336  maximum resident set size
```

It works. 1.29 MB — no more than the tail-recursive version.

The explanation is that Rill compiles through LLVM at `-O3`, and LLVM performs
its own transformation: when the pending operation is **associative**, as `+`
is, it can rewrite accumulator-style recursion into a loop by itself. The
compiler did to `sum_up` exactly what §7.3 told you to do by hand.

Three lessons, and the third is the one worth keeping:

**One.** The language guarantee and the optimizer are different things. Tail
position is guaranteed by Rill. Everything else is LLVM being clever, and LLVM
being clever is not a specification.

**Two.** Do not measure this with a toy. LLVM at `-O3` is aggressive enough to
fuse whole pipelines: `len(range(1, 5000000))` never allocates the list at all,
because the optimizer inlines both functions and discovers it is counting. Any
microbenchmark you write to "prove" something about allocation may be measuring
a program that was deleted.

**Three.** Write the accumulator version anyway. It costs one parameter, it
holds when the pending operation is not associative, it holds when the
recursion allocates, and it holds when you compile with `--debug`. Relying on
an optimizer to rescue a shape you could have written correctly is a bet you
will lose eventually, at scale, in production.

---

## 7.5 Loops that never end

A server loop is a tail-recursive function with no exit condition:

```
fn worker(jobs, results) =
  n = recv(jobs)
  send(results, n * 2)
  worker(jobs, results)
```

That call is in tail position, so this runs forever in constant stack. There is
no `while true`, and there does not need to be — an infinite loop is just a
recursion that never reaches its base case.

The same shape appears in the frame loop of a game, the accept loop of a
server, and the read loop of a parser. Chapter 16 uses it throughout.

---

## 7.6 When not to write the loop at all

Most everyday iteration should not be a hand-written recursion. The prelude's
combinators are themselves tail recursive where it matters, and they say what
they mean:

```
range(1, 10) |> filter(\x -> x % 2 == 0) |> map(\x -> x * x) |> sum
```

Reach for explicit recursion when:

- the loop carries **several** pieces of state that do not fit a fold,
- it walks something that is **not a list** — a tree, a grid, a parser's input,
- it must **stop early**, which `filter` and `map` cannot express,
- or the shape genuinely is a loop and naming it makes the code clearer.

Otherwise use the combinator. The reason is Chapter 1's: `map` tells the reader
the shape of the answer before they read the body, and a recursion does not.

---

## 7.7 Exercises

1. Write `factorial(n)` twice — once naively, once with an accumulator — and
   check both at `n = 20`. What happens at `n = 30`, and why is that a
   different problem than stack depth?
2. Write `count_down(n)` that prints `n` down to 1 and then `"done"`. Keep it
   tail recursive.
3. Write `fib(n)` with two accumulators, so that it is linear rather than
   exponential. Compare `fib(40)` against the naive two-call version.
4. Rewrite `len` from the prelude to be tail recursive. Does `map` admit the
   same treatment? What would you have to give up?
5. Take the pipeline in §7.6 and write it as one hand-rolled tail recursion.
   Which version would you rather find in unfamiliar code?

---

Chapter 8 covers the last piece of the language proper: traits, which let your
own types plug into `println`, `<` and the rest.
