package main

import "fmt"

func main() {
	first := make(chan int)
	in := first
	for i := 0; i < 100000; i++ {
		out := make(chan int)
		go func(in, out chan int) { out <- <-in + 1 }(in, out)
		in = out
	}
	go func() { first <- 0 }()
	fmt.Println(<-in)
}
