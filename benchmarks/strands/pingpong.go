package main

import "fmt"

func main() {
	a := make(chan int)
	b := make(chan int)
	go func() {
		for {
			x := <-a
			b <- x + 1
		}
	}()
	x := 0
	for k := 0; k < 1000000; k++ {
		a <- x
		x = <-b
	}
	fmt.Println(x)
}
