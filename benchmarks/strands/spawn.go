package main

import "fmt"

func main() {
	n := 200000
	ch := make(chan int, 1000)
	for i := 0; i < n; i++ {
		go func(i int) { ch <- i }(i)
	}
	acc := 0
	for k := 0; k < n; k++ {
		acc += <-ch
	}
	fmt.Println(acc)
}
