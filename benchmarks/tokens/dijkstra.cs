using System;

class Dijkstra {
    static long[] hd = [];
    static int[] hn = [];
    static int hs;

    static void Push(long d, int v) {
        hd[hs] = d;
        hn[hs] = v;
        int i = hs++;
        while (i > 0 && hd[(i - 1) / 2] > hd[i]) {
            int p = (i - 1) / 2;
            (hd[i], hd[p]) = (hd[p], hd[i]);
            (hn[i], hn[p]) = (hn[p], hn[i]);
            i = p;
        }
    }

    static void Pop() {
        hs--;
        hd[0] = hd[hs];
        hn[0] = hn[hs];
        int i = 0;
        while (true) {
            int l = 2 * i + 1;
            if (l >= hs) return;
            int c = (l + 1 < hs && hd[l + 1] < hd[l]) ? l + 1 : l;
            if (hd[i] <= hd[c]) return;
            (hd[i], hd[c]) = (hd[c], hd[i]);
            (hn[i], hn[c]) = (hn[c], hn[i]);
            i = c;
        }
    }

    static string text = "";
    static int pos;

    static long Next() {
        while (pos < text.Length && char.IsWhiteSpace(text[pos])) pos++;
        long v = 0;
        while (pos < text.Length && !char.IsWhiteSpace(text[pos])) v = v * 10 + (text[pos++] - '0');
        return v;
    }

    static void Main() {
        text = Console.In.ReadToEnd();
        int n = (int)Next(), m = (int)Next(), s = (int)Next(), t = (int)Next();
        var eu = new int[m];
        var ev = new int[m];
        var ew = new long[m];
        for (int i = 0; i < m; i++) { eu[i] = (int)Next(); ev[i] = (int)Next(); ew[i] = Next(); }

        var off = new int[n + 2];
        for (int i = 0; i < m; i++) off[eu[i] + 1]++;
        for (int i = 1; i <= n; i++) off[i] += off[i - 1];
        var cur = (int[])off.Clone();
        var to = new int[m];
        var wt = new long[m];
        for (int i = 0; i < m; i++) { int p = cur[eu[i]]++; to[p] = ev[i]; wt[p] = ew[i]; }

        var dist = new long[Math.Max(n, 1)];
        Array.Fill(dist, long.MaxValue);
        dist[s] = 0;
        hd = new long[m + 1];
        hn = new int[m + 1];
        Push(0, s);
        while (hs > 0) {
            long d = hd[0];
            int u = hn[0];
            Pop();
            if (u == t) { Console.WriteLine(d); return; }
            if (d > dist[u]) continue;
            for (int e = off[u]; e < off[u + 1]; e++) {
                long nd = d + wt[e];
                if (nd < dist[to[e]]) { dist[to[e]] = nd; Push(nd, to[e]); }
            }
        }
        Console.WriteLine(-1);
    }
}
