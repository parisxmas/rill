// Idiomatic C#: null is the empty tree, as in the Go version.
using System;

sealed class Tree {
    public Tree? L, R;
    public int V;
    public Tree(Tree? l, Tree? r, int v) { L = l; R = r; V = v; }
}

class Trees {
    static Tree? Build(int d) => d == 0 ? null : new Tree(Build(d - 1), Build(d - 1), d);

    static int Check(Tree? t) => t == null ? 1 : 1 + Check(t.L) + Check(t.R);

    static void Main() {
        int acc = 0;
        for (int k = 0; k < 10; k++) acc += Check(Build(18));
        Console.WriteLine(acc);
    }
}
