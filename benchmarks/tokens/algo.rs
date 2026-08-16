struct Heap {
    d: Vec<i64>,
    v: Vec<usize>,
    n: usize,
}

impl Heap {
    fn push(&mut self, d: i64, v: usize) {
        self.d[self.n] = d;
        self.v[self.n] = v;
        let mut i = self.n;
        self.n += 1;
        while i > 0 && self.d[(i - 1) / 2] > self.d[i] {
            let p = (i - 1) / 2;
            self.d.swap(i, p);
            self.v.swap(i, p);
            i = p;
        }
    }

    fn pop(&mut self) {
        self.n -= 1;
        self.d[0] = self.d[self.n];
        self.v[0] = self.v[self.n];
        let mut i = 0;
        loop {
            let l = 2 * i + 1;
            if l >= self.n {
                return;
            }
            let c = if l + 1 < self.n && self.d[l + 1] < self.d[l] { l + 1 } else { l };
            if self.d[i] <= self.d[c] {
                return;
            }
            self.d.swap(i, c);
            self.v.swap(i, c);
            i = c;
        }
    }
}

fn main() {
    let (n, m, s, t) = (200_000usize, 2_000_000usize, 0usize, 199_999usize);
    let mut seed: i64 = 12345;
    let mut rnd = || {
        seed = (seed.wrapping_mul(1103515245) + 12345) & 2147483647;
        seed
    };
    let (mut eu, mut ev, mut ew) = (vec![0usize; m], vec![0usize; m], vec![0i64; m]);
    for i in 0..m {
        eu[i] = (rnd() % n as i64) as usize;
        ev[i] = (rnd() % n as i64) as usize;
        ew[i] = rnd() % 100;
    }
    let mut off = vec![0usize; n + 2];
    for i in 0..m {
        off[eu[i] + 1] += 1;
    }
    for i in 1..=n {
        off[i] += off[i - 1];
    }
    let mut cur = off.clone();
    let (mut to, mut wt) = (vec![0usize; m], vec![0i64; m]);
    for i in 0..m {
        let p = cur[eu[i]];
        to[p] = ev[i];
        wt[p] = ew[i];
        cur[eu[i]] = p + 1;
    }
    let mut dist = vec![i64::MAX; n.max(1)];
    dist[s] = 0;
    let mut h = Heap { d: vec![0; m + 1], v: vec![0; m + 1], n: 0 };
    h.push(0, s);
    while h.n > 0 {
        let (d, u) = (h.d[0], h.v[0]);
        h.pop();
        if u == t {
            println!("{d}");
            return;
        }
        if d > dist[u] {
            continue;
        }
        for e in off[u]..off[u + 1] {
            let nd = d + wt[e];
            if nd < dist[to[e]] {
                dist[to[e]] = nd;
                h.push(nd, to[e]);
            }
        }
    }
    println!("-1");
}
