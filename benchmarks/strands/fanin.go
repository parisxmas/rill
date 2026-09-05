package main

import "fmt"

func main() {
	np, k := 100, 10000
	ch := make(chan int, 100)
	for p := 0; p < np; p++ {
		go func(v int) {
			for i := 0; i < k; i++ {
				ch <- v
			}
		}(p)
	}
	acc := 0
	for n := 0; n < np*k; n++ {
		acc += <-ch
	}
	fmt.Println(acc)
}
