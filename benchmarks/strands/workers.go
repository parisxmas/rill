package main

import "fmt"

func main() {
	n := 1000000
	jobs := make(chan int, 1000)
	results := make(chan int, 1000)
	for w := 0; w < 8; w++ {
		go func() {
			for j := range jobs {
				results <- j * j % 7
			}
		}()
	}
	go func() {
		for i := 0; i < n; i++ {
			jobs <- i
		}
	}()
	acc := 0
	for k := 0; k < n; k++ {
		acc += <-results
	}
	fmt.Println(acc)
}
