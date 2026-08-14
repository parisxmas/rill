package main

import "fmt"

// Idiomatic Go: nil is the empty tree (note: Rill allocates a box even for
// Leaf, so Rill does roughly 2x the allocations in this benchmark).
type Tree struct {
	l, r *Tree
	v    int
}

func build(d int) *Tree {
	if d == 0 {
		return nil
	}
	return &Tree{build(d - 1), build(d - 1), d}
}

func check(t *Tree) int {
	if t == nil {
		return 1
	}
	return 1 + check(t.l) + check(t.r)
}

func main() {
	acc := 0
	for k := 0; k < 10; k++ {
		acc += check(build(18))
	}
	fmt.Println(acc)
}
