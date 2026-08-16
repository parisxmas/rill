using System;

class Fib {
    static int F(int n) => n < 2 ? n : F(n - 1) + F(n - 2);

    static void Main() => Console.WriteLine(F(35));
}
