package main

import "fmt"

func main() {
	seed, acc := 42, 0
	for k := 100000000; k > 0; k-- {
		acc += seed % 100
		seed = (seed*1103515245 + 12345) % 2147483648
	}
	fmt.Println(acc)
}
