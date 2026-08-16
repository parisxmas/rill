package main

import "fmt"

const inf = int64(1) << 62

var hd, hn []int64
var hsize int

func swap(b []int64, i, j int) { b[i], b[j] = b[j], b[i] }

func hpush(d, v int64) {
	hd[hsize] = d
	hn[hsize] = v
	i := hsize
	hsize++
	for i > 0 && hd[(i-1)/2] > hd[i] {
		p := (i - 1) / 2
		swap(hd, i, p)
		swap(hn, i, p)
		i = p
	}
}

func hpop() {
	hsize--
	hd[0] = hd[hsize]
	hn[0] = hn[hsize]
	i := 0
	for {
		l := 2*i + 1
		if l >= hsize {
			return
		}
		c := l
		if l+1 < hsize && hd[l+1] < hd[l] {
			c = l + 1
		}
		if hd[i] <= hd[c] {
			return
		}
		swap(hd, i, c)
		swap(hn, i, c)
		i = c
	}
}

var seed int64 = 12345

func rnd() int64 { seed = (seed*1103515245 + 12345) & 2147483647; return seed }

func main() {
	n, m, s, t := 200000, 2000000, 0, 200000-1
	eu := make([]int, m)
	ev := make([]int, m)
	ew := make([]int64, m)
	for i := 0; i < m; i++ {
		eu[i] = int(rnd() % int64(n))
		ev[i] = int(rnd() % int64(n))
		ew[i] = rnd() % 100
	}
	off := make([]int, n+1)
	for i := 0; i < m; i++ {
		off[eu[i]+1]++
	}
	for i := 1; i <= n; i++ {
		off[i] += off[i-1]
	}
	cur := make([]int, n+1)
	copy(cur, off)
	to := make([]int, m)
	wt := make([]int64, m)
	for i := 0; i < m; i++ {
		p := cur[eu[i]]
		to[p] = ev[i]
		wt[p] = ew[i]
		cur[eu[i]] = p + 1
	}
	dist := make([]int64, n)
	for i := range dist {
		dist[i] = inf
	}
	dist[s] = 0
	hd = make([]int64, m+1)
	hn = make([]int64, m+1)
	hpush(0, int64(s))
	for hsize > 0 {
		d, u := hd[0], int(hn[0])
		hpop()
		if u == t {
			fmt.Println(d)
			return
		}
		if d > dist[u] {
			continue
		}
		for e := off[u]; e < off[u+1]; e++ {
			nd := d + wt[e]
			if nd < dist[to[e]] {
				dist[to[e]] = nd
				hpush(nd, int64(to[e]))
			}
		}
	}
	fmt.Println(-1)
}
