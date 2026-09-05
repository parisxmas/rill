package main

import "fmt"

func main() {
	n := 20000
	first := make(chan int)
	inp := first
	for k := 0; k < 50; k++ {
		out := make(chan int)
		go func(inp, out chan int) {
			for x := range inp {
				out <- x + 1
			}
		}(inp, out)
		inp = out
	}
	go func() {
		for i := 0; i < n; i++ {
			first <- i
		}
	}()
	acc := 0
	for k := 0; k < n; k++ {
		acc += <-inp
	}
	fmt.Println(acc)
}
