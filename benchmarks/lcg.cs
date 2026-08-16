using System;

class Lcg {
    static void Main() {
        long seed = 42, acc = 0;
        for (long k = 100000000; k > 0; k--) {
            acc += seed % 100;
            seed = (seed * 1103515245 + 12345) % 2147483648;
        }
        Console.WriteLine(acc);
    }
}
