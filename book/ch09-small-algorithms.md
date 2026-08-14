# Chapter 9. Small Algorithms, Compared

From here on the language is assumed and the subject is programs. Each chapter
takes a family of algorithms, writes them in Rill, and puts them beside the same
algorithm in a language you probably know — not to score points, but because
the differences are where the ideas of Chapter 1 become concrete.

This chapter does the small ones: the problems that fit on half a page and turn
up in every interview and every teaching language.

---

## 9.1 FizzBuzz

```rill
# book/code/ch09/fizzbuzz.rill
fn word(n) =
  by3 = n % 3 == 0
  by5 = n % 5 == 0
  if by3 && by5 then "FizzBuzz" else if by3 then "Fizz" else if by5 then "Buzz" else int_to_str(n)

fn main() = println(join(map(range(1, 20), word), " "))
```

```
$ rill run fizzbuzz.rill
1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz 16 17 Fizz 19 Buzz
```

The imperative version everyone writes:

```python
for i in range(1, 21):
    if i % 15 == 0:   print("FizzBuzz")
    elif i % 3 == 0:  print("Fizz")
    elif i % 5 == 0:  print("Buzz")
    else:             print(i)
```

The two programs are the same length and equally clear, and anyone claiming a
big win for either is selling something. But there is one real structural
difference, and it is the pattern the rest of the book leans on:

**The Rill version separates deciding from printing.** `word` is a pure
function from a number to a string. You can call it, test it, put its results
in a list, join them with a comma instead of a space, or send them over a
socket. The Python version has the printing welded into the decision, and
getting a testable `word` out of it means rewriting it.

That is the "pure core, effectful edge" shape from Chapter 1, and at this scale
it looks like pedantry. At the scale of a real program it is the difference
between a codebase you can test and one you cannot.

---

## 9.2 Greatest common divisor

Euclid's algorithm is a tail recursion in its natural form, which makes it the
one classic algorithm that is *shorter* in this style:

```rill
# book/code/ch09/gcd.rill
fn gcd(a, b) = if b == 0 then a else gcd(b, a % b)

fn lcm(a, b) = a / gcd(a, b) * b

fn main() =
  println(gcd(1071, 462))
  println(gcd(17, 5))
  println(lcm(4, 6))
  println(fold(range(1, 10), 1, lcm))
```

```
$ rill run gcd.rill
21
1
12
2520
```

Beside the imperative version:

```python
def gcd(a, b):
    while b != 0:
        a, b = b, a % b
    return a
```

These are the same algorithm, and the Rill one is what the mathematical
definition says: *gcd(a, b) is a when b is zero, and gcd(b, a mod b)
otherwise.* The Python version has to introduce a simultaneous assignment to
avoid needing a temporary, which is a fact about Python rather than about
Euclid.

The last line is worth noticing on its own:

```rill
fold(range(1, 10), 1, lcm)
```

The smallest number divisible by everything from 1 to 10, as a one-liner,
because `lcm` is exactly the kind of two-argument function `fold` wants. Once
you have the combinators, "combine a list with this operation" stops being a
loop you write and becomes a phrase you say.

---

## 9.3 Primes, two ways

```rill
# book/code/ch09/primes.rill
fn divides(d, n) = n % d == 0

fn no_factor_from(n, d) =
  d * d > n || (if divides(d, n) then false else no_factor_from(n, d + 1))

fn is_prime(n) = n >= 2 && no_factor_from(n, 2)

fn sieve(l) = match l
  Nil -> Nil
  Cons(p, rest) -> Cons(p, sieve(filter(rest, \x -> x % p != 0)))

fn main() =
  println(filter(range(1, 50), is_prime))
  println(sieve(range(2, 50)))
  println(len(filter(range(2, 10000), is_prime)))
```

```
$ rill run primes.rill
Cons(2, Cons(3, Cons(5, Cons(7, ... Cons(47, Nil)))))
Cons(2, Cons(3, Cons(5, Cons(7, ... Cons(47, Nil)))))
1229
```

Two different algorithms, same answer.

**Trial division** (`is_prime`) tests each candidate on its own. The loop is
`no_factor_from`, tail recursive, with `d` as the loop variable and `d * d > n`
as the exit — the usual "stop at the square root" without ever computing a
square root.

> **That line was written differently**
>
> It was first written `if d * d > n then true else if divides(d, n) then
> false else ...`, and `rill fmt` rewrote it to the `||` above. The two are the
> same program: `||` in Rill is parser sugar that desugars to exactly that
> `if`, and the formatter recovers the operator when it sees the shape. It is a
> small reminder of what canonical formatting is for — there is one spelling,
> and the tool knows it even when you do not.

**The sieve** (`sieve`) is the one worth studying. Read it as a sentence: *the
sieve of a list is its first element, followed by the sieve of everything in
the rest that the first element does not divide.* That is four words of
recursion and one `filter`, and it is the whole of Eratosthenes' idea.

```python
# the imperative sieve
def sieve(n):
    mark = [True] * (n + 1)
    for p in range(2, int(n ** 0.5) + 1):
        if mark[p]:
            for m in range(p * p, n + 1, p):
                mark[m] = False
    return [i for i in range(2, n + 1) if mark[i]]
```

Here the comparison is genuinely uneven, and in both directions.

The Rill version is clearer: no array, no bounds, no nested loop, no
off-by-one. It is a definition rather than a procedure.

The Python version is much faster. The array sieve crosses off multiples in
place, which is O(n log log n) over contiguous memory. The functional sieve
builds a new list at every step and filters it, which is closer to O(n²/log n)
with allocation on top. This is precisely the cost Chapter 1 warned about:
*some algorithms genuinely want mutation*, and a sieve is the textbook one.

The honest conclusion is that you should write the first version for clarity,
and if the sieve turns out to be your bottleneck, reach for a `Buf` (Chapter
15) and write the array version — which Rill can also express.

---

## 9.4 Collatz: a loop with a maximum

```rill
# book/code/ch09/collatz.rill
fn step(n) = if n % 2 == 0 then n / 2 else 3 * n + 1

fn length_from(n, acc) = if n == 1 then acc else length_from(step(n), acc + 1)

fn chain(n) = length_from(n, 0)

fn best(l, bn, bl) = match l
  Nil -> Cons(bn, Cons(bl, Nil))
  Cons(n, rest) -> if chain(n) > bl then best(rest, n, chain(n)) else best(rest, bn, bl)

fn main() =
  println(chain(27))
  println(best(range(1, 10000), 1, 0))
```

```
$ rill run collatz.rill
111
Cons(6171, Cons(261, Nil))
```

Two loops of different shapes, which is why this problem is here.

`length_from` is a plain accumulator loop: one number in, one number out, exit
when it reaches 1.

`best` is an **argmax** — a fold that carries two pieces of state, the best
input seen and its score. This is the pattern that a plain `fold` handles
awkwardly (you would need a pair type, which Rill 0.2 has no literal for) and
that explicit recursion handles naturally. It is the first case in this book
where writing the recursion yourself is the better choice.

There is a bug-shaped inefficiency in `best` worth spotting: `chain(n)` is
called twice on the winning branch, and `chain` is not cheap. Fixing it means
computing it once and passing it along.

The obvious fix — binding `c` inside the arm — does not compile, and it is
worth showing the error because it is the roughest edge in the language:

```
collatz.rill: syntax error at line 6, column 5: expected an expression, found indent
```

**A `match` arm's body cannot be an indented block.** So the binding moves into
a helper function instead:

```rill
# book/code/ch09/collatz_fixed.rill
fn best(l, bn, bl) = match l
  Nil -> Cons(bn, Cons(bl, Nil))
  Cons(n, rest) -> best_on(rest, n, chain(n), bn, bl)

fn best_on(rest, n, c, bn, bl) = if c > bl then best(rest, n, c) else best(rest, bn, bl)
```

```
$ rill run collatz_fixed.rill
Cons(6171, Cons(261, Nil))
```

Same answer, and `chain` is now called once per candidate instead of twice.
Chapter 12 hits this limitation again in a parser, where it costs three extra
functions.

Note in passing that hoisting the call out like this is *always* safe here.
In an impure language you could not be sure — the second call might return
something different, or do something. That is referential transparency being
useful rather than being a definition.

---

## 9.5 Digits and palindromes

```rill
# book/code/ch09/digits.rill
fn digits_of(n) = if n < 10 then Cons(n, Nil) else append(digits_of(n / 10), Cons(n % 10, Nil))

fn is_palindrome(n) =
  d = digits_of(n)
  d == reverse(d)

fn main() =
  println(digits_of(9021))
  println(sum(digits_of(9021)))
  println(is_palindrome(12321))
  println(is_palindrome(12320))
  println(filter(range(100, 140), is_palindrome))
```

```
$ rill run digits.rill
Cons(9, Cons(0, Cons(2, Cons(1, Nil))))
12
true
false
Cons(101, Cons(111, Cons(121, Cons(131, Nil))))
```

`d == reverse(d)` deserves a moment. That is **structural equality on a data
type**, working for free: no `equals` method, no `__eq__`, no comparator. Rill
compares two lists element by element because that is what equality on a data
value means, and the same line would work for a tree or any other type you
declare.

In Java that expression is `d.equals(reversed)` and only if someone wrote
`equals` correctly. In C it is a loop. In Python it works, because Python's
lists do the same thing — which is a good reminder that structural equality is
not a functional-programming idea, just a good one.

`digits_of` also shows the cost of building a list in the wrong direction.
`append(digits_of(n / 10), ...)` walks the whole accumulated list on every
digit, so it is O(d²) in the number of digits. For a 4-digit number that is
nothing; the exercise at the end asks you to fix it, and the fix is the
accumulator pattern from Chapter 7.

---

## 9.6 What these five have in common

Looking back at the chapter, three habits appear in every listing:

**A loop is a function with the loop variables as parameters.** `no_factor_from`,
`length_from`, `best` — each one is a `while` turned inside out, and each one is
tail recursive so it costs nothing.

**The recursive definition of a problem is often the program.** The sieve and
Euclid's algorithm are the clearest examples: the code and the mathematical
statement are the same sentence.

**Deciding is separated from doing.** `word`, `is_prime`, `chain` and
`is_palindrome` are pure functions that return values. Nothing prints except
`main`.

And one warning, which the sieve made concrete: **an elegant functional
algorithm is sometimes asymptotically worse than the array version**. It is not
always, and the cases where it is are predictable — they involve in-place
updates of a large structure. Knowing which you are in is part of the job.

---

## 9.7 Exercises

1. Rewrite `digits_of` with an accumulator so it is linear. Check it against
   the original for a few hundred numbers.
2. Write `is_perfect(n)` — a number equal to the sum of its proper divisors —
   and find all of them below 10,000.
3. Write the Collatz `best` with a `fold` by declaring a two-field type to
   carry the pair. Which version do you prefer, and why?
4. Write `to_binary(n)` returning a `Str`. Then `from_binary(s)`, and check
   that they round-trip.
5. Time `len(filter(range(2, 100000), is_prime))` against the functional sieve
   at the same limit. Predict the ratio before you measure it.

---

Chapter 10 takes the first algorithm where the functional and imperative
versions really do diverge: sorting.
